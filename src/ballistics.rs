//! Shared shell flight, collision queries, penetration, and impacts.

use std::sync::Arc;
use std::time::Instant;

use avian3d::prelude::{Collider, Forces, Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
// `shot_trace::record` only evaluates its closure when tracing is armed.
use serde_json::json;

use crate::damage::{DamageConsequences, VolumeOf, hit_ancestor};
use crate::damage_cam::recorder::{MarchRecorder, ShotRecorder, ShotRecording};
use crate::state::{GameplaySet, SimPhase};
use crate::terrain_grid::HeightGrid;
use crate::{ClientReplica, Layer, ShotClock, ShotId};

/// Server-sanctioned outcome bookkeeping: the client's reconciliation buffer and the bounded
/// catch-up that replays it into a drawable flight. Netcode, not ballistics — the flight,
/// penetration and spall math below never reads it.
mod sanctioned;

/// The §13 union field walk — the pure resolution core (overlaps charge their union, micro-gaps
/// ε-weld, the shell samples the world as a caliber-wide disc). The march runs it through
/// `resolve::decide`, the Avian adapter that collects the corridor. Everything in there
/// is a pure function over pre-collected hit lists, which is what makes the §13.6 invariants
/// testable to the bit.
mod walk;
pub(crate) use walk::VolumeMaterial;

/// Round mechanisms: how a round's state advances through the ordered encounters of a crossing.
/// Solid shot is the only one built.
mod mechanism;

/// The spall cone's shape data: half-angle, axis rule and the directions it is sampled by.
pub(crate) mod spall_cone;

/// The §13 walk driven against the live world: corridor collection, the staged `begin`/`finish`, and
/// the mapping from its declarative plan onto this module's impacts, spall, damage and impulses.
mod resolve;

/// The witness pass (ADR-0040): the round's line and the field's directions intersected with the
/// witness volumes the hard world never sees.
mod wound;

/// The spatial half of the §13 walk: turning a corridor of world rays into EVERY face crossing along
/// them, with each face's true (winding) orientation. Avian's own all-hits query cannot express it —
/// see the module doc.
#[allow(
    dead_code,
    reason = "slice-2 adapter; wired into the march by the resolver swap"
)]
mod collect;

/// The §13.6 ray fuzzer: random rays at the BOUND tank, the union-field invariants machine-checked,
/// and every corridor reaching crew or ammunition reported with its admitting caliber and per-caliber
/// η. A corridor fails the gate by name, unconditionally: an effectively unarmoured route into the
/// tank is a defect in the model.
#[cfg(any(feature = "dev_tools", test))]
pub mod fuzz;

/// Frozen end-to-end shots through the real march at the real Tiger — CHARACTERIZATION, not
/// specification. A red golden here means the physics moved; see the module doc before re-pinning.
#[cfg(test)]
mod goldens;

/// The spall-cone casts digested to the bit and compared with a pinned digest, run on every
/// shipped target by CI: the instrument for whether a client may re-cast a field itself.
#[cfg(test)]
mod field_casts;

/// The pen cam recorder: scripted shots at the bound tank read back as pen cam records, and the
/// scenario files the demo plays, checked current.
#[cfg(test)]
mod pen_cam;

/// The crew-outcome sweep (ignored; run on demand): random main-gun hits on the bound tank under
/// the witness law and the law it replaced, counted per crewman.
#[cfg(test)]
mod crew_sweep;

/// The game's pen cam recorder on the fixed tick: rounds fired at fixture tanks read back as the
/// records the authority emits.
#[cfg(test)]
mod recording_tests;

/// The PRESENTATION half of this module: the shell scene, the tracer streak, and the visibility a
/// hold draws with. Mounted by the client roots alone ([`view::plugin`]) — the dedicated server
/// composes [`sim_plugin`] and therefore opens no view artifact and emits no render component.
mod view;

pub(crate) use view::ShellVisual;
pub use view::{TracerStreak, plugin as view_plugin};

pub(crate) use sanctioned::{
    SanctionedBounce, SanctionedBounceInsert, SanctionedShots, SanctionedTerminal,
};
use sanctioned::{SanctionedCatchUp, catch_up_sanctioned_chain};

/// The march's boundary nudge: a crossing that leaves a face it resolved — a perforation's exit, a
/// bounce's struck face — resumes this far past it, so the next sweep does not re-touch it. Never
/// applied past a face nothing resolved: a fresh disc starts where it is laid, with its membership
/// established there ([`resolve::certified`]).
///
/// Shared with [`resolve`] because the two must agree: the interior seed a crossing hands forward
/// describes the point the NEXT corridor starts from, and that point is nudged. A seed read one
/// nudge earlier can say "inside" about a plate the corridor has already left, which the walk then
/// reports as an entry with no exit.
pub(crate) const MARCH_EPS: f32 = 1.0e-3;

/// Gravity applied to shells each fixed tick (m/s²).
const GRAVITY: Vec3 = Vec3::new(0.0, -9.81, 0.0);

/// World-floor height (m). Shells below it have left the playable world and are culled.
const KILL_FLOOR: f32 = -100.0;

/// Tunable form constant for the quadratic air-drag model `dv/dt = −k·v²`.
const DRAG_FORM: f32 = 0.263;

/// A shell's quadratic-drag coefficient `k` (1/m), shared with the range table.
pub fn drag_k(caliber: f32, mass: f32) -> f32 {
    DRAG_FORM * caliber * caliber / mass
}

/// One free-flight integration step shared by the live march and range table.
///
/// Invariant: drag uses `v / (1 + k * v * dt)` so both paths produce identical velocity.
pub fn freeflight_step(velocity: Vec3, drag_k: f32, dt: f32) -> Vec3 {
    let v = velocity + GRAVITY * dt;
    let speed = v.length();
    if speed == 0.0 {
        return v;
    }
    (v / speed) * (speed / (1.0 + drag_k * speed * dt))
}

/// Free-flight catch-up from the muzzle. Returns the caught-up state plus its path; callers must
/// resolve any armor crossed by the skipped chords.
///
/// Positionally equivalent to the live march's open air: the axis is carried as the march carries
/// it, an anchor re-taken at each step's start plus an anchor-relative offset advanced by
/// [`freeflight_step`]'s velocity × `dt` ([`resolve::Interior::rebased`], [`resolve::Flight::fly`]),
/// so neither path re-rounds the accumulated position at world spacing. The two agree to the
/// rounding of the march's rebuilt velocity, not bit for bit; the caught-up shell's next step lays
/// a fresh disc at the returned position, dropping one world rounding's residual.
pub(crate) fn fast_forward_shell(
    origin: Vec3,
    velocity: Vec3,
    drag_k: f32,
    dt: f32,
    ticks: u32,
) -> (Vec3, Vec3, Vec<Vec3>) {
    let mut axis = resolve::Interior::laid(origin, vec![Vec3::ZERO]);
    let mut vel = velocity;
    let mut points = Vec::with_capacity(ticks as usize + 1);
    points.push(origin);
    for _ in 0..ticks {
        axis = axis.rebased();
        vel = freeflight_step(vel, drag_k, dt);
        axis.origin += vel * dt;
        points.push(axis.world_origin());
    }
    (axis.world_origin(), vel, points)
}

/// Wrapping elapsed ticks on the same half-range rule Lightyear's `Tick - Tick -> i32` uses, kept
/// net-neutral for the ballistics layer. Returns `None` when `then` is actually ahead of `now`; a
/// genuine elapsed interval may cross `u32::MAX` and still returns its small positive distance.
fn elapsed_ticks(now: u32, then: u32) -> Option<u32> {
    let elapsed = now.wrapping_sub(then);
    (elapsed <= i32::MAX as u32).then_some(elapsed)
}

/// CEILING (chosen bound, not derived): the work/absurdity limit on cosmetic shell catch-up. A
/// cursor-released shot normally ages ~0–1 tick at the cursor crossing; the arrival-clock path
/// (`net::client::fire_catch_up_ticks`) reads this bound only as the consume-time validity bar
/// and in the pre-sync arrival-presentation fallback. A fire tick older than the bound is stale
/// or corrupt off the wire and fails closed rather than drawing a shortened, invented trajectory.
pub(crate) const MAX_COSMETIC_CATCH_UP_TICKS: u32 = 100;

/// Reference steel's material facts: the substance registry's `Reference` entry
/// (`assets/materials/materials.ron`), parsed once from the embedded file — data, never an asset,
/// so it is in hand synchronously wherever a round is bound.
fn reference_steel() -> &'static crate::substances::MaterialFacts {
    static FACTS: std::sync::LazyLock<crate::substances::MaterialFacts> =
        std::sync::LazyLock::new(|| {
            crate::substances::SubstanceRegistry::shipped_static().reference_facts()
        });
    &FACTS
}

/// Target inertia factor `B` of the force law (dimensionless): a named approximation, weakly
/// constrained by the available plate data (published compressible fits 1.0–1.3, incompressible
/// theory 1.5).
const B_INERTIA: f32 = 1.0;

/// What the capability law reads off a round: mass (kg), calibre (m) and nose factor `N`
/// (dimensionless): the frictionless nose factor of the cavity-expansion form, in `(0, 1]`, derived
/// from the spec's nose geometry ([`crate::spec::NoseShape`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RoundBody {
    pub(crate) mass: f32,
    pub(crate) caliber: f32,
    pub(crate) nose_factor: f32,
}

impl RoundBody {
    /// The law's two round terms against the reference steel `reference`: `m / (2 A k)` (m) and
    /// `k / a` (s²/m²), `k = B_INERTIA · N · ρ_ref` the inertia coefficient (kg/m³) and `a` the
    /// reference's cavity strength term.
    ///
    /// In f64: `m / (2 A k)` grows as `1 / N` and leaves f32's range for a nose factor near f32's
    /// smallest normal, while the capability it multiplies stays finite.
    fn terms_against(self, reference: &crate::substances::MaterialFacts) -> (f64, f64) {
        let caliber = self.caliber as f64;
        let area = std::f64::consts::FRAC_PI_4 * caliber * caliber;
        let k = B_INERTIA as f64 * self.nose_factor as f64 * reference.density;
        (
            self.mass as f64 / (2.0 * area * k),
            k / reference.cavity_strength(),
        )
    }

    /// [`terms_against`](Self::terms_against) the shipped reference steel.
    fn terms(self) -> (f64, f64) {
        self.terms_against(reference_steel())
    }
}

/// The force law's readout in reference-mm for a round of `terms` ([`RoundBody::terms_against`]) at
/// `speed` m/s: `1000 · m / (2 A k) · ln(1 + k v² / a)`.
///
/// The shell-state transcendentals (`ln(1 + x)` here, `exp(x) − 1` in [`speed_for`] and the
/// mechanism, `ln` in the reference's cavity strength) are `libm`'s, not the platform's: the same
/// functions, computed by one implementation on every shipped target, so a shot's speeds, spends
/// and spall strengths are the same bits everywhere.
fn capability_of((scale, inertia): (f64, f64), speed: f64) -> f64 {
    1000.0 * scale * libm::log1p(inertia * speed * speed)
}

/// `round`'s mechanism, bound against the shipped substance registry.
fn bound(round: RoundBody) -> mechanism::SolidShot {
    mechanism::SolidShot::bind(
        round,
        crate::substances::SubstanceRegistry::shipped_static(),
    )
}

/// Reference-mm `round` can still defeat at `speed` m/s — the force law
/// `C = 1000 · m / (2 A k) · ln(1 + k v² / a)`, `A = π c² / 4`, `k = B_INERTIA · N · ρ_ref`, `a`
/// the reference steel's cavity strength term
/// ([`crate::substances::MaterialFacts::cavity_strength`], ≈ 3.711e9 Pa, DERIVED).
///
/// Invariants: replica scaling (mass ×s³, calibre ×s gives capability ×s); strictly decreasing in
/// calibre at fixed mass; local speed exponent 2 at low speed, below 2 above it. Evaluated in f64
/// and cast once: finite for every round [`round_in_domain`] admits.
fn capability(round: RoundBody, speed: f32) -> f32 {
    capability_of(round.terms(), speed as f64) as f32
}

/// Inverse of [`capability`] for a fixed round: the speed carrying `capability` reference-mm.
/// Spending cost then inverting composes — two spends equal one spend of their sum — and gives the
/// Lambert–Jonas residual-velocity shape (design doc §3).
fn speed_for(round: RoundBody, capability: f32) -> f32 {
    let (scale, inertia) = round.terms();
    (libm::expm1(capability as f64 / (1000.0 * scale)) / inertia).sqrt() as f32
}

/// Whether the capability law can state `round` fired at `speed`: every input finite, positive
/// and normal, `N ≤ 1`, and the capability at that speed and its inverse finite and positive.
/// Read at spec load, so a round outside the law's domain never reaches the march.
pub(crate) fn round_in_domain(round: RoundBody, speed: f32) -> Result<(), String> {
    for (field, value) in [
        ("mass", round.mass),
        ("caliber", round.caliber),
        ("nose factor", round.nose_factor),
        ("speed", speed),
    ] {
        if !value.is_normal() || value < 0.0 {
            return Err(format!(
                "{field} must be finite, positive and normal (got {value})"
            ));
        }
    }
    if round.nose_factor > 1.0 {
        return Err(format!(
            "nose factor must be ≤ 1 (got {})",
            round.nose_factor
        ));
    }
    let capability = capability(round, speed);
    if !capability.is_normal() || capability < 0.0 {
        return Err(format!("capability at {speed} m/s is {capability}"));
    }
    let back = speed_for(round, capability);
    if !back.is_normal() || back < 0.0 {
        return Err(format!("capability {capability} inverts to {back} m/s"));
    }
    Ok(())
}

/// Main-penetrator transit damage = the cost it paid crossing that volume × this (design §6).
pub(crate) const TRANSIT_K: f32 = 1.0;

/// HP points per reference-mm of dose: one HP point is 0.1 reference-mm, the quantum a box's dose
/// from one shot is rounded to (design doc §6). RESOLUTION: the unit every component's `hp` in the
/// tank data is authored in, and the quantum below which a spall cone is not thrown; it is a unit,
/// not a balance value, so changing it rescales every authored pool with it.
pub(crate) const HP_PER_REFERENCE_MM: u64 = 10;

/// Fixed-point dose units per reference-mm. Every dose contribution is quantized to whole units
/// before it is summed, so a box's per-shot total is an integer sum: the same for every order its
/// contributions arrive in. RESOLUTION: 1 unit = 1e-9 reference-mm = 1e-8 HP point, finer than an
/// f32 dose's own spacing from 0.01 reference-mm up (ulp(0.01) = 9.3e-10, DERIVED), so quantizing
/// costs no more than the f32 dose already did. Decimal, so the HP quantum is exactly
/// [`DOSE_UNITS_PER_HP`] units and a tie is an exact integer. A u64 total holds 1.8e10
/// reference-mm (DERIVED).
pub(crate) const DOSE_UNITS_PER_REFERENCE_MM: u64 = 1_000_000_000;

/// Dose units per HP point: [`DOSE_UNITS_PER_REFERENCE_MM`] / [`HP_PER_REFERENCE_MM`], exact.
const DOSE_UNITS_PER_HP: u64 = DOSE_UNITS_PER_REFERENCE_MM / HP_PER_REFERENCE_MM;
const _: () = assert!(DOSE_UNITS_PER_HP * HP_PER_REFERENCE_MM == DOSE_UNITS_PER_REFERENCE_MM);

/// One dose contribution of `dose` reference-mm in whole dose units: widened to f64 (exact), one
/// IEEE multiply and a round to nearest, ties away from zero, so it is the same on every target. A
/// negative or NaN dose is none; the cast saturates.
pub(crate) fn dose_units(dose: f32) -> u64 {
    (f64::from(dose) * DOSE_UNITS_PER_REFERENCE_MM as f64).round() as u64
}

/// The whole HP points `units` dose units convert to, rounded to nearest, ties away from zero.
pub(crate) fn hp_of_units(units: u64) -> u32 {
    let points =
        units / DOSE_UNITS_PER_HP + u64::from(units % DOSE_UNITS_PER_HP >= DOSE_UNITS_PER_HP / 2);
    u32::try_from(points).unwrap_or(u32::MAX)
}

/// The whole HP points one dose of `dose` reference-mm converts to: [`dose_units`] then
/// [`hp_of_units`], the rounding a box's per-shot total takes. A negative or NaN dose converts to
/// none.
pub(crate) fn hp_points(dose: f32) -> u32 {
    hp_of_units(dose_units(dose))
}

/// One shot's dose to each box it has reached, in whole dose units ([`dose_units`]), the HP points
/// already taken for it and the HP its pool actually lost. A box loses `hp_of_units(total)` over the
/// shot, its total from the line, a bounce's shock and every spall cone summed first: each
/// contribution quantized, the integers summed, the sum rounded once per box per shot, never per
/// direction or per source, and never dependent on the order the contributions arrive in.
/// [`Self::settle`] takes what the running total has added since it last ran, so HP falls as the
/// shot goes and lands on the rounded total when it ends.
///
/// Every figure here is this shot's own, never a difference of pool HP: a march step resolves one
/// shell whole before the next, and one settle closes each armour mark, so what a shell finds in a
/// pool at a mark and takes from it there is its own even where other rounds dose the box between
/// its marks ([`Settle`]).
///
/// Invariant: created with the projectile, never attached after replication. See ADR-0014.
#[derive(Component, Default)]
pub(crate) struct ShotDose {
    tallies: Vec<BoxTally>,
    /// The boxes whose pool this shot took to zero, in the order it emptied them (authority only).
    emptied: Vec<Entity>,
}

/// A box's running dose this shot (dose units), the units its last settle took HP for, whether a
/// dose has been added since, and each settle that took a dose of its pool (authority only).
struct BoxTally {
    address: Entity,
    units: u64,
    settled: u64,
    fresh: bool,
    settles: Vec<Settle>,
}

/// One settle of a box's pool for a mark that dosed it: the armour mark it closed (its index in the
/// shell's log), the HP the pool held before it, this shot's dose units on the box before that
/// mark, and the HP the pool lost. Every deposit of that mark is the shot's own and lands between
/// the two pool readings, so the deposit that emptied a pool is the first of the mark's, in path
/// order, whose running total from `units_before` takes `held` points.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settle {
    pub mark: usize,
    pub held: u32,
    pub units_before: u64,
    pub lost: u32,
}

impl Settle {
    /// This settle took the pool to zero.
    pub(crate) fn emptied(&self) -> bool {
        self.lost > 0 && self.lost == self.held
    }
}

impl ShotDose {
    /// Add `amount` reference-mm to `address`'s dose.
    fn add(&mut self, address: Entity, amount: f32) {
        let units = dose_units(amount);
        match self
            .tallies
            .iter_mut()
            .find(|entry| entry.address == address)
        {
            Some(entry) => {
                entry.units = entry.units.saturating_add(units);
                entry.fresh = true;
            }
            None => self.tallies.push(BoxTally {
                address,
                units,
                settled: 0,
                fresh: true,
                settles: Vec::new(),
            }),
        }
    }

    /// Add each witness's line dose of a [`ArmourMark::Wound`]: its `cost` as it stands, the wound
    /// rate being the witness law's whole scale ([`TRANSIT_K`] is the hard world's).
    fn add_wounds(&mut self, volumes: &[VolumeCrossing]) {
        for crossed in volumes {
            self.add(crossed.volume, crossed.cost);
        }
    }

    /// Add every dose of `doses`, a line dose scaled by [`TRANSIT_K`]; a field dose is already in
    /// the count unit.
    fn add_all(&mut self, doses: &[mechanism::Dose]) {
        for dose in doses {
            let amount = match dose.source {
                mechanism::DoseSource::Line { .. } => dose.amount * TRANSIT_K,
                mechanism::DoseSource::Field => dose.amount,
            };
            self.add(dose.address, amount);
        }
    }

    /// Close armour mark `mark` (its index in the shell's log): take from each box's HP pool the
    /// points its rounded total has gained since the last settle, floored at zero, and return the HP
    /// removed. A box with no HP pool takes nothing. Only authority writes HP (`deposit`).
    fn settle(
        &mut self,
        health: &mut Query<&mut ComponentHealth>,
        deposit: bool,
        mark: usize,
    ) -> u32 {
        if !deposit {
            return 0;
        }
        let mut removed = 0;
        for entry in &mut self.tallies {
            let fresh = std::mem::take(&mut entry.fresh);
            let units_before = std::mem::replace(&mut entry.settled, entry.units);
            let due = hp_of_units(entry.units).saturating_sub(hp_of_units(units_before));
            if !fresh && due == 0 {
                continue;
            }
            let Ok(mut hp) = health.get_mut(entry.address) else {
                continue;
            };
            let held = hp.current;
            let lost = due.min(held);
            hp.current -= lost;
            removed += lost;
            entry.settles.push(Settle {
                mark,
                held,
                units_before,
                lost,
            });
            if lost > 0 && hp.current == 0 {
                self.emptied.push(entry.address);
            }
        }
        removed
    }

    /// Each box this shot has reached and the HP its pool has lost to the shot so far: zero for one
    /// with no HP pool or one already empty, and for every box where HP is not deposited (a
    /// replica).
    pub(crate) fn losses(&self) -> impl Iterator<Item = (Entity, u32)> + '_ {
        self.tallies.iter().map(|entry| {
            let lost = entry.settles.iter().map(|settle| settle.lost).sum();
            (entry.address, lost)
        })
    }

    /// Each settle of `address`'s pool, in mark order; empty for a box this shot has not reached,
    /// one with no HP pool, or on a replica.
    pub(crate) fn settles(&self, address: Entity) -> &[Settle] {
        self.tallies
            .iter()
            .find(|entry| entry.address == address)
            .map_or(&[], |entry| &entry.settles)
    }

    /// The boxes whose pool this shot took to zero, in the order it emptied them.
    pub(crate) fn emptied(&self) -> &[Entity] {
        &self.emptied
    }
}

/// Mirror a travel direction about a surface normal — the specular deflection of a ricochet.
fn reflect(dir: Dir3, normal: Dir3) -> Dir3 {
    let d = Vec3::from(dir);
    let n = Vec3::from(normal);
    Dir3::new(d - 2.0 * d.dot(n) * n).unwrap_or(dir)
}

/// Rotate `dir` toward `target` by at most `angle` radians.
fn bend_toward(dir: Dir3, target: Dir3, angle: f32) -> Dir3 {
    let d = Vec3::from(dir);
    let t = Vec3::from(target);
    let between = d.angle_between(t);
    if between < 1.0e-5 || angle <= 0.0 {
        return dir;
    }
    let Ok(axis) = Dir3::new(d.cross(t)) else {
        return dir;
    };
    Dir3::new(Quat::from_axis_angle(Vec3::from(axis), angle.min(between)) * d).unwrap_or(dir)
}

/// Ray-cast predicate that excludes the firing tank's ballistic volumes for the whole flight.
///
/// Invariant: source identity controls collision filtering only; [`crate::ClientReplica`] controls
/// authority to deposit damage. Remote cosmetic shells therefore retain the source identity.
fn not_own_volume(
    entity: Entity,
    shooter: Option<Entity>,
    owners: &Query<&VolumeOf>,
    parents: &Query<&ChildOf>,
) -> bool {
    let Some(shooter) = shooter else {
        return true;
    };
    // Ownership sits on the hit's ancestry (`hit_ancestor`, the shared hierarchy-resolution rule) —
    // the same walk `aim_distance` makes for the aim ray.
    hit_ancestor(entity, owners, parents).is_none_or(|(_, owner)| owner.tank() != shooter)
}

/// One first-surface contact along a march segment (see [`cast_march_segment`]).
enum SegmentHit {
    /// A parry hit on the `Armor` layer — real, entity-addressed geometry the penetration
    /// resolution then probes (thickness/exit casts, ancestry, HP).
    Armor(avian3d::prelude::RayHitData),
    /// The ground.
    Terrain { distance: f32, normal: Vec3 },
}

/// First surface along one march segment: armor and terrain probed SEPARATELY, nearer wins.
///
/// Armor KEEPS the Avian/parry spatial query — tank volumes are posed, entity-owned geometry the
/// march must then keep probing (thickness, exit faces, ancestry, HP). Terrain, though, comes
/// from [`HeightGrid::cast_ray`] whenever the heightmap world is live: parry's raycast float
/// path is not one the deterministic sim may depend on — parry ≤ 0.29's raycasts were not
/// cross-platform reproducible (SIMD dot-product non-associativity; parry 0.29 changelog) — and
/// we cannot pin a third-party float path, while the ground is the one world-spanning surface
/// every shell crosses. The exact caster reads the SAME triangular surface the collider carries
/// (the ONE-SURFACE invariant, `terrain_grid` module doc), so within float rounding the hit is
/// unchanged; the arithmetic is now ours to keep bit-stable. The flat fallback world (slab +
/// authored test course, dev-only — no [`HeightGrid`] resource) keeps the parry terrain cast.
///
/// Splitting the old single `Terrain | Armor` cast into two and folding by min preserves its
/// semantics exactly: parry returned the nearest hit across both layers, and the predicate was
/// vacuous on terrain (nothing on the `Terrain` layer has `VolumeOf` ancestry).
fn cast_march_segment(
    spatial: &SpatialQuery,
    grid: Option<&HeightGrid>,
    origin: Vec3,
    dir: Dir3,
    max: f32,
    not_own: &dyn Fn(Entity) -> bool,
) -> Option<SegmentHit> {
    let armor = spatial.cast_ray_predicate(
        origin,
        dir,
        max,
        true,
        &SpatialQueryFilter::from_mask(Layer::Armor),
        not_own,
    );
    let terrain: Option<(f32, Vec3)> = match grid {
        Some(grid) => grid
            .cast_ray(origin, Vec3::from(dir), max)
            .map(|hit| (hit.t, hit.normal)),
        None => spatial
            .cast_ray(
                origin,
                dir,
                max,
                true,
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            )
            .map(|hit| (hit.distance, hit.normal)),
    };
    match (armor, terrain) {
        (Some(armor), Some((distance, normal))) if distance < armor.distance => {
            Some(SegmentHit::Terrain { distance, normal })
        }
        (Some(armor), _) => Some(SegmentHit::Armor(armor)),
        (None, Some((distance, normal))) => Some(SegmentHit::Terrain { distance, normal }),
        (None, None) => None,
    }
}

/// First contact along one march segment, with ARMOUR probed as the caliber-wide disc it is.
///
/// The axis alone is not first contact: §13.5's shell meets the world as a disc, so a round whose
/// centre threads a gap while its rim clips a plate MUST resolve — otherwise the whole η law is
/// unreachable in the live march and every graze silently becomes a clean miss.
///
/// Terrain stays axis-only. The ground is one continuous surface that stops the round outright; there
/// is no η to grade and no union to integrate, and [`HeightGrid::cast_ray`] is the deterministic
/// caster the sim depends on.
///
/// Cost posture: one broad-phase AABB traversal decides whether ANY armour is near the corridor. In
/// open air — where a shell spends nearly all of its life — that is the whole armour cost, cheaper
/// than the ray cast it replaces. The k sample casts are paid only within reach of geometry, and only
/// by rounds big enough for the ring to mean anything (see `resolve::DISC_MIN_CALIBER`).
fn cast_disc_segment(
    world: &ProjectileMarchWorld,
    // Each sample's start relative to `origin`, the axis sample's (`offsets[0]` is zero).
    offsets: &[Vec3],
    origin: Vec3,
    dir: Dir3,
    max: f32,
    not_own: &dyn Fn(Entity) -> bool,
) -> Option<SegmentHit> {
    // The broad phase pads by the farthest start, which a staggered continuation puts further off
    // the axis than the radius.
    let reach = offsets
        .iter()
        .map(|offset| offset.length())
        .fold(0.0, f32::max);
    let armor = if offsets.len() <= 1 {
        world.spatial.cast_ray_predicate(
            origin,
            dir,
            max,
            true,
            &SpatialQueryFilter::from_mask(Layer::Armor),
            not_own,
        )
    } else {
        let mut near_armor = false;
        world.spatial.aabb_intersections_with_aabb_callback(
            collect::swept_aabb(origin, Vec3::from(dir), max, reach),
            |entity| {
                // ACCUMULATE, never assign. Avian 0.7 walks its collider trees with `for_each`, so a
                // `false` return ends only the CURRENT tree's traversal — the remaining trees are
                // walked regardless. An assignment therefore lets whatever is visited after the
                // armour candidate overwrite the answer, and the static tree's terrain heightfield,
                // whose AABB is the whole world box, is in every corridor there is.
                near_armor |= world.armour(entity).is_some() && not_own(entity);
                // Stop this tree at the first candidate: this asks IF, not WHICH.
                !near_armor
            },
        );
        if near_armor {
            let mut nearest: Option<avian3d::prelude::RayHitData> = None;
            for &offset in offsets {
                let hit = world.spatial.cast_ray_predicate(
                    origin + offset,
                    dir,
                    max,
                    true,
                    &SpatialQueryFilter::from_mask(Layer::Armor),
                    not_own,
                );
                if let Some(hit) = hit
                    && nearest.is_none_or(|best| hit.distance < best.distance)
                {
                    nearest = Some(hit);
                }
            }
            nearest
        } else {
            None
        }
    };
    let terrain: Option<(f32, Vec3)> = match world.grid.as_deref() {
        Some(grid) => grid
            .cast_ray(origin, Vec3::from(dir), max)
            .map(|hit| (hit.t, hit.normal)),
        None => world
            .spatial
            .cast_ray(
                origin,
                dir,
                max,
                true,
                &SpatialQueryFilter::from_mask(Layer::Terrain),
            )
            .map(|hit| (hit.distance, hit.normal)),
    };
    match (armor, terrain) {
        (Some(armor), Some((distance, normal))) if distance < armor.distance => {
            Some(SegmentHit::Terrain { distance, normal })
        }
        (Some(armor), _) => Some(SegmentHit::Armor(armor)),
        (None, Some((distance, normal))) => Some(SegmentHit::Terrain { distance, normal }),
        (None, None) => None,
    }
}

/// Whether a spent shell freezes in place — keeping its stuck mesh, tracer, and penetration marks
/// for inspection — instead of despawning. The game despawns (default); the sandbox opts in.
#[derive(Resource, Default)]
pub struct RetainSpentShells(pub bool);

/// How the shell march is integrated. The game uses `Real`; the sandbox can toggle to `Demo`.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarchMode {
    /// On the fixed server-rate step (`Time<Fixed>`) — the true simulation cadence, so slow-motion
    /// shows the actual discrete hops.
    #[default]
    Real,
    /// Per-frame on virtual time (`Time<Virtual>`) — smooth continuous motion for demoing
    /// (frame-rate dependent; the velocity, hence penetration, is unchanged).
    Demo,
}

fn march_real(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Real
}

fn march_demo(mode: Res<MarchMode>) -> bool {
    *mode == MarchMode::Demo
}

/// Firing tank and weapon slot. Included in the initial shell bundle for self-exclusion and server
/// fire attribution.
#[derive(Clone, Copy, Component)]
pub struct ShotSource {
    /// The tank root the shell was fired from.
    pub tank: Entity,
    /// The firing weapon's slot in `TankSim::weapons` — its spawn-time `WeaponIndex`.
    pub weapon: usize,
}

/// Network shot identity supplied in the initial shell bundle.
#[derive(Component, Clone, Copy)]
pub(crate) struct Shot(pub ShotId);

/// Whether a [`FireShell`] was locally authored or reconstructed from the fire stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireShellOrigin {
    /// `shooting` or a local sandbox raised the event.
    Local,
    /// `net::client::receive_fire_events` rebuilt the event from a received [`FireEvent`](crate::net::protocol::FireEvent).
    Reconstructed,
}

/// Hidden replica shell waiting for an authority bounce or terminal.
///
/// Invariant: re-age from `ShotClock - bounce_tick` when available; `waited` counts only
/// forward ticks after this client created the hold. See ADR-0021.
#[derive(Component)]
pub(crate) struct Held {
    /// Fixed ticks spent actually waiting for a verdict after this client created the hold.
    waited: u32,
    /// Re-age fallback when [`ShotClock`] is unavailable.
    age: u32,
    /// Local contact normal for the first sanctioned bounce after this hold re-seeds; `None` for a
    /// hold with no struck face (a fresh start that could not be certified, [`uncertified_start`]).
    normal: Option<Vec3>,
}

/// Configured replica grace window. It bounds the time an unresolved armor contact remains hidden;
/// traces record the configured value and observed holds for tuning.
pub(crate) const RICOCHET_HOLD_TICKS: u32 = 16;

/// Configured margin before a replica consumes a known outcome that its local path missed.
pub(crate) const OVERDUE_MARGIN_TICKS: u32 = 6;

/// Authority ricochet for a keyed shot.
#[derive(Event)]
pub(crate) struct ShellRicochet {
    pub shot: ShotId,
    pub origin: Vec3,
    pub direction: Vec3,
    pub speed: f32,
    pub sequence: u32,
}

/// Authority armor terminal for a keyed shot.
#[derive(Event)]
pub(crate) struct ShellTerminal {
    pub shot: ShotId,
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal.
    pub normal: Vec3,
    /// The server's verdict at the struck face (its `entered()` gates the flame lick on the client, as
    /// in the local read).
    pub verdict: TerminalVerdict,
    /// Ricochets resolved before this terminal.
    pub after_bounces: u32,
}

/// Authority-only report that a keyed shot first lowered an HP pool. [`DamageReport`] emits it at
/// most once; the private wire receipt intentionally carries no HP amount.
#[derive(Event)]
pub(crate) struct ShellDamage {
    pub shot: ShotId,
    pub amount: u32,
}

/// Trigger-agnostic shell spawn seam used by guns and sandbox tools.
#[derive(Event)]
pub struct FireShell {
    pub origin: Vec3,
    pub direction: Dir3,
    pub speed: f32,
    /// Shell calibre (m), used for overmatch and spall-hole size.
    pub caliber: f32,
    /// Projectile mass (kg).
    pub mass: f32,
    /// Nose factor (dimensionless, `0 < N ≤ 1`), read by the capability law.
    pub nose_factor: f32,
    /// Wound rate: reference-mm of dose per metre of chord through a witness (ADR-0040).
    pub wound: f32,
    /// Fire mechanism at the source.
    pub mechanism: crate::spec::FireMechanism,
    /// Firing source for self-exclusion and authority attribution.
    pub shooter: Option<ShotSource>,
    /// Whether this round has a tracer visual; flight and collision are unaffected.
    pub tracer: bool,
    /// Whether this event was locally authored or reconstructed from the network.
    pub shot_origin: FireShellOrigin,
    /// Free-flight ticks to apply at spawn for a reconstructed remote shell.
    pub catch_up_ticks: u32,
    /// Network identity; `None` is valid for authority and sandbox shells.
    pub shot: Option<ShotId>,
}

/// A shell in flight. Kinematic — integrated by hand, no physics engine.
#[derive(Component)]
#[require(ShotDose)]
pub(crate) struct Projectile {
    velocity: Vec3,
    /// The round's mechanism, bound at spawn from its fire data and the substance registry
    /// ([`mechanism::SolidShot::bind`]) — data, never an asset.
    round: mechanism::SolidShot,
    /// Reference-mm of dose per metre of chord the round's line cuts through a witness (ADR-0040).
    /// Read only by the witness pass; the hard walk never sees it.
    wound: f32,
    /// Quadratic-drag coefficient (1/m), from the shell's sectional density at spawn (see [`drag_k`]).
    drag_k: f32,
    /// The basis the §13.5 sample ring is laid out in — SIM state, not a view detail: it decides
    /// where the disc's rays go and therefore what the shell hits.
    ///
    /// Anchored at spawn from the fire direction ([`DiscFrame::anchored`]) and parallel-transported
    /// on every direction change afterwards — gravity's per-tick bend, normalization, a ricochet.
    /// Rebuilding it from the current direction instead would snap the sample pattern mid-flight the
    /// moment the direction crossed the anchoring branch.
    ///
    /// Deterministic from spawn inputs alone, so a replica reconstructs the identical frame from the
    /// same `FireShell`; nothing about it rides the wire.
    disc: walk::DiscFrame,
    /// Where each sample's next sweep starts and what it is inside there ([`resolve::Interior`]) —
    /// SIM state, carried from one step to the next so a tick boundary never re-lays the disc.
    ///
    /// `None` at spawn, in every spawn path: the first step lays the disc on [`disc`](Self::disc)
    /// at the muzzle. Each step then leaves the starts it ended on. A sanctioned catch-up clears
    /// them: it moved the round without sweeping its samples, so the next step lays a fresh disc
    /// and establishes its membership ([`resolve::certified`]); nothing about the catch-up's own
    /// path is certified. Never on the wire: a replica derives its own from the same spawn and
    /// march.
    continuation: Option<resolve::Interior>,
}

impl Projectile {
    /// The round's caliber (m) — the same value [`Impact::caliber`] carries, read by view code that
    /// sizes an effect off a shell in flight rather than off a landed impact.
    pub(crate) fn caliber(&self) -> f32 {
        self.round.body().caliber
    }

    /// What the capability law reads off this round.
    fn body(&self) -> RoundBody {
        self.round.body()
    }

    /// The round's current speed (m/s) — read by view code that scales an effect off a shell in
    /// flight (the tracer streak's nominal length).
    pub(crate) fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// A shell of `caliber` at rest, for view-layer tests that need a projectile ENTITY and none of
    /// the flight state (the view never marches one). Not compiled into a shipped binary.
    #[cfg(test)]
    pub(crate) fn view_test_round(caliber: f32) -> Self {
        Self {
            velocity: Vec3::ZERO,
            round: bound(RoundBody {
                mass: 1.0,
                caliber,
                nose_factor: 0.3,
            }),
            wound: 30.0,
            drag_k: 0.0,
            disc: walk::DiscFrame {
                u: Vec3::X,
                v: Vec3::Y,
            },
            continuation: None,
        }
    }
}

/// Per-shell latch: one [`ShellDamage`] report per damaging shot.
///
/// Invariant: created with the projectile, never attached after replication. See ADR-0014.
#[derive(Component, Default)]
struct DamageReport(bool);

/// Per-shell latch: one [`ShellTerminal`] report per shot.
///
/// Invariant: created with the projectile, never attached after replication. See ADR-0014.
#[derive(Component, Default)]
struct TerminalReport(bool);

/// The shell's flight path, accumulated one point per step.
#[derive(Component, Default)]
pub struct ShellPath {
    pub points: Vec<Vec3>,
    /// Point indices that begin disconnected authority-corrected view segments; index zero is implicit.
    pub segment_starts: Vec<usize>,
}

impl ShellPath {
    /// Start a disconnected segment before the next appended point. Duplicate/empty starts are
    /// suppressed so every entry names a real point once it is appended.
    fn begin_segment(&mut self) {
        let start = self.points.len();
        if start > 0 && self.segment_starts.last().copied() != Some(start) {
            self.segment_starts.push(start);
        }
    }
}

/// A ballistic volume: a solid the penetrator marches *through*, taxing it over the geometric
/// line-of-sight distance (the unified primitive — armor plates and modules alike, design doc §2).
/// On the `Armor` layer.
///
/// Both fields come from the SUBSTANCE the model's primitives wear (§12, classifier precedent
/// 2026-08-07): `bake` resolves the material datablock name against `assets/materials/materials.ron`
/// once, at extraction, so the walk never does a string lookup per query and no per-node number is
/// authored anywhere.
#[derive(Component)]
pub struct BallisticVolume {
    /// The substance's identity and its §13.2 field value (reference-mm of armour per metre of
    /// chord).
    pub material: walk::VolumeMaterial,
    /// The substance's registry name — identity, for diagnostics/inspection and the paint livery.
    /// Never parsed for behaviour.
    #[allow(
        dead_code,
        reason = "carried from the bake for the x-ray readout and the paintable-livery pass; the \
                  walk reads only the factor, deliberately"
    )]
    pub substance: String,
}

/// A witness volume (ADR-0040): a crew box. Not matter — it is no layer of the armour walk, never
/// resists the shell, never shadows a spall direction and never throws one. It is wounded by what
/// the hard world sends through it ([`wound`]). Its colliders carry [`WitnessSurface`] and sit on
/// [`crate::Layer::Witness`]; it carries a [`ComponentHealth`] pool like any box.
#[derive(Component)]
pub struct WitnessVolume {
    /// The substance's registry name — identity, for diagnostics and inspection.
    #[allow(
        dead_code,
        reason = "carried from the bake for the x-ray readout; no law reads it"
    )]
    pub substance: String,
}

/// A witness volume's collider: excluded from every armour broad phase, read by the witness pass.
#[derive(Component)]
pub struct WitnessSurface;

/// The bake's manifold certificate, carried onto the collider entity so the corridor collector can
/// name the surface and the welded feature a crossing came from (`walk::ShellKey`,
/// `walk::Contact`).
///
/// Both tables are index-aligned with the trimesh's own triangle order, which is the order the
/// buffers were handed to `Collider::trimesh_with_config`: `MERGE_DUPLICATE_VERTICES` re-indexes
/// vertices and never reorders or drops a face. `Arc` because every spawned tank shares one bake.
#[derive(Component)]
pub struct BallisticSurfaces {
    /// Closed shell per triangle.
    pub shells: Arc<[u32]>,
    /// Welded vertex ids per triangle corner.
    pub corners: Arc<[[u32; 3]]>,
}

/// Role tags layered on a ballistic volume for the sandbox's visibility passes: armor plates vs
/// internal components (modules / crew / ammo). Attached at bind alongside `BallisticVolume`; the
/// game ignores them.
#[derive(Component)]
pub struct ArmorVolume;

/// Armour outside the main armour ([`crate::spec::VolumeSpec::External`]), on every entity that
/// carries the volume's [`BallisticVolume`]. The walk never reads it; the pen cam's shot verdict
/// does not count perforating it.
#[derive(Component)]
pub struct ExternalArmour;

#[derive(Component)]
pub struct ComponentVolume;

/// A component's HP pool (crew/module/ammo), in whole HP points ([`HP_PER_REFERENCE_MM`]). A shot
/// takes from it its dose rounded once per box per shot ([`ShotDose`]): what the main penetrator
/// spent crossing it, a bounce's shock, and each spall cone direction entering it (design §§5–6).
/// `current` stops at 0; the *consequences* of reaching 0 (cookoff, crew death, knock-out) are later
/// increments (§§7–8).
#[derive(Component)]
pub struct ComponentHealth {
    pub current: u32,
    pub max: u32,
}

/// One crossing of a ballistic volume by the penetrator: where it entered and exited the solid.
/// `(exit - entry).length()` is the geometric line-of-sight thickness — slope captured by geometry,
/// no cosine term (design doc §2).
pub struct PenetrationEvent {
    pub entry: Vec3,
    pub exit: Vec3,
    /// Whether this crossing was an overmatch (calibre ≫ plate thickness): ricochet suppressed,
    /// slope largely cancelled.
    pub overmatched: bool,
}

/// The volume crossings a shell has made this flight — what the sandbox draws to inspect the march.
/// Public, like `ShellPath`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct PenetrationMarks {
    pub events: Vec<PenetrationEvent>,
    /// Points where the shell ricocheted off a too-oblique face (deflected, did not enter).
    pub ricochets: Vec<Vec3>,
    /// Every armour event the authority's march resolved, in march order: the whole path a pen cam
    /// record is read from. A replica's march decides no armour and logs nothing.
    pub log: Vec<ArmourMark>,
}

/// One armour event of a shell's flight, as the authority's march resolved it. Positions WORLD,
/// speeds m/s.
#[derive(Clone, Debug)]
pub enum ArmourMark {
    /// Deflected off a face without entering it.
    Bounce {
        position: Vec3,
        normal: Vec3,
        incoming: Vec3,
        outgoing: Vec3,
        speed_in: f32,
        speed_out: f32,
        /// The volume owning the struck face.
        struck: Entity,
        /// HP the bounce jarred into `struck` before its floor; armour has no pool and takes none.
        shock: f32,
    },
    /// Entered the material: came to rest inside it or out the far side.
    Crossing {
        entrance: Vec3,
        normal: Vec3,
        incoming: Vec3,
        speed_in: f32,
        overmatched: bool,
        /// Every volume the line put a chord into, in volume order.
        volumes: Vec<VolumeCrossing>,
        end: CrossingEnd,
        /// The spall cones this crossing threw: the next `cones` of the shell's [`SpallMarks`].
        cones: usize,
    },
    /// A walk that failed closed: the round stopped at contact with no crossing resolved.
    Failed,
    /// The round's line through witness volumes (ADR-0040) along the legs it flew since the last
    /// mark, logged before the mark of the decision whose legs they are: each witness's disc-mean
    /// chord and its dose `chord × wound` as `cost`. No event of the flight; it moves nothing.
    Wound {
        /// Direction of travel and speed (m/s) on the first leg that met a witness.
        incoming: Vec3,
        speed: f32,
        /// Every witness the line put a chord into, in order of first entry.
        volumes: Vec<VolumeCrossing>,
    },
}

/// How a crossing ended.
#[derive(Clone, Copy, Debug)]
pub enum CrossingEnd {
    Rest { at: Vec3 },
    Exit { at: Vec3, speed: f32 },
}

/// How far a crossing's line got through one volume.
#[derive(Clone, Copy, Debug)]
pub struct VolumeCrossing {
    pub volume: Entity,
    /// Where the line entered it and where it left it or came to rest in it, on the crossing's axis
    /// (WORLD).
    pub entry: Vec3,
    pub end: Vec3,
    /// Disc-mean chord (m).
    pub chord: f32,
    /// The line's dose to it (reference-mm): chord × factor for hard matter, chord × the round's
    /// wound rate for a witness ([`ArmourMark::Wound`]).
    pub cost: f32,
    /// The whole disc came out of it.
    pub perforated: bool,
}

/// One direction of a spall cone, as marched: where it stopped (its strength exhausted, or where it
/// left the struck tank), and whether it dosed a box on the way.
pub struct SpallRay {
    pub end: Vec3,
    pub deposited: bool,
    /// The strength (reference-mm) the direction was thrown with: the cone's `S` times its profile.
    pub strength: f32,
    /// The doses it addressed (volume, reference-mm), in entry order.
    pub doses: Vec<(Entity, f32)>,
}

/// One spall cone — the field thrown from a body exit (design doc §5): origin, axis and half-angle
/// describe its shape; `spend`, the speeds and `strength` are its source's
/// ([`walk::SpallSource`]); `rays` are its directions as marched, in stratum order.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the source parameters are read only by the field-casts pin, which digests them"
    )
)]
pub struct SpallCone {
    pub origin: Vec3,
    pub axis: Dir3,
    pub half_angle: f32,
    /// Reference-mm the round spent across the body it left.
    pub spend: f32,
    /// m/s where the round entered the body.
    pub entry_speed: f32,
    /// m/s where it left it.
    pub exit_speed: f32,
    /// The on-axis strength `S` (reference-mm) the directions were thrown with.
    pub strength: f32,
    pub rays: Vec<SpallRay>,
}

/// The spall a shell has thrown this flight — one cone per body exit it reached. Public like
/// `PenetrationMarks`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct SpallMarks {
    pub cones: Vec<SpallCone>,
    /// Body exits reached whose cone was below the HP quantum and not thrown.
    pub gated: usize,
}

/// Live per-shell readout for the sandbox's info layer — current speed (m/s) and remaining
/// penetration capability (reference-mm). Public; refreshed each step.
#[derive(Component, Default)]
pub struct ShellReadout {
    pub speed: f32,
    pub capability: f32,
}

/// Visual calibre boundary shared by shell spawning and view dressing.
pub(crate) const TRACER_MAX_CALIBER: f32 = 0.02;

/// Catch-up age beyond which cosmetic muzzle/impact reads are suppressed. Shared with muzzle VFX;
/// authority damage is unaffected.
pub(crate) const STALE_FIRE_TICKS: u32 = 16;

/// This round carries a tracer — a fact of the BELT it came out of ([`FireShell::tracer`]), and so a
/// simulation fact: it says what the round IS, not that anything is drawn. Flight and collision are
/// identical with or without it; only [`view`] reads it, to decide whether to hang a streak.
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct TracerRound;

/// The impact surface class, resolved from ballistic-volume ancestry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImpactSurface {
    Terrain,
    Armor,
}

/// What the round did at the surface it struck: the public outcome of an impact.
///
/// Invariant: `Ricochet` is exactly a resolved deflection off armour, and only it carries a
/// deflected direction (the local [`ImpactOutcome`] makes that structural). `Struck`
/// is a stop at the face with no crossing resolved: every terrain strike, and an armour contact
/// that ends without a crossing (a fail-closed walk, an unkeyed replica's neutral spark, the
/// short-circuit probe). `Stopped` entered the material and ended inside it; `Through` came out the
/// far side. `Struck` on armour is a walk that failed closed, which opens no pen cam
/// ([`crate::damage_cam::ShotVerdict`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Verdict {
    Through,
    Stopped,
    Ricochet,
    Struck,
}

impl Verdict {
    /// The round entered the material: it stopped inside it or went through.
    pub fn entered(self) -> bool {
        matches!(self, Self::Stopped | Self::Through)
    }
}

/// What the round did at the surface it struck, carrying the outgoing direction exactly when there
/// is one. The public unit [`Verdict`] is derived from it ([`verdict`](Self::verdict)), so a
/// deflection without a ricochet, or a ricochet without a direction, cannot be written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum ImpactOutcome {
    Through,
    Stopped,
    Struck,
    /// Deflected off armour, leaving along `direction`.
    Ricochet {
        direction: Vec3,
    },
}

impl ImpactOutcome {
    pub(crate) fn verdict(self) -> Verdict {
        match self {
            Self::Through => Verdict::Through,
            Self::Stopped => Verdict::Stopped,
            Self::Struck => Verdict::Struck,
            Self::Ricochet { .. } => Verdict::Ricochet,
        }
    }

    /// The deflected direction, for a ricochet.
    pub(crate) fn deflection(self) -> Option<Vec3> {
        match self {
            Self::Ricochet { direction } => Some(direction),
            _ => None,
        }
    }
}

/// How an armour terminal ended: the round entered the material and stayed in it or came out.
///
/// A terminal is never a bounce — a ricochet is a [`ShellRicochet`], and the march goes on — so the
/// type has no such variant. Wire receipt validates into it ([`TryFrom<Verdict>`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TerminalVerdict {
    Through,
    Stopped,
}

impl TerminalVerdict {
    /// Every terminal entered the material; kept so trace call sites read as the public verdict does.
    pub(crate) fn entered(self) -> bool {
        Verdict::from(self).entered()
    }
}

impl From<TerminalVerdict> for Verdict {
    fn from(terminal: TerminalVerdict) -> Self {
        match terminal {
            TerminalVerdict::Through => Verdict::Through,
            TerminalVerdict::Stopped => Verdict::Stopped,
        }
    }
}

impl From<TerminalVerdict> for ImpactOutcome {
    fn from(terminal: TerminalVerdict) -> Self {
        match terminal {
            TerminalVerdict::Through => ImpactOutcome::Through,
            TerminalVerdict::Stopped => ImpactOutcome::Stopped,
        }
    }
}

impl TryFrom<Verdict> for TerminalVerdict {
    type Error = Verdict;

    /// `Err` carries a verdict no terminal can have: `Ricochet` or `Struck`.
    fn try_from(verdict: Verdict) -> Result<Self, Verdict> {
        match verdict {
            Verdict::Through => Ok(Self::Through),
            Verdict::Stopped => Ok(Self::Stopped),
            other => Err(other),
        }
    }
}

/// A replica drew the authority outcome `ordinal` of its keyed shell `shot`: its bounce of that
/// [`ShellRicochet::sequence`], or its end, whose ordinal is its [`ShellTerminal::after_bounces`].
/// The ordinals of one shot's outcomes are distinct, so each names the one impact it drew. Raised
/// beside the outcome's [`Impact`], which names no shot.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct AuthorityOutcomeDrawn {
    pub(crate) shot: ShotId,
    pub(crate) ordinal: u32,
}

/// A local shell impact consumed by simulation and view observers.
#[derive(Event)]
pub(crate) struct Impact {
    pub(crate) position: Vec3,
    /// Outward surface normal from the raycast; consumers normalize with a fallback.
    pub(crate) normal: Vec3,
    /// Striking round caliber (m), used by impact visuals.
    pub(crate) caliber: f32,
    /// Surface class resolved from volume ancestry.
    pub(crate) surface: ImpactSurface,
    /// What the round did at the struck surface; [`ImpactOutcome::Struck`] for terrain.
    pub(crate) outcome: ImpactOutcome,
}

/// Tags a debug impact marker for view observers.
#[derive(Component)]
pub struct ImpactMarker;

/// Default-off A/B cost probe that stops sub-20 mm rounds at their first surface.
///
/// Invariant: it does not apply to main-gun rounds and is never enabled by default.
#[derive(Resource, Clone, Copy, Default)]
pub struct MgShortCircuit(pub bool);

/// Caliber ceiling (m) for [`MgShortCircuit`].
const MG_SHORTCIRCUIT_CALIBER_MAX: f32 = 0.020;

/// The SIMULATION half: the flight, the armor resolution, and the impact seam. It opens no asset,
/// builds no mesh or material, and writes no `Visibility` — a shell born here is a bare simulation
/// entity, and [`view::plugin`] (client roots only) is what dresses one.
pub fn sim_plugin(app: &mut App) {
    // The spall cone's shape is authored data: parsed and validated here, while the app is built,
    // so a malformed file stops startup rather than the first throw.
    spall_cone::ConeShape::shipped();
    app.init_resource::<RetainSpentShells>()
        .init_resource::<MarchMode>()
        .insert_resource(MgShortCircuit(crate::env_flag(
            "SPIKE_MG_SHORTCIRCUIT",
            false,
        )))
        .add_observer(on_fire_shell)
        .add_observer(on_impact)
        // The same march, integrated on whichever clock the mode selects: `Real` on the fixed
        // server step (`Res<Time>` is `Time<Fixed>` here), `Demo` per-frame on virtual time
        // (`Res<Time>` is `Time<Virtual>` here). One reads as the true sim, the other as smooth.
        .add_systems(
            FixedUpdate,
            integrate_projectiles
                .in_set(GameplaySet)
                .in_set(SimPhase::ProjectileMarch)
                .before(DamageConsequences)
                .run_if(march_real),
        )
        .add_systems(
            Update,
            integrate_projectiles.in_set(GameplaySet).run_if(march_demo),
        );
}

/// Spawn a shell from `FireShell`, using fixed-tick catch-up when requested.
///
/// Invariant: each skipped chord is checked. Terrain may end locally; a keyed replica armor contact
/// becomes a [`Held`] stop until an authority outcome arrives. Replica ballistics never decides armor.
fn on_fire_shell(
    fire: On<FireShell>,
    // The FIXED timestep, NOT `Res<Time>`: this observer can fire from `Update` (the net client
    // re-raises `FireShell` at render rate), where `Res<Time>` is `Time<Virtual>` (a render-frame dt).
    // The catch-up counts fixed SERVER ticks, so it must step the fixed timestep the live march also
    // uses in `Real` mode. Unused when `catch_up_ticks == 0` (the loop never runs).
    fixed_time: Res<Time<Fixed>>,
    // The catch-up contact scan below; inert for a local shell (`catch_up_ticks == 0`).
    spatial: SpatialQuery,
    // Volume ancestry, to classify a catch-up candidate as armor vs terrain by the same rule the live
    // march uses (`hit_ancestor`). Cheap to thread through; read only on a catch-up contact.
    volumes: Query<&BallisticVolume>,
    // Volume OWNERSHIP, for the shooter self-exclusion the already-landed test needs (see
    // [`not_own_volume`]): a muzzle that sits inside its own tank's geometry — the coax, whose
    // recoiling barrel retracts its muzzle behind the STATIC mantlet on every round after the first —
    // would otherwise report "already landed" 1 cm out and swallow the shell whole.
    owners: Query<&VolumeOf>,
    parents: Query<&ChildOf>,
    // The heightmap ground, when the heightmap world is live: catch-up terrain contacts read the
    // exact deterministic caster, not parry (see [`cast_march_segment`]). Absent on the flat
    // fallback world, where the parry terrain cast remains.
    grid: Option<Res<HeightGrid>>,
    // The shared network protocol's net-neutral tick — the tick every cosmetic shell lives at, and
    // the one the shot-lifecycle recorder stamps its rows with. Absent on SP / sandbox, where an
    // unstamped row's tick is 0.
    shot_clock: Option<Res<ShotClock>>,
    // The shot-lifecycle recorder (`SPIKE_SHOT_TRACE`): absent unless armed, so an unrecorded run pays
    // one `Option` check per shot. `FireShellOrigin` preserves local-vs-reconstructed attribution;
    // `ClientReplica` distinguishes the two locally authored roles (`own` vs `auth`).
    replica: Option<Res<ClientReplica>>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    // The pen cam recorder, where one runs; it records the authority's main-gun rounds only.
    recorder: Option<Res<ShotRecorder>>,
    weapons: Query<(
        &crate::tank::Weapon,
        &crate::tank::WeaponIndex,
        &crate::tank::TankRoot,
    )>,
    mut commands: Commands,
) {
    let now = shot_clock.as_deref().map_or(0, |clock| clock.0);
    if fire.catch_up_ticks > MAX_COSMETIC_CATCH_UP_TICKS {
        warn!(
            catch_up_ticks = fire.catch_up_ticks,
            "ballistics: rejected FireShell catch-up beyond the cosmetic horizon"
        );
        if let Some(shot) = fire.shot {
            crate::shot_trace::record(
                &mut shot_trace,
                "end",
                now,
                shot,
                || json!({ "why": "catch_up_reject" }),
            );
        }
        return;
    }
    let drag = drag_k(fire.caliber, fire.mass);
    let dt = fixed_time.timestep().as_secs_f32();
    let (mut position, velocity, mut points) = fast_forward_shell(
        fire.origin,
        fire.direction * fire.speed,
        drag,
        dt,
        fire.catch_up_ticks,
    );

    // A keyed armor candidate crossed during catch-up becomes a hidden hold, not an observer-authored
    // impact. It is calculated before spawning so the shell is born complete.
    let mut catch_up_hold = None;

    // Net catch-up only: walk the exact per-fixed-tick free-flight chords `fast_forward_shell` returned,
    // rather than one muzzle-to-present chord. The live march casts those same stepped segments, so static
    // terrain catch-up agrees with normal flight even when gravity makes the multi-tick arc visibly curved.
    // A pose-dependent armor hit is only a CANDIDATE on a client: keep a keyed shell hidden there until
    // the authority sends a bounce/terminal, instead of emitting a local impact and destroying the only
    // consumer that could carry a ricochet through.
    if fire.catch_up_ticks > 0 {
        // DERIVED numerical guard: match the live march's 1 mm boundary nudge so catch-up casts
        // neither begin inside the muzzle surface nor end by re-touching the next chord boundary.
        const EPS: f32 = 1.0e-3;
        // The shooter's own volumes are transparent to its own round — the same rule the live march
        // applies (see [`not_own_volume`]).
        let shooter = fire.shooter.map(|source| source.tank);
        let not_own = |entity: Entity| not_own_volume(entity, shooter, &owners, &parents);
        for (segment_index, segment) in points.windows(2).enumerate() {
            let step = segment[1] - segment[0];
            let Ok(dir) = Dir3::new(step) else {
                continue;
            };
            let reach = (step.length() - EPS).max(0.0);
            if let Some(hit) = cast_march_segment(
                &spatial,
                grid.as_deref(),
                segment[0] + Vec3::from(dir) * EPS,
                dir,
                reach,
                &not_own,
            ) {
                // An Armor-layer hit without volume ancestry stays classified terrain — the
                // exact rule the live march's `resolved` branch applies.
                let (distance, normal, surface) = match &hit {
                    SegmentHit::Armor(hit) => (
                        hit.distance,
                        hit.normal,
                        if hit_ancestor(hit.entity, &volumes, &parents).is_some() {
                            ImpactSurface::Armor
                        } else {
                            ImpactSurface::Terrain
                        },
                    ),
                    SegmentHit::Terrain { distance, normal } => {
                        (*distance, *normal, ImpactSurface::Terrain)
                    }
                };
                let contact = segment[0] + Vec3::from(dir) * (EPS + distance);

                if surface == ImpactSurface::Armor && fire.shot.is_some() {
                    // Preserve only the honest pre-contact trail. `segment_index` starts at p0→p1, so
                    // retaining `segment_index + 1` points keeps p0..p_i, then the exact contact closes
                    // the path. The skipped ticks after that candidate seed the fallback hold counter;
                    // composed net clients re-age from `present - bounce_tick` directly on resolution.
                    points.truncate(segment_index + 1);
                    points.push(contact);
                    position = contact;
                    let contact_tick = segment_index as u32 + 1;
                    catch_up_hold = Some(Held {
                        waited: 0,
                        age: fire.catch_up_ticks.saturating_sub(contact_tick),
                        normal: Some(normal),
                    });
                    if let Some(shot) = fire.shot {
                        crate::shot_trace::record(&mut shot_trace, "catchup", now, shot, || {
                            json!({
                                "res": "armor_hold",
                                "cu": fire.catch_up_ticks,
                                "after": fire.catch_up_ticks.saturating_sub(contact_tick),
                            })
                        });
                    }
                    break;
                }

                // A static terrain hit, or an unkeyed replica armor candidate, ends during catch-up.
                // Stale cosmetic impact reads are suppressed; authority damage is unaffected.
                if fire.catch_up_ticks <= STALE_FIRE_TICKS {
                    commands.trigger(Impact {
                        position: contact,
                        normal,
                        caliber: fire.caliber,
                        surface,
                        outcome: ImpactOutcome::Struck,
                    });
                }
                // The shot's picture ends here without a tracer ever flying — its whole flight fitted
                // inside the catch-up skip. Recorded as an `end`, so the analyzer never counts this
                // shot as a MISSING spawn (it is a legitimate, if late-informed, terminal).
                if let Some(shot) = fire.shot {
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "end",
                        now,
                        shot,
                        || json!({ "why": "catchup_landed", "cu": fire.catch_up_ticks }),
                    );
                }
                return;
            }
        }
    }

    // Travel direction after any catch-up (gravity/drag bend it); fall back to the bore for a
    // degenerate zero velocity so a spent-to-rest catch-up never trips `Dir3`.
    let travel = Dir3::new(velocity).unwrap_or(fire.direction);
    let speed = velocity.length();
    // The sim shell is IDENTICAL for every round — it flies and raycasts the same whether or not it is
    // drawn (a non-tracer MG round still bounces, ricochets, and lands; dead-reckoned streaks were
    // rejected). What the round IS rides along as [`TracerRound`]; whether anything is DRAWN for it is
    // decided by `ballistics::view`, which a server never mounts.
    let shell_base = (
        Projectile {
            velocity,
            round: bound(RoundBody {
                mass: fire.mass,
                caliber: fire.caliber,
                nose_factor: fire.nose_factor,
            }),
            wound: fire.wound,
            drag_k: drag,
            // From the BORE, not from the post-catch-up travel direction: a shell reconstructed with
            // catch-up must anchor its ring exactly where the locally fired one did.
            disc: walk::DiscFrame::anchored(Vec3::from(fire.direction)).unwrap_or(
                walk::DiscFrame {
                    u: Vec3::X,
                    v: Vec3::Y,
                },
            ),
            continuation: None,
        },
        DamageReport::default(),
        TerminalReport::default(),
        ShellPath {
            points,
            segment_starts: Vec::new(),
        },
        PenetrationMarks::default(),
        SpallMarks::default(),
        ShellReadout {
            speed,
            capability: capability(
                RoundBody {
                    mass: fire.mass,
                    caliber: fire.caliber,
                    nose_factor: fire.nose_factor,
                },
                speed,
            ),
        },
        Transform::from_translation(position).looking_to(travel, Vec3::Y),
    );

    // Every component the shell is born with is in this ONE spawn transaction — `view`'s dressing
    // observer reads the finished round (Bevy 0.19 writes the whole bundle before any `Add` observer
    // runs), so a late `.insert` here would hand it an incomplete shell. `Option<T>` is not a Bundle,
    // hence the explicit branches; [`spawn_shell`] folds the tracer and the recording in so they do
    // not multiply.
    let born = Born {
        tracer: fire.tracer,
        recording: (recorder.is_some() && replica.is_none() && ShotRecording::records(&fire)).then(
            || {
                let weapon = fire.shooter.and_then(|source| {
                    weapons
                        .iter()
                        .find(|(_, slot, root)| root.0 == source.tank && slot.0 == source.weapon)
                        .map(|(weapon, ..)| weapon)
                });
                ShotRecording::of(&fire, weapon)
            },
        ),
    };
    match (fire.shot, fire.shooter, catch_up_hold) {
        (Some(shot), Some(source), Some(held)) => {
            spawn_shell(&mut commands, (shell_base, Shot(shot), source, held), born);
        }
        (Some(shot), Some(source), None) => {
            spawn_shell(&mut commands, (shell_base, Shot(shot), source), born);
        }
        (Some(shot), None, Some(held)) => {
            spawn_shell(&mut commands, (shell_base, Shot(shot), held), born);
        }
        (Some(shot), None, None) => {
            spawn_shell(&mut commands, (shell_base, Shot(shot)), born);
        }
        (None, Some(source), Some(held)) => {
            spawn_shell(&mut commands, (shell_base, source, held), born);
        }
        (None, Some(source), None) => spawn_shell(&mut commands, (shell_base, source), born),
        (None, None, Some(held)) => spawn_shell(&mut commands, (shell_base, held), born),
        (None, None, None) => spawn_shell(&mut commands, shell_base, born),
    }

    // Lifecycle trace attribution is explicit: a received `FireEvent` can legitimately reconstruct
    // at the same tick (`catch_up_ticks == 0`), so timing cannot distinguish it from local fire.
    if let Some(shot) = fire.shot {
        let src = match fire.shot_origin {
            FireShellOrigin::Reconstructed => "obs",
            FireShellOrigin::Local if replica.is_some() => "own",
            FireShellOrigin::Local => "auth",
        };
        crate::shot_trace::record(
            &mut shot_trace,
            "spawn",
            now,
            shot,
            || json!({ "src": src, "cu": fire.catch_up_ticks }),
        );
    }
}

/// The optional facts a shell is born with besides its identity and hold.
struct Born {
    /// The belt says the round carries a tracer.
    tracer: bool,
    /// The pen cam recorder records the round.
    recording: Option<ShotRecording>,
}

/// Spawn one complete shell, with [`TracerRound`] and its [`ShotRecording`] folded into the SAME
/// bundle when it has them. Generic over the caller's branch so the eight identity/hold arms above
/// do not multiply — and so those facts are born with the shell rather than inserted after it,
/// which is what lets `view`'s `Add` observer read a finished round.
fn spawn_shell(commands: &mut Commands, shell: impl Bundle, born: Born) {
    match (born.tracer, born.recording) {
        (true, Some(recording)) => commands.spawn((shell, TracerRound, recording)),
        (true, None) => commands.spawn((shell, TracerRound)),
        (false, Some(recording)) => commands.spawn((shell, recording)),
        (false, None) => commands.spawn(shell),
    };
}

/// Optional multiplayer state used by the shared projectile march. Grouping it keeps the march's
/// simulation queries visible without exceeding Bevy's system-parameter arity.
#[derive(SystemParam)]
struct ProjectileMarchNet<'w> {
    replica: Option<Res<'w, ClientReplica>>,
    sanctioned: Option<Res<'w, SanctionedShots>>,
    shot_clock: Option<Res<'w, ShotClock>>,
}

/// The world one shell march probes: armor geometry, the ownership hierarchy that classifies a hit,
/// and the deterministic ground. Grouped as a [`SystemParam`] so the march's phase helpers share one
/// parameter instead of five, exactly as [`ProjectileMarchNet`] groups the net state.
#[derive(SystemParam)]
struct ProjectileMarchWorld<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    volumes: Query<'w, 's, &'static BallisticVolume>,
    owners: Query<'w, 's, &'static VolumeOf>,
    parents: Query<'w, 's, &'static ChildOf>,
    // The heightmap ground, when the heightmap world is live: the march's terrain contacts read
    // the exact deterministic caster, not parry (see [`cast_march_segment`]). Absent on the flat
    // fallback world, where the parry terrain cast remains.
    grid: Option<Res<'w, HeightGrid>>,
    // The cone shape the march throws when a world names one (the cost probe); the shipped shape
    // otherwise.
    spall_law: Option<Res<'w, spall_cone::SpallLaw>>,
    // Witness volumes (ADR-0040) and their colliders: read by the witness pass, excluded from every
    // armour classification.
    witnesses: Query<'w, 's, &'static WitnessVolume>,
    witness_surfaces: Query<'w, 's, (), With<WitnessSurface>>,
}

impl ProjectileMarchWorld<'_, '_> {
    /// The armour volume a collider belongs to: its nearest [`BallisticVolume`] ancestor. `None`
    /// for terrain and for a witness's collider, whatever node a witness is parented under — a
    /// witness is never a layer of the walk (ADR-0040).
    fn armour(&self, collider: Entity) -> Option<(Entity, &BallisticVolume)> {
        if self.witness_surfaces.contains(collider) {
            return None;
        }
        hit_ancestor(collider, &self.volumes, &self.parents)
    }

    /// The volume node a collider belongs to, of either kind: a witness's own node for a witness
    /// collider, the armour volume otherwise. What a "the struck tank's" admission reads.
    fn volume_of(&self, collider: Entity) -> Option<Entity> {
        if self.witness_surfaces.contains(collider) {
            hit_ancestor(collider, &self.witnesses, &self.parents).map(|(node, _)| node)
        } else {
            self.armour(collider).map(|(node, _)| node)
        }
    }
}

/// The one shell a march phase is resolving: its pose, flight state, and the per-flight record every
/// phase appends to. Grouped so each phase takes the shell as a single parameter.
///
/// The component wrappers are kept as [`Mut`] rather than plain `&mut`: a phase that writes nothing
/// then marks nothing changed, exactly as the single fused loop did.
struct MarchingShell<'a, 'w> {
    entity: Entity,
    transform: &'a mut Mut<'w, Transform>,
    projectile: &'a mut Mut<'w, Projectile>,
    terminal_report: &'a mut Mut<'w, TerminalReport>,
    path: &'a mut Mut<'w, ShellPath>,
    marks: &'a mut Mut<'w, PenetrationMarks>,
    readout: &'a mut Mut<'w, ShellReadout>,
    spall: &'a mut Mut<'w, SpallMarks>,
    dose: &'a mut Mut<'w, ShotDose>,
    /// The shell's network identity, when it has one. `None` for SP and sandbox shells.
    shot: Option<&'a Shot>,
    /// The tank that fired this shell, read ONLY to exclude its own volumes from the march.
    source: Option<&'a ShotSource>,
}

/// Where a shell's ray-march ended this tick, and how it ended.
struct MarchStep {
    /// Final position — the point the trail closes on.
    position: Vec3,
    /// Final travel direction; the mesh is re-oriented onto it.
    direction: Dir3,
    /// Final speed (m/s).
    speed: f32,
    /// The shell reached a terminal (terrain, embed, or a consumed authority verdict) and retires.
    stopped: bool,
    /// The shell froze at armor contact awaiting its authority verdict — distinct from `stopped`,
    /// which despawns.
    holding: bool,
    /// HP the authority actually removed this step, across every crossing and every spall fragment.
    damage_dealt: u32,
}

/// What a replica shell does at an armor contact its own march found (see
/// [`resolve_replica_armor_contact`]).
enum ReplicaArmorContact {
    /// A buffered bounce re-seeded the shell from authority truth; the leftover step keeps marching.
    Reseed {
        /// The exact server bounce point the shell restarts from.
        origin: Vec3,
        /// `None` when the sanctioned direction is degenerate — the shell keeps its heading.
        direction: Option<Dir3>,
        speed: f32,
    },
    /// The shot ended at this contact: a buffered terminal, or an unkeyed shell failing closed.
    Stopped,
    /// A keyed shell froze hidden here, waiting for the authority's verdict (or expiry).
    Holding,
}

fn integrate_projectiles(
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &mut Projectile,
        &mut DamageReport,
        &mut TerminalReport,
        &mut ShellPath,
        &mut PenetrationMarks,
        &mut ShellReadout,
        &mut SpallMarks,
        &mut ShotDose,
        // OBSERVER-only, both `Option`: `Shot` is the shell's network identity (keyframe-eligible when
        // present), `Held` marks a shell frozen at armor waiting for its sanctioned bounce keyframe.
        Option<&Shot>,
        Option<&mut Held>,
        // The tank that fired this shell, when it was fired BY one — `on_fire_shell` attaches it from
        // `FireShell::shooter` on every attributed shot (the authority's, the shooter's own predicted
        // round, AND an observer's replica). Read here ONLY to exclude that tank's own volumes from the
        // march ([`not_own_volume`]) — never to deposit anything. `None` for the sandbox's tank-less
        // camera fire, which excludes nothing.
        Option<&ShotSource>,
        // The pen cam recording of a recorded round (authority main-gun rounds only).
        Option<&mut ShotRecording>,
    )>,
    world: ProjectileMarchWorld,
    mut recorder: MarchRecorder,
    // Collider poses + shapes, for the §13 corridor collector: it walks each candidate's geometry
    // itself rather than asking Avian for a nearest hit (see `ballistics::collect`).
    colliders: Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static BallisticSurfaces>,
    )>,
    mut bodies: Query<Forces>,
    mut health: Query<&mut ComponentHealth>,
    retain: Res<RetainSpentShells>,
    net: ProjectileMarchNet,
    // EXPERIMENTAL cost-attribution A/B lever (`SPIKE_MG_SHORTCIRCUIT`, default off — see the type).
    shortcircuit: Res<MgShortCircuit>,
    // Sim-cost recorder attribution sink (`SPIKE_COST_TRACE`): absent unless the recorder is armed, so
    // an unmeasured run pays only the `Option` check. This system's whole wall-time is stamped into it.
    mut cost: Option<ResMut<crate::cost::CostTrace>>,
    // Shot-lifecycle recorder sink (`SPIKE_SHOT_TRACE`): absent unless armed, same `Option` discipline
    // as the cost sink above. Every row below is client-side (`!deposit`) — this is the CONSUMING half
    // of a shot's life (contact → hold → re-seed / terminal / dissolve), the half whose timings size
    // `RICOCHET_HOLD_TICKS`. Authority emissions are recorded in `net::shot_transport`.
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let ProjectileMarchNet {
        replica,
        sanctioned,
        shot_clock,
    } = net;
    // March-cost attribution timer (`SPIKE_COST_TRACE`): only sampled when the recorder is armed, so an
    // unmeasured run never touches the clock. Covers the whole march (query iteration + every cast).
    let march_t0 = cost.as_ref().map(|_| Instant::now());
    let dt = time.delta_secs();
    // Authority = not a replica: only then does a hit actually mutate health here.
    let deposit = replica.is_none();
    let sanctioned = sanctioned.as_deref();
    // The published fixed tick — the clock the overdue-consumption check below compares each
    // sanctioned outcome's server tick against. Absent on SP / sandbox.
    let present = shot_clock.map(|clock| clock.0);
    // The tick every shot-lifecycle row this march writes is stamped with (the tick each cosmetic
    // shell lives at). Never read on the authority — every row site is `!deposit`.
    let now = present.unwrap_or(0);

    for (
        entity,
        mut transform,
        mut projectile,
        mut damage_report,
        mut terminal_report,
        mut path,
        mut marks,
        mut readout,
        mut spall,
        mut dose,
        shot,
        held,
        source,
        recording,
    ) in &mut projectiles
    {
        let mut shell = MarchingShell {
            entity,
            transform: &mut transform,
            projectile: &mut projectile,
            terminal_report: &mut terminal_report,
            path: &mut path,
            marks: &mut marks,
            readout: &mut readout,
            spall: &mut spall,
            dose: &mut dose,
            shot,
            source,
        };

        if let Some(mut held) = held {
            resolve_held_shell(
                &mut shell,
                &mut held,
                sanctioned,
                present,
                now,
                dt,
                &mut shot_trace,
                &mut commands,
            );
            continue;
        }

        if consume_overdue_sanctioned_outcome(
            &mut shell,
            deposit,
            sanctioned,
            present,
            dt,
            now,
            &mut shot_trace,
            &mut commands,
        ) {
            continue;
        }

        let (marks_from, emptied_from) = (shell.marks.log.len(), shell.dose.emptied().len());
        let Some(step) = march_shell_step(
            &mut shell,
            &world,
            &colliders,
            &mut health,
            &mut bodies,
            sanctioned,
            deposit,
            shortcircuit.0,
            dt,
            now,
            &mut shot_trace,
            &mut commands,
        ) else {
            continue;
        };
        if deposit {
            recorder.note(
                entity,
                recording,
                &shell.marks.log,
                marks_from,
                shell.dose,
                emptied_from,
                &world.owners,
                &health,
            );
        }

        // Reorient the shell to its travel direction so the mesh follows the (gravity-curved,
        // ricochet-bent) path instead of holding its launch heading.
        shell.transform.translation = step.position;
        shell.transform.look_to(step.direction, Vec3::Y);
        shell.path.points.push(step.position);

        // A state snapshot cannot preserve how many shots caused its resulting HP. Raise one discrete,
        // shot-attributed fact; `net::shot_transport` routes it reliably to the fire-time owner.
        if deposit
            && step.damage_dealt > 0
            && !damage_report.0
            && let Some(shot) = shot
        {
            commands.trigger(ShellDamage {
                shot: shot.0,
                amount: step.damage_dealt,
            });
            damage_report.0 = true;
        }

        if step.holding {
            // Frozen at armor awaiting the bounce keyframe: keep the entity and its velocity untouched
            // (`resolve_held_shell` drives the wait next tick). The freeze point was just recorded
            // once, so the trail ends cleanly at the plate while the shell holds.
        } else if step.stopped {
            if retain.0 {
                // Sandbox: freeze where it stopped — drop the live components so it is no longer
                // integrated or labelled, but keep the stuck shell, its path, and its penetration
                // marks on screen for study (the sandbox's `c` command clears them).
                commands
                    .entity(entity)
                    .remove::<(Projectile, ShellReadout)>();
            } else {
                // Game: the spent shell is done.
                commands.entity(entity).despawn();
            }
        } else if step.position.y < KILL_FLOOR {
            // Left the world: cleared the map edge and fell into the void below the terrain. Despawn
            // outright — there is no impact to inspect, so this ignores the sandbox's retain (unlike a
            // real impact). This is what bounds a shell that never hits terrain; see `KILL_FLOOR`.
            if !deposit && let Some(shot) = shot {
                crate::shot_trace::record(
                    &mut shot_trace,
                    "end",
                    now,
                    shot.0,
                    || json!({ "why": "kill_floor" }),
                );
            }
            commands.entity(entity).despawn();
        } else {
            shell.projectile.velocity = Vec3::from(step.direction) * step.speed;
            shell.readout.speed = step.speed;
            shell.readout.capability = capability(shell.projectile.body(), step.speed);
        }
    }

    // Attribute this system's whole wall-time to the current fixed tick (`SPIKE_COST_TRACE`). Inert
    // (both `Option`s empty) unless the recorder is armed.
    if let (Some(cost), Some(t0)) = (cost.as_mut(), march_t0) {
        cost.record_march(t0.elapsed().as_secs_f64() * 1.0e6);
    }
}

/// Resolve a shell frozen at armor waiting for the authority's verdict on that contact.
///
/// NET-CLIENT HOLD: a `Shot`-carrying shell — an observer's replica OR the shooter's own predicted
/// round — frozen (and hidden, see the hold insert in [`resolve_replica_armor_contact`]) at armor
/// contact, waiting the grace window for the server's verdict on this contact. It does NOT
/// free-flight while held — it resolves from whichever sanctioned outcome arrives (a bounce keyframe
/// → re-seed and continue; a terminal confirm → the full honest armor read at the server's position)
/// or, past the window, degrades to fail-closed truncation. The authority never holds (it resolves
/// shots for real), and a shell with no `Shot` never enters this state (it fail-closes on first
/// contact instead).
///
/// The shell is fully resolved for this tick either way: the caller never marches it.
fn resolve_held_shell(
    shell: &mut MarchingShell,
    held: &mut Mut<Held>,
    sanctioned: Option<&SanctionedShots>,
    present: Option<u32>,
    now: u32,
    dt: f32,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) {
    let entity = shell.entity;
    let shot = shell.shot;
    // The bounce we are waiting on (the next unconsumed ordinal for this shot), if it arrived.
    let arrived = shot.zip(sanctioned).and_then(|(shot, buf)| {
        buf.next(shot.0, shell.marks.ricochets.len())
            .map(|bounce| (shot.0, buf, bounce))
    });
    if let Some((shot_id, sanctioned, first_bounce)) = arrived {
        // RE-SEED through every authority outcome already buffered up to the present. A pure
        // fast-forward may cover only the interval before the NEXT known bounce/terminal; it
        // must not draw through a verdict the client already possesses.
        let initial_age = present
            .and_then(|present| elapsed_ticks(present, first_bounce.bounce_tick))
            .unwrap_or(held.age);
        let caught_up = match catch_up_sanctioned_chain(
            shot_id,
            shell.marks.ricochets.len(),
            first_bounce,
            present,
            held.age,
            sanctioned,
            shell.projectile.velocity,
            shell.projectile.drag_k,
            dt,
        ) {
            Ok(caught_up) => caught_up,
            Err(reject) => {
                crate::shot_trace::record(
                    shot_trace,
                    "overdue",
                    now,
                    shot_id,
                    || json!({ "res": "reject", "why": reject.trace_reason() }),
                );
                crate::shot_trace::record(
                    shot_trace,
                    "end",
                    now,
                    shot_id,
                    || json!({ "why": "catch_up_reject" }),
                );
                commands.entity(entity).despawn();
                return;
            }
        };
        for (index, segment) in caught_up.segments.iter().enumerate() {
            // The candidate contact and every later authority correction can be spatially
            // displaced. Each server origin begins a disconnected view segment, never a
            // fictional correction chord.
            shell.path.begin_segment();
            shell.path.points.extend(segment.points.iter().copied());
            shell.marks.ricochets.push(segment.bounce.origin);
            commands.trigger(Impact {
                position: segment.bounce.origin,
                normal: held
                    .normal
                    .filter(|_| index == 0)
                    .unwrap_or(segment.bounce.direction),
                caliber: shell.projectile.caliber(),
                surface: ImpactSurface::Armor,
                outcome: ImpactOutcome::Ricochet {
                    direction: segment.bounce.direction,
                },
            });
            commands.trigger(AuthorityOutcomeDrawn {
                shot: shot_id,
                ordinal: segment.bounce.sequence,
            });
            if index == 0 {
                crate::shot_trace::record(shot_trace, "hold", now, shot_id, || {
                    json!({
                        "held": held.waited,
                        "aged": initial_age,
                        "res": "bounce",
                        "seq": segment.bounce.sequence,
                        "bt": segment.bounce.bounce_tick,
                    })
                });
            } else {
                let late = present
                    .and_then(|present| elapsed_ticks(present, segment.bounce.bounce_tick))
                    .unwrap_or_default();
                crate::shot_trace::record(
                    shot_trace,
                    "overdue",
                    now,
                    shot_id,
                    || json!({ "res": "bounce", "late": late, "seq": segment.bounce.sequence, "via": "chain" }),
                );
            }
        }

        if let Some(terminal) = caught_up.terminal {
            finish_at_sanctioned_terminal(shell, &terminal, commands);
            let late = present
                .and_then(|present| elapsed_ticks(present, terminal.impact_tick))
                .unwrap_or_default();
            crate::shot_trace::record(
                shot_trace,
                "overdue",
                now,
                shot_id,
                || json!({ "res": "terminal", "late": late, "pen": terminal.verdict.entered(), "via": "chain" }),
            );
            crate::shot_trace::record(
                shot_trace,
                "end",
                now,
                shot_id,
                || json!({ "why": "terminal" }),
            );
            commands.entity(entity).despawn();
            return;
        }

        // The one contact normal this client has is the hold's own, for the bounce it held on: only
        // a chain ending on that bounce can be lifted off its face, and a hold with no struck face
        // lifts nothing.
        let lift = held.normal.filter(|_| caught_up.segments.len() == 1);
        resume_from_catch_up(shell, &caught_up, lift);
        // Release the hold and resume marching next tick. `Held` is the whole fact — the view's
        // `Remove` adapter un-hides a dressed shell off it.
        commands.entity(entity).remove::<Held>();
        return;
    }
    // Consume a terminal only after every preceding bounce; re-anchor at server truth.
    let terminal = shot
        .zip(sanctioned)
        .and_then(|(s, buf)| buf.terminal(s.0, shell.marks.ricochets.len()));
    if let Some(terminal) = terminal {
        finish_at_sanctioned_terminal(shell, &terminal, commands);
        if let Some(shot) = shot {
            crate::shot_trace::record(shot_trace, "hold", now, shot.0, || {
                json!({
                    "held": held.waited,
                    "res": "terminal",
                    "it": terminal.impact_tick,
                    "pen": terminal.verdict.entered(),
                })
            });
            crate::shot_trace::record(
                shot_trace,
                "end",
                now,
                shot.0,
                || json!({ "why": "terminal" }),
            );
        }
        commands.entity(entity).despawn();
        return;
    }
    // Still waiting. Past the grace window, the shell degrades to the fail-closed fallback:
    // an unavailable keyframe/confirm must never leave a round frozen forever. Otherwise stay
    // frozen this tick.
    held.waited += 1;
    held.age += 1;
    if held.waited > RICOCHET_HOLD_TICKS {
        // F3(ii) — QUIET DISSOLVE, not a fabricated spark. No sanctioned outcome means either
        // transport did not supply the authority verdict before this hold expired, or this
        // client contacted interpolated geometry the authority missed. A spark would fabricate
        // a confirmed contact in the latter case, so the hidden shell ends silently. The trace
        // distinguishes a late/lost authority fact from a pose-divergent contact.
        if let Some(shot) = shot {
            crate::shot_trace::record(
                shot_trace,
                "hold",
                now,
                shot.0,
                || json!({ "held": held.waited, "res": "expired" }),
            );
            crate::shot_trace::record(
                shot_trace,
                "end",
                now,
                shot.0,
                || json!({ "why": "bounce_dissolve" }),
            );
        }
        commands.entity(entity).despawn();
    }
}

/// Consume an authority outcome this replica's own march missed, once it is overdue by more than
/// [`OVERDUE_MARGIN_TICKS`].
///
/// Replica fallback: consume a known outcome by its tick when interpolated geometry missed it. The
/// client's tank poses can differ from the server's, so a shell may fly clean past a plate the
/// authority says it struck; past the margin the buffered verdict wins and the shell re-anchors on
/// it.
///
/// Returns `true` when the shell was resolved (re-seeded, finalized, or despawned) and must not
/// march this tick.
#[must_use]
fn consume_overdue_sanctioned_outcome(
    shell: &mut MarchingShell,
    // Authority = not a replica. The authority resolves shots for real and never consumes.
    deposit: bool,
    sanctioned: Option<&SanctionedShots>,
    present: Option<u32>,
    dt: f32,
    now: u32,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) -> bool {
    if deposit {
        return false;
    }
    let (Some(shot), Some(buf), Some(present)) = (shell.shot, sanctioned, present) else {
        return false;
    };
    let entity = shell.entity;
    let consumed = shell.marks.ricochets.len();
    if let Some(bounce) = buf.next(shot.0, consumed) {
        if let Some(re_age) = elapsed_ticks(present, bounce.bounce_tick)
            && re_age > OVERDUE_MARGIN_TICKS
        {
            // Re-seed through every already-buffered outcome up to the present. This is the
            // same authority-bounded catch-up as the held path: no free-flight segment may
            // cross a later bounce/terminal the client already knows about.
            let caught_up = match catch_up_sanctioned_chain(
                shot.0,
                consumed,
                bounce,
                Some(present),
                re_age,
                buf,
                shell.projectile.velocity,
                shell.projectile.drag_k,
                dt,
            ) {
                Ok(caught_up) => caught_up,
                Err(reject) => {
                    crate::shot_trace::record(
                        shot_trace,
                        "overdue",
                        now,
                        shot.0,
                        || json!({ "res": "reject", "why": reject.trace_reason() }),
                    );
                    crate::shot_trace::record(
                        shot_trace,
                        "end",
                        now,
                        shot.0,
                        || json!({ "why": "catch_up_reject" }),
                    );
                    commands.entity(entity).despawn();
                    return true;
                }
            };
            for segment in &caught_up.segments {
                shell.path.begin_segment();
                shell.path.points.extend(segment.points.iter().copied());
                shell.marks.ricochets.push(segment.bounce.origin);
                commands.trigger(Impact {
                    position: segment.bounce.origin,
                    // The keyframe does not carry the surface normal; preserve the existing
                    // overdue-path approximation from its sanctioned outgoing direction.
                    normal: segment.bounce.direction,
                    caliber: shell.projectile.caliber(),
                    surface: ImpactSurface::Armor,
                    outcome: ImpactOutcome::Ricochet {
                        direction: segment.bounce.direction,
                    },
                });
                commands.trigger(AuthorityOutcomeDrawn {
                    shot: shot.0,
                    ordinal: segment.bounce.sequence,
                });
                let late = elapsed_ticks(present, segment.bounce.bounce_tick).unwrap_or_default();
                crate::shot_trace::record(
                    shot_trace,
                    "overdue",
                    now,
                    shot.0,
                    || json!({ "res": "bounce", "late": late, "seq": segment.bounce.sequence }),
                );
            }
            if let Some(terminal) = caught_up.terminal {
                finish_at_sanctioned_terminal(shell, &terminal, commands);
                let late = elapsed_ticks(present, terminal.impact_tick).unwrap_or_default();
                crate::shot_trace::record(
                    shot_trace,
                    "overdue",
                    now,
                    shot.0,
                    || json!({ "res": "terminal", "late": late, "pen": terminal.verdict.entered(), "via": "chain" }),
                );
                crate::shot_trace::record(
                    shot_trace,
                    "end",
                    now,
                    shot.0,
                    || json!({ "why": "terminal" }),
                );
                commands.entity(entity).despawn();
                return true;
            }
            // The keyframes carry no surface normal, so nothing lifts this resume off a face: a
            // cosmetic discontinuity the next authority outcome governs (see there).
            resume_from_catch_up(shell, &caught_up, None);
            return true;
        }
    } else if let Some(terminal) = buf.terminal(shot.0, consumed)
        && let Some(late) = elapsed_ticks(present, terminal.impact_tick)
        && late > OVERDUE_MARGIN_TICKS
    {
        // Finalize at the server's read — position, normal, and the verdict
        // that gates the flame lick — the full honest armor read the authority resolved,
        // even though this client's shell never touched the (mis-posed) plate. The trail
        // reaches the server impact point, then the shell ends. (The `else if` is keyed on NO
        // bounce being owed: `buf.terminal` would return `None` anyway while a bounce's
        // keyframe is still in flight, by its `after_bounces` gate.)
        finish_at_sanctioned_terminal(shell, &terminal, commands);
        crate::shot_trace::record(
            shot_trace,
            "overdue",
            now,
            shot.0,
            || json!({ "res": "terminal", "late": late, "pen": terminal.verdict.entered() }),
        );
        crate::shot_trace::record(
            shot_trace,
            "end",
            now,
            shot.0,
            || json!({ "why": "terminal" }),
        );
        commands.entity(entity).despawn();
        return true;
    }

    false
}

/// Draw the shell's trail to the authority's terminal and read that impact exactly as the server did
/// — position, struck-face normal, and the verdict that gates the flame lick.
///
/// The server's read is spatially disconnected from wherever this client's shell actually was, so
/// the trail begins a new view segment rather than inventing a correction chord to it.
fn finish_at_sanctioned_terminal(
    shell: &mut MarchingShell,
    terminal: &SanctionedTerminal,
    commands: &mut Commands,
) {
    let caliber = shell.projectile.caliber();
    shell.path.begin_segment();
    shell.path.points.push(terminal.position);
    commands.trigger(Impact {
        position: terminal.position,
        normal: terminal.normal,
        caliber,
        surface: ImpactSurface::Armor,
        outcome: terminal.verdict.into(),
    });
    if let Some(shot) = shell.shot {
        commands.trigger(AuthorityOutcomeDrawn {
            shot: shot.0,
            ordinal: terminal.after_bounces,
        });
    }
}

/// Re-anchor a cosmetic shell on the state its authority catch-up ended at, and refresh the readout
/// the sandbox's info layer reads.
///
/// The catch-up flies from the sanctioned bounce point, which lies on the struck face; the
/// authority resumed from that point lifted off the face along its normal by the disc's radius and
/// nudged one [`MARCH_EPS`] along the outgoing direction (`resolve::Bounce::resume`). With `lift` —
/// the struck face's outward normal as THIS client's raycast read it — the resume takes an
/// approximate cosmetic lift: the radius along that client-local normal, plus the nudge along the
/// CAUGHT-UP direction rather than the bounce's outgoing one, applied at the end of the caught-up
/// flight. It is not the authority's lift, and an aged catch-up, which nothing sweeps, is not
/// guaranteed to stay outside the face it bounced off; it only keeps a zero-age release from laying
/// its fresh disc behind that face. Without it the resume stays on the authority's unlifted chain.
/// Either way any discontinuity is cosmetic, the next authority outcome governs, and the next
/// step's fresh disc has its membership established ([`resolve::certified`]) rather than declared.
fn resume_from_catch_up(
    shell: &mut MarchingShell,
    caught_up: &SanctionedCatchUp,
    lift: Option<Vec3>,
) {
    shell.transform.translation = caught_up.position;
    if let Ok(direction) = Dir3::new(caught_up.velocity) {
        if let Some(normal) = lift {
            shell.transform.translation += normal.normalize_or_zero()
                * resolve::disc_radius(shell.projectile.caliber())
                + direction * MARCH_EPS;
        }
        shell.transform.look_to(direction, Vec3::Y);
        // The sampling basis is carried through the catch-up's direction change, exactly as the
        // march carries it through gravity's bend and the authority's own ricochet. A frame left
        // anchored on the pre-catch-up heading is not merely mis-rolled: it has a component ALONG
        // the new travel axis, which puts ring samples in front of and behind the disc, and a sample
        // that starts inside geometry the axis has not reached reports an exit it never entered.
        shell.projectile.disc = shell
            .projectile
            .disc
            .transport(shell.projectile.velocity, Vec3::from(direction));
    }
    // The catch-up moved the round without sweeping its samples: the next step lays a fresh disc
    // and establishes its membership.
    shell.projectile.continuation = None;
    shell.projectile.velocity = caught_up.velocity;
    shell.readout.speed = caught_up.velocity.length();
    shell.readout.capability = capability(shell.projectile.body(), caught_up.velocity.length());
}

/// AUTHORITY: the witness pass over `legs` (ADR-0040, [`wound::line`]), through the witnesses
/// `admit` keeps. Doses each witness the line put a chord into, logs one [`ArmourMark::Wound`] and
/// settles it at its own index; returns the HP removed. A replica takes nothing.
///
/// The hard world is already resolved when this runs and nothing here reaches back into it, so a
/// witness walk that fails closed doses nothing and leaves the round's flight as it was.
fn wound_legs(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    legs: &[resolve::Leg],
    speed: f32,
    health: &mut Query<&mut ComponentHealth>,
) -> u32 {
    if !context.deposit {
        return 0;
    }
    let (volumes, incoming) = match wound::line(context, admit, legs, shell.projectile.wound) {
        Ok(found) => found,
        Err(error) => {
            error!(
                shot = ?shell.shot.map(|shot| shot.0),
                shell = ?shell.entity,
                ?error,
                "ballistics: the witness pass failed along the round's line; it doses nothing"
            );
            return 0;
        }
    };
    let Some(incoming) = incoming else {
        return 0;
    };
    shell.dose.add_wounds(&volumes);
    shell.marks.log.push(ArmourMark::Wound {
        incoming: Vec3::from(incoming),
        speed,
        volumes,
    });
    let mark = shell.marks.log.len() - 1;
    shell.dose.settle(health, true, mark)
}

/// Ray-march one tick's travel budget for a single shell, resolving every surface it reaches.
///
/// Free flight until a surface, then resolve it — terrain stops the shell; a ballistic volume
/// ricochets (too oblique) or is crossed (normalize → spend cost → perforate or embed) — and keep
/// marching the leftover budget along the new direction. Returns `None` for a degenerate zero
/// velocity, which leaves the shell untouched this tick.
fn march_shell_step(
    shell: &mut MarchingShell,
    world: &ProjectileMarchWorld,
    colliders: &Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static BallisticSurfaces>,
    )>,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<Forces>,
    sanctioned: Option<&SanctionedShots>,
    // Authority = not a replica: only then does a hit actually mutate health here.
    deposit: bool,
    // EXPERIMENTAL cost-attribution A/B arm (`SPIKE_MG_SHORTCIRCUIT`, default off — see
    // [`MgShortCircuit`]).
    shortcircuit: bool,
    dt: f32,
    now: u32,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) -> Option<MarchStep> {
    let shot = shell.shot;
    // Accumulate the authority's actual HP decrease across every direct crossing and spall fragment
    // this step. The caller's per-shell latch turns the aggregate into at most one discrete confirm.
    let mut damage_dealt = 0;
    // SHOOTER SELF-EXCLUSION (see [`not_own_volume`]): this round is transparent to the tank that
    // fired it, for every cast below. Identical on the authority and on a replica — the one place
    // both ends must agree about the shooter's own geometry, or the server's damage model and the
    // client's cosmetic model describe different worlds.
    let shooter = shell.source.map(|source| source.tank);
    let not_own = |entity: Entity| not_own_volume(entity, shooter, &world.owners, &world.parents);
    // Gravity and drag on the velocity through the free-flight kernel the FireEvent catch-up folds
    // ([`fast_forward_shell`]). An unbent open-air step flies exactly `stepped × dt`. The march may
    // *bend* the direction (normalization / ricochet), so we carry direction + speed and rebuild the
    // velocity at the end rather than assuming a straight step.
    let stepped = freeflight_step(shell.projectile.velocity, shell.projectile.drag_k, dt);
    let Ok(dir) = Dir3::new(stepped) else {
        return None;
    };
    let speed = stepped.length();
    let mut budget = resolve::Budget::new(speed * dt);
    let mut stopped = false;
    // Set when an observer shell freezes at armor contact to await its bounce keyframe (see the
    // `!deposit` branch): the shell keeps its entity and velocity, and the `Held` handler above
    // drives the wait on subsequent ticks — distinct from `stopped` (which despawns).
    let mut holding = false;
    // Whether the march has bent the shell off its original free-flight segment. Until it does,
    // an open-air fly-out moves the axis by exactly `stepped × dt` (the catch-up kernel's step);
    // after a bend the leftover budget flies along the new direction instead.
    let mut bent = false;
    // AUTHORITY: whether this shell's ONE terminal (`ShellTerminal` — embed/perforation) has been
    // emitted. A perforated shell can keep marching through the interior across fixed ticks, so
    // this spawn-time latch supplies both the same-tick and cross-tick halves of the invariant
    // without destroying `Shot`, which remains the damage-attribution identity. It also mutes
    // post-terminal `ShellRicochet`s: the client's cosmetic shell ended at the terminal, so an
    // interior bounce after it must not ride the wire.
    let mut terminal_emitted = shell.terminal_report.0;

    // Ray-march the step: free flight until a surface, then resolve it — terrain stops the
    // shell; a ballistic volume ricochets (too oblique) or is crossed (normalize → spend cost →
    // perforate or embed) — and keep marching the leftover budget along the new direction.
    // The disc's sampling basis, carried through this tick's gravity bend so the ring never re-rolls
    // (§13.5: the frame is spawn-anchored and transported, never rebuilt from the current direction).
    // Transported from the direction the frame was LAST left on — the pre-step velocity, which is
    // also the direction the last step's sweep ran along — not from the shell's visual forward:
    // `looking_to` is degenerate for a round travelling straight down, and the sampling basis is sim
    // state that must not read a view detail.
    let previous = Dir3::new(shell.projectile.velocity).unwrap_or(dir);
    let round = shell.projectile.round;
    let radius = resolve::disc_radius(shell.projectile.caliber());
    let context = resolve::ResolveContext {
        world,
        colliders,
        deposit,
        laws: walk::WalkLaws::default(),
    };
    // The crossing state machine shared with the §13.6 gate, resumed from the starts the last step
    // left (`resolve::Interior`), or from a fresh disc whose membership it establishes. A
    // perforation that leaves a sample inside material is resolved on within this step, past the
    // travel budget if need be (see [`next_surface`]), within the step's limits
    // (`resolve::Budget`); what crosses the tick is the starts, never an owed exit.
    let launched = resolve::Flight::launched(
        &context,
        &not_own,
        shell.transform.translation,
        previous,
        dir,
        speed,
        shell.projectile.disc,
        radius,
        shell.projectile.continuation.take(),
    );
    let mut flight = match launched {
        Ok(flight) => flight,
        Err(error) => {
            let position = shell.transform.translation;
            let holding = uncertified_start(
                shell, deposit, position, dir, &error, now, shot_trace, commands,
            );
            return Some(MarchStep {
                position,
                direction: dir,
                speed,
                stopped: !holding,
                holding,
                damage_dealt,
            });
        }
    };

    loop {
        // A bounce or a re-seed leaves a fresh disc: its membership is established before anything
        // sweeps from it, or the round ends here ([`uncertified_start`]).
        if let Err(error) = flight.settle(&context, &not_own, radius) {
            holding = uncertified_start(
                shell,
                deposit,
                flight.position,
                flight.direction,
                &error,
                now,
                shot_trace,
                commands,
            );
            stopped = !holding;
            break;
        }
        if !budget.continues(&flight) {
            break;
        }
        let origin = flight.sweep_origin();
        let dir = flight.direction;
        let hit = match next_surface(world, &flight, radius, budget.remaining(), &not_own) {
            // Open air — fly out the rest of the step. On the original (unbent) segment this is
            // exactly the catch-up kernel's displacement; a `continue` past this point only ever
            // follows a bend, so `bent` is the exact discriminant.
            Ahead::Open => {
                let displacement = if bent {
                    dir * budget.remaining()
                } else {
                    stepped * dt
                };
                if let Ok(along) = Dir3::new(displacement) {
                    let leg = resolve::Leg::new(
                        origin,
                        along,
                        displacement.length(),
                        &flight.sample_offsets(radius),
                    );
                    damage_dealt += wound_legs(shell, &context, &not_own, &[leg], speed, health);
                }
                flight.fly(displacement);
                break;
            }
            // The ground stops the shell: same `Impact` read whether or not the MG short-circuit
            // is armed (the B-arm's terrain stop was already identical); only the lifecycle row
            // differs — the short-circuit records nothing, exactly as before.
            Ahead::Terrain { distance, normal } => {
                let entry = origin + dir * distance;
                let leg = resolve::Leg::new(origin, dir, distance, &flight.sample_offsets(radius));
                damage_dealt += wound_legs(shell, &context, &not_own, &[leg], speed, health);
                let shorted =
                    shortcircuit && shell.projectile.caliber() < MG_SHORTCIRCUIT_CALIBER_MAX;
                commands.trigger(Impact {
                    position: entry,
                    normal,
                    caliber: shell.projectile.caliber(),
                    surface: ImpactSurface::Terrain,
                    outcome: ImpactOutcome::Struck,
                });
                // A terrain stop is the one shot terminal that needs NO confirm: static,
                // pose-independent geometry, so both ends already agree (ADR-0021's
                // invariant). Recorded on the client so the analyzer can close the lifecycle
                // of a shot that simply never reached armor.
                if !shorted
                    && !deposit
                    && let Some(shot) = shot
                {
                    crate::shot_trace::record(
                        shot_trace,
                        "end",
                        now,
                        shot.0,
                        || json!({ "why": "terrain" }),
                    );
                }
                flight.position = entry;
                stopped = true;
                break;
            }
            Ahead::Armor(hit) => Some(hit),
            // A held continuation with no surface in reach: its material's exit is still owed, and
            // the continuation alone knows where it starts.
            Ahead::Held => None,
        };
        let contact = hit.map_or(0.0, |hit| hit.distance);
        let entry = origin + dir * contact;
        let contact_normal = hit.map_or(-Vec3::from(dir), |hit| hit.normal);

        if let Some(hit) = hit {
            // EXPERIMENTAL B-arm (`SPIKE_MG_SHORTCIRCUIT`): a sub-20 mm round stops dead at the
            // first surface, skipping the entire penetration-resolution march below
            // (thickness/span probes, ricochet, spall, HP). Population-preserving — same
            // despawn-on-contact as the live path — so the A−B tick-cost delta isolates the
            // resolution machinery. Default off (see the type).
            if shortcircuit && shell.projectile.caliber() < MG_SHORTCIRCUIT_CALIBER_MAX {
                // Classify the first surface from the hit's volume ancestry (the same
                // `hit_ancestor` rule the full march uses just below) so the read is honest even in
                // the B-arm. The short-circuit stops dead without resolving penetration, so
                // `Struck`.
                let surface = if world.armour(hit.entity).is_some() {
                    ImpactSurface::Armor
                } else {
                    ImpactSurface::Terrain
                };
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: shell.projectile.caliber(),
                    surface,
                    outcome: ImpactOutcome::Struck,
                });
                flight.position = entry;
                stopped = true;
                break;
            }

            // The struck `BallisticVolume` sits on the hit's ancestry (`hit_ancestor`, the shared
            // hierarchy-resolution rule). No volume in the ancestry ⇒ classified terrain (an
            // Armor-layer entity without ownership — same rule as before the cast split). The
            // crossing itself finds every volume for itself; this read only tells armour from
            // terrain.
            if world.armour(hit.entity).is_none() {
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: shell.projectile.caliber(),
                    surface: ImpactSurface::Terrain,
                    outcome: ImpactOutcome::Struck,
                });
                // A terrain stop is the one shot terminal that needs NO confirm: static,
                // pose-independent geometry, so both ends already agree (ADR-0021's invariant).
                // Recorded on the client so the analyzer can close the lifecycle of a shot that
                // simply never reached armor, instead of filing it as never-consumed.
                if !deposit && let Some(shot) = shot {
                    crate::shot_trace::record(
                        shot_trace,
                        "end",
                        now,
                        shot.0,
                        || json!({ "why": "terrain" }),
                    );
                }
                flight.position = entry;
                stopped = true;
                break;
            }

            // Replica armor state machine: consume a buffered bounce/terminal, otherwise hold a
            // keyed shell hidden; an unkeyed replica shell ends locally. INVARIANT: every replica
            // arm below continues or breaks, so the physical armor resolution after it is
            // authority-only.
            if !deposit {
                let hit_normal = hit.normal;
                match resolve_replica_armor_contact(
                    shell, entry, hit.normal, sanctioned, now, shot_trace, commands,
                ) {
                    ReplicaArmorContact::Reseed {
                        origin,
                        direction,
                        speed: sanctioned_speed,
                    } => {
                        // The same step limits the authority's crossings are held to, charged from
                        // the flight's position.
                        if let Err(error) = budget.charge(hit.distance + MARCH_EPS) {
                            warn_once!(
                                "ballistics: a replica's step ran out of its limits, stopping the \
                                 round: {error:?}"
                            );
                            flight.position = entry;
                            stopped = true;
                            break;
                        }
                        if let Some(new_dir) = direction {
                            // Carry the sampling basis through the sanctioned bend, exactly as the
                            // authority's own ricochet does. A replica that re-seeds on server
                            // truth without transporting its frame keeps a basis built for the
                            // INCOMING heading — one whose vectors have a component along the new
                            // travel axis, so its ring samples sit in front of and behind the disc
                            // rather than across it.
                            flight.frame =
                                flight.frame.transport(Vec3::from(dir), Vec3::from(new_dir));
                            flight.direction = new_dir;
                        }
                        flight.speed = sanctioned_speed;
                        flight.continuation = None;
                        // Lift the shell's BODY clear of the face it bounced off and nudge it off
                        // along the new heading, exactly as the authority's own ricochet does
                        // (`resolve::Bounce::resume`). At oblique incidence a disc's lateral
                        // offsets lie mostly along the struck surface's normal, so resuming at the
                        // contact point leaves half the ring behind the face and first contact
                        // fires again on the very surface just left — which on a replica means
                        // holding for a keyframe that was already consumed. The lift clears that
                        // face only; the fresh disc's membership against everything else is
                        // established before it sweeps (`resolve::Flight::settle`). The RECORDED
                        // bounce point is untouched; only where the march resumes moves.
                        flight.position = origin
                            + Vec3::new(hit_normal.x, hit_normal.y, hit_normal.z)
                                .normalize_or_zero()
                                * radius
                            + flight.direction * MARCH_EPS;
                        bent = true;
                        continue;
                    }
                    ReplicaArmorContact::Stopped => {
                        flight.position = entry;
                        stopped = true;
                        break;
                    }
                    ReplicaArmorContact::Holding => {
                        flight.position = entry;
                        holding = true;
                        break;
                    }
                }
            }
        }

        // The §13 union walk: decide the crossing from the flight as it stands, apply its
        // consequences, and advance the flight to where the next one starts.
        // FAIL CLOSED. A structured walk error means the corridor could not be resolved honestly —
        // unpairable topology, an unprobeable collider, a crossing that would not close inside 50 m,
        // a step that would run past its limits. The round stops where it was: no perforation, no
        // spall, no transit damage. Free penetration is the one outcome worse than a stopped shell,
        // because it is indistinguishable from armour that was never modelled. The budget is charged
        // BEFORE the crossing is applied, so a refused crossing lands none of its consequences.
        let decided = resolve::decide(&context, &not_own, &round, &flight, contact)
            .and_then(|decision| budget.charge(decision.charged()).map(|()| decision));
        let decision = match decided {
            Ok(decision) => {
                damage_dealt += wound_legs(
                    shell,
                    &context,
                    &not_own,
                    decision.legs(),
                    flight.speed,
                    health,
                );
                decision
            }
            Err(error) => {
                error!(
                    shot = ?shell.shot.map(|shot| shot.0),
                    shell = ?shell.entity,
                    struck = ?hit.map(|hit| hit.entity),
                    ?error,
                    "ballistics: union walk failed, stopping the round at contact; it opens no \
                     pen cam"
                );
                shell.marks.log.push(ArmourMark::Failed);
                commands.trigger(Impact {
                    position: entry,
                    normal: contact_normal,
                    caliber: shell.projectile.caliber(),
                    surface: ImpactSurface::Armor,
                    outcome: ImpactOutcome::Struck,
                });
                flight.position = entry;
                stopped = true;
                break;
            }
        };
        damage_dealt += resolve::apply(
            &decision,
            shell,
            &context,
            &mut terminal_emitted,
            health,
            bodies,
            commands,
        );
        // Every outcome leaves the round off its original free-flight segment (see the open-air
        // break above), whether it bounced off the face or bent into the plate.
        bent = true;
        // The budget was charged what the crossing actually flew, summed over its segments by the
        // only code that knows them (see `resolve::Bite::travel`) — never the distance to the
        // NEAREST sample's contact, which at oblique incidence leads the axis handoff by
        // `r·tan(incidence)`.
        if flight.follow(decision) {
            stopped = true;
            break;
        }
    }
    // Carry the (possibly re-rolled) basis and the per-sample starts back onto the shell, so the
    // next tick transports from where this one left off and resumes every sample where its ray
    // stopped. A held round is frozen at contact, not at a start; its release lays a fresh disc, as
    // does the next step of a flight whose bounce ended this one before it settled.
    shell.projectile.disc = flight.frame;
    shell.projectile.continuation = if holding {
        None
    } else {
        flight.continuation.take()
    };

    Some(MarchStep {
        position: flight.position,
        direction: flight.direction,
        speed: flight.speed,
        stopped,
        holding,
        damage_dealt,
    })
}

/// End the step at a fresh start whose membership could not be established
/// ([`resolve::certified`]); the start is never declared outside. Returns whether the shell holds;
/// otherwise it has stopped.
///
/// The authority fails the round closed where it stands, as for any walk error. A replica owns no
/// armour verdict to show for it: a keyed shell holds hidden for the authority's next outcome, the
/// lifecycle of an unresolved armour contact ([`resolve_replica_armor_contact`]) with no struck face
/// to lift off, and an unkeyed one, which no outcome can reach, ends quietly.
#[expect(
    clippy::too_many_arguments,
    reason = "the march's own state threaded through; a struct would be built for this one call"
)]
fn uncertified_start(
    shell: &mut MarchingShell,
    deposit: bool,
    position: Vec3,
    direction: Dir3,
    error: &walk::WalkError,
    now: u32,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) -> bool {
    if deposit {
        error!(
            shot = ?shell.shot.map(|shot| shot.0),
            shell = ?shell.entity,
            ?position,
            ?error,
            "ballistics: a fresh start could not be certified, stopping the round; it opens no \
             pen cam"
        );
        shell.marks.log.push(ArmourMark::Failed);
        commands.trigger(Impact {
            position,
            normal: -Vec3::from(direction),
            caliber: shell.projectile.caliber(),
            surface: ImpactSurface::Armor,
            outcome: ImpactOutcome::Struck,
        });
        return false;
    }
    warn_once!(
        "ballistics: a replica's fresh start could not be certified, awaiting the authority: \
         {error:?}"
    );
    let Some(shot) = shell.shot else {
        return false;
    };
    commands.entity(shell.entity).insert(Held {
        waited: 0,
        age: 0,
        normal: None,
    });
    crate::shot_trace::record(
        shot_trace,
        "contact",
        now,
        shot.0,
        || json!({ "res": "hold", "why": "uncertified" }),
    );
    true
}

/// What lies ahead of a flight, for the next crossing.
enum Ahead {
    Open,
    Terrain {
        distance: f32,
        normal: Vec3,
    },
    Armor(avian3d::prelude::RayHitData),
    /// The flight holds a continuation inside material and the cast found no surface: the
    /// continuation's own corridor resolves it.
    Held,
}

/// The next surface a flight meets within `budget` metres — shared by the runtime march and the
/// §13.6 gate, so both cut successive crossings at the same places.
///
/// Cast from every sample's own start ([`resolve::Flight::sample_offsets`]), not from a disc
/// re-laid at the axis: a sample's start is the only point its membership is known at.
///
/// A flight holding material ([`resolve::Flight::holds_material`]) is not done with it when the
/// budget is: its exit lies ahead with its entry behind, and only the continuation declares that. So
/// such a flight looks as far as a corridor may reach ([`resolve::MAX_CORRIDOR`]) and its crossing
/// resolves now, past the budget if need be: a step never yields owing an exit, so what crosses a
/// tick is the per-sample starts alone. How far past is bounded: [`resolve::Budget`] stops a drain
/// at [`resolve::MAX_OVERRUN`].
fn next_surface(
    world: &ProjectileMarchWorld,
    flight: &resolve::Flight,
    radius: f32,
    budget: f32,
    not_own: &dyn Fn(Entity) -> bool,
) -> Ahead {
    let held = flight.holds_material();
    let reach = if held {
        budget.max(resolve::MAX_CORRIDOR)
    } else {
        budget
    };
    match cast_disc_segment(
        world,
        &flight.sample_offsets(radius),
        flight.sweep_origin(),
        flight.direction,
        reach,
        not_own,
    ) {
        Some(SegmentHit::Armor(hit)) => Ahead::Armor(hit),
        Some(SegmentHit::Terrain { distance, normal }) => Ahead::Terrain { distance, normal },
        None if held => Ahead::Held,
        None => Ahead::Open,
    }
}

/// Resolve one armor contact a REPLICA's own march found, where ballistics is cosmetic and the
/// authority owns every armor verdict.
///
/// INVARIANT: every arm ends the caller's surface resolution — re-seed from a buffered bounce, stop
/// on a buffered terminal (or, unkeyed, fail closed), or freeze hidden until a verdict arrives — so
/// the physical armor resolution after it is authority-only.
fn resolve_replica_armor_contact(
    shell: &mut MarchingShell,
    entry: Vec3,
    // The struck face's outward normal, straight from this client's raycast.
    hit_normal: Vec3,
    sanctioned: Option<&SanctionedShots>,
    now: u32,
    shot_trace: &mut Option<ResMut<crate::shot_trace::ShotTrace>>,
    commands: &mut Commands,
) -> ReplicaArmorContact {
    let entity = shell.entity;
    let shot = shell.shot;
    let caliber = shell.projectile.caliber();
    let consumed = shell.marks.ricochets.len();
    let next_bounce = shot
        .zip(sanctioned)
        .and_then(|(s, buf)| buf.next(s.0, consumed));
    if let Some(bounce) = next_bounce {
        // Buffered bounce: re-seed from authority truth and keep the remaining step.
        commands.trigger(Impact {
            position: bounce.origin,
            normal: hit_normal,
            caliber,
            surface: ImpactSurface::Armor,
            outcome: ImpactOutcome::Ricochet {
                direction: bounce.direction,
            },
        });
        shell.path.begin_segment();
        shell.path.points.push(bounce.origin);
        shell.marks.ricochets.push(bounce.origin);
        // Trace a buffered bounce separately from a shell that never contacted.
        if let Some(shot) = shot {
            commands.trigger(AuthorityOutcomeDrawn {
                shot: shot.0,
                ordinal: bounce.sequence,
            });
            crate::shot_trace::record(
                shot_trace,
                "contact",
                now,
                shot.0,
                || json!({ "res": "pre_bounce", "seq": bounce.sequence, "bt": bounce.bounce_tick }),
            );
        }
        return ReplicaArmorContact::Reseed {
            origin: bounce.origin,
            direction: Dir3::new(bounce.direction).ok(),
            speed: bounce.speed,
        };
    }
    // Buffered terminal: resolve at the authority read without a hold.
    let terminal = shot
        .zip(sanctioned)
        .and_then(|(s, buf)| buf.terminal(s.0, consumed));
    if let Some(terminal) = terminal {
        finish_at_sanctioned_terminal(shell, &terminal, commands);
        if let Some(shot) = shot {
            crate::shot_trace::record(
                shot_trace,
                "contact",
                now,
                shot.0,
                || json!({ "res": "pre_term", "it": terminal.impact_tick, "pen": terminal.verdict.entered() }),
            );
            crate::shot_trace::record(
                shot_trace,
                "end",
                now,
                shot.0,
                || json!({ "why": "terminal" }),
            );
        }
        return ReplicaArmorContact::Stopped;
    }
    if let Some(shot) = shot {
        // Hold a keyed shell until the authority outcome or expiry. The stop is invisible where
        // anything is drawn at all: the view's `Add` adapter hides a dressed shell off this.
        commands.entity(entity).insert(Held {
            waited: 0,
            age: 0,
            normal: Some(hit_normal),
        });
        // The corresponding `hold` row closes this trace interval.
        crate::shot_trace::record(
            shot_trace,
            "contact",
            now,
            shot.0,
            || json!({ "res": "hold" }),
        );
        return ReplicaArmorContact::Holding;
    }
    // No identity — fail closed immediately (pre-slice behaviour).
    commands.trigger(Impact {
        position: entry,
        normal: hit_normal,
        caliber,
        surface: ImpactSurface::Armor,
        outcome: ImpactOutcome::Struck,
    });
    ReplicaArmorContact::Stopped
}

/// Throw the spall cone of one body exit, appending it to the shell's record and its doses to the
/// shot's [`ShotDose`] (design doc §5). Returns whether it was thrown.
///
/// Strength `S` is [`walk::SpallSource::strength`]. A cone whose whole emission `S/2` converts to
/// no HP point ([`hp_points`]) is not thrown: no direction is walked and nothing is recorded. `S/2`
/// is the most one box can take from it, since every direction doses a box at most once and the
/// shape's weights emit half the on-axis strength ([`spall_cone::EMITTED_SHARE`]). Otherwise
/// direction `k` of the shape ([`spall_cone::ConeShape`]) carries `S·f_k` and stands for `1/N` of the
/// cone, walked through the volumes of `tank`, the struck tank, alone
/// ([`resolve::field_direction`]). A direction whose walk fails is fail-closed: it doses nothing, and
/// the cone logs its first error. A direction whose witness pass alone fails keeps its hard doses
/// and reach and doses no witness; the cone logs that once too.
fn throw_spall_cone(
    spall: &mut Mut<SpallMarks>,
    dose: &mut Mut<ShotDose>,
    source: &walk::SpallSource,
    tank: Option<Entity>,
    context: &resolve::ResolveContext<'_, '_, '_>,
    is_box: &dyn Fn(Entity) -> bool,
) -> bool {
    let world = context.world;
    let shape = world
        .spall_law
        .as_deref()
        .map_or(spall_cone::ConeShape::shipped(), |law| &law.0);
    let strength = source.strength();
    if hp_points(strength * spall_cone::EMITTED_SHARE) == 0 {
        spall.gated += 1;
        return false;
    }
    let direction = Dir3::new(source.direction).unwrap_or(Dir3::NEG_Z);
    let axis = shape.axis(source.face, direction);
    let weight = 1.0 / shape.directions_count() as f32;
    let within = |collider: Entity| {
        world
            .volume_of(collider)
            .is_some_and(|node| world.owners.get(node).ok().map(VolumeOf::tank) == tank)
    };
    let mut cone = SpallCone {
        origin: source.position,
        axis,
        half_angle: shape.half_angle(),
        spend: source.spend,
        entry_speed: source.entry_speed,
        exit_speed: source.exit_speed,
        strength,
        rays: Vec::with_capacity(shape.directions_count()),
    };
    let mut failed: Option<(usize, walk::WalkError)> = None;
    let mut unwitnessed: Option<(usize, walk::WalkError)> = None;
    for (ray_dir, profile) in shape.directions(axis) {
        let thrown = strength * profile;
        let walked = resolve::field_direction(
            context,
            &within,
            is_box,
            &source.exited,
            source.position,
            ray_dir,
            thrown,
            weight,
            shape.passage(),
        );
        let ray = match walked {
            Ok(ray) => ray,
            Err(error) => {
                failed = Some(failed.map_or((1, error), |(count, first)| (count + 1, first)));
                resolve::FieldRay {
                    end: source.position,
                    doses: Vec::new(),
                    witness_error: None,
                }
            }
        };
        if let Some(error) = ray.witness_error.clone() {
            unwitnessed = Some(unwitnessed.map_or((1, error), |(count, first)| (count + 1, first)));
        }
        dose.add_all(&ray.doses);
        cone.rays.push(SpallRay {
            end: ray.end,
            deposited: !ray.doses.is_empty(),
            strength: thrown,
            doses: ray
                .doses
                .iter()
                .map(|dose| (dose.address, dose.amount))
                .collect(),
        });
    }
    if let Some((count, error)) = failed {
        error!(
            origin = ?source.position,
            ?error,
            "ballistics: {count} spall cone direction(s) could not be walked; each doses nothing"
        );
    }
    if let Some((count, error)) = unwitnessed {
        error!(
            origin = ?source.position,
            ?error,
            "ballistics: the witness pass failed on {count} spall cone direction(s); each doses no \
             witness and keeps its hard doses and reach"
        );
    }
    spall.cones.push(cone);
    true
}

/// Apply a crossing's momentum share to the struck body. The declared `Forces` query keeps this
/// immediate velocity write visible to Bevy's scheduler; static or non-rigid owners do not match.
fn apply_hit_impulse(bodies: &mut Query<Forces>, body: Entity, impulse: Vec3, point: Vec3) {
    if let Ok(forces) = bodies.get_mut(body) {
        crate::track::sim::apply_explicit_impulse(forces, impulse, point);
    }
}

fn on_impact(impact: On<Impact>) {
    // `debug!`, not `info!`: this fires once per shell/pellet impact, so sustained MG fire made it
    // the dominant console spam. Kept as a diagnostic under `RUST_LOG=overmatch=debug`.
    debug!("shell impact at {:?}", impact.position);
    // The sim-side seam: the armor penetration march/spall hook in here. The debug marker is a
    // separate, view-side observer on this same event (`debug::spawn_impact_marker`), kept out of
    // the sim per ADR-0014.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dose converts to the nearest whole HP point, ties away from zero; none for a negative or
    /// NaN dose.
    #[test]
    fn a_dose_converts_to_the_nearest_hp_point() {
        assert_eq!(hp_points(0.0), 0);
        assert_eq!(hp_points(0.04), 0);
        assert_eq!(
            hp_points(0.25),
            3,
            "2.5 points, a tie, round away from zero"
        );
        assert_eq!(hp_points(3.0), 30);
        assert_eq!(hp_points(-1.0), 0);
        assert_eq!(hp_points(f32::NAN), 0);
    }

    /// A box loses its shot's dose rounded once on its total: doses each below half a point add up
    /// before they are rounded, a settle mid-shot takes only what the rounded total has gained
    /// since the last, and the pool stops at zero. Fixture doses (reference-mm).
    #[test]
    fn a_shots_dose_is_rounded_once_per_box() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let target = world
            .spawn(ComponentHealth {
                current: 30,
                max: 30,
            })
            .id();
        world.spawn(ShotDose::default());
        let dose = |world: &mut World, amounts: &[f32]| -> u32 {
            let amounts = amounts.to_vec();
            world
                .run_system_once(
                    move |mut shot: Query<&mut ShotDose>,
                          mut health: Query<&mut ComponentHealth>| {
                        let mut shot = shot.single_mut().unwrap();
                        for amount in &amounts {
                            shot.add(target, *amount);
                        }
                        shot.settle(&mut health, true, 0)
                    },
                )
                .unwrap()
        };
        assert_eq!(
            dose(&mut world, &[0.04; 10]),
            4,
            "ten 0.4-point doses take 4 points"
        );
        assert_eq!(
            dose(&mut world, &[0.03]),
            0,
            "4.3 points rounds to the 4 taken"
        );
        assert_eq!(dose(&mut world, &[0.03]), 1, "4.6 points rounds to 5");
        assert_eq!(world.get::<ComponentHealth>(target).unwrap().current, 25);
        assert_eq!(dose(&mut world, &[100.0]), 25, "the pool stops at zero");
        assert_eq!(world.get::<ComponentHealth>(target).unwrap().current, 0);
    }

    /// A box's HP lost depends only on the multiset of its contributions, never their order:
    /// several cones' rays summed forward, reversed, sorted either way, interleaved and shuffled
    /// take the same points, including totals beside a rounding tie. Fixture doses (reference-mm),
    /// each cone 256 equal rays.
    #[test]
    fn a_boxs_hp_lost_is_independent_of_the_order_of_its_doses() {
        let target = Entity::from_raw_u32(1).unwrap();
        let cone = |total: f32| vec![total / 256.0; 256];
        let lost = |amounts: &[f32]| -> u32 {
            let mut shot = ShotDose::default();
            for amount in amounts {
                shot.add(target, *amount);
            }
            hp_of_units(shot.tallies[0].units)
        };
        let shot_sets: [Vec<Vec<f32>>; 4] = [
            vec![cone(0.05), cone(0.1)],
            vec![cone(0.05), cone(0.1), vec![0.0001; 7]],
            vec![cone(0.03), cone(0.015), cone(0.005), vec![0.0123, 0.0456]],
            vec![cone(0.149_999), vec![5.0e-7; 2]],
        ];
        for cones in shot_sets {
            let forward: Vec<f32> = cones.iter().flatten().copied().collect();
            let mut orders = vec![
                forward.clone(),
                forward.iter().rev().copied().collect(),
                cones.iter().rev().flatten().copied().collect(),
            ];
            let mut ascending = forward.clone();
            ascending.sort_by(f32::total_cmp);
            orders.push(ascending.iter().rev().copied().collect());
            orders.push(ascending);
            let longest = cones.iter().map(Vec::len).max().unwrap();
            orders.push(
                (0..longest)
                    .flat_map(|i| cones.iter().filter_map(move |cone| cone.get(i)))
                    .copied()
                    .collect(),
            );
            let mut shuffled = forward.clone();
            let mut state = 0x9e37_79b9_u32;
            for i in (1..shuffled.len()).rev() {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                shuffled.swap(i, state as usize % (i + 1));
            }
            orders.push(shuffled);
            let expected = lost(&forward);
            for order in &orders {
                assert_eq!(lost(order), expected, "{cones:?}");
            }
        }
        assert_eq!(
            hp_of_units(3 * DOSE_UNITS_PER_HP / 2),
            2,
            "1.5 points, a tie, rounds away from zero"
        );
        assert_eq!(hp_of_units(3 * DOSE_UNITS_PER_HP / 2 - 1), 1);
    }

    /// Rounds the capability-law invariants are swept over. Fixture data, not a spec: the laws
    /// below hold for any round.
    const LAW_ROUNDS: [RoundBody; 4] = [
        RoundBody {
            mass: 0.0118,
            caliber: 0.0079,
            nose_factor: 0.5,
        },
        RoundBody {
            mass: 0.685,
            caliber: 0.037,
            nose_factor: 0.6,
        },
        RoundBody {
            mass: 10.2,
            caliber: 0.088,
            nose_factor: 7.0 / 24.0,
        },
        RoundBody {
            mass: 25.0,
            caliber: 0.122,
            nose_factor: 1.0,
        },
    ];

    /// Replica scaling: mass ×s³ and calibre ×s at fixed speed and nose factor scales capability
    /// by s.
    #[test]
    fn capability_law_scales_with_replica_size() {
        for round in LAW_ROUNDS {
            for speed in [200.0, 773.0, 1500.0] {
                let base = capability(round, speed);
                for s in [0.5f32, 2.0, 3.0] {
                    let scaled = RoundBody {
                        mass: round.mass * s * s * s,
                        caliber: round.caliber * s,
                        ..round
                    };
                    let got = capability(scaled, speed);
                    assert!(
                        (got / (s * base) - 1.0).abs() < 1.0e-4,
                        "{round:?} at {speed} m/s, ×{s}: {got} vs {}",
                        s * base
                    );
                }
            }
        }
    }

    /// At fixed mass, speed and nose factor, a wider round has strictly less capability.
    #[test]
    fn capability_law_strictly_decreases_in_calibre() {
        for round in LAW_ROUNDS {
            for speed in [200.0, 773.0, 1500.0] {
                let mut last = f32::INFINITY;
                for step in 1..=40 {
                    let caliber = 0.005 * step as f32;
                    let c = capability(RoundBody { caliber, ..round }, speed);
                    assert!(
                        c < last,
                        "{round:?} at {speed} m/s: calibre {caliber} gives {c} ≥ {last}"
                    );
                    last = c;
                }
            }
        }
    }

    /// Spending `k1` then `k2` through two inversions lands on the same speed as spending
    /// `k1 + k2` in one.
    #[test]
    fn capability_law_spends_compose() {
        for round in LAW_ROUNDS {
            for speed in [300.0, 773.0, 1500.0] {
                let full = capability(round, speed);
                for (f1, f2) in [(0.1, 0.2), (0.3, 0.3), (0.05, 0.9)] {
                    let (k1, k2) = (f1 * full, f2 * full);
                    let after_k1 = speed_for(round, full - k1);
                    let two = speed_for(round, capability(round, after_k1) - k2);
                    let one = speed_for(round, full - (k1 + k2));
                    assert!(
                        (two - one).abs() < 1.0e-3 * speed,
                        "{round:?} at {speed} m/s, spends {k1}+{k2}: {two} vs {one}"
                    );
                }
            }
        }
    }

    /// `speed_for` inverts `capability` across the speed range a round meets.
    #[test]
    fn capability_law_inverse_round_trips() {
        for round in LAW_ROUNDS {
            for step in 1..=60 {
                let speed = 30.0 * step as f32;
                let back = speed_for(round, capability(round, speed));
                assert!(
                    (back / speed - 1.0).abs() < 1.0e-4,
                    "{round:?}: {speed} m/s round-trips to {back}"
                );
            }
        }
    }

    /// The local speed exponent `d ln C / d ln v` is the dimensionless law's own,
    /// `2x / ((1 + x) ln(1 + x))` with `x = k v² / a`: 2 in the limit `x → 0` and strictly below 2
    /// at every positive speed, for every geometry.
    ///
    /// Swept over the round fixtures and every nose family across its parameter range, from 1 m/s
    /// to 2 km/s. The exponent is read as a central difference of `ln C` over `v (1 ± h)`: with
    /// `h = 1e-2` the truncation error is `O(h²)` (≈ 1e-4, DERIVED) and f32 rounding of `C`
    /// contributes `ε / h` (≈ 1e-5, DERIVED), so a tolerance of 1e-3 cannot hide a departure from
    /// the law and cannot be tripped by arithmetic.
    #[test]
    fn capability_law_speed_exponent_is_the_dimensionless_law_s() {
        use crate::spec::NoseShape;
        const H: f32 = 1.0e-2;
        const TOLERANCE: f64 = 1.0e-3;
        let mut noses: Vec<f32> = LAW_ROUNDS.iter().map(|round| round.nose_factor).collect();
        for parameter in [0.5f32, 0.75, 1.0, 2.0, 5.0, 10.0, 30.0] {
            noses.extend([
                NoseShape::Ogive { crh: parameter }.factor(),
                NoseShape::Cone {
                    length_calibres: parameter,
                }
                .factor(),
                NoseShape::Cap {
                    radius_calibres: parameter,
                }
                .factor(),
            ]);
        }
        noses.push(NoseShape::Flat.factor());
        for body in LAW_ROUNDS {
            for &nose_factor in &noses {
                let round = RoundBody {
                    nose_factor,
                    ..body
                };
                let (_, inertia) = round.terms();
                for speed in [1.0f32, 10.0, 100.0, 300.0, 773.0, 1000.0, 1500.0, 2000.0] {
                    let measured = ((capability(round, speed * (1.0 + H))
                        / capability(round, speed * (1.0 - H)))
                    .ln()
                        / ((1.0 + H) / (1.0 - H)).ln()) as f64;
                    let x = inertia * speed as f64 * speed as f64;
                    let law = 2.0 * x / ((1.0 + x) * x.ln_1p());
                    assert!(
                        law < 2.0,
                        "{round:?} at {speed} m/s: the law's exponent {law}"
                    );
                    assert!(
                        (measured - law).abs() < TOLERANCE,
                        "{round:?} at {speed} m/s: exponent {measured} vs the law's {law}"
                    );
                }
            }
        }
    }

    /// The law keeps its finite value across the nose factor's whole normal f32 domain. As
    /// `N → 0` it tends to the inertia-free limit `1000 · m · v² / (2 A a)`; a nose factor at the
    /// smallest normal f32 must reproduce that limit, and its inverse the speed, rather than overflow
    /// an intermediate (`m / (2 A k)` alone is ~1e37 m there, DERIVED, past f32's range).
    #[test]
    fn capability_law_is_finite_across_the_nose_domain() {
        for body in LAW_ROUNDS {
            for nose_factor in [f32::MIN_POSITIVE, 1.0e-37, 1.0e-30, 1.0e-12] {
                let round = RoundBody {
                    nose_factor,
                    ..body
                };
                for speed in [100.0f32, 773.0, 2000.0] {
                    let got = capability(round, speed);
                    let caliber = round.caliber as f64;
                    let area = std::f64::consts::FRAC_PI_4 * caliber * caliber;
                    let limit = 1000.0 * round.mass as f64 * (speed as f64).powi(2)
                        / (2.0 * area * reference_steel().cavity_strength());
                    assert!(
                        got.is_finite() && (got as f64 / limit - 1.0).abs() < 1.0e-4,
                        "{round:?} at {speed} m/s: {got} vs the limit {limit}"
                    );
                    let back = speed_for(round, got);
                    assert!(
                        (back / speed - 1.0).abs() < 1.0e-4,
                        "{round:?}: {speed} m/s round-trips to {back}"
                    );
                }
            }
        }
    }

    /// At a fixed round, capability strictly increases with speed.
    #[test]
    fn capability_law_strictly_increases_in_speed() {
        for round in LAW_ROUNDS {
            let mut last = 0.0;
            for step in 1..=60 {
                let speed = 30.0 * step as f32;
                let c = capability(round, speed);
                assert!(c > last, "{round:?}: {speed} m/s gives {c} ≤ {last}");
                last = c;
            }
        }
    }

    /// After one crossing whose limit speed is `v_l`, the residual obeys
    /// `v_r² = (v² − v_l²) / (1 + k v_l² / a)`.
    #[test]
    fn capability_law_residual_matches_the_limit_speed_identity() {
        for round in LAW_ROUNDS {
            let inertia = round.terms().1 as f32;
            for speed in [400.0f32, 773.0, 1500.0] {
                for fraction in [0.1f32, 0.5, 0.9] {
                    let limit = fraction * speed;
                    let residual =
                        speed_for(round, capability(round, speed) - capability(round, limit));
                    let expected =
                        ((speed * speed - limit * limit) / (1.0 + inertia * limit * limit)).sqrt();
                    assert!(
                        (residual / expected - 1.0).abs() < 1.0e-3,
                        "{round:?}: {speed} m/s through v_l {limit}: {residual} vs {expected}"
                    );
                }
            }
        }
    }

    /// `drag_k` is calibrated so the 88 keeps its hand-tuned coefficient, and a light-for-bore round
    /// (the 7.9 mm coax) bleeds far faster from sectional density alone — the reason a coax drops more
    /// than the main gun at the same range, with no per-weapon drag field.
    #[test]
    fn drag_k_calibration() {
        let main = drag_k(0.088, 10.2); // 88 mm, 10.2 kg
        let coax = drag_k(0.0079, 0.0118); // 7.9 mm, 11.8 g
        assert!(
            (main - 2.0e-4).abs() < 1.0e-5,
            "88 drag k should be ≈ 2e-4, got {main}"
        );
        assert!(
            coax > 6.0 * main,
            "coax should bleed far faster than the 88 (got {coax} vs {main})"
        );
    }

    /// Drag only slows a shell — never speeds it up or reverses it — and gravity always pulls the
    /// vertical component down. Guards the analytic drag step against a sign or stability slip.
    #[test]
    fn freeflight_step_bleeds_speed_and_falls() {
        let v0 = Vec3::new(700.0, 0.0, 0.0);
        let v1 = freeflight_step(v0, drag_k(0.088, 10.2), 0.01);
        assert!(v1.length() < v0.length(), "drag must reduce speed");
        assert!(v1.y < 0.0, "gravity must pull the shell down");
    }

    /// The "one integrator" property: fast-forwarding a shell N ticks folds the same per-tick
    /// [`freeflight_step`] the live march steps in open air, so its velocity is N single-tick steps
    /// exactly. Guards against re-deriving the catch-up as a closed-form trajectory. Its position is
    /// held to the march's own by `march_tests::catch_up_lands_where_the_march_flies`.
    #[test]
    fn fast_forward_matches_single_tick_advances() {
        let origin = Vec3::new(1.0, 50.0, -3.0);
        let v0 = Vec3::new(600.0, 20.0, 0.0);
        let k = drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        let n = 7;

        let mut vel = v0;
        for _ in 0..n {
            vel = freeflight_step(vel, k, dt);
        }

        let (ff_pos, ff_vel, path) = fast_forward_shell(origin, v0, k, dt, n);
        assert_eq!(ff_vel, vel, "fast-forward velocity == N single steps");
        // One point per stepped tick plus the origin, and the trail starts AT the muzzle (requirement:
        // the tracer trail must not start 64 m behind the shell).
        assert_eq!(path.len(), n as usize + 1);
        assert_eq!(path[0], origin, "the trail starts at the muzzle");
        assert_eq!(*path.last().unwrap(), ff_pos, "the trail ends at the shell");
    }

    /// Zero catch-up is an exact no-op: the shell stays at the muzzle with its launch velocity and a
    /// one-point trail — byte-identical to a locally fired shell (SP / sandbox / own predicted), which
    /// always passes `catch_up_ticks: 0`.
    #[test]
    fn zero_catch_up_is_noop() {
        let origin = Vec3::new(0.0, 2.0, 0.0);
        let v0 = Vec3::new(800.0, 0.0, 0.0);
        let k = drag_k(0.088, 10.2);
        let (pos, vel, path) = fast_forward_shell(origin, v0, k, 1.0 / 64.0, 0);
        assert_eq!(pos, origin, "no catch-up leaves the shell at the muzzle");
        assert_eq!(vel, v0, "no catch-up leaves the launch velocity");
        assert_eq!(path, vec![origin], "no catch-up traces only the muzzle");
    }

    #[test]
    fn elapsed_ticks_is_wrap_aware_and_rejects_future_ticks() {
        assert_eq!(elapsed_ticks(105, 103), Some(2));
        assert_eq!(
            elapsed_ticks(3, u32::MAX - 2),
            Some(6),
            "a real six-tick interval survives the u32 wrap"
        );
        assert_eq!(
            elapsed_ticks(103, 105),
            None,
            "a future outcome is not misread as billions of elapsed ticks"
        );
    }
}

/// Physics-backed march tests: an Avian world with a single steel `BallisticVolume` plate, an 88
/// round marched into it, and every `Impact` captured — so the new armor triggers (ricochet +
/// perforation) and the surface classification are exercised through the REAL `integrate_projectiles`
/// resolution, not mocked. Modelled on the sandbox's plate targets (`sandbox::spawn_targets`).
#[cfg(test)]
mod march_tests {
    use std::time::Duration;

    use avian3d::prelude::{
        AngularInertia, AngularVelocity, Collider, CollisionLayers, GravityScale, LayerMask,
        LinearVelocity, Mass, NoAutoAngularInertia, NoAutoMass, PhysicsPlugins, RigidBody,
    };
    use bevy::ecs::system::RunSystemOnce;
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// The sampling basis a hand-built test shell is born with — anchored on ITS OWN fire direction,
    /// exactly as `on_fire_shell` would. A frame anchored on some other direction is not merely
    /// mis-rolled: it has a component ALONG this shell's travel, which puts ring samples in front of
    /// and behind the disc.
    fn test_disc(direction: Vec3) -> walk::DiscFrame {
        walk::DiscFrame::anchored(direction).expect("a fixture fires along a real direction")
    }

    /// One captured impact — the fields the armor read branches on.
    #[derive(Clone, Copy)]
    struct Captured {
        position: Vec3,
        surface: ImpactSurface,
        verdict: Verdict,
        deflection: Option<Vec3>,
    }

    /// The capture sink: every `Impact` the march fires lands here (view-only observer stand-in).
    #[derive(Resource, Default)]
    struct ImpactLog(Vec<Captured>);

    fn capture_impact(impact: On<Impact>, mut log: ResMut<ImpactLog>) {
        log.0.push(Captured {
            position: impact.position,
            surface: impact.surface,
            verdict: impact.outcome.verdict(),
            deflection: impact.outcome.deflection(),
        });
    }

    #[derive(Resource, Default)]
    struct DamageLog(Vec<(ShotId, u32)>);

    fn capture_damage(damage: On<ShellDamage>, mut log: ResMut<DamageLog>) {
        log.0.push((damage.shot, damage.amount));
    }

    /// Steel: reference-mm of armor per metre of material, so a plate's cost ≈ its thickness in mm
    /// (matches `sandbox::spawn_targets`).
    const STEEL: f32 = 1000.0;

    /// Fixture rounds: the 88 and the 7.92 mm as the fixture tank's spec authors them.
    pub(super) const FIXTURE_88: RoundBody = RoundBody {
        mass: 10.2,
        caliber: 0.088,
        nose_factor: 7.0 / 24.0,
    };
    pub(super) const FIXTURE_MG: RoundBody = RoundBody {
        mass: 0.0118,
        caliber: 0.0079,
        nose_factor: 0.5,
    };
    /// Fixture wound rate (reference-mm per metre of witness chord) for [`fixture_round`]: the
    /// main-gun value `rounds.ron` tunes, a labelled fixture number.
    pub(super) const FIXTURE_WOUND: f32 = 30.0;

    /// A fixture round's flight state and its initial readout, both from one body, so the readout
    /// the march starts from is the capability of the round it marches.
    pub(super) fn fixture_round(body: RoundBody, velocity: Vec3) -> (Projectile, ShellReadout) {
        let speed = velocity.length();
        (
            Projectile {
                velocity,
                round: bound(body),
                wound: FIXTURE_WOUND,
                drag_k: drag_k(body.caliber, body.mass),
                // Anchored on THIS round's fire direction, exactly as `on_fire_shell` would.
                disc: test_disc(velocity.normalize()),
                continuation: None,
            },
            ShellReadout {
                speed,
                capability: capability(body, speed),
            },
        )
    }

    /// Build an Avian world with one static steel plate (full extents `size`, centred at `at`, facing
    /// ±Z) on the `Armor` layer, register the real march + the impact capture, and settle the physics
    /// so the spatial-query pipeline includes the plate before any shell is marched.
    ///
    /// SIM ONLY — the server's composition. A shell here is born bare, which is what almost every
    /// gate in this module wants; the ones that assert on what a round is DRAWN as use
    /// [`view_world_with_plate`] instead.
    fn world_with_plate(size: Vec3, at: Vec3) -> App {
        plate_world(size, at, false)
    }

    /// [`world_with_plate`] plus the PRESENTATION half — a CLIENT's composition, the way
    /// `ClientPlugin` mounts the two. The fixture for the hold-visibility law: `Held` is the sim
    /// fact, and only with [`view::plugin`] mounted does anything derive a `Visibility` from it.
    fn view_world_with_plate(size: Vec3, at: Vec3) -> App {
        plate_world(size, at, true)
    }

    fn plate_world(size: Vec3, at: Vec3, view: bool) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            // Avian's collider cache reads `AssetEvent<Mesh>`, so the asset system must be present
            // even though these cuboid colliders carry no mesh handle.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        .init_resource::<RetainSpentShells>()
        .insert_resource(MgShortCircuit(false))
        .init_resource::<ImpactLog>()
        .add_observer(capture_impact)
        // The real march, run every Update (the `march_demo`/`march_real` run-if only selects the
        // clock; here `Res<Time>` is the virtual clock the manual duration steps).
        .add_systems(Update, integrate_projectiles);

        if view {
            // The asset stores `view::setup_assets` fills; the plate world only needed `Mesh`.
            app.init_asset::<StandardMaterial>()
                .init_asset::<WorldAsset>()
                .add_plugins(view::plugin);
        }

        // Drive plugin finish/cleanup by hand (a bare `update()` loop skips it) — Avian registers its
        // diagnostics resources in `Plugin::finish`, and the spatial-query systems require them.
        // Every plugin must be mounted BEFORE this: `add_plugins` panics once an App is finished.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();

        app.world_mut().spawn((
            Transform::from_translation(at),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "RHA".to_string(),
            },
        ));

        // Settle: let Avian register the static collider and build the spatial-query pipeline before
        // a shell is marched against it.
        for _ in 0..8 {
            app.update();
        }
        app
    }

    /// Spawn an 88 round at `origin` travelling `dir` (unit) at `speed`, then march until an impact is
    /// captured (or the bound trips). Returns every impact fired.
    fn fire_and_capture(app: &mut App, origin: Vec3, dir: Vec3, speed: f32) -> Vec<Captured> {
        app.world_mut().spawn((
            fixture_round(FIXTURE_88, dir * speed),
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![origin],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            Transform::from_translation(origin).looking_to(dir, Vec3::Y),
        ));
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<ImpactLog>().0.is_empty() {
                break;
            }
        }
        app.world().resource::<ImpactLog>().0.clone()
    }

    /// A volume crossing transfers momentum to its authority-owned tank body, while a replica keeps
    /// both linear and angular velocity untouched. The off-centre plate makes both effects visible.
    #[test]
    fn hit_impulse_changes_only_the_authority_body() {
        for replica in [false, true] {
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
            if replica {
                app.insert_resource(crate::ClientReplica);
            }
            let body = app
                .world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Transform::default(),
                    Mass(100.0),
                    AngularInertia::new(Vec3::splat(50.0)),
                    NoAutoMass,
                    NoAutoAngularInertia,
                    GravityScale(0.0),
                ))
                .id();
            let mut plates = app
                .world_mut()
                .query_filtered::<Entity, With<BallisticVolume>>();
            let plate = plates.single(app.world()).expect("one plate");
            app.world_mut().entity_mut(plate).insert(VolumeOf(body));
            app.update();

            let before = (
                app.world()
                    .get::<LinearVelocity>(body)
                    .expect("dynamic body has linear velocity")
                    .0,
                app.world()
                    .get::<AngularVelocity>(body)
                    .expect("dynamic body has angular velocity")
                    .0,
            );
            let impacts = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
            assert_eq!(impacts.len(), 1, "the owned plate was crossed once");
            let after = (
                app.world()
                    .get::<LinearVelocity>(body)
                    .expect("dynamic body keeps linear velocity")
                    .0,
                app.world()
                    .get::<AngularVelocity>(body)
                    .expect("dynamic body keeps angular velocity")
                    .0,
            );

            if replica {
                assert_eq!(after, before, "a replica never applies authority momentum");
            } else {
                assert_ne!(after.0, before.0, "the authority body absorbs momentum");
                assert_ne!(
                    after.1, before.1,
                    "the off-centre authority hit imparts angular velocity",
                );
            }
        }
    }

    /// SHOOTER SELF-EXCLUSION ([`not_own_volume`]): a round is transparent to the tank that FIRED it.
    ///
    /// The tiger's coax fires from inside its own mantlet on every round after a burst's first (its
    /// recoiling barrel retracts the muzzle ~10 cm; the muzzle clears the mantlet by ~7 cm), so the
    /// march's very first cast struck the shooter's own armour a centimetre out — embedding the round on
    /// the authority, and fail-closing the tracer on every net client. The plate here stands for that
    /// mantlet: a round fired from INSIDE it must fly straight out if the plate belongs to its shooter,
    /// and must still be stopped by the very same plate if it does not. That second half is the point —
    /// this is an exclusion, not a hole in the armour.
    #[test]
    fn a_shell_ignores_the_tank_that_fired_it() {
        use crate::damage::VolumeOf;

        // A thick plate the shell starts INSIDE (origin at z = 0, the plate's centre) — the muzzle
        // buried in its own mask.
        for own in [true, false] {
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.4), Vec3::new(0.0, 2.0, 0.0));
            // The "tank" that owns the plate, and the shooter — the same entity or not, per the arm.
            let tank = app.world_mut().spawn(Transform::default()).id();
            let shooter = if own {
                tank
            } else {
                app.world_mut().spawn(Transform::default()).id()
            };
            let mut plates = app
                .world_mut()
                .query_filtered::<Entity, With<BallisticVolume>>();
            let plate = plates.single(app.world()).expect("one plate");
            app.world_mut().entity_mut(plate).insert(VolumeOf(tank));

            let origin = Vec3::new(0.0, 2.0, 0.0);
            let dir = Vec3::NEG_Z;
            let shell = app
                .world_mut()
                .spawn((
                    fixture_round(FIXTURE_MG, dir * 755.0),
                    DamageReport::default(),
                    TerminalReport::default(),
                    ShellPath {
                        points: vec![origin],
                        segment_starts: Vec::new(),
                    },
                    PenetrationMarks::default(),
                    SpallMarks::default(),
                    // The attribution the shell carries from `FireShell::shooter` — on the authority's
                    // shell, the shooter's own predicted shell, AND (since the coax fix) an observer's
                    // replica shell alike.
                    ShotSource {
                        tank: shooter,
                        weapon: 0,
                    },
                    Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                ))
                .id();
            app.update();

            let hits = app.world().resource::<ImpactLog>().0.clone();
            if own {
                assert!(
                    hits.is_empty(),
                    "a round fired from inside its OWN tank's armour must pass straight through it — \
                     the coax fires from inside its own mantlet every burst; got {} impact(s)",
                    hits.len(),
                );
                let flown = app
                    .world()
                    .get::<Transform>(shell)
                    .expect("the shell survives its own tank")
                    .translation
                    .distance(origin);
                assert!(
                    flown > 10.0,
                    "the round should fly a full step (~11.8 m at 755 m/s) out of its own tank; it \
                     moved {flown:.2} m",
                );
            } else {
                assert_eq!(
                    hits.len(),
                    1,
                    "the SAME plate must still stop a round from any other source — self-exclusion is \
                     an exclusion, not a hole in the armour",
                );
                assert_eq!(hits[0].surface, ImpactSurface::Armor);
            }
        }
    }

    /// Regression: cosmetic termination does not end authority damage or its one-shot report latch.
    #[test]
    fn damage_confirmation_survives_cosmetic_terminal_and_latches_first_positive_step() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.01), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<DamageLog>()
            .add_observer(capture_damage);

        let spawn_component = |app: &mut App, z: f32, current: u32| {
            app.world_mut()
                .spawn((
                    Transform::from_translation(Vec3::new(0.0, 2.0, z)),
                    RigidBody::Static,
                    Collider::cuboid(3.0, 3.0, 0.10),
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material: walk::VolumeMaterial::fixture(1.0),
                        substance: "RHA".to_string(),
                    },
                    ComponentHealth { current, max: 1000 },
                ))
                .id()
        };
        let spent = spawn_component(&mut app, -20.0, 0);
        let first_live = spawn_component(&mut app, -40.0, 1000);
        let second_live = spawn_component(&mut app, -60.0, 1000);
        for _ in 0..8 {
            app.update();
        }

        let shot = a_shot();
        let origin = Vec3::new(0.0, 2.0, 2.0);
        let shell = app
            .world_mut()
            .spawn((
                fixture_round(FIXTURE_88, Vec3::NEG_Z * 800.0),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Shot(shot),
                Transform::from_translation(origin).looking_to(Vec3::NEG_Z, Vec3::Y),
            ))
            .id();

        app.update();
        assert!(
            app.world()
                .get::<TerminalReport>(shell)
                .is_some_and(|r| r.0),
            "the first non-health perforation emits the cosmetic terminal"
        );
        assert_eq!(
            app.world().get::<Shot>(shell).map(|shot| shot.0),
            Some(shot),
            "cosmetic termination preserves authority damage attribution"
        );
        assert!(
            app.world().resource::<DamageLog>().0.is_empty(),
            "non-health armor emits no damage confirmation"
        );

        for _ in 0..8 {
            app.update();
        }
        let health = |entity| app.world().get::<ComponentHealth>(entity).unwrap().current;
        assert_eq!(health(spent), 0, "zero HP cannot produce a fake decrease");
        assert!(
            health(first_live) < 1000,
            "the first live component took damage"
        );
        assert!(
            health(second_live) < 1000,
            "the penetrator kept damaging later geometry"
        );
        assert_eq!(
            app.world().resource::<DamageLog>().0.len(),
            1,
            "one damaging shot produces one discrete confirmation across all later deposits"
        );
        assert_eq!(app.world().resource::<DamageLog>().0[0].0, shot);
        assert!(app.world().resource::<DamageLog>().0[0].1 > 0);
    }

    /// Regression: a clean perforation emits one penetrating armor impact at the entry face.
    #[test]
    fn head_on_perforation_fires_one_penetrating_armor_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
        assert_eq!(
            hits.len(),
            1,
            "a clean perforation fires exactly one impact"
        );
        let hit = hits[0];
        assert_eq!(
            hit.surface,
            ImpactSurface::Armor,
            "the struck face is armor"
        );
        assert_eq!(
            hit.verdict,
            Verdict::Through,
            "a clean perforation goes through (flame lick earned)"
        );
        assert!(hit.deflection.is_none(), "a perforation does not deflect");
        assert!(
            (hit.position.z - 0.025).abs() < 0.05,
            "the impact reads at the entry face, got z={}",
            hit.position.z
        );
    }

    /// Regression: an oblique non-overmatched strike emits one deflecting armor impact.
    #[test]
    fn oblique_ricochet_fires_one_deflecting_non_penetrating_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let hits = fire_and_capture(&mut app, Vec3::new(-1.0, 2.0, 0.6), dir, 800.0);
        assert_eq!(hits.len(), 1, "a ricochet fires exactly one impact");
        let hit = hits[0];
        assert_eq!(
            hit.surface,
            ImpactSurface::Armor,
            "the struck face is armor"
        );
        assert_eq!(
            hit.verdict,
            Verdict::Ricochet,
            "a ricochet bit no steel — no flame lick"
        );
        let deflect = hit
            .deflection
            .expect("a ricochet carries its outgoing direction");
        assert!(
            deflect.z > 0.0,
            "the bounce deflects back off the face (+Z), got {deflect:?}"
        );
    }

    /// The public verdict names all three armour outcomes from the real resolution: a graze that
    /// deflects is `Ricochet`, a round that bites a plate thicker than its capability is `Stopped`, and
    /// one that comes out the far side is `Through`. Plate thicknesses and the 75° graze are fixture
    /// data.
    #[test]
    fn the_impact_verdict_names_ricochet_stopped_and_through() {
        let armor_verdicts = |size: Vec3, origin: Vec3, dir: Vec3| -> Vec<Verdict> {
            let mut app = world_with_plate(size, Vec3::new(0.0, 2.0, 0.0));
            fire_and_capture(&mut app, origin, dir, 800.0)
                .iter()
                .filter(|hit| hit.surface == ImpactSurface::Armor)
                .map(|hit| hit.verdict)
                .collect()
        };
        let graze = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let head_on = Vec3::new(0.0, 2.0, 2.0);
        assert_eq!(
            armor_verdicts(Vec3::new(3.0, 3.0, 0.10), Vec3::new(-1.0, 2.0, 0.6), graze),
            [Verdict::Ricochet],
        );
        assert_eq!(
            armor_verdicts(Vec3::new(3.0, 3.0, 0.5), head_on, Vec3::NEG_Z),
            [Verdict::Stopped],
        );
        assert_eq!(
            armor_verdicts(Vec3::new(3.0, 3.0, 0.05), head_on, Vec3::NEG_Z),
            [Verdict::Through],
        );
    }

    /// A terminal entered the material, so the public verdicts it round-trips through are exactly
    /// `Through` and `Stopped`; a received verdict of any other kind is refused, not buffered.
    #[test]
    fn a_terminal_is_never_a_ricochet() {
        for terminal in [TerminalVerdict::Through, TerminalVerdict::Stopped] {
            let public = Verdict::from(terminal);
            assert!(public.entered());
            assert_eq!(TerminalVerdict::try_from(public), Ok(terminal));
            assert_eq!(ImpactOutcome::from(terminal).verdict(), public);
        }
        for refused in [Verdict::Ricochet, Verdict::Struck] {
            assert_eq!(TerminalVerdict::try_from(refused), Err(refused));
        }
    }

    /// `Ricochet` is only a resolved deflection off armour: every impact carries a deflected
    /// direction iff its verdict is `Ricochet`, and a stop that resolved no crossing — the terrain,
    /// a replica's fail-closed armour contact — is `Struck`. The 75° graze is fixture data.
    #[test]
    fn ricochet_is_only_a_deflection_off_armour() {
        let graze = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let graze_from = Vec3::new(-1.0, 2.0, 0.6);
        let plate =
            |thickness: f32| world_with_plate(Vec3::new(3.0, 3.0, thickness), Vec3::Y * 2.0);
        let head_on = Vec3::new(0.0, 2.0, 2.0);

        let bounce = fire_and_capture(&mut plate(0.10), graze_from, graze, 800.0);
        let stopped = fire_and_capture(&mut plate(0.5), head_on, Vec3::NEG_Z, 800.0);
        let through = fire_and_capture(&mut plate(0.05), head_on, Vec3::NEG_Z, 800.0);
        let mut replica = plate(0.10);
        replica.insert_resource(crate::ClientReplica);
        let replica = fire_and_capture(&mut replica, graze_from, graze, 800.0);
        let mut ground = world_with_plate(Vec3::splat(0.5), Vec3::splat(900.0));
        ground.insert_resource(flat_grid(30.0));
        let ground = fire_and_capture(
            &mut ground,
            Vec3::new(10.0, 80.0, -20.0),
            Vec3::NEG_Y,
            800.0,
        );

        let read = |hits: &[Captured]| -> Vec<(ImpactSurface, Verdict)> {
            hits.iter().map(|hit| (hit.surface, hit.verdict)).collect()
        };
        assert_eq!(read(&bounce), [(ImpactSurface::Armor, Verdict::Ricochet)]);
        assert_eq!(read(&replica), [(ImpactSurface::Armor, Verdict::Struck)]);
        assert_eq!(read(&ground), [(ImpactSurface::Terrain, Verdict::Struck)]);
        for hit in [bounce, stopped, through, replica, ground].iter().flatten() {
            assert_eq!(
                hit.verdict == Verdict::Ricochet,
                hit.deflection.is_some(),
                "a deflected direction iff a ricochet: {:?} {:?}",
                hit.verdict,
                hit.deflection,
            );
        }
    }

    /// Raise an unkeyed catch-up shell and return its fallback impacts.
    fn fire_shell_catch_up(app: &mut App, catch_up_ticks: u32) -> Vec<Captured> {
        app.add_observer(on_fire_shell);
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            nose_factor: FIXTURE_88.nose_factor,
            wound: 30.0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: None,
            tracer: true,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks,
            shot: None,
        });
        app.world_mut().flush();
        app.world().resource::<ImpactLog>().0.clone()
    }

    /// Regression: a keyed projectile has its shot identity at spawn.
    #[test]
    fn on_fire_shell_spawns_a_keyed_projectile_with_shot_already_present() {
        #[derive(Resource, Default)]
        struct SpawnedShot(Option<ShotId>);

        fn capture(
            add: On<Add, Projectile>,
            shots: Query<&Shot>,
            mut spawned: ResMut<SpawnedShot>,
        ) {
            spawned.0 = shots.get(add.entity).ok().map(|shot| shot.0);
        }

        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<SpawnedShot>()
            .add_observer(on_fire_shell)
            .add_observer(capture);

        let shot = a_shot();
        let shooter = Entity::PLACEHOLDER;
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            nose_factor: FIXTURE_88.nose_factor,
            wound: 30.0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: Some(ShotSource {
                tank: shooter,
                weapon: shot.weapon as usize,
            }),
            tracer: true,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks: 0,
            shot: Some(shot),
        });
        app.world_mut().flush();

        assert_eq!(
            app.world().resource::<SpawnedShot>().0,
            Some(shot),
            "Shot is present in the Projectile's initial spawn bundle",
        );
    }

    /// A FRESH unkeyed catch-up (≤ STALE_FIRE_TICKS) has no authority outcome to await, so its
    /// fail-closed fallback still fires one cosmetic impact read.
    #[test]
    fn fresh_unkeyed_catch_up_fires_the_fallback_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(&mut app, 5);
        assert_eq!(hits.len(), 1, "a fresh catch-up hit reads once");
        assert_eq!(hits[0].surface, ImpactSurface::Armor, "the plate is armor");
    }

    /// A STALE unkeyed catch-up (> STALE_FIRE_TICKS) whose flight fully resolves in the skip fires NO impact:
    /// the flash moment is long over, so the phantom would erupt a full splash + ground scar late from
    /// bare ground. It is suppressed by the same staleness bound the muzzle dressing uses.
    #[test]
    fn stale_unkeyed_catch_up_suppresses_the_fallback_impact() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(&mut app, STALE_FIRE_TICKS + 1);
        assert!(
            hits.is_empty(),
            "a stale catch-up must fire no late phantom impact, got {}",
            hits.len()
        );
    }

    /// Every `FireShell` producer shares the allocation boundary, not only network receive. An
    /// oversized catch-up therefore fails closed before it can materialize a shell or path.
    #[test]
    fn oversized_fire_shell_catch_up_fails_closed_before_spawning() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(&mut app, MAX_COSMETIC_CATCH_UP_TICKS + 1);

        assert!(hits.is_empty(), "a rejected catch-up produces no impact");
        let mut projectiles = app.world_mut().query_filtered::<Entity, With<Projectile>>();
        assert!(
            projectiles.iter(app.world()).next().is_none(),
            "a rejected catch-up creates no projectile"
        );
    }

    /// REPRO: a remote shell whose catch-up span crosses armor must keep a keyed consumer alive for the
    /// authority's later ricochet. The client is not allowed to turn its interpolated-pose chord into an
    /// impact and discard the shell: doing so produces exactly the reported picture — an impact at the
    /// plate, followed by no post-bounce round or trail when the real keyframe arrives.
    #[test]
    fn armor_catch_up_waits_for_sanctioned_bounce_and_continues() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        app.insert_resource(crate::ClientReplica);
        app.init_resource::<SanctionedShots>();
        app.insert_resource(crate::ShotClock(120));
        app.add_observer(on_fire_shell);

        let shot = a_shot();
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            nose_factor: FIXTURE_88.nose_factor,
            wound: 30.0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: None,
            tracer: true,
            shot_origin: FireShellOrigin::Reconstructed,
            catch_up_ticks: 20,
            shot: Some(shot),
        });
        app.world_mut().flush();

        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "an observer may not improvise an armor impact during catch-up"
        );
        let shell = app
            .world_mut()
            .query_filtered::<Entity, (With<Projectile>, With<Shot>, With<Held>)>()
            .single(app.world())
            .expect("the armor catch-up keeps one held, keyed shell waiting for authority");
        // `Held` IS the invisible stop — the view derives `Visibility::Hidden` from it, and only
        // where something is drawn at all (`ballistics::view`, which this sim fixture never mounts).
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "the candidate shell stops and waits for authority instead of resolving locally"
        );

        // The shot was already old when its fire event arrived, but that age must not consume the
        // grace window. Give the verdict one full client fixed tick to arrive; the shell must remain.
        app.update();
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "pre-receive catch-up age is not time spent waiting for a verdict"
        );

        // The authoritative outcome arrives after the fire. Its point/direction deliberately come from
        // the server, not from the client's catch-up chord. The lateral displacement makes any
        // accidental correction chord large enough that the segment-break assertion has teeth.
        let bounce_origin = Vec3::new(4.0, 2.0, 0.03);
        let bounce_direction = Vec3::Z;
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce_origin,
                direction: bounce_direction,
                speed: 480.0,
                bounce_tick: 101,
                sequence: 0,
            },
        );
        app.update();

        // The hold is released, which is also what un-hides the round wherever it is drawn.
        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the sanctioned keyframe releases the held catch-up shell and it flies on"
        );
        let marks = app
            .world()
            .get::<PenetrationMarks>(shell)
            .expect("the shell survives the bounce");
        assert_eq!(
            marks.ricochets,
            vec![bounce_origin],
            "the shell consumes exactly the server's sanctioned bounce"
        );
        let path = app
            .world()
            .get::<ShellPath>(shell)
            .expect("the continued shell keeps its trail source");
        let bounce_index = path
            .points
            .iter()
            .rposition(|point| point.distance_squared(bounce_origin) < 1.0e-6)
            .expect("the authoritative bounce origin re-anchors ShellPath");
        assert_eq!(
            path.segment_starts.last().copied(),
            Some(bounce_index),
            "the authority re-seed is disconnected from the client-only candidate contact"
        );
        assert!(
            path.points[bounce_index + 1..]
                .iter()
                .any(|p| (*p - bounce_origin).dot(bounce_direction) > 1.0),
            "ShellPath contains travel strictly after the bounce for the remote trail"
        );
    }

    /// THE HOLD-VISIBILITY LAW, on a client's composition: `Held` is the sim fact, and `Visibility`
    /// is derived from it by `ballistics::view` alone. This is the CATCH-UP half — a shell born
    /// holding, from the same spawn bundle.
    ///
    /// The two are separate tests because the two hides reach the round by different routes:
    /// dressing reads `Has<Held>` on a round that is ALREADY holding when it is born, while
    /// [`a_mid_flight_hold_hides_the_drawn_round`] goes through the `Add` adapter on a round that
    /// was drawn visible first. Removing either one leaves a shell stuck on the plate or invisible
    /// for the rest of its flight, and no assertion on `Held` alone can tell.
    #[test]
    fn a_shell_born_holding_is_hidden_from_its_first_frame() {
        let mut app = view_world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        app.insert_resource(crate::ClientReplica);
        app.init_resource::<SanctionedShots>();
        app.insert_resource(crate::ShotClock(120));
        app.add_observer(on_fire_shell);

        let shot = a_shot();
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            nose_factor: FIXTURE_88.nose_factor,
            wound: 30.0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: None,
            tracer: true,
            shot_origin: FireShellOrigin::Reconstructed,
            catch_up_ticks: 20,
            shot: Some(shot),
        });
        // FLUSH, not update: the spawn, its `Add` observers and their commands all land in this one
        // flush, so this asserts the round's FIRST frame — it is never drawn on the plate, not even
        // for the frame between being born and the first hide.
        app.world_mut().flush();

        let shell = app
            .world_mut()
            .query_filtered::<Entity, (With<Projectile>, With<Held>)>()
            .single(app.world())
            .expect("the armor catch-up keeps one held, keyed shell waiting for authority");
        assert!(
            app.world().get::<ShellVisual>(shell).is_some(),
            "an 88 is dressed even while it holds — otherwise this test proves nothing about \
             visibility",
        );
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Hidden),
            "a shell BORN holding must be hidden from its first frame — a visible one is a round \
             frozen on the plate while the client waits for the authority",
        );

        // The authority's keyframe releases the hold, and the same law shows the round again.
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(4.0, 2.0, 0.03),
                direction: Vec3::Z,
                speed: 480.0,
                bounce_tick: 101,
                sequence: 0,
            },
        );
        app.update();

        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the keyframe releases the hold",
        );
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "releasing the hold must show the round again — a shell that stays hidden flies the \
             rest of its trajectory invisibly",
        );
    }

    /// THE HOLD-VISIBILITY LAW, mid-flight half: a round drawn visible reaches armor on a replica,
    /// holds, and must go hidden — then show again on the authority's re-seed. See
    /// [`a_shell_born_holding_is_hidden_from_its_first_frame`] for why the two are split.
    #[test]
    fn a_mid_flight_hold_hides_the_drawn_round() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = view_world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        app.insert_resource(crate::ClientReplica);
        app.insert_resource(SanctionedShots::default()); // keyframe not yet arrived
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().flush();

        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "a round with no hold is drawn — the mid-flight hide below has to change something",
        );

        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        assert!(froze, "the keyed shell holds at armor contact");
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Hidden),
            "the hold is an INVISIBLE stop — no frozen round hanging on the plate",
        );

        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update();

        assert!(app.world().get::<Held>(shell).is_none(), "hold cleared");
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "the re-seeded shell is shown again",
        );
    }

    /// Count the live `Projectile`s in the world (shells still in flight).
    fn live_projectiles(app: &mut App) -> usize {
        app.world_mut()
            .query::<&Projectile>()
            .iter(app.world())
            .count()
    }

    /// The oblique-ricochet setup from `oblique_ricochet_fires_one_deflecting_non_penetrating_impact`,
    /// but marched to resolution: how many `Impact`s, and does the shell SURVIVE the bounce (continue
    /// in flight)? Parameterised by whether the world is a net client (`ClientReplica` present).
    fn oblique_ricochet_outcome(replica: bool) -> (Vec<Captured>, usize) {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        if replica {
            app.insert_resource(crate::ClientReplica);
        }
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let hits = fire_and_capture(&mut app, Vec3::new(-1.0, 2.0, 0.6), dir, 800.0);
        let survivors = live_projectiles(&mut app);
        (hits, survivors)
    }

    /// REPRO (symptom 2 — post-ricochet trail loss). AUTHORITY path (no `ClientReplica`): the server
    /// ricochets the 88 off the plate and the shell CONTINUES in flight (survives), carrying its trail
    /// on past the bounce. This is correct on the authority.
    #[test]
    fn authority_ricochet_shell_survives_and_continues() {
        let (hits, survivors) = oblique_ricochet_outcome(false);
        assert_eq!(hits.len(), 1, "one bounce impact");
        assert!(hits[0].deflection.is_some(), "authority reads a ricochet");
        assert_eq!(survivors, 1, "the ricocheted shell continues in flight");
    }

    /// FIX (symptom 2). REPLICA path (`ClientReplica` present — the remote observer): the client must
    /// NOT re-simulate the authoritative bounce against interpolated geometry. Fail-closed — the
    /// cosmetic shell STOPS dead at first armor contact (despawned, no survivor), firing a NEUTRAL
    /// armor spark (no deflection fan, no flame lick), so its trail ends at contact instead of chasing
    /// an improvised deflection the server never sanctioned.
    #[test]
    fn replica_ricochet_fails_closed_at_first_armor_contact() {
        let (hits, survivors) = oblique_ricochet_outcome(true);
        assert_eq!(hits.len(), 1, "one armor-contact spark");
        assert_eq!(hits[0].surface, ImpactSurface::Armor, "it hit armor");
        assert!(
            hits[0].deflection.is_none(),
            "no improvised bounce fan — the client cannot know the outcome"
        );
        assert!(!hits[0].verdict.entered(), "no flame lick — neutral spark");
        assert_eq!(
            survivors, 0,
            "the cosmetic shell stops dead at contact (trail ends there)"
        );
    }

    /// The fail-closed guard is REPLICA-ONLY: a head-on perforation on the AUTHORITY still drives the
    /// round through the plate and out the far side (the server's shell continues), unaffected by the
    /// guard. Pins that the `!deposit` gate did not leak into the authority march.
    #[test]
    fn authority_perforation_still_drives_through() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
        assert_eq!(hits.len(), 1, "one perforation impact");
        assert!(
            hits[0].verdict.entered(),
            "authority perforates (flame lick)"
        );
    }

    // --- Keyframe carry-through (piece 2) ------------------------------------------------------------

    /// The fields of a `ShellRicochet` the authority raised — the server-sanctioned bounce.
    #[derive(Clone, Copy)]
    struct CapturedBounce {
        origin: Vec3,
        direction: Vec3,
        speed: f32,
        sequence: u32,
    }

    #[derive(Resource, Default)]
    struct RicochetLog(Vec<CapturedBounce>);

    fn capture_ricochet(ricochet: On<ShellRicochet>, mut log: ResMut<RicochetLog>) {
        log.0.push(CapturedBounce {
            origin: ricochet.origin,
            direction: ricochet.direction,
            speed: ricochet.speed,
            sequence: ricochet.sequence,
        });
    }

    /// The oblique-ricochet geometry (the same steep graze the existing ricochet test uses), started at
    /// the standard point and carrying `shot` so the authority march raises a `ShellRicochet`.
    fn spawn_oblique_shell(app: &mut App, shot: ShotId) -> Entity {
        let origin = Vec3::new(-1.0, 2.0, 0.6);
        let dir = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let speed = 800.0;
        app.world_mut()
            .spawn((
                fixture_round(FIXTURE_88, dir * speed),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// A shell fired into OPEN AIR (away from any plate), carrying `shot` — used by the F1/F3 tests
    /// that need a cosmetic round which free-flies without ever contacting armor.
    fn spawn_free_shell(
        app: &mut App,
        origin: Vec3,
        dir: Vec3,
        speed: f32,
        shot: ShotId,
    ) -> Entity {
        app.world_mut()
            .spawn((
                fixture_round(FIXTURE_88, dir.normalize() * speed),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// Run the AUTHORITY (no `ClientReplica`) oblique ricochet and return the sanctioned bounce it
    /// raises — the server truth an observer must re-seed from.
    fn authority_bounce(shot: ShotId) -> CapturedBounce {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<RicochetLog>();
        app.add_observer(capture_ricochet);
        spawn_oblique_shell(&mut app, shot);
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<RicochetLog>().0.is_empty() {
                break;
            }
        }
        *app.world()
            .resource::<RicochetLog>()
            .0
            .first()
            .expect("the authority raised a ShellRicochet for the oblique shot")
    }

    fn a_shot() -> ShotId {
        ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 100,
        }
    }

    /// A replica world (`ClientReplica` + a `SanctionedShots` buffer + the plate) ready to march an
    /// observer shell.
    fn replica_world(bounces: SanctionedShots) -> App {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        app.insert_resource(crate::ClientReplica);
        app.insert_resource(bounces);
        app
    }

    /// CARRY-THROUGH (pre-armed, the common case). The keyframe is already buffered when the observer
    /// shell reaches armor contact: it re-seeds onto server truth and CONTINUES — the trail runs through
    /// the exact bounce point (no gap), the post-bounce direction matches the server's within
    /// integration tolerance, one directional (sanctioned) spark reads, and the shell survives.
    #[test]
    fn observer_reseeds_from_prearmed_keyframe_and_continues() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);
        assert_eq!(bounce.sequence, 0, "the first bounce is ordinal 0");

        let mut buf = SanctionedShots::default();
        buf.insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `ShotClock` resource → the overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        // March until the shell re-seeds (a ricochet recorded) or the bound trips.
        for _ in 0..8 {
            app.update();
            if !app
                .world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .is_empty()
            {
                break;
            }
        }

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1, "exactly one re-seed");
        assert!(
            marks.ricochets[0].distance(bounce.origin) < 1.0e-3,
            "re-seeded at the exact server bounce point (tracer clamp re-anchors here)",
        );
        let path = app.world().get::<ShellPath>(shell).unwrap();
        assert!(
            path.points
                .iter()
                .any(|p| p.distance(bounce.origin) < 1.0e-3),
            "the trail runs THROUGH the bounce point — no gap in the ribbon",
        );
        let velocity = app.world().get::<Projectile>(shell).unwrap().velocity;
        let angle = velocity
            .normalize()
            .angle_between(bounce.direction.normalize());
        assert!(
            angle < 0.05,
            "post-bounce direction matches server truth within integration tolerance (got {angle} rad)",
        );
        assert!(
            velocity.z > 0.0,
            "the shell deflects back off the +Z face and flies on — the server's outcome",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives the bounce (it continues, it does not truncate)",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "one bounce spark");
        assert!(
            impacts[0].deflection.is_some(),
            "the sanctioned bounce reads with its directional fan, not the neutral fail-closed spark",
        );
    }

    /// HOLD-THEN-ARRIVE. The keyframe is delayed past contact but lands inside the grace window: the
    /// shell FREEZES at the plate (no spark yet), then re-seeds when the bounce arrives — re-aged
    /// forward by exactly the ticks it held, so its resumed position is consistent with the present
    /// timeline (matches the shared integrator advanced by the hold duration).
    #[test]
    fn observer_holds_then_reseeds_when_keyframe_arrives_in_window() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = replica_world(SanctionedShots::default()); // buffer starts EMPTY
        let shell = spawn_oblique_shell(&mut app, shot);

        // March until the shell freezes at armor (no keyframe yet).
        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        assert!(
            froze,
            "the shell holds at armor contact awaiting its keyframe"
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "a held shell fires NO spark — it waits (no improvised impact)",
        );

        // Hold a few more ticks (still inside the grace window), then the keyframe lands.
        const HELD_EXTRA: u32 = 4;
        for _ in 0..HELD_EXTRA {
            app.update();
        }
        assert_eq!(
            app.world().get::<Held>(shell).unwrap().waited,
            HELD_EXTRA,
            "the hold accumulates one tick per frozen tick (the catch-up it will re-age by)",
        );
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert in these tests (no `ShotClock` resource → the overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update(); // the Held handler re-seeds this tick

        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the hold clears on re-seed",
        );
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1);
        assert!(
            marks.ricochets[0].distance(bounce.origin) < 1.0e-3,
            "the tracer clamp re-anchors at the true bounce point",
        );
        // Re-aged forward by the hold: the resumed position is the sanctioned state fast-forwarded
        // HELD_EXTRA ticks — the exact shared integrator, so it is consistent with the present
        // timeline — lifted off the struck face (the plate's `+Z`) as the authority's resume was.
        let dt = 0.016; // the test world's ManualDuration
        let (caught_up, velocity, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
        let expected_pos = caught_up
            + Vec3::Z * resolve::disc_radius(FIXTURE_88.caliber)
            + velocity.normalize() * MARCH_EPS;
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "resumed position = sanctioned bounce fast-forwarded by the hold duration, lifted (got {pos}, want {expected_pos})",
        );
        assert!(
            pos.distance(bounce.origin) > 1.0e-2,
            "the re-aged shell is downrange of the bounce point (it caught back up to the present)",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives and continues after the delayed re-seed",
        );
    }

    /// A HELD ROUND RELEASED WITH NO AGE LEAVES THE FACE IT BOUNCED OFF.
    ///
    /// The keyframe lands the tick after the hold, so the catch-up is zero ticks old and ends on
    /// the sanctioned bounce point itself — on the struck face. A fresh outgoing disc laid there
    /// has half its ring behind that face, inside the plate, and its next step contacts the plate
    /// again and holds for a second bounce the authority never sent. The released round must fly on
    /// instead.
    #[test]
    fn a_round_released_with_no_age_does_not_contact_the_consumed_face() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);
        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        assert!(
            froze,
            "the shell holds at armor contact awaiting its keyframe"
        );
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                // Inert: no `ShotClock`, so the age is the hold's own, zero on its first tick.
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update(); // released, zero ticks old
        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the hold clears on re-seed"
        );
        for _ in 0..4 {
            app.update();
            assert!(
                app.world().get::<Held>(shell).is_none(),
                "the released round contacted the face it already bounced off and holds again",
            );
        }
        assert_eq!(
            app.world()
                .get::<PenetrationMarks>(shell)
                .map(|marks| marks.ricochets.len()),
            Some(1),
            "the round flies on with the one sanctioned bounce",
        );
    }

    /// Regression: an unresolved hold expires without a fabricated impact.
    #[test]
    fn observer_hold_expires_to_quiet_dissolve() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default()); // no keyframe ever arrives
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..(RICOCHET_HOLD_TICKS + 4) {
            app.update();
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shell is finalized (despawned) once the grace window expires",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "NO spark — a fabricated contact the authority never confirmed would violate the honesty \
             doctrine; the shell dissolves quietly",
        );
    }

    /// Where the uncertifiable fixture sits: far from the plate, in open air. Fixture data.
    const UNCERTIFIABLE_AT: Vec3 = Vec3::new(0.0, 20.0, 5.0);

    /// An armour volume no containment probe can read — a compound, neither a certified trimesh nor
    /// convex — around [`UNCERTIFIABLE_AT`], so a fresh start laid there cannot be certified.
    /// Settled before returning.
    fn spawn_uncertifiable(app: &mut App) {
        app.world_mut().spawn((
            Transform::from_translation(UNCERTIFIABLE_AT),
            RigidBody::Static,
            Collider::compound(vec![(
                Vec3::ZERO,
                Quat::IDENTITY,
                Collider::cuboid(1.0, 1.0, 1.0),
            )]),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "fixture".to_string(),
            },
        ));
        for _ in 0..8 {
            app.update();
        }
    }

    /// A REPLICA THAT CANNOT CERTIFY A FRESH START SHOWS NO ARMOUR VERDICT FOR IT.
    ///
    /// The authority owns every armour verdict. A keyed replica whose fresh start cannot be
    /// certified holds hidden, as at an unresolved armour contact, emitting nothing; the authority's
    /// next outcome then governs it.
    #[test]
    fn a_replica_that_cannot_certify_a_start_holds_for_the_authority() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        spawn_uncertifiable(&mut app);
        let shell = spawn_free_shell(&mut app, UNCERTIFIABLE_AT, Vec3::Z, 800.0, shot);
        app.update();
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no armour verdict is fabricated for an uncertified start",
        );
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "the shell holds for the authority's outcome rather than ending",
        );

        let server_pos = UNCERTIFIABLE_AT + Vec3::Z * 0.4;
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: server_pos,
                    normal: Vec3::Z,
                    verdict: TerminalVerdict::Through,
                    impact_tick: 0, // inert (no `ShotClock` — overdue path off)
                    after_bounces: 0,
                },
            );
        app.update();
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "the authority's terminal is the one read");
        assert_eq!(impacts[0].verdict, Verdict::Through);
        assert!(impacts[0].position.distance(server_pos) < 1.0e-6);
        assert!(
            app.world().get_entity(shell).is_err(),
            "the sanctioned terminal ends the shell",
        );
    }

    /// A pen cam opens only for a hit on armour that resolved one of its verdicts, read
    /// from the shell's whole logged path: the ricochet, the stop and the perforation each open
    /// one; a ground hit and a fail-closed armour contact open none. Plate thicknesses and the 75°
    /// graze are fixture data.
    #[test]
    fn a_pen_cam_opens_only_for_a_resolved_armour_hit() {
        use crate::damage_cam::{ShotVerdict, record::path_steps};
        let opened = |app: &mut App| -> Option<ShotVerdict> {
            let shells: Vec<Entity> = app
                .world_mut()
                .query_filtered::<Entity, With<PenetrationMarks>>()
                .iter(app.world())
                .collect();
            assert_eq!(shells.len(), 1, "one shell, retained");
            ShotVerdict::of_path(&path_steps(app.world(), shells[0], None))
        };
        let retained = |mut app: App| {
            app.insert_resource(RetainSpentShells(true));
            app
        };
        let graze = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        )
        .normalize();
        let plate = |thickness: f32| {
            retained(world_with_plate(
                Vec3::new(3.0, 3.0, thickness),
                Vec3::Y * 2.0,
            ))
        };
        let head_on = Vec3::new(0.0, 2.0, 2.0);
        for (thickness, origin, direction, verdict) in [
            (
                0.10,
                Vec3::new(-1.0, 2.0, 0.6),
                graze,
                ShotVerdict::Ricochet,
            ),
            (0.5, head_on, Vec3::NEG_Z, ShotVerdict::NonPen),
            (0.05, head_on, Vec3::NEG_Z, ShotVerdict::Penetration),
        ] {
            let mut app = plate(thickness);
            fire_and_capture(&mut app, origin, direction, 800.0);
            assert_eq!(opened(&mut app), Some(verdict), "{thickness} m plate");
        }

        let mut ground = retained(world_with_plate(Vec3::splat(0.5), Vec3::splat(900.0)));
        ground.insert_resource(flat_grid(30.0));
        let hits = fire_and_capture(
            &mut ground,
            Vec3::new(10.0, 80.0, -20.0),
            Vec3::NEG_Y,
            800.0,
        );
        assert_eq!(hits.len(), 1, "one ground read");
        assert_eq!(hits[0].surface, ImpactSurface::Terrain);
        assert_eq!(opened(&mut ground), None);

        let mut failed = plate(0.10);
        spawn_uncertifiable(&mut failed);
        spawn_free_shell(&mut failed, UNCERTIFIABLE_AT, Vec3::Z, 800.0, a_shot());
        failed.update();
        let hits = failed.world().resource::<ImpactLog>().0.clone();
        assert_eq!(hits.len(), 1, "one fail-closed read");
        assert_eq!(hits[0].surface, ImpactSurface::Armor);
        assert_eq!(opened(&mut failed), None);
    }

    /// The authority fails an uncertified fresh start closed, where it stands.
    #[test]
    fn the_authority_fails_an_uncertified_start_closed() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        spawn_uncertifiable(&mut app);
        let shell = spawn_free_shell(&mut app, UNCERTIFIABLE_AT, Vec3::Z, 800.0, a_shot());
        app.update();
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "one fail-closed read: {}", impacts.len());
        assert_eq!(impacts[0].verdict, Verdict::Struck);
        assert!(impacts[0].position.distance(UNCERTIFIABLE_AT) < 1.0e-6);
        assert!(app.world().get_entity(shell).is_err(), "the round stops");
    }

    /// Regression: sanctioned bounces are consumed strictly by ordinal.
    #[test]
    fn observer_consumes_two_bounces_in_order() {
        let shot = a_shot();
        let b0 = SanctionedBounce {
            origin: Vec3::new(0.0, 2.0, 0.5),
            direction: Vec3::new(0.12, 0.0, -1.0).normalize(),
            speed: 480.0,
            bounce_tick: 0,
            sequence: 0,
        };
        let b1 = SanctionedBounce {
            origin: Vec3::new(0.0, 2.0, 0.05),
            direction: Vec3::Z,
            speed: 300.0,
            bounce_tick: 0,
            sequence: 1,
        };
        let mut buf = SanctionedShots::default();
        buf.insert(shot, b0);
        buf.insert(shot, b1);
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err()
                || app
                    .world()
                    .get::<PenetrationMarks>(shell)
                    .is_some_and(|m| m.ricochets.len() >= 2)
            {
                break;
            }
        }

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(
            marks.ricochets.len(),
            2,
            "both sanctioned bounces were consumed",
        );
        assert!(
            marks.ricochets[0].distance(b0.origin) < 1.0e-3,
            "bounce 0 (ordinal 0) re-seeds first",
        );
        assert!(
            marks.ricochets[1].distance(b1.origin) < 1.0e-3,
            "bounce 1 (ordinal 1) re-seeds second — strict order",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "after the second bounce the shell flies on",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 2, "two directional bounce sparks");
        assert!(impacts.iter().all(|i| i.deflection.is_some()));
    }

    /// Regression: an own shell follows the same hidden-hold and re-seed path as an observer shell.
    #[test]
    fn own_shell_holds_hidden_then_reseeds_when_keyframe_arrives() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);

        let mut app = replica_world(SanctionedShots::default()); // keyframe not yet arrived
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        let mut froze = false;
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                froze = true;
                break;
            }
        }
        // `Held` is the whole stop, and the whole reason the round goes invisible where it is drawn:
        // no frozen shell hanging on the plate. The visibility itself is `ballistics::view`'s.
        assert!(
            froze,
            "the own shell holds at armor contact like any Shot-carrying shell"
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no spark while holding",
        );

        const HELD_EXTRA: u32 = 3;
        for _ in 0..HELD_EXTRA {
            app.update();
        }
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                bounce_tick: 0,
                sequence: 0,
            },
        );
        app.update();

        // Hold cleared — which is also what shows the re-seeded round again, in the view.
        assert!(app.world().get::<Held>(shell).is_none(), "hold cleared");
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(
            marks.ricochets.len(),
            1,
            "the sanctioned bounce re-anchored the clamp"
        );
        // Re-aged by the held ticks — the same present − bounce_tick arithmetic as an observer
        // shell — and lifted off the struck face like it.
        let dt = 0.016;
        let (caught_up, velocity, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
        let expected_pos = caught_up
            + Vec3::Z * resolve::disc_radius(FIXTURE_88.caliber)
            + velocity.normalize() * MARCH_EPS;
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "own shell resumes at the sanctioned state fast-forwarded by the hold (got {pos}, want {expected_pos})",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shooter's own bounced round flies on — the fall-of-shot read",
        );
    }

    /// Regression: an own shell also expires without a fabricated impact.
    #[test]
    fn own_shell_keyframe_lost_dissolves_quietly() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default()); // no keyframe, ever
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        for _ in 0..(RICOCHET_HOLD_TICKS + 4) {
            app.update();
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "the own shell finalizes once the grace window expires",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no spark — the own shell dissolves quietly too (no fabricated contact)",
        );
    }

    /// Regression: an overdue authority bounce re-seeds a pose-divergent client miss.
    #[test]
    fn overdue_bounce_reseeds_a_pose_divergent_miss() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::ShotClock(shot.fire_tick + 20));
        let bounce_origin = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce_origin,
                direction: Vec3::Z,
                speed: 500.0,
                bounce_tick: shot.fire_tick + 5,
                sequence: 0,
            },
        );
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update();

        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(marks.ricochets.len(), 1, "the overdue bounce is consumed");
        assert!(
            marks.ricochets[0].distance(bounce_origin) < 1.0e-3,
            "re-seeded at the SERVER bounce point, not where the client's round flew",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the shell survives and flies on from the server bounce",
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(bounce_origin) > 1.0e-2,
            "re-aged forward of the bounce (caught up to the present)",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "the sanctioned bounce sparks");
        assert!(
            impacts[0].deflection.is_some(),
            "with its directional fan (server truth), not a fabricated neutral spark",
        );
    }

    /// CLIENT-MISS / SERVER-HIT (terminal). Same pose-divergence, but the server resolved an
    /// embed/perforation. The overdue path finalizes the shell at the SERVER's impact with the honest
    /// armor read (position, normal, `verdict`) rather than holding for a contact that never comes.
    #[test]
    fn overdue_terminal_finalizes_a_pose_divergent_miss() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::ShotClock(shot.fire_tick + 20));
        let impact_pos = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: impact_pos,
                    normal: Vec3::Z,
                    verdict: TerminalVerdict::Through,
                    impact_tick: shot.fire_tick + 5,
                    after_bounces: 0,
                },
            );
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shell finalizes at the server terminal",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "the server terminal reads");
        assert!(
            impacts[0].position.distance(impact_pos) < 1.0e-3,
            "at the SERVER's impact position",
        );
        assert!(
            impacts[0].verdict.entered(),
            "with the server's penetration verdict (the flame lick MP would otherwise miss)",
        );
    }

    /// The margin guard: a sanctioned outcome only a tick or two old is NOT force-consumed — the shell
    /// is still allowed to reach the plate and hold/contact normally, so a legitimately-imminent
    /// contact isn't snapped away mid-air.
    #[test]
    fn a_sanctioned_outcome_within_the_margin_is_not_force_consumed() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::ShotClock(shot.fire_tick + 5));
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(1.0, 2.0, 3.0),
                direction: Vec3::Z,
                speed: 500.0,
                // present(105) − bounce_tick(104) = 1 <= OVERDUE_MARGIN_TICKS — inside the margin.
                bounce_tick: shot.fire_tick + 4,
                sequence: 0,
            },
        );
        let shell = spawn_free_shell(&mut app, Vec3::new(0.0, 20.0, 5.0), Vec3::Z, 800.0, shot);

        app.update();

        assert!(
            app.world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .is_empty(),
            "inside the margin the shell is not force-consumed — it flies on toward a real contact",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "no premature spark inside the margin",
        );
    }

    // --- Terminal confirms (ImpactConfirm carry-through) ---------------------------------------------

    /// The fields of a `ShellTerminal` the authority raised — the server-sanctioned armor end.
    #[derive(Clone, Copy)]
    struct CapturedTerminal {
        position: Vec3,
        verdict: Verdict,
        after_bounces: u32,
    }

    #[derive(Resource, Default)]
    struct TerminalLog(Vec<CapturedTerminal>);

    fn capture_terminal(terminal: On<ShellTerminal>, mut log: ResMut<TerminalLog>) {
        log.0.push(CapturedTerminal {
            position: terminal.position,
            verdict: terminal.verdict.into(),
            after_bounces: terminal.after_bounces,
        });
    }

    /// A head-on shell at the standard plate (fired from z=+2 down −Z), carrying `shot`.
    fn spawn_headon_shell(app: &mut App, shot: ShotId) -> Entity {
        spawn_headon_shell_at(app, shot, Vec3::new(0.0, 2.0, 2.0))
    }

    /// The same shell, anywhere on the map. Position is a LAW input here, not decoration: f32 spacing
    /// coarsens with distance from the world origin, so a fixture that only ever fires two metres
    /// from it cannot see the tolerances that matter at combat range.
    fn spawn_headon_shell_at(app: &mut App, shot: ShotId, origin: Vec3) -> Entity {
        let dir = Vec3::NEG_Z;
        let speed = 800.0;
        app.world_mut()
            .spawn((
                fixture_round(FIXTURE_88, dir * speed),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// AUTHORITY EMISSION — embed. A 500 mm plate defeats the 88 head-on (cost 500 > ~263 cap): the
    /// march raises exactly ONE `ShellTerminal`, `Stopped` (an embed bit steel), zero prior bounces.
    #[test]
    fn authority_embed_emits_one_stopped_terminal() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.5), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        spawn_headon_shell(&mut app, a_shot());
        for _ in 0..8 {
            app.update();
            if !app.world().resource::<TerminalLog>().0.is_empty() {
                break;
            }
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "an embed emits exactly one terminal");
        assert_eq!(
            terminals[0].verdict,
            Verdict::Stopped,
            "an embed bit steel and stayed in it"
        );
        assert_eq!(terminals[0].after_bounces, 0);
    }

    /// AUTHORITY EMISSION — perforation, the documented choice: the terminal reads at the ENTRY face
    /// and the AUTHORITATIVE shell continues (it is not truncated by the cosmetic-terminal decision);
    /// a later embed of the same shot (a 500 mm backstop behind the plate) emits NO second terminal —
    /// at most one per shot, even across the same march step.
    #[test]
    fn authority_perforation_emits_one_terminal_and_marches_on() {
        // 50 mm plate at z=0 (perforates head-on) + a 500 mm backstop at z=-2 (embeds the residual).
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, 2.0, -2.0)),
            RigidBody::Static,
            Collider::cuboid(3.0, 3.0, 0.5),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "RHA".to_string(),
            },
        ));
        for _ in 0..8 {
            app.update(); // settle the new collider into the spatial pipeline
        }
        spawn_headon_shell(&mut app, a_shot());
        // March to full resolution (the backstop embed despawns the shell).
        for _ in 0..8 {
            app.update();
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(
            terminals.len(),
            1,
            "one terminal per shot — the perforation; the later embed is muted",
        );
        assert_eq!(
            terminals[0].verdict,
            Verdict::Through,
            "a perforation breached the plate"
        );
        assert!(
            (terminals[0].position.z - 0.025).abs() < 0.05,
            "the terminal reads at the plate's ENTRY face, got z={}",
            terminals[0].position.z
        );
        // The authoritative shell marched past the plate (into the backstop) — the cosmetic-terminal
        // choice truncates nothing on the authority: the impact log shows the perforation AND the
        // backstop embed.
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts.len() >= 2,
            "the authority resolves the perforation and then the backstop embed (got {})",
            impacts.len()
        );
    }

    /// HELD SHELL + CONFIRM: the full honest armor read, at the server's position, on receipt — the
    /// neutral fail-closed spark never fires. This is the read that gives MP the SP-grade armor
    /// feedback (flame lick via `verdict.entered()`), typically ≈(P−S)+OWL after contact instead of
    /// the fail-closed window.
    #[test]
    fn held_shell_resolves_on_confirm_with_full_honest_read() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);

        // March to contact; the shell holds (hidden, no spark).
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(app.world().get::<Held>(shell).is_some(), "shell held");
        assert!(app.world().resource::<ImpactLog>().0.is_empty());

        // The server's terminal arrives (an embed read at ITS position, slightly off the local
        // contact) → resolve immediately with the full read.
        let server_pos = Vec3::new(0.05, 2.0, 0.08);
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: server_pos,
                    normal: Vec3::Z,
                    verdict: TerminalVerdict::Through,
                    impact_tick: 0, // inert (no `ShotClock` — overdue path off)
                    after_bounces: 0,
                },
            );
        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the confirmed shell resolves (despawns) on receipt — not at the window",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            1,
            "exactly one impact — no neutral spark ever"
        );
        assert!(
            impacts[0].verdict.entered(),
            "the SERVER's penetration verdict rides through — the flame lick is earned in MP",
        );
        assert!(
            impacts[0].position.distance(server_pos) < 1.0e-4,
            "the read lands at the SERVER's position",
        );
        assert_eq!(impacts[0].surface, ImpactSurface::Armor);
        assert!(
            impacts[0].deflection.is_none(),
            "a terminal is not a bounce"
        );
    }

    /// PRE-ARMED CONFIRM: the terminal is already buffered when the shell reaches the plate → resolve
    /// INSTANTLY at contact (no hold, never hidden), with the server's full read.
    #[test]
    fn prearmed_confirm_resolves_at_contact_instantly() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        let server_pos = Vec3::new(0.02, 2.0, 0.09);
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: server_pos,
                normal: Vec3::Z,
                verdict: TerminalVerdict::Through,
                impact_tick: 0, // inert (no `ShotClock` — overdue path off)
                after_bounces: 0,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err() {
                break;
            }
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "a pre-armed confirm resolves the shell at contact",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1, "one impact, immediately at contact");
        assert!(impacts[0].verdict.entered(), "server verdict rides through");
        assert!(impacts[0].position.distance(server_pos) < 1.0e-4);
    }

    /// ORDERING: a terminal that follows a bounce (`after_bounces: 1`) must NOT resolve a shell that
    /// has not re-seeded through that bounce yet — the shell keeps holding for the (late) bounce
    /// keyframe, re-seeds when it lands, and only its NEXT contact consumes the terminal.
    #[test]
    fn terminal_waits_for_owed_bounce_then_resolves() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        // The terminal is ordered AFTER one bounce; the bounce keyframe is late (not yet buffered).
        let server_pos = Vec3::new(0.0, 2.0, 0.06);
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: server_pos,
                normal: Vec3::Z,
                verdict: TerminalVerdict::Stopped,
                impact_tick: 0, // inert (no `ShotClock` — overdue path off)
                after_bounces: 1,
            },
        );
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);

        // Contact: the shell must HOLD (the terminal is not its next event — a bounce is owed).
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "an after-bounces terminal must not resolve a shell that still owes a bounce",
        );
        assert!(app.world().resource::<ImpactLog>().0.is_empty());

        // The late bounce keyframe lands — fabricated to throw the shell back at the plate so a
        // second contact happens (the multi-bounce test's trick).
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(0.0, 2.0, 0.5),
                direction: Vec3::new(0.12, 0.0, -1.0).normalize(),
                speed: 480.0,
                bounce_tick: 0, // inert (no `ShotClock` — overdue path off)
                sequence: 0,
            },
        );
        for _ in 0..8 {
            app.update();
            if app.world().get_entity(shell).is_err() {
                break;
            }
        }

        assert!(
            app.world().get_entity(shell).is_err(),
            "after the bounce re-seed, the next contact consumes the terminal",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 2, "the bounce spark, then the terminal read");
        assert!(
            impacts[0].deflection.is_some(),
            "first: the sanctioned bounce"
        );
        assert!(impacts[1].deflection.is_none(), "second: the terminal");
        assert!(impacts[1].position.distance(server_pos) < 1.0e-4);
    }

    /// THE SHOOTER'S OWN SHELL consumes its confirm exactly like an observer's (no special-casing) —
    /// the own-shape shell (`ShotSource` riding it) held at contact resolves on the confirm with the
    /// full honest read.
    #[test]
    fn own_shell_confirm_applies() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });

        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: Vec3::new(0.0, 2.0, 0.07),
                    normal: Vec3::Z,
                    verdict: TerminalVerdict::Through,
                    impact_tick: 0, // inert (no `ShotClock` — overdue path off)
                    after_bounces: 0,
                },
            );
        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shooter's own shell resolves on its confirm — the honest read on their own hit",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1);
        assert!(
            impacts[0].verdict.entered(),
            "own hit shows the flame lick in MP"
        );
    }

    /// TERMINAL DEDUP: a shot has at most one terminal, so first insert wins.
    #[test]
    fn terminal_insert_is_first_wins_idempotent() {
        let shot = a_shot();
        let mut buf = SanctionedShots::default();
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: Vec3::X,
                normal: Vec3::Z,
                verdict: TerminalVerdict::Through,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 0,
            },
        );
        // Even a corrupt divergent duplicate must not replace the first terminal.
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: Vec3::Y * 99.0,
                normal: Vec3::Z,
                verdict: TerminalVerdict::Stopped,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 3,
            },
        );
        let stored = buf.terminal(shot, 0).expect("terminal stored");
        assert_eq!(stored.position, Vec3::X, "first insert wins");
        assert_eq!(
            stored.verdict,
            TerminalVerdict::Through,
            "first insert's verdict kept"
        );
    }

    /// A flat [`HeightGrid`] at `height` metres spanning the world square (raw meters, like the
    /// production resource).
    fn flat_grid(height: f32) -> HeightGrid {
        let size = 33usize;
        HeightGrid::new(
            vec![height; size * size].into(),
            size as u32,
            crate::terrain_grid::FIXTURE_EXTENT,
        )
    }

    /// Terrain stops come from the exact `HeightGrid` caster when the heightmap world is live:
    /// the app carries the grid resource but NO terrain collider at all — a parry-only march
    /// would fly straight through — and the shell must still stop on the grid surface with a
    /// terrain read (position on the surface, up normal, no penetration).
    #[test]
    fn terrain_stop_reads_the_height_grid_surface() {
        // The plate is parked far off the flight path: this is the plain march app.
        let mut app = world_with_plate(Vec3::splat(0.5), Vec3::new(900.0, 900.0, 900.0));
        app.insert_resource(flat_grid(30.0));
        let impacts = fire_and_capture(&mut app, Vec3::new(10.0, 80.0, -20.0), Vec3::NEG_Y, 800.0);
        assert_eq!(impacts.len(), 1, "one terrain stop");
        let hit = impacts[0];
        assert_eq!(hit.surface, ImpactSurface::Terrain);
        assert!(
            !hit.verdict.entered(),
            "terrain never reads as a penetration"
        );
        assert!(
            (hit.position - Vec3::new(10.0, 30.0, -20.0)).length() < 1e-2,
            "the shell must stop ON the grid surface, got {:?}",
            hit.position
        );
    }

    /// Nearer-hit selection with BOTH sources live: a plate above the ground resolves as armor
    /// (the parry path unchanged), and the perforating round then ends on the grid terrain
    /// below — armor first, terrain second, in that order.
    #[test]
    fn armor_above_the_ground_still_resolves_before_the_terrain() {
        // A thin horizontal 50 mm plate at y = 50, ground grid at y = 30.
        let mut app = world_with_plate(Vec3::new(6.0, 0.05, 6.0), Vec3::new(0.0, 50.0, 0.0));
        app.insert_resource(flat_grid(30.0));
        fire_and_capture(&mut app, Vec3::new(0.0, 60.0, 0.0), Vec3::NEG_Y, 800.0);
        // Keep marching past the first capture until the round lands.
        for _ in 0..8 {
            app.update();
        }
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts.len() >= 2,
            "expected the armor read then the terrain stop, got {}",
            impacts.len()
        );
        let armor = impacts[0];
        assert_eq!(armor.surface, ImpactSurface::Armor, "the nearer plate wins");
        assert!(armor.verdict.entered(), "an 88 perforates 50 mm flat");
        assert!((armor.position.y - 50.0).abs() < 0.2, "at the plate face");
        let last = *impacts.last().unwrap();
        assert_eq!(last.surface, ImpactSurface::Terrain);
        assert!(
            (last.position.y - 30.0).abs() < 1e-2,
            "the round ends on the grid ground, got {:?}",
            last.position
        );
    }

    // -----------------------------------------------------------------------------------------
    // §13.1's pathology table, end to end
    // -----------------------------------------------------------------------------------------
    //
    // The core tests state these as laws over synthetic hit lists. These state them as OUTCOMES, on
    // real trimesh colliders, through the live march — because §13.1 is a table of things the
    // resolver DID, and only the resolver can be asked whether it still does them.

    /// A box as an outward-wound triangle mesh, with the shell table the collector requires — the
    /// shape production armour actually is, spawned the way the bind spawns it.
    ///
    /// Winding is the whole point: the collector reads a face's orientation from it (parry's normal
    /// is flipped to oppose the ray and cannot tell entry from exit), so a mesh wound inwards would
    /// invert every crossing. Each face is listed counter-clockwise seen from OUTSIDE. One closed
    /// box is one shell, so every triangle carries shell `0`.
    pub(super) fn box_trimesh(size: Vec3) -> (Collider, BallisticSurfaces) {
        let h = size * 0.5;
        let vertices: Vec<Vec3> = [
            (-1.0, -1.0, -1.0),
            (1.0, -1.0, -1.0),
            (1.0, 1.0, -1.0),
            (-1.0, 1.0, -1.0),
            (-1.0, -1.0, 1.0),
            (1.0, -1.0, 1.0),
            (1.0, 1.0, 1.0),
            (-1.0, 1.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y, z)| Vec3::new(x * h.x, y * h.y, z * h.z))
        .collect();
        let indices = vec![
            [0, 3, 2],
            [0, 2, 1], // -Z
            [4, 5, 6],
            [4, 6, 7], // +Z
            [0, 1, 5],
            [0, 5, 4], // -Y
            [3, 7, 6],
            [3, 6, 2], // +Y
            [0, 4, 7],
            [0, 7, 3], // -X
            [1, 2, 6],
            [1, 6, 5], // +X
        ];
        // One closed box is one shell; the welded ids are the box's own corner indices, which are
        // already welded (each corner appears once in `vertices`).
        let surfaces = BallisticSurfaces {
            shells: vec![0u32; indices.len()].into(),
            corners: indices.as_slice().into(),
        };
        (Collider::trimesh(vertices, indices), surfaces)
    }

    /// A world of outward-wound trimesh plates, all one substance.
    pub(super) fn world_with_trimesh_plates(plates: &[(Vec3, Vec3)]) -> App {
        let mut app = world_with_plate(Vec3::splat(0.01), Vec3::new(0.0, -500.0, 0.0));
        for (size, at) in plates {
            app.world_mut().spawn((
                Transform::from_translation(*at),
                RigidBody::Static,
                box_trimesh(*size),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "RHA".to_string(),
                },
            ));
        }
        for _ in 0..8 {
            app.update();
        }
        app
    }

    /// Fire head-on at the plates and report whether the round CAME OUT the far side.
    ///
    /// Perforate-versus-defeat is the observable that needs no velocity read and no tolerance: a
    /// defeated round despawns where it stopped, a perforating one flies on. Straddling the
    /// capability with the arrangement under test turns "was that plate charged" into a boolean.
    fn perforates(plates: &[(Vec3, Vec3)]) -> bool {
        let mut app = world_with_trimesh_plates(plates);
        let shell = app
            .world_mut()
            .spawn((
                fixture_round(FIXTURE_88, Vec3::NEG_Z * 800.0),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![Vec3::new(0.0, 2.0, 2.0)],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Transform::from_translation(Vec3::new(0.0, 2.0, 2.0))
                    .looking_to(Vec3::NEG_Z, Vec3::Y),
            ))
            .id();
        for _ in 0..6 {
            app.update();
        }
        app.world().get::<Projectile>(shell).is_some()
    }

    fn slab(thickness: f32, centre: f32) -> (Vec3, Vec3) {
        (Vec3::new(3.0, 3.0, thickness), Vec3::new(0.0, 2.0, centre))
    }

    /// §13.1's headline defect, as an outcome. The serial resolver entered an overlapping second
    /// plate from inside, found no exit face ahead, and crossed it for FREE; the union charges the
    /// space once — neither zero nor twice.
    ///
    /// Two arrangements, each pinning one direction against the test round's capability (~121
    /// reference-mm, DERIVED): a union of 160 mm must defeat the round even though its first plate
    /// alone (70 mm) would not, and a union of 115 mm must let it through even though the two plates
    /// SUMMED (140 mm) would not.
    #[test]
    fn overlapping_trimesh_plates_charge_their_union_once() {
        // Undercharge — the defect that shipped. Union 160 mm.
        assert!(
            perforates(&[slab(0.07, 0.025)]),
            "the first plate alone does not defeat the round",
        );
        assert!(
            !perforates(&[slab(0.07, 0.025), slab(0.10, -0.05)]),
            "the overlapped second plate is charged, not crossed for free",
        );

        // Overcharge — War Thunder's failure mode. Union 115 mm, sum 140 mm.
        assert!(
            perforates(&[slab(0.07, 0.0225), slab(0.07, -0.0225)]),
            "the shared 25 mm is charged ONCE; summing it would have defeated the round",
        );
        assert!(
            !perforates(&[slab(0.14, 0.0)]),
            "140 mm really is beyond it — the control that makes the line above mean something",
        );
    }

    /// SEAM INVISIBILITY as an outcome: plates meeting exactly resolve as the one plate they add up
    /// to, on BOTH sides of the capability. Under the serial resolver the 1 mm nudge hopped the
    /// shared face and the second plate was free, so perfect abutment bought nothing — which is why
    /// the authoring standard and the resolver had to be redesigned together.
    #[test]
    fn exactly_abutting_trimesh_plates_resolve_as_one_plate() {
        assert!(
            perforates(&[slab(0.115, 0.0)]) && !perforates(&[slab(0.16, 0.0)]),
            "the two totals straddle the test round's capability",
        );
        for (total, split) in [
            (0.115f32, [slab(0.0575, 0.02875), slab(0.0575, -0.02875)]),
            (0.16, [slab(0.07, 0.045), slab(0.09, -0.035)]),
        ] {
            assert_eq!(
                perforates(&split),
                perforates(&[slab(total, 0.0)]),
                "abutment must resolve as the {total} m plate it is",
            );
        }
    }

    /// η, end to end. A round whose AXIS misses an oblique plate while its rim clips it deflects —
    /// which the point model could not do at all, the centre ray having missed — and deflects LESS
    /// than the same plate struck square-on. That is §13.5's graded weakspot and §13.5's "a graze IS
    /// a partial ricochet" in one observable: the deflection angle scales with the engagement.
    #[test]
    fn a_rim_only_graze_deflects_less_than_a_full_engagement() {
        // A 75°-oblique plate whose +Y edge sits at y = 2.0.
        let plate = |app: &mut App| {
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(0.0, 1.7, 0.0))
                    .with_rotation(Quat::from_rotation_y(75.0_f32.to_radians())),
                RigidBody::Static,
                box_trimesh(Vec3::new(3.0, 0.6, 0.1)),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "RHA".to_string(),
                },
            ));
        };
        let deflection_at = |height: f32| -> Option<Vec3> {
            let mut app = world_with_trimesh_plates(&[]);
            plate(&mut app);
            for _ in 0..8 {
                app.update();
            }
            let impacts =
                fire_and_capture(&mut app, Vec3::new(0.0, height, 2.0), Vec3::NEG_Z, 800.0);
            impacts.first().and_then(|impact| impact.deflection)
        };

        let full = deflection_at(1.7).expect("a square-on oblique hit deflects");
        let rim = deflection_at(2.02).expect("a rim clip still deflects — the disc reaches it");
        let turn = |d: Vec3| Vec3::NEG_Z.angle_between(d.normalize());
        assert!(
            turn(rim) > 0.0,
            "the graze really does turn the round: {rim:?}"
        );
        assert!(
            turn(rim) < turn(full) * 0.9,
            "a rim clip turns it LESS than a full engagement: {} vs {}",
            turn(rim),
            turn(full)
        );
    }

    /// SPACED ARMOUR, end to end: two 50 mm trimesh plates with 900 mm of air between them.
    ///
    /// The air is what this pins, from both sides. It must fabricate nothing — 900 mm is eighteen
    /// weld lookaheads, so the two plates are two crossings and the gap costs the round nothing but
    /// flight — and it must not swallow anything either: an 88 defeats 50 mm of RHA head-on without
    /// effort, so BOTH plates are punched, in sequence, each read at its own real face.
    ///
    /// It is also the arrangement that caught the collector's prune boundary: a transit corridor's
    /// origin IS the entry face's own position, and one ULP of rounding put it past that face, so
    /// the face's degenerate AABB fell outside the corridor's box, the entry vanished, and the walk
    /// reported an exit it had never entered. The round stopped dead on a plate it should have gone
    /// through — no terminal, no perforation, one bare `Verdict::Struck` spark.
    #[test]
    fn plates_across_a_900_mm_gap_are_two_crossings_and_both_perforate() {
        let mut app = world_with_trimesh_plates(&[slab(0.05, 0.5), slab(0.05, -0.45)]);
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        spawn_headon_shell(&mut app, a_shot());
        for _ in 0..8 {
            app.update();
        }

        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            2,
            "two plates, two armour reads — the gap merges nothing and hides nothing",
        );
        for (impact, face) in impacts.iter().zip([0.525, -0.425]) {
            assert!(matches!(impact.surface, ImpactSurface::Armor));
            assert!(
                impact.verdict.entered(),
                "an 88 goes through 50 mm of RHA head-on: {:?}",
                impact.position,
            );
            assert!(
                (impact.position.z - face).abs() < 1.0e-3,
                "the read is at the plate's own entry face (expected z={face}, got {})",
                impact.position.z,
            );
        }
        // Nothing may be reported in the air itself: a fabricated weld across the gap would report
        // ONE crossing spanning both plates, and a fabricated surface would report a third read
        // between them. The faces themselves are read to the same 1 mm the reads above are.
        assert!(
            impacts
                .iter()
                .all(|impact| !(-0.425 + 1.0e-3..=0.475 - 1.0e-3).contains(&impact.position.z)),
            "no armour was invented in 900 mm of air: {impacts:?}",
            impacts = impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );

        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "one terminal per shot");
        assert!(
            terminals[0].verdict.entered(),
            "the terminal breached the plate"
        );
        assert_eq!(terminals[0].after_bounces, 0, "nothing deflected head-on");
        assert!(
            (terminals[0].position.z - 0.525).abs() < 1.0e-3,
            "the terminal reads at the FIRST plate's entry face, got z={}",
            terminals[0].position.z,
        );
    }

    /// The same spaced pair, out where the map ends.
    ///
    /// Position is a LAW input, and this is the fixture that says so. f32 spacing coarsens with
    /// distance, so at 2.4 km the transit handoff lands ~1.5e-4 m off the face it was computed from
    /// — four orders coarser than the 2.4e-8 that produced the original defect two metres from the
    /// world origin, and the reason the prune margin scales instead of being a constant. It is also
    /// where the margin's CEILING starts to bind: the unclamped scaling term reaches the march's own
    /// boundary nudge at 2.5 km, and a margin that large re-collects the exit face of the plate just
    /// perforated. Combat happens here, so the outcome is asserted here.
    ///
    /// HONEST SCOPE: this is an end-to-end guard, not the deterministic statement of either bound.
    /// Which side of the plane the handoff rounds to is a property of the coordinates, and a sweep of
    /// 480 far-map arrangements found it landing SHORT every time — so this fixture would survive the
    /// margin being deleted. The bounds themselves are pinned where they can be stated exactly, in
    /// the collector: `a_face_the_origin_sits_a_hair_past_is_still_collected` walks the ULP ladder at
    /// this same distance, and `the_face_the_march_stepped_past_is_not_re_collected` holds the
    /// ceiling at 2.5 km.
    #[test]
    fn spaced_plates_perforate_at_the_far_edge_of_the_map() {
        let far = Vec3::new(2400.0, 2.0, 2400.0);
        let plate = |z: f32| (Vec3::new(3.0, 3.0, 0.05), far + Vec3::new(0.0, 0.0, z));
        let mut app = world_with_trimesh_plates(&[plate(-2.0), plate(-2.95)]);
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        spawn_headon_shell_at(&mut app, a_shot(), far);
        for _ in 0..8 {
            app.update();
        }

        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            2,
            "two plates, two armour reads, 2.4 km from the world origin: {:?}",
            impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );
        for (impact, face) in impacts.iter().zip([-1.975, -2.925]) {
            assert!(matches!(impact.surface, ImpactSurface::Armor));
            assert!(
                impact.verdict.entered(),
                "an 88 still goes through 50 mm out here"
            );
            assert!(
                (impact.position.z - (far.z + face)).abs() < 1.0e-3,
                "the read is at the plate's own entry face (expected z={}, got {})",
                far.z + face,
                impact.position.z,
            );
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "one terminal per shot");
        assert!(terminals[0].verdict.entered());
    }

    /// A PERFORATION'S SEEDS AND THE NEXT CORRIDOR DESCRIBE ONE POINT, ANYWHERE ON THE MAP.
    ///
    /// After a perforation the march resumes one [`MARCH_EPS`] past the exit, carrying which
    /// primitives each sample is inside there ([`resolve::Interior::seeds`]). The seed was read on
    /// the transit corridor's own anchor-relative ray; the next corridor must start from that same
    /// point. A resume point rebuilt in world f32 is quantized at the world's own spacing —
    /// 2.44e-4 m past 2048 m, DERIVED — so an entry face that close to the resumption plane is
    /// owned by the seed AND re-collected ahead of the origin (`UnexpectedEntry`), or by nobody
    /// (`UnexpectedExit`), and the round fails closed on armour it had already bitten.
    ///
    /// The near-origin half pins the other link of the same chain: the resumed corridor's crossing
    /// of the volume it began inside hands off at its own origin, and the declared presence must be
    /// seeded there too ([`walk::ShellPresence::spans`]).
    ///
    /// Generic geometry: a thin plate, then a second volume tilted so its entry face does not weld
    /// to the plate's exit, its entry swept across the resumption plane in steps finer than that
    /// spacing. Every arrangement must resolve without a walk error, near the world origin and far
    /// from it. A sub-disc round keeps the case to the axis sample, which is the sample the
    /// resumption plane is read on.
    #[test]
    fn a_perforation_resumes_where_its_seeds_were_read_far_from_the_origin() {
        const TILT: f32 = 65.0;
        let body = FIXTURE_MG;
        assert!(
            body.caliber < resolve::DISC_MIN_CALIBER,
            "a one-sample round"
        );
        let plate = Vec3::new(1.0, 1.0, 0.005);
        let tilted = Vec3::new(1.0, 1.0, 0.1);
        for far in [Vec3::new(0.0, 2.0, 2.0), Vec3::new(2500.0, 2.0, 2500.0)] {
            // The plate's exit face and the resumption plane one nudge past it, along −Z.
            let exit = far.z as f64 - 1.0 - plate.z as f64 * 0.5;
            let resume = exit - MARCH_EPS as f64;
            let centre_z = (resume - 0.12) as f32;
            let mut failed = Vec::new();
            for step in -20i32..=20 {
                // `ahead > 0`: the tilted volume's entry lies BEFORE the resumption plane, so the
                // seed owns it; `< 0`: after it, so the next corridor does.
                let ahead = step as f64 * 2.0e-5;
                let entry = resume + ahead;
                // Its +Z face, rotated by TILT about Y, crosses the axis `half / cos(TILT)` in front
                // of its centre.
                let half = ((entry - centre_z as f64) * (TILT as f64).to_radians().cos()) as f32;
                let mut app =
                    world_with_trimesh_plates(&[(plate, far + Vec3::new(0.0, 0.0, -1.0))]);
                app.world_mut().spawn((
                    Transform::from_translation(Vec3::new(far.x, far.y, centre_z))
                        .with_rotation(Quat::from_rotation_y(TILT.to_radians())),
                    RigidBody::Static,
                    box_trimesh(Vec3::new(tilted.x, tilted.y, half * 2.0)),
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material: walk::VolumeMaterial::fixture(STEEL * 0.01),
                        substance: "soft".to_string(),
                    },
                ));
                for _ in 0..8 {
                    app.update();
                }
                let origin = far + Vec3::new(0.0, 0.0, 2.0);
                app.world_mut().spawn((
                    fixture_round(body, Vec3::NEG_Z * 800.0),
                    DamageReport::default(),
                    TerminalReport::default(),
                    ShellPath {
                        points: vec![origin],
                        segment_starts: Vec::new(),
                    },
                    PenetrationMarks::default(),
                    SpallMarks::default(),
                    Transform::from_translation(origin).looking_to(Vec3::NEG_Z, Vec3::Y),
                ));
                for _ in 0..4 {
                    app.update();
                }
                let impacts = app.world().resource::<ImpactLog>().0.clone();
                assert!(
                    impacts
                        .first()
                        .is_some_and(|impact| impact.verdict.entered()),
                    "the plate is perforated at {far} ({ahead:e} m): {:?}",
                    impacts
                        .iter()
                        .map(|i| (i.position, i.verdict))
                        .collect::<Vec<_>>(),
                );
                // A walk error stops the round with a bare armour spark (`Struck`): no crossing, not
                // deflected. Nothing else in this arrangement reads that way.
                if impacts.iter().any(|impact| {
                    matches!(impact.surface, ImpactSurface::Armor)
                        && impact.verdict == Verdict::Struck
                }) {
                    failed.push(ahead);
                }
            }
            assert!(
                failed.is_empty(),
                "at {far} the march failed closed with the tilted entry these distances ahead of \
                 the resumption plane: {failed:?}",
            );
        }
    }

    /// Spawn an unkeyed round of `body` at `origin` travelling `velocity`, for the march.
    pub(super) fn spawn_round(
        app: &mut App,
        body: RoundBody,
        origin: Vec3,
        velocity: Vec3,
    ) -> Entity {
        app.world_mut()
            .spawn((
                fixture_round(body, velocity),
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                Transform::from_translation(origin).looking_to(
                    velocity,
                    if velocity.normalize().y.abs() > 0.9 {
                        Vec3::Z
                    } else {
                        Vec3::Y
                    },
                ),
            ))
            .id()
    }

    /// Whether the march failed closed on armour: a walk error stops the round with a bare armour
    /// spark (`Struck`) — no crossing, not deflected.
    fn failed_closed(impacts: &[Captured]) -> bool {
        impacts.iter().any(|impact| {
            matches!(impact.surface, ImpactSurface::Armor) && impact.verdict == Verdict::Struck
        })
    }

    /// A CONTINUATION SURVIVES THE TICK BOUNDARY.
    ///
    /// The resume fixture above, with the round's travel for the tick ending half a nudge past the
    /// plate's exit: the perforation leaves the axis sample inside the tilted volume (its entry lies
    /// before the resumption plane, so the seed owns it), and the tick's budget runs out before that
    /// volume's exit. A march that lets the continuation lapse with the tick starts the next one
    /// inside the volume with nothing declared, reads its exit as `UnexpectedExit`, and fails closed
    /// on armour it had already entered.
    #[test]
    fn a_continuation_survives_the_tick_boundary() {
        const TILT: f32 = 65.0;
        const DT: f32 = 0.016;
        let body = FIXTURE_MG;
        assert!(
            body.caliber < resolve::DISC_MIN_CALIBER,
            "a one-sample round"
        );
        let plate = Vec3::new(1.0, 1.0, 0.005);
        let far = Vec3::new(0.0, 2.0, 2.0);
        let face = far.z - 1.0 + plate.z * 0.5;
        let exit = far.z - 1.0 - plate.z * 0.5;
        let resume = exit - MARCH_EPS;
        let entry = resume + 5.0e-4;
        let centre_z = resume - 0.12;
        let half = (entry - centre_z) * TILT.to_radians().cos();
        let mut app = world_with_trimesh_plates(&[(plate, far + Vec3::new(0.0, 0.0, -1.0))]);
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(far.x, far.y, centre_z))
                .with_rotation(Quat::from_rotation_y(TILT.to_radians())),
            RigidBody::Static,
            box_trimesh(Vec3::new(1.0, 1.0, half * 2.0)),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL * 0.01),
                substance: "soft".to_string(),
            },
        ));
        for _ in 0..8 {
            app.update();
        }
        // The tick's travel is the shared advance's speed × dt; place the round so that what is left
        // after the plate (`budget − approach − thickness`) is half the march's nudge.
        let velocity = Vec3::NEG_Z * 800.0;
        let stepped = freeflight_step(velocity, drag_k(body.caliber, body.mass), DT);
        let budget = stepped.length() * DT;
        let origin = Vec3::new(far.x, far.y, face + (budget - plate.z - 0.5 * MARCH_EPS));
        spawn_round(&mut app, body, origin, velocity);
        for _ in 0..4 {
            app.update();
        }
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts
                .first()
                .is_some_and(|impact| impact.verdict.entered()),
            "the plate is perforated: {:?}",
            impacts
                .iter()
                .map(|i| (i.position, i.verdict))
                .collect::<Vec<_>>(),
        );
        assert!(
            !failed_closed(&impacts),
            "the continuation lapsed at the tick boundary and the march failed closed: {:?}",
            impacts
                .iter()
                .map(|i| (i.position, i.verdict))
                .collect::<Vec<_>>(),
        );
    }

    /// THE SHELL'S POSE IS ITS CONTINUATION'S AXIS START, AT ANY MAP COORDINATE.
    ///
    /// Fixture data: a round at X = 2500 m flying 16 mm per step (1 m/s at the test world's 16 ms
    /// step) through 100 open-air steps. A world f32 there is spaced 2.44e-4 m (DERIVED), so a
    /// position re-rounded at world spacing every step drifts by up to half that per step, while the
    /// continuation's anchor plus offset keeps what rounding lost. There is one position: the
    /// transform may differ from the continuation's world start by one world rounding, not by an
    /// error that grows with the steps flown.
    #[test]
    fn open_air_keeps_one_axis_position_at_map_scale() {
        const STEPS: usize = 100;
        let origin = Vec3::new(2500.0, 400.0, 0.0);
        let mut app = world_with_trimesh_plates(&[]);
        let shell = spawn_round(&mut app, FIXTURE_88, origin, Vec3::X);
        for _ in 0..STEPS {
            app.update();
        }
        let pose = app.world().get::<Transform>(shell).unwrap().translation;
        let start = app
            .world()
            .get::<Projectile>(shell)
            .unwrap()
            .continuation
            .as_ref()
            .expect("an open-air flight carries its starts across the tick")
            .world_origin();
        assert!(
            (1.5..1.7).contains(&(pose.x - origin.x)),
            "the round flew its open-air steps: {pose}",
        );
        let spacing = f32::from_bits(origin.x.to_bits() + 1) - origin.x;
        assert!(
            (pose - start).abs().max_element() <= spacing,
            "the shell's pose {pose} and its continuation's axis start {start} are two positions \
             {:?} apart (one world rounding is {spacing})",
            pose - start,
        );
    }

    /// CATCH-UP LANDS WHERE THE MARCH FLIES, AT ANY MAP COORDINATE.
    ///
    /// The fixture above, flown natively, against [`fast_forward_shell`] over the same number of
    /// steps. Both carry the axis as anchor plus offset, so they agree to a world rounding; a
    /// catch-up that re-rounds its position at world spacing every step does not.
    #[test]
    fn catch_up_lands_where_the_march_flies() {
        const STEPS: usize = 100;
        let origin = Vec3::new(2500.0, 400.0, 0.0);
        let mut app = world_with_trimesh_plates(&[]);
        let shell = spawn_round(&mut app, FIXTURE_88, origin, Vec3::X);
        for _ in 0..STEPS {
            app.update();
        }
        let flown = app.world().get::<ShellPath>(shell).unwrap().points.len() - 1;
        let pose = app.world().get::<Transform>(shell).unwrap().translation;
        let (caught_up, _, _) = fast_forward_shell(
            origin,
            Vec3::X,
            drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            0.016,
            flown as u32,
        );
        let spacing = f32::from_bits(origin.x.to_bits() + 1) - origin.x;
        assert!(
            (pose - caught_up).abs().max_element() <= spacing,
            "the march flew to {pose} and the catch-up to {caught_up} over {flown} steps \
             (one world rounding is {spacing})",
        );
    }

    /// A BOUNCE THEN A CONTACT IN ONE TICK SAMPLES THE SECOND SURFACE ON THE CURRENT FRAME.
    ///
    /// The march transports the sampling basis through the bounce; resolution must read that
    /// transported basis, not the one the shell started the tick with. A basis left on the incoming
    /// heading has a component ALONG the outgoing travel — `sin 30° = 0.5` for a 75° bounce, DERIVED —
    /// so half the ring starts up to 22 mm (an 88's radius × 0.5) ahead of the disc. A second face
    /// 10 mm past the bounce's resume point then has ring samples starting inside it with nothing
    /// declared, and the walk fails closed on a face the round never reached.
    ///
    /// Generic geometry: a thick plate grazed at 75°, and a plate square to the outgoing ray, its
    /// face 10 mm past the resume point and its lower edge kept clear of the first plate.
    #[test]
    fn a_bounce_then_a_contact_samples_the_current_frame() {
        let incidence = 75.0_f32.to_radians();
        let incoming = Vec3::new(incidence.sin(), 0.0, -incidence.cos());
        let normal = Vec3::Z;
        let outgoing = incoming - 2.0 * incoming.dot(normal) * normal;
        let first = (Vec3::new(3.0, 3.0, 0.1), Vec3::new(0.0, 2.0, 0.0));
        let bounce = Vec3::new(0.0, 2.0, 0.05);
        let radius = FIXTURE_88.caliber * 0.5;
        let resume = bounce + normal * radius;
        // The second plate's frame: local +Z faces back up the outgoing ray, local +Y leans away from
        // the first plate.
        let back = -outgoing;
        let up = (normal - outgoing * normal.dot(outgoing)).normalize();
        let side = up.cross(back);
        let rotation = Quat::from_mat3(&Mat3::from_cols(side, up, back));
        let thickness = 0.05;
        // In-plane extent along `up`: from 40 mm below the axis to 300 mm above it.
        let (low, high) = (-0.040, 0.300);
        let centre = resume + outgoing * (0.010 + thickness * 0.5) + up * ((low + high) * 0.5);
        let mut app = world_with_trimesh_plates(&[first]);
        app.world_mut().spawn((
            Transform::from_translation(centre).with_rotation(rotation),
            RigidBody::Static,
            box_trimesh(Vec3::new(1.0, high - low, thickness)),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "RHA".to_string(),
            },
        ));
        for _ in 0..8 {
            app.update();
        }
        spawn_round(
            &mut app,
            FIXTURE_88,
            bounce - incoming * 3.0,
            incoming * 800.0,
        );
        for _ in 0..4 {
            app.update();
        }
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        let read: Vec<(Vec3, Verdict)> = impacts.iter().map(|i| (i.position, i.verdict)).collect();
        assert_eq!(
            impacts.first().map(|impact| impact.verdict),
            Some(Verdict::Ricochet),
            "the graze bounces: {read:?}",
        );
        assert!(
            !failed_closed(&impacts),
            "the contact after the bounce failed closed: {read:?}",
        );
        assert!(
            impacts
                .get(1)
                .is_some_and(|impact| impact.verdict.entered()),
            "the round bit into the second plate: {read:?}",
        );
    }

    /// Whether a round fired at a face from one march nudge in front reports it: impacts, all on
    /// armour, the first an entry, and exactly one terminal.
    fn assert_face_reported(app: &App, at: &str) {
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            !impacts.is_empty(),
            "the round met the plate {at} and something was reported for it — a silent \
             pass-through is free armour",
        );
        assert!(
            impacts
                .iter()
                .all(|impact| matches!(impact.surface, ImpactSurface::Armor)),
            "and what was reported is the plate, not the ground beyond it: {impacts:?}",
            impacts = impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );
        // With the face collected the crossing resolves honestly: an 88 through 50 mm.
        assert!(
            impacts[0].verdict.entered(),
            "{at} the crossing resolved, rather than failing closed: {impacts:?}",
            impacts = impacts
                .iter()
                .map(|i| (i.position, i.verdict))
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            app.world().resource::<TerminalLog>().0.len(),
            1,
            "and it reported its one terminal",
        );
    }

    /// FREE PENETRATION IS THE ONE OUTCOME WORSE THAN A STOPPED SHELL.
    ///
    /// Every observed instance of the prune defect landed on a TRANSIT corridor, where a dropped
    /// entry leaves a bare exit and the walk shouts `UnexpectedExit` — wrong, but loud, and the
    /// driver fails closed on it. The same dropped face on an ENTRANCE corridor is silent: `begin`
    /// sees no material, returns [`walk::Begin::Miss`], and `resolve::decide` flies the round
    /// through the whole examined corridor with zero cost, zero damage and NO `Impact` at all. That
    /// is armour that was never modelled, which is precisely the defect class §13 exists to kill,
    /// and it would leave no trace in any log.
    ///
    /// So: spawn the shell one march nudge in front of a plate's face, and a few ULP either side of
    /// it. Whatever else happens, the round may not pass through unremarked.
    ///
    /// Fired level, so gravity bends the travel by `θ` in the first tick: a disc re-laid square to
    /// the bent travel at the muzzle would straddle the face by up to `r·θ` (DERIVED: 9 µm for an 88
    /// at 800 m/s after one 16 ms bend). The samples start where they were laid instead, square to
    /// the fire direction at the muzzle.
    #[test]
    fn an_entrance_corridor_that_starts_on_a_face_never_passes_through_unremarked() {
        let face = 0.525_f32;
        for step in -4i32..=4 {
            let spawn = f32::from_bits((face + MARCH_EPS).to_bits().wrapping_add_signed(step));
            let mut app = world_with_trimesh_plates(&[slab(0.05, 0.5)]);
            app.init_resource::<TerminalLog>();
            app.add_observer(capture_terminal);
            spawn_headon_shell_at(&mut app, a_shot(), Vec3::new(0.0, 2.0, spawn));
            for _ in 0..8 {
                app.update();
            }
            assert_face_reported(&app, &format!("at {step} ULP"));
        }
    }

    /// The same, falling straight down onto a level plate: gravity cannot tilt the travel, so the
    /// disc stays flush with the face and every sample meets it at the same `t`.
    #[test]
    fn a_falling_entrance_corridor_that_starts_on_a_face_never_passes_through_unremarked() {
        let face = 0.525_f32;
        for step in -4i32..=4 {
            let spawn = f32::from_bits((face + MARCH_EPS).to_bits().wrapping_add_signed(step));
            let mut app =
                world_with_trimesh_plates(&[(Vec3::new(3.0, 0.05, 3.0), Vec3::new(0.0, 0.5, 0.0))]);
            app.init_resource::<TerminalLog>();
            app.add_observer(capture_terminal);
            let shell = spawn_round(
                &mut app,
                FIXTURE_88,
                Vec3::new(0.1, spawn, -0.2),
                Vec3::NEG_Y * 800.0,
            );
            app.world_mut().entity_mut(shell).insert(Shot(a_shot()));
            for _ in 0..8 {
                app.update();
            }
            assert_face_reported(&app, &format!("at {step} ULP"));
        }
    }

    /// A ROUND LAUNCHED JUST OFF A FACE NEVER STARTS A SAMPLE INSIDE IT.
    ///
    /// Each sample's sweep starts where its membership is known. Two ways of starting past that put a
    /// sample inside a plate it is declared outside of: a boundary nudge, which steps over any face
    /// within one `MARCH_EPS` ahead; and re-laying the disc square to the tick's gravity-bent travel,
    /// which puts its leading samples up to `r·sin θ` ahead of where they were laid (DERIVED: 9 µm
    /// for an 88 at 800 m/s after one 16 ms bend). Fired level from a few ULP to half a nudge off a
    /// face, the crossing must resolve.
    #[test]
    fn a_round_launched_just_off_a_face_never_starts_a_sample_inside_it() {
        let face = 0.525_f32;
        for clearance in [0.0, 1.0e-6, 4.0e-6, 8.0e-6, 2.0e-5, 0.5 * MARCH_EPS] {
            for step in 0i32..=2 {
                let spawn = f32::from_bits((face + clearance).to_bits().wrapping_add_signed(step));
                let mut app = world_with_trimesh_plates(&[slab(0.05, 0.5)]);
                app.init_resource::<TerminalLog>();
                app.add_observer(capture_terminal);
                spawn_headon_shell_at(&mut app, a_shot(), Vec3::new(0.0, 2.0, spawn));
                for _ in 0..8 {
                    app.update();
                }
                assert_face_reported(&app, &format!("{clearance:e} m + {step} ULP off it"));
            }
        }
    }

    /// A step's flight launched in open air from a fresh start: [`resolve::Flight::launched`] with
    /// no persisted starts, in a world with no armour near.
    fn launched_in_open_air(
        position: Vec3,
        swept: Dir3,
        stepped: Vec3,
        frame: walk::DiscFrame,
    ) -> resolve::Flight {
        let mut app = world_with_trimesh_plates(&[]);
        app.world_mut()
            .run_system_once(
                move |world: ProjectileMarchWorld,
                      colliders: Query<(
                    &'static Position,
                    &'static Rotation,
                    &'static Collider,
                    Option<&'static BallisticSurfaces>,
                )>| {
                    let context = resolve::ResolveContext {
                        world: &world,
                        colliders: &colliders,
                        deposit: false,
                        laws: walk::WalkLaws::default(),
                    };
                    resolve::Flight::launched(
                        &context,
                        &|_| true,
                        position,
                        swept,
                        Dir3::new(stepped).expect("a direction"),
                        stepped.length(),
                        frame,
                        resolve::disc_radius(FIXTURE_88.caliber),
                        None,
                    )
                },
            )
            .expect("the launch system runs")
            .expect("open air certifies")
    }

    /// Every sample's start after `launched` is a point the last sweep left it at — the disc laid on
    /// `frame` at `position` — whatever the bend to the new direction.
    fn assert_starts_on_the_last_sweep(
        flight: &resolve::Flight,
        position: Vec3,
        frame: &walk::DiscFrame,
    ) {
        let radius = resolve::disc_radius(FIXTURE_88.caliber);
        let origin = flight.sweep_origin();
        let starts: Vec<Vec3> = flight
            .sample_offsets(radius)
            .into_iter()
            .map(|offset| origin + offset)
            .collect();
        let cleared: Vec<Vec3> = resolve::layout(frame, radius)
            .into_iter()
            .map(|offset| position + offset)
            .collect();
        assert_eq!(starts.len(), cleared.len(), "one start per sample");
        for (sample, (start, cleared)) in starts.iter().zip(&cleared).enumerate() {
            assert!(
                start.is_finite() && start.distance(*cleared) < 1.0e-6,
                "sample {sample} starts at {start:?}, not where the last sweep left it ({cleared:?})",
            );
        }
    }

    /// A BENT FRESH SWEEP STARTS ON GROUND THE LAST SWEEP CLEARED, NOT MERELY BEHIND ITS END PLANE.
    ///
    /// Stepping a disc square to the new travel back by `r·tan θ` puts it behind the plane the last
    /// sweep ended on, and laterally outside that sweep: its rim reaches `r / cos θ` from the old
    /// axis. Fixture data: an 88 (r = 44 mm) last swept along `+Z`, bent 30° toward `−Y` by one
    /// 16 ms tick at 0.272 m/s, under a slab whose face is 47 mm above the axis. The old disc stays
    /// at or below 44 mm; the stepped-back rim reaches 50.8 mm (DERIVED), inside a slab no sample
    /// ever entered, and reads its exit with nothing declared.
    #[test]
    fn a_bent_fresh_sweep_starts_on_ground_the_last_sweep_cleared() {
        const DT: f32 = 0.016;
        let velocity = Vec3::Z * 0.272;
        let stepped = freeflight_step(velocity, drag_k(FIXTURE_88.caliber, FIXTURE_88.mass), DT);
        let bend = stepped.angle_between(velocity).to_degrees();
        assert!(
            (29.0..31.0).contains(&bend),
            "the fixture bends the travel by about 30° in one tick (got {bend}°)",
        );

        let frame = test_disc(Vec3::Z);
        let position = Vec3::new(0.0, 2.0, 0.0);
        let flight = launched_in_open_air(position, Dir3::Z, stepped, frame);
        assert_starts_on_the_last_sweep(&flight, position, &frame);

        let face = position.y + 0.047;
        let mut app = world_with_trimesh_plates(&[(
            Vec3::new(3.0, 1.0, 3.0),
            Vec3::new(0.0, face + 0.5, 0.0),
        )]);
        spawn_round(&mut app, FIXTURE_88, position, velocity);
        for _ in 0..4 {
            app.update();
        }
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts.is_empty(),
            "the round falls away from a slab it never reached, and nothing is reported for it: {:?}",
            impacts
                .iter()
                .map(|i| (i.position, i.verdict))
                .collect::<Vec<_>>(),
        );
    }

    /// A NEAR-REVERSAL STARTS ITS SAMPLES WHERE THE LAST SWEEP LEFT THEM.
    ///
    /// Fixture data: an 88 rising at `(0.01, 0.08, 0)` m/s, the residual of a near-exhausted upward
    /// perforation; one 16 ms tick of gravity turns it back down, past a right angle. A step-back of
    /// `r·tan θ` has no meaning there — with the cosine clamped positive it is 9.4e35 m (DERIVED) —
    /// so the starts must not depend on the bend at all.
    #[test]
    fn a_near_reversal_starts_its_samples_where_the_last_sweep_left_them() {
        const DT: f32 = 0.016;
        let velocity = Vec3::new(0.01, 0.08, 0.0);
        let stepped = freeflight_step(velocity, drag_k(FIXTURE_88.caliber, FIXTURE_88.mass), DT);
        assert!(
            stepped.dot(velocity) < 0.0,
            "the fixture turns the travel past a right angle in one tick",
        );
        let swept = Dir3::new(velocity).expect("a direction");
        let frame = test_disc(velocity.normalize());
        let position = Vec3::new(0.0, 2.0, 0.0);
        let flight = launched_in_open_air(position, swept, stepped, frame);
        assert_starts_on_the_last_sweep(&flight, position, &frame);
    }

    /// AN OBLIQUE PERFORATION THAT ENDS A TICK RESUMES ON ITS SEEDS.
    ///
    /// A disc that perforates an oblique plate resumes each sample one nudge past the aggregate exit
    /// along its own ray, so the starts are staggered along the plate — up to `r·tan(incidence)`
    /// ahead of or behind the axis (DERIVED: 76 mm for an 88 at 60°). When that perforation is the
    /// step's last crossing, the next tick must resume from those same starts
    /// ([`resolve::Interior`]); a disc laid square at the axis's resume point puts every sample the
    /// stagger had ahead of it back inside the plate it left, reading that plate's exit with nothing
    /// declared, and the round fails closed in mid-air past the plate.
    ///
    /// Fixture data: an 88 at 60° through 20 mm, from 200 stand-offs 0.2 mm apart across a 4 cm
    /// window centred on the tick's end, so the perforation lands at every phase of the boundary.
    #[test]
    fn an_oblique_perforation_that_ends_a_tick_resumes_on_its_seeds() {
        const DT: f32 = 0.016;
        const STANDOFFS: usize = 200;
        let incidence = 60.0_f32.to_radians();
        let dir = Vec3::new(incidence.sin(), 0.0, -incidence.cos());
        let velocity = dir * 800.0;
        let stepped = freeflight_step(velocity, drag_k(FIXTURE_88.caliber, FIXTURE_88.mass), DT);
        let budget = stepped.length() * DT;
        let face = Vec3::new(0.0, 2.0, 0.51);
        let mut failed = Vec::new();
        for index in 0..STANDOFFS {
            // The face lies this far short of the tick's travel: the crossing ends the tick for
            // stand-offs whose perforation spends what is left of it.
            let short = 0.02 - 0.04 * index as f32 / STANDOFFS as f32;
            let mut app = world_with_trimesh_plates(&[slab(0.02, 0.5)]);
            spawn_round(
                &mut app,
                FIXTURE_88,
                face - dir * (budget - short),
                velocity,
            );
            for _ in 0..3 {
                app.update();
            }
            let impacts = app.world().resource::<ImpactLog>().0.clone();
            if failed_closed(&impacts) {
                failed.push(short);
            }
        }
        assert!(
            failed.is_empty(),
            "{} of {STANDOFFS} stand-offs failed closed past the plate they perforated, with the face \
             this far short of the tick's travel: {failed:?}",
            failed.len(),
        );
    }

    /// A SANCTIONED BOUNCE IS A DIRECTION CHANGE, SO IT TRANSPORTS THE SAMPLING BASIS.
    ///
    /// The disc frame is spawn-anchored and parallel-transported through every bend the round takes
    /// — gravity's, normalization's, the authority's own ricochet. A replica adopting server truth
    /// takes the same bend, and must carry the basis through it for the same reason: `transport`
    /// re-orthogonalizes, and a basis left behind on the incoming heading is not merely mis-rolled.
    /// It has a component ALONG the new travel axis, so the ring samples sit in front of and behind
    /// the disc instead of across it, and a sample ray that begins inside geometry the axis has not
    /// reached reports an exit it never entered.
    ///
    /// The 75° fixture makes the defect exactly measurable rather than merely present: `v` is
    /// `dir × Y`, the reflection off the +Z face turns the round back through the same angle, and
    /// the stale basis vector ends up with a 0.5 component along travel — half the disc's reach
    /// pointing down the barrel.
    ///
    /// BOTH re-seed paths, because they are different code: the march's own arm when the keyframe is
    /// already buffered at contact, and the catch-up chain when it arrives while the shell is held.
    #[test]
    fn a_sanctioned_reseed_transports_the_disc_frame() {
        // `u` and `v` span the disc, so both must be perpendicular to travel, and they must still be
        // an orthonormal pair — a frame is what `transport` promises to return, not a rotated pair.
        let assert_frame_spans_the_disc = |app: &App, shell: Entity, path: &str| {
            let projectile = app.world().get::<Projectile>(shell).expect("shell alive");
            let travel = projectile.velocity.normalize();
            let frame = projectile.disc;
            for (name, basis) in [("u", frame.u), ("v", frame.v)] {
                assert!(
                    basis.dot(travel).abs() < 1.0e-5,
                    "{path}: {name} lies across travel, not along it (got {})",
                    basis.dot(travel),
                );
                assert!(
                    (basis.length() - 1.0).abs() < 1.0e-5,
                    "{path}: {name} is a unit vector",
                );
            }
            assert!(
                frame.u.dot(frame.v).abs() < 1.0e-5,
                "{path}: the pair is still orthogonal",
            );
        };

        let shot = a_shot();
        let bounce = authority_bounce(shot);
        let keyframe = SanctionedBounce {
            origin: bounce.origin,
            direction: bounce.direction,
            speed: bounce.speed,
            bounce_tick: 0,
            sequence: 0,
        };

        // PATH ONE — pre-armed: the march's own re-seed arm consumes it at contact.
        let mut buf = SanctionedShots::default();
        buf.insert(shot, keyframe);
        let mut app = replica_world(buf);
        let shell = spawn_oblique_shell(&mut app, shot);
        for _ in 0..8 {
            app.update();
            if !app
                .world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .is_empty()
            {
                break;
            }
        }
        assert_eq!(
            app.world()
                .get::<PenetrationMarks>(shell)
                .unwrap()
                .ricochets
                .len(),
            1,
            "the pre-armed keyframe was consumed",
        );
        assert_frame_spans_the_disc(&app, shell, "pre-armed reseed");

        // PATH TWO — held, then caught up: `resume_from_catch_up` re-anchors on the chain's end
        // state, which is a different direction change through different code.
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);
        app.world_mut().entity_mut(shell).insert(ShotSource {
            tank: Entity::PLACEHOLDER,
            weapon: 0,
        });
        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(
            app.world().get::<Held>(shell).is_some(),
            "the shell held for its verdict",
        );
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert(shot, keyframe);
        app.update();
        assert!(app.world().get::<Held>(shell).is_none(), "the hold cleared");
        assert_frame_spans_the_disc(&app, shell, "catch-up reseed");
    }

    /// A CORRIDOR WHOLLY INSIDE ARMOUR IS NOT OPEN AIR.
    ///
    /// The convex narrow phase probes both ends with `solid: true`, and deep inside a solid both
    /// answer zero while the backward boundary probe reaches no face. Reporting nothing is then the
    /// obvious thing to do and the one thing that must not happen: an empty corridor is exactly what
    /// open air looks like, so the walk finds no material, `begin` returns `Miss`, and the round
    /// flies on through solid armour at zero cost with no `Impact` at all. Codex measured a shell
    /// advancing a full tick inside a 100 m volume with nothing reported.
    ///
    /// Undeclared containment is now a `WalkError`, so the driver fails closed on it: the round
    /// stops where it is, and there is a neutral armour read to see it by. Free penetration is the
    /// one outcome worse than a stopped shell, because armour that silently is not there is
    /// indistinguishable from armour that was never modelled.
    #[test]
    fn a_shell_inside_a_convex_volume_fails_closed_instead_of_flying_through() {
        // A hundred metres of steel, and the shell starts in the middle of it. `world_with_plate`
        // builds its plate as a CUBOID, which is the convex narrow phase.
        let mut app = world_with_plate(Vec3::splat(100.0), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        let origin = Vec3::new(0.0, 2.0, 0.0);
        let shell = spawn_headon_shell_at(&mut app, a_shot(), origin);
        app.update();

        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            1,
            "the round inside the block is reported, not flown through: {:?}",
            impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );
        assert!(matches!(impacts[0].surface, ImpactSurface::Armor));
        assert!(
            !impacts[0].verdict.entered(),
            "fail-closed is a neutral read: no perforation was resolved, so none is claimed",
        );
        assert!(
            impacts[0].position.distance(origin) < 0.1,
            "and it stops where it was, not a tick downrange: {:?}",
            impacts[0].position,
        );
        assert!(
            app.world().get::<Projectile>(shell).is_none(),
            "a fail-closed round is consumed, not left marching",
        );
    }

    /// A CROSSING IS TWO SEGMENTS, AND THE TICK PAYS FOR BOTH.
    ///
    /// The round flies from the corridor origin to the AXIS handoff along the way in, then from
    /// there to the exit along the bend. The driver knew only where its own cast first touched
    /// armour — the nearest of thirteen sample rays — and charged the budget to THAT. At oblique
    /// incidence the nearest ray leads the axis by `r·tan(incidence)`: codex measured an 88 at 72°
    /// charged 0.3934 m for a path whose segments total 0.5288 m, so 135.4 mm of the tick was flown
    /// for free. Free travel inside a tick moves subsequent impacts across tick and REV-25
    /// accounting boundaries, which is a netcode bug wearing a geometry costume.
    ///
    /// The observable needs no internals: a tick's route is a POLYLINE — start to entry, entry to
    /// exit, exit to wherever the leftover budget ran out — and its total length is the budget. An
    /// undercharge shows up as a shell that ended the tick further along than its own velocity
    /// could carry it, by exactly the metres that were skipped.
    #[test]
    fn a_crossing_charges_the_whole_path_it_flew() {
        let start = Vec3::new(0.0, 2.0, 2.0);
        let dt = 0.016_f32;
        // The budget the march works from: this tick's velocity after gravity and drag, times dt.
        let stepped = freeflight_step(Vec3::NEG_Z * 800.0, drag_k(0.088, 10.2), dt);
        let budget = stepped.length() * dt;

        for degrees in [0.0_f32, 40.0, 72.0] {
            let mut app = world_with_trimesh_plates(&[]);
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))
                    .with_rotation(Quat::from_rotation_y(degrees.to_radians())),
                RigidBody::Static,
                // 20 mm: thin enough that an 88 is overmatched at 72° and transits rather than
                // bouncing, so every incidence under test resolves the same way.
                box_trimesh(Vec3::new(3.0, 3.0, 0.02)),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "RHA".to_string(),
                },
            ));
            for _ in 0..8 {
                app.update();
            }

            let shell = spawn_headon_shell_at(&mut app, a_shot(), start);
            app.update();

            let marks = app
                .world()
                .get::<PenetrationMarks>(shell)
                .expect("the shell survives a 20 mm plate");
            assert_eq!(
                marks.events.len(),
                1,
                "{degrees}°: one crossing, so the polyline below has one bend in it",
            );
            let (entry, exit) = (marks.events[0].entry, marks.events[0].exit);
            let end = app.world().get::<Transform>(shell).unwrap().translation;

            // Approach, transit, and the free flight the leftover budget bought.
            let approach = start.distance(entry);
            let through = entry.distance(exit);
            let onward = exit.distance(end);
            let flown = approach + through + onward;
            assert!(
                (flown - budget).abs() < 1.0e-3,
                "{degrees}°: the tick flew {flown} m on a {budget} m budget                  (approach {approach}, through {through}, onward {onward})",
            );
            // The defect is invisible head-on and grows with the ring's spread, so the oblique arms
            // are the ones that mean anything: name the reach they are testing.
            let lead = 0.044 * degrees.to_radians().tan();
            assert!(
                degrees == 0.0 || lead > 0.03,
                "{degrees}°: the nearest sample leads the axis by {lead} m, which is what was skipped",
            );
        }
    }

    /// `MISS` MUST ADVANCE PAST THE GROUND IT JUST EXAMINED.
    ///
    /// First contact is a cast; the disc is the resolution. When they disagree — the cast touched
    /// something, the walk found no material in it — the round flies on. What it must NOT do is
    /// advance only to the contact, because the contact is exactly where the next cast will find the
    /// same thing again: the round creeps forward one boundary nudge at a time, burning the tick's
    /// budget in place, and ends a tick that should have carried it twelve metres a few millimetres
    /// past a plate it never touched.
    ///
    /// Codex's mutant ledger had this one SURVIVING both its supposed catchers, so here is a fixture
    /// that cannot miss it. A zero-factor volume is a `Miss` by construction rather than by
    /// geometric knife-edge — the cast finds it, being real armour-layer geometry under a
    /// `BallisticVolume`, and the union field never rises above zero, so there is nothing to resolve.
    /// Nothing there means nothing there: no impact, and the tick lands exactly where free flight
    /// would have put it.
    #[test]
    fn a_miss_flies_the_whole_corridor_it_examined() {
        let start = Vec3::new(0.0, 2.0, 2.0);
        let dt = 0.016_f32;
        let (free_flight, _, _) =
            fast_forward_shell(start, Vec3::NEG_Z * 800.0, drag_k(0.088, 10.2), dt, 1);

        let mut app = world_with_plate(Vec3::splat(0.01), Vec3::new(0.0, -500.0, 0.0));
        app.world_mut().spawn((
            Transform::from_translation(Vec3::new(0.0, 2.0, 0.5)),
            RigidBody::Static,
            box_trimesh(Vec3::new(3.0, 3.0, 0.05)),
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            // Real geometry, no material. The cast still finds it; the field never rises. The
            // substance name is empty to match: this fixture is deliberately un-declared, and
            // naming a registry substance here would assert the opposite of what it tests.
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(0.0),
                substance: String::new(),
            },
        ));
        for _ in 0..8 {
            app.update();
        }

        let shell = spawn_headon_shell_at(&mut app, a_shot(), start);
        app.update();

        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "nothing was there, so nothing is reported: {:?}",
            app.world()
                .resource::<ImpactLog>()
                .0
                .iter()
                .map(|i| i.position)
                .collect::<Vec<_>>(),
        );
        let end = app
            .world()
            .get::<Transform>(shell)
            .expect("the shell flies on")
            .translation;
        assert!(
            end.distance(free_flight) < 1.0e-3,
            "the tick lands where free flight would: expected {free_flight}, got {end}",
        );
        // Stated again as the thing that actually goes wrong, so the failure reads as the defect:
        // creeping leaves the round at the plate instead of twelve metres past it.
        assert!(
            end.z < -10.0,
            "the round did not creep at the surface it examined: z = {}",
            end.z,
        );
    }

    /// CODEX'S MEASURED POINT: `(2499.9, 924.963, 1524.939)` at 38° incidence.
    ///
    /// The corridor origin used to be a WORLD position, and f32 out here resolves to 0.24 mm — so
    /// the handoff and the very face it was computed from were quantised to that grid
    /// INDEPENDENTLY, by different routes. Codex swept the live handoff arithmetic and measured it
    /// landing 0.3848 mm off its own plane at this point: past the prune margin's 0.25 mm ceiling,
    /// inside the playable envelope, and therefore a real entry face pruned in real combat. The
    /// round then reported an exit it never entered and stopped dead on a plate it should have
    /// crossed.
    ///
    /// No margin could fix that, because the floor the geometry demanded and the ceiling safety
    /// demanded had crossed. Anchoring removes the cause: the world-scale subtraction happens once,
    /// on the anchor, and the handoff is a small offset from the same origin the face positions were
    /// measured from. The error is now corridor-scale wherever the shot is.
    #[test]
    fn a_crossing_resolves_at_the_far_corner_of_the_map() {
        // Codex's point, and the incidence it measured there.
        let at = Vec3::new(2499.9, 924.963, 1524.939);
        let mut app = world_with_trimesh_plates(&[]);
        // TWO plates, spaced. One crossing exercises the handoff once; a second corridor re-anchors
        // downrange and exercises it again on geometry the first crossing already bent the round
        // toward — which is where the error used to compound.
        for z in [0.0_f32, -0.9] {
            app.world_mut().spawn((
                Transform::from_translation(at + Vec3::new(0.0, 0.0, z))
                    .with_rotation(Quat::from_rotation_y(38.0_f32.to_radians())),
                RigidBody::Static,
                box_trimesh(Vec3::new(3.0, 3.0, 0.02)),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "RHA".to_string(),
                },
            ));
        }
        for _ in 0..8 {
            app.update();
        }

        app.init_resource::<TerminalLog>();
        app.add_observer(capture_terminal);
        let shell = spawn_headon_shell_at(&mut app, a_shot(), at + Vec3::new(0.0, 0.0, 2.0));
        app.update();

        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(
            impacts.len(),
            2,
            "two plates, two reads — not a pruned entry and a fail-closed stop: {:?}",
            impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );
        assert!(
            impacts.iter().all(|impact| impact.verdict.entered()),
            "an 88 crosses 20 mm at 38° here exactly as it does at the world origin",
        );
        assert_eq!(
            app.world().resource::<TerminalLog>().0.len(),
            1,
            "and the crossing reported its terminal",
        );
        assert!(
            app.world().get::<Projectile>(shell).is_some(),
            "the round flew on; it was not stopped by arithmetic",
        );
    }
}
