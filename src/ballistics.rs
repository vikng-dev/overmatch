//! The game's damage model: shell flight, penetration, dose and impact events; game state only.
//! [`sim_plugin`] mounts authority resolution; [`view_plugin`] mounts client presentation.
//! Design narrative: `.agents/docs/design/armor-penetration-and-damage.md`.

use std::sync::Arc;
use std::time::Instant;

use avian3d::prelude::{
    Collider, ColliderAabb, Forces, Position, Rotation, SimpleCollider, SpatialQuery,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
// `shot_trace::record` only evaluates its closure when tracing is armed.
use serde_json::json;

use crate::damage::{DamageConsequences, VolumeOf, hit_ancestor};
use crate::damage_cam::recorder::{MarchRecorder, ShotRecorder, ShotRecording};
use crate::spec::NoseShape;
use crate::state::{GameplaySet, SimPhase};
use crate::terrain_grid::HeightGrid;
use crate::{ClientReplica, Layer, ShotClock, ShotId};

pub(crate) mod path;
/// Client reconciliation buffer and bounded catch-up for drawable flight; resolution never reads it.
mod sanctioned;
pub(crate) use path::{FlightOutcome, PathClock};

/// Pure resolution over collected hits: union charging, ε-welded micro-gaps and a caliber-wide
/// disc (§13). [`resolve::decide`] adapts the live world; §13.6 invariants are bit-testable here.
mod walk;
pub(crate) use walk::VolumeMaterial;

/// The game's damage model: shared environment and cavity-law stand-ins.
use crate::physical_laws;

/// Solid-shot state across ordered crossing encounters (the only round mechanism).
mod mechanism;

/// A burst round's derived burst and the fuze that times it.
pub(crate) mod burst;

/// Cone shape (half-angle, axis, sampling), field energy and mechanism-specific HP-per-joule units.
pub(crate) mod spall_cone;

/// The energy-to-HP rate every dose of energy converts at: joules per HP point.
pub(crate) mod energy_dose;

#[cfg(test)]
mod authored;

mod authored_constants {
    include!(concat!(env!("OUT_DIR"), "/ballistics_authored.rs"));
}

/// The game's damage model: the shared authored crew HP pool and geometry-based pen cam bands.
pub(crate) mod box_classes;

/// Live §13 adapter: corridor collection, staged `begin`/`finish`, and plan-to-effect mapping.
mod resolve;

/// The witness pass (ADR-0040): the round's line and the field's directions intersected with the
/// witness volumes the hard world never sees.
mod wound;

/// Every face crossing along world rays, with winding orientation; the module doc explains why
/// Avian's all-hits query cannot express this.
mod collect;
pub(crate) use collect::convex_interval;

mod motion;
pub(crate) mod plate_capacity;

/// §13.6 fuzzer at the bound tank: checks union-field invariants and reports corridors reaching
/// crew HP pools or ammunition with admitting caliber and per-caliber η. Effectively unarmoured
/// routes fail by name, unconditionally.
pub mod fuzz;

#[cfg(test)]
mod field_casts;

/// The pen cam recorder: scripted shots at the bound tank read back as pen cam records, and the
/// scenario files the demo plays, checked current.
#[cfg(test)]
mod pen_cam;

/// The game's damage-model outcome sweep (ignored; on demand): random hits by station,
/// direction-count comparisons and aimed shot sequences to tank knockout.
#[cfg(test)]
mod crew_sweep;

/// What the contact walk does per geometric class (ignored; run on demand): real shots at the
/// shipped hull through the real march, read per exchange.
#[cfg(test)]
mod contact_classes;

/// The game's pen cam recorder on the fixed tick: rounds fired at fixture tanks read back as the
/// records the authority emits.
#[cfg(test)]
mod recording_tests;

/// Client-only shell scenes and tracers ([`view::plugin`]). The dedicated server
/// mounts only [`sim_plugin`], opening no view artifact and emitting no render component.
mod view;

#[cfg(test)]
pub(crate) use view::ShellVisual;
pub use view::{TracerStreak, plugin as view_plugin};

pub(crate) use sanctioned::{SanctionedInsert, SanctionedShots};

/// The game's damage model: Resolution exit nudge (m). A perforation resumes this far past
/// its resolved exit so the next collection does not re-touch that face. A skid resumes one body
/// radius off its face instead ([`resolve::REST_CLEARANCE`]); discovery establishes membership there.
///
/// Shared with [`resolve`]: the seed handed forward describes the next corridor's actual start.
/// A perforation seed read one nudge earlier can still name the plate the corridor has left,
/// producing an unmatched boundary when the same face is collected again.
pub(crate) const MARCH_EPS: f32 = 1.0e-3;

/// The game's damage model: uniform gravity from the shared environment stand-in (m/s²).
fn gravity() -> Vec3 {
    Vec3::NEG_Y * physical_laws::gravity_mps2()
}

/// Drag force ½ρC_d(πd²/4)v² gives form units C_d·ρπ/8; π/8 is exact algebra.
fn form_per_drag_coefficient() -> f64 {
    physical_laws::PhysicalLaws::shipped().air_density * std::f64::consts::PI / 8.0
}

/// Drag form = base + nose share × flight [`NoseShape::factor`]; a pointed nose flies on less.
/// Form units are drag coefficient × ρπ/8 ([`form_per_drag_coefficient`]). The `drag` block in
/// `assets/ballistics/round_types.ron` sources the base coefficient and an anchor nose's form.
/// The nose share is DERIVED to reproduce that form to the bit. Evaluate in f64 and cast once,
/// rounding the form rather than the factor. Stand-in: no Mach dependence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DragLaw {
    base_form: f64,
    nose_form: f64,
}

impl DragLaw {
    /// The law over base drag coefficient `base_drag_coefficient` that flies `anchor_nose` on
    /// `anchor_form` (form units), or why those parameters state no drag law: each finite and
    /// positive, and the anchor above the base, so that a blunter nose never flies on less.
    pub(crate) fn stated(
        base_drag_coefficient: f64,
        anchor_nose: NoseShape,
        anchor_form: f64,
    ) -> Result<Self, String> {
        let base_form = base_drag_coefficient * form_per_drag_coefficient();
        if !(base_form.is_finite() && base_form > 0.0) {
            return Err(format!(
                "base_drag_coefficient must be finite and positive (got {base_drag_coefficient})"
            ));
        }
        if !(anchor_form.is_finite() && anchor_form > base_form) {
            return Err(format!(
                "anchor_form must be finite and above the base share {base_form} (got \
                 {anchor_form}) — the nose share is what a blunter nose flies on in addition"
            ));
        }
        Ok(Self {
            base_form,
            nose_form: (anchor_form - base_form) / anchor_nose.factor_f64(),
        })
    }

    /// The form of a round flying on `flight_nose`.
    pub(crate) fn form(&self, flight_nose: NoseShape) -> f32 {
        (self.base_form + self.nose_form * flight_nose.factor_f64()) as f32
    }
}

/// FIXTURE DATA: the drag law every test fixture flies, the shipped table's values when the
/// fixtures were measured (base drag coefficient 0.20, a 1-crh ogive on form 0.263).
#[cfg(test)]
pub(crate) fn fixture_drag_law() -> DragLaw {
    DragLaw::stated(0.20, FIXTURE_DRAG_NOSE, 0.263).expect("the fixture law's parameters are valid")
}

/// FIXTURE DATA: the nose [`fixture_drag_law`] is anchored on and every fixture flies on.
#[cfg(test)]
const FIXTURE_DRAG_NOSE: NoseShape = NoseShape::Ogive { crh: 1.0 };

/// A shell's quadratic-drag coefficient `k` (1/m) for the model `dv/dt = −k·v²`: `form` ×
/// `caliber²` / `mass`, of the body that flies ([`crate::rounds::Round::drag_k`]). The live shell
/// and the range table read the same value, so the aim solution and the flight bleed speed alike.
pub fn drag_k(form: f32, caliber: f32, mass: f32) -> f32 {
    form * caliber * caliber / mass
}

/// Fixture drag coefficient for `caliber` (m), `mass` (kg) and [`FIXTURE_DRAG_NOSE`] under
/// [`fixture_drag_law`].
#[cfg(test)]
pub(crate) fn fixture_drag_k(caliber: f32, mass: f32) -> f32 {
    drag_k(fixture_drag_law().form(FIXTURE_DRAG_NOSE), caliber, mass)
}

/// One free-flight integration step shared by the live march and range table.
///
/// Invariant: drag uses `v / (1 + k * v * dt)` so both paths produce identical velocity.
pub fn freeflight_step(velocity: Vec3, drag_k: f32, dt: f32) -> Vec3 {
    let v = velocity + gravity() * dt;
    let speed = v.length();
    if speed == 0.0 {
        return v;
    }
    (v / speed) * (speed / (1.0 + drag_k * speed * dt))
}

/// The game's damage model: free-flight state and path from physical birth. Callers must resolve
/// armor crossed by the skipped chords.
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

/// Resolution: maximum fixed steps a cosmetic catch-up may evaluate, bounding per-shot work.
/// The arrival-clock consumer also refuses older wire cues rather than inventing a shortened
/// trajectory. Cursor release and the pre-sync arrival path share this work ceiling.
pub(crate) const MAX_COSMETIC_CATCH_UP_TICKS: u32 = 100;

/// The registry's `Reference` facts (`assets/materials/materials.ron`), parsed once from embedded
/// data and synchronously available wherever a round is bound.
fn reference_steel() -> &'static crate::substances::MaterialFacts {
    static FACTS: std::sync::LazyLock<crate::substances::MaterialFacts> =
        std::sync::LazyLock::new(|| {
            crate::substances::SubstanceRegistry::shipped_static().reference_facts()
        });
    &FACTS
}

/// What the capability law reads off a round: mass (kg), calibre (m) and nose factor `N`
/// (dimensionless): the frictionless nose factor of the cavity-expansion form, in `(0, 1]`, derived
/// from the spec's nose geometry ([`crate::spec::NoseShape`]).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RoundBody {
    pub(crate) mass: f32,
    pub(crate) caliber: f32,
    pub(crate) nose_factor: f32,
}

impl RoundBody {
    /// The law's two round terms against the reference steel `reference`: `m / (2 A k)` (m) and
    /// `k / a` (s²/m²), `k = B · N · ρ_ref` the inertia coefficient (kg/m³) and `a` the
    /// reference's cavity strength term.
    ///
    /// In f64: `m / (2 A k)` grows as `1 / N` and leaves f32's range for a nose factor near f32's
    /// smallest normal, while the capability it multiplies stays finite.
    fn terms_against(self, reference: &crate::substances::MaterialFacts) -> (f64, f64) {
        let caliber = self.caliber as f64;
        let area = std::f64::consts::FRAC_PI_4 * caliber * caliber;
        let k = physical_laws::PhysicalLaws::shipped().cavity_inertia
            * self.nose_factor as f64
            * reference.density;
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

/// Force-law readout (reference-mm) at `speed` (m/s), from [`RoundBody::terms_against`].
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

/// The game's damage model: bulk-work readout in reference-mm at `speed` m/s — the force law
/// `C = 1000 · m / (2 A k) · ln(1 + k v² / a)`, `A = π c² / 4`, `k = B · N · ρ_ref` (`B` the shared cavity-inertia stand-in), `a`
/// the reference steel's cavity strength term
/// ([`crate::substances::MaterialFacts::cavity_strength`], ≈ 3.711e9 Pa, DERIVED).
///
/// Invariants: replica scaling (mass ×s³, calibre ×s gives capability ×s); strictly decreasing in
/// calibre at fixed mass; local speed exponent 2 at low speed, below 2 above it. Evaluated in f64
/// and cast once: finite for every round [`round_in_domain`] admits. Thin-plate geometry changes
/// the charged work, not this bulk reference unit (design doc §3b).
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

/// The game's crew-line HP unit, admitted from `assets/ballistics/crew_line.ron` at build time.
/// Only witness chord dose uses it; hard line and shock energy use [`energy_units`], fields their
/// mechanism's unit through [`field_units`].
pub(crate) const HP_PER_REFERENCE_MM: u64 = authored_constants::HP_PER_REFERENCE_MM;

/// Resolution: 1 dose unit = 1e-8 HP bounds each contribution's rounding error at 5e-9 HP
/// (DERIVED), below the pool resolution while retaining a large accumulator range.
/// Contributions quantize before summation, making the total independent of arrival order.
/// Decimal units represent HP and pool units exactly. A u64 holds 1.8e11 HP (DERIVED).
const DOSE_UNITS_PER_HP: u64 = 100_000_000;

/// The crew-line unit changes dose magnitude, never the fixed-point HP resolution. Its admitted
/// u32 scale times the HP resolution fits u64 (DERIVED: at most 4.3e17 units).
pub(crate) const DOSE_UNITS_PER_REFERENCE_MM: u64 = HP_PER_REFERENCE_MM * DOSE_UNITS_PER_HP;

/// Pool units per HP point: what an HP pool holds ([`ComponentHealth`]), every settle of a shot's
/// dose and every pool figure a record or the wire carries are whole pool units, a thousandth of a
/// point. A box's dose from one shot converts to pool units once ([`pool_units_of`]), so no
/// fraction of a point is dropped and doses below a point accumulate in the pool. Only the display
/// rounds to whole points ([`shown_points`]).
///
/// RESOLUTION: a shot's dose loses at most half a unit, 0.0005 HP, to the conversion, so every
/// dose of 0.05 HP or more stays within 1 % (DERIVED). Decimal, so a pool unit is exactly
/// [`DOSE_UNITS_PER_POOL_UNIT`] dose units. A u32 pool holds [`MAX_POOL_POINTS`] points.
pub const POOL_UNITS_PER_HP: u32 = 1_000;

/// The largest pool a u32 of pool units holds, in whole HP points: 4 294 967 (DERIVED). A tank
/// file or a class file authoring a larger pool is refused at parse.
pub const MAX_POOL_POINTS: u32 = u32::MAX / POOL_UNITS_PER_HP;

/// Dose units per pool unit: [`DOSE_UNITS_PER_HP`] / [`POOL_UNITS_PER_HP`], exact.
const DOSE_UNITS_PER_POOL_UNIT: u64 = DOSE_UNITS_PER_HP / POOL_UNITS_PER_HP as u64;
const _: () = assert!(DOSE_UNITS_PER_POOL_UNIT * POOL_UNITS_PER_HP as u64 == DOSE_UNITS_PER_HP);

/// One dose contribution of `dose` reference-mm in whole dose units: widened to f64 (exact), one
/// IEEE multiply and a round to nearest, ties away from zero, so it is the same on every target. A
/// finite, nonnegative internal dose is required; a broken deposit is a bug. The cast saturates
/// a valid contribution above the accumulator's range.
pub(crate) fn dose_units(dose: f32) -> u64 {
    assert!(
        dose.is_finite() && dose >= 0.0,
        "the game's damage model quantizes finite nonnegative dose from validated deposits"
    );
    (f64::from(dose) * DOSE_UNITS_PER_REFERENCE_MM as f64).round() as u64
}

/// Convert `energy` (J) at [`energy_dose::joules_per_hp`] to [`DOSE_UNITS_PER_HP`] units for hard
/// line deposits and bounce shock; fields use [`field_units`]. The shot tally and pen cam's
/// emptying deposit use this same conversion: exact f64 widening, one IEEE division and multiply,
/// nearest rounding with ties away from zero, identical on every target. Internal energy must be
/// finite and nonnegative; a broken deposit is a bug. Valid contributions above the range saturate.
pub(crate) fn energy_units(energy: f32) -> u64 {
    assert!(
        energy.is_finite() && energy >= 0.0,
        "the game's damage model quantizes finite nonnegative energy from resolved losses"
    );
    (f64::from(energy) / energy_dose::joules_per_hp() * DOSE_UNITS_PER_HP as f64).round() as u64
}

/// The sim's only field J-to-HP conversion: a direction's deposit in any box, hard or witness,
/// at its mechanism's `unit`, in [`DOSE_UNITS_PER_HP`] units. The field carries source energy,
/// which constructions read as crushing energy. Shot tally and pen cam ledger use this same conversion:
/// exact f64 widening, two IEEE multiplies, nearest rounding with ties away from zero, identical
/// on every target. Internal energy must be finite and nonnegative; a broken deposit is a bug.
/// Contributions above the range saturate.
pub(crate) fn field_units(energy: f32, unit: spall_cone::FieldUnit) -> u64 {
    assert!(
        energy.is_finite() && energy >= 0.0,
        "the game's damage model quantizes finite nonnegative field energy from validated sources"
    );
    (f64::from(energy) * unit.hp_per_joule() * DOSE_UNITS_PER_HP as f64).round() as u64
}

/// The pool units ([`POOL_UNITS_PER_HP`]) `units` dose units convert to, rounded to nearest, ties
/// away from zero, in integer arithmetic; saturating.
pub(crate) fn pool_units_of(units: u64) -> u32 {
    let pool = units / DOSE_UNITS_PER_POOL_UNIT
        + u64::from(units % DOSE_UNITS_PER_POOL_UNIT >= DOSE_UNITS_PER_POOL_UNIT / 2);
    u32::try_from(pool).unwrap_or(u32::MAX)
}

/// The HP points `units` dose units carry, unrounded: what a record shows of a dose.
pub(crate) fn hp_before_rounding(units: u64) -> f32 {
    (units as f64 / DOSE_UNITS_PER_HP as f64) as f32
}

/// The HP points a pool holding `held` pool units shows: whole points, rounded up. The one display
/// rule every view of a pool reads (the HUD's cues, the damage panel, the pen cam): a pool short
/// of one whole point lost shows full, and shows damaged only once a whole point is gone; a pool
/// shows none only when it is emptied, at the same instant the sim knocks it out.
pub fn shown_points(held: u32) -> u32 {
    held.div_ceil(POOL_UNITS_PER_HP)
}

/// The shown points ([`shown_points`]) a pool holding `held` pool units loses when `taken` of them
/// are taken: what every view counts as a point lost. Taking past empty takes what was held.
pub fn shown_lost(held: u32, taken: u32) -> u32 {
    shown_points(held) - shown_points(held.saturating_sub(taken))
}

/// The least dose, in HP points, a full pool shows a lost point at: the dose whose pool units
/// ([`pool_units_of`]) reach one whole point ([`shown_points`]), half a pool unit short of a point.
/// A dose below it leaves a full pool showing full.
pub(crate) fn least_shown_hp() -> f64 {
    (DOSE_UNITS_PER_HP - DOSE_UNITS_PER_POOL_UNIT / 2) as f64 / DOSE_UNITS_PER_HP as f64
}

/// Test witness-chord dose (reference-mm) through the production per-shot conversion and
/// invalid-dose invariant.
#[cfg(test)]
pub(crate) fn pool_units(dose: f32) -> u32 {
    pool_units_of(dose_units(dose))
}

/// Per-shot, per-box dose totals and pool losses. Quantize each deposit through [`energy_units`]
/// (hard line and shock), [`field_units`] (fields), or [`dose_units`] at [`HP_PER_REFERENCE_MM`]
/// (witness chords), then sum integers independently of arrival order. Convert the total to pool
/// units once per box per shot, never per direction or source. [`Self::settle`] takes each increase
/// in that converted total; sub-point losses accumulate across shots without whole-point rounding.
///
/// Figures belong to this shot, never a difference of pool HP. A march step resolves one shell
/// before the next and each settle closes one armour mark, isolating its readings and losses even
/// when other rounds dose the box between marks ([`Settle`]).
///
/// Invariant: created with the projectile, never attached after replication. See ADR-0014.
#[derive(Component, Default)]
pub(crate) struct ShotDose {
    tallies: Vec<BoxTally>,
    /// The boxes whose pool this shot took to zero, in the order it emptied them (authority only).
    emptied: Vec<Entity>,
}

/// A box's running and settled dose units, freshness and authority-only settlement history.
struct BoxTally {
    address: Entity,
    units: u64,
    settled: u64,
    fresh: bool,
    settles: Vec<Settle>,
}

/// The game's damage model: one mark's pool settlement. The pen cam uses `units_before`
/// and `held` to locate the emptying contribution in that mark's own recorded deposits.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Settle {
    pub mark: usize,
    pub held: u32,
    pub units_before: u64,
    pub lost: u32,
}

impl Settle {
    pub(crate) fn emptied(&self) -> bool {
        self.lost > 0 && self.lost == self.held
    }

    pub(crate) fn shown_lost(&self) -> u32 {
        shown_lost(self.held, self.lost)
    }
}

impl ShotDose {
    /// The game's damage model: convert crew-line coefficient units to HP at the authored scale.
    fn add(&mut self, address: Entity, amount: f32) {
        self.add_units(address, dose_units(amount));
    }

    /// Convert a bounce's shock energy through the common energy-to-HP rate.
    fn add_energy(&mut self, address: Entity, energy: f32) {
        self.add_units(address, energy_units(energy));
    }

    fn add_units(&mut self, address: Entity, units: u64) {
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

    /// Each witness supplies its own chord dose; the round's crew line rate owns its scale.
    fn add_wounds(&mut self, volumes: &[VolumeCrossing]) {
        for crossed in volumes {
            self.add(crossed.volume, crossed.cost);
        }
    }

    /// Convert field joules through their mechanism's unit. Line dose here is an internal bug.
    fn add_field(&mut self, doses: &[mechanism::Dose], unit: spall_cone::FieldUnit) {
        for dose in doses {
            assert!(
                dose.line_chord().is_none(),
                "the game's damage model converts only field doses at a field's unit"
            );
            self.add_units(dose.address, field_units(dose.amount, unit));
        }
    }

    /// Close armour mark `mark` (its index in the shell's log): take from each box's HP pool the
    /// pool units its converted total has gained since the last settle, floored at zero, and return
    /// the pool units removed. A box with no HP pool takes nothing; only authority settles dose.
    fn settle(&mut self, health: &mut Query<&mut ComponentHealth>, mark: usize) -> u64 {
        let mut removed = 0;
        for entry in &mut self.tallies {
            let fresh = std::mem::take(&mut entry.fresh);
            let units_before = std::mem::replace(&mut entry.settled, entry.units);
            let due = pool_units_of(entry.units).saturating_sub(pool_units_of(units_before));
            if !fresh && due == 0 {
                continue;
            }
            let Ok(mut hp) = health.get_mut(entry.address) else {
                continue;
            };
            let held = hp.held();
            let lost = hp.take(due);
            removed += u64::from(lost);
            entry.settles.push(Settle {
                mark,
                held,
                units_before,
                lost,
            });
            if lost > 0 && hp.is_empty() {
                self.emptied.push(entry.address);
            }
        }
        removed
    }

    /// Each box this shot has reached and the pool units its pool has lost to the shot so far: zero
    /// for one with no HP pool or one already empty, and for every box where a pool is not written
    /// (a replica).
    pub(crate) fn losses(&self) -> impl Iterator<Item = (Entity, u32)> + '_ {
        self.tallies.iter().map(|entry| {
            let lost = entry.settles.iter().map(|settle| settle.lost).sum();
            (entry.address, lost)
        })
    }

    /// Each box this shot has reached and the shown points ([`Settle::shown_lost`]) its settles
    /// took: what a view counts as lost to the shot. A settle short of a whole point shows none
    /// until a later one, of this shot or another, takes the pool past it.
    pub(crate) fn shown_losses(&self) -> impl Iterator<Item = (Entity, u32)> + '_ {
        self.tallies.iter().map(|entry| {
            let lost = entry.settles.iter().map(Settle::shown_lost).sum();
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

/// The game's damage model: hit confirmation counts [`ShotDose::shown_losses`] only on another
/// tank's [`VolumeOf`] pools, matching its display. Unowned pools and the shooter's pools
/// receive dose but confirm no hit; [`walk_field`] reaches volumes regardless of owner.
fn owned_losses(dose: &ShotDose, owners: &Query<&VolumeOf>, shooter: Option<Entity>) -> u32 {
    dose.shown_losses()
        .filter(|(volume, _)| {
            owners
                .get(*volume)
                .is_ok_and(|owner| Some(owner.tank()) != shooter)
        })
        .map(|(_, lost)| lost)
        .sum()
}

/// Rotate `dir` through `share` of the angle to `target`. The angle comes from the cross product:
/// an `acos` of the dot reads zero under 3.45e-4 rad (DERIVED: acos(1 - 2^-24)), and a heading
/// blended toward a face there stops converging on it. Directions exactly parallel or opposed name
/// no plane to turn in, so `dir` stands. `libm`, so one contact reads the same bits on every
/// platform, as the rest of the decision does.
fn turned_toward(dir: Dir3, target: Dir3, share: f32) -> Dir3 {
    let (d, t) = (Vec3::from(dir), Vec3::from(target));
    let across = d.cross(t);
    let Ok(axis) = Dir3::new(across) else {
        return dir;
    };
    let between = libm::atan2f(across.length(), d.dot(t));
    Dir3::new(Quat::from_axis_angle(Vec3::from(axis), share * between) * d)
        .expect("a unit direction turned by a unit quaternion keeps unit length")
}

/// The game's damage model: a source-body-local box enclosing its colliders at both ends of
/// this physics interval. Using the body's frame keeps its translation from ending clearance.
///
/// Launch clearance belongs to the whole disc: the source stays transparent while any sample start
/// is inside the box ([`Self::holds_any`]), so a membership read with the source transparent is
/// handed on only where no start can be inside a source volume (every volume lies in the box).
#[derive(Clone, Copy)]
struct SourceBox {
    min: Vec3,
    max: Vec3,
    body: motion::Poses,
}

impl SourceBox {
    fn contains(self, point: Vec3, fraction: f32) -> bool {
        let local = self.body.at(fraction).local(point);
        local.cmpge(self.min).all() && local.cmple(self.max).all()
    }

    fn holds_any(self, starts: &[Vec3], fraction: f32) -> bool {
        starts.iter().any(|start| self.contains(*start, fraction))
    }

    /// [`Self::exit_seconds`] of the last of `starts` inside the box: when the whole disc has left
    /// it, nudged past the boundary. `None` when a start inside stays inside over the remaining
    /// chord, or no start is inside.
    fn clearance_seconds(
        self,
        starts: &[Vec3],
        velocity: Vec3,
        from: f32,
        remaining: f32,
    ) -> Option<f32> {
        let mut last = None;
        for &start in starts.iter().filter(|start| self.contains(**start, from)) {
            let exit = self.exit_seconds(start, velocity, from, remaining)?;
            last = Some(last.map_or(exit, |last: f32| last.max(exit)));
        }
        last
    }

    /// The game's damage model uses the same local endpoint chord as moving-collider discovery.
    /// Return the seconds to its first box exit, nudged beyond the boundary for rediscovery.
    fn exit_seconds(self, point: Vec3, velocity: Vec3, from: f32, remaining: f32) -> Option<f32> {
        let start = self.body.at(from).local(point);
        let end = self.body.to.local(point + velocity * remaining);
        let chord = end - start;
        let mut exit = f32::INFINITY;
        for axis in 0..3 {
            let along = chord[axis];
            if along != 0.0 {
                let face = if along > 0.0 {
                    self.max[axis]
                } else {
                    self.min[axis]
                };
                exit = exit.min(((face - start[axis]) / along).max(0.0));
            }
        }
        if exit < 1.0 {
            Some((remaining * (exit + MARCH_EPS / chord.length())).min(remaining))
        } else {
            None // No exit on this remaining relative chord, including relative rest.
        }
    }
}

/// First-surface contact along a march segment ([`cast_disc_segment`]).
enum SegmentHit {
    /// A parry hit on the `Armor` layer — real, entity-addressed geometry the penetration
    /// resolution then probes (thickness/exit casts, ancestry, HP).
    Armor(avian3d::prelude::RayHitData),
    Terrain {
        distance: f32,
        normal: Vec3,
    },
}

/// First contact tests armour as a caliber-wide disc (§13.5): a rim clipping a plate must resolve
/// even when the axis threads a gap, or η never grades grazes. Terrain stays axis-only through
/// deterministic [`HeightGrid::cast_ray`]: continuous ground stops the round, with no η or union.
/// One broad-phase AABB traversal is the entire armour cost in open air, cheaper than a ray cast.
/// Ring casts run only near geometry at or above `resolve::DISC_MIN_CALIBER`.
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
    let armor = {
        let sweep = world.sweep.get().unwrap_or(motion::Sweep {
            from: 0.0,
            to: 1.0,
            dt: 1.0,
        }); // Standalone static queries have no interval clock.
        let mut nearest: Option<(avian3d::prelude::RayHitData, motion::ContactMotion)> = None;
        world.spatial.aabb_intersections_with_aabb_callback(
            collect::swept_aabb(origin, *dir, max, reach + world.motion_padding()),
            |entity| {
                if !world
                    .layers
                    .get(entity)
                    .is_ok_and(|layers| layers.memberships.has_all(Layer::Armor))
                {
                    return true;
                }
                let (node, _) = world.armour(entity).expect(
                    "game's damage model: an Armor collider requires BallisticVolume ancestry",
                );
                if !not_own(entity) {
                    return true;
                }
                let (position, rotation, collider, surfaces) = world
                    .geometry
                    .get(entity)
                    .expect("game's damage model: every tree candidate has resolved geometry");
                let poses = if world.sweep.get().is_some() {
                    match world.motion.as_ref() {
                        Some(history) => *history.colliders.get(&entity).expect(
                            "game's damage model: every tree candidate has captured endpoint poses",
                        ),
                        None => {
                            let pose = motion::Pose {
                                position: position.0,
                                rotation: rotation.0,
                            };
                            motion::Poses {
                                from: pose,
                                to: pose,
                            } // Standalone static fixtures.
                        }
                    }
                } else {
                    let pose = world.pose(entity).unwrap_or(motion::Pose {
                        position: position.0,
                        rotation: rotation.0,
                    });
                    motion::Poses {
                        from: pose,
                        to: pose,
                    }
                };
                for &offset in offsets {
                    let swept = motion::sweep_collider(
                        poses,
                        sweep,
                        node,
                        entity,
                        collider,
                        surfaces,
                        origin + offset,
                        origin + offset + dir * max,
                    );
                    let (mut contact, hit) = if poses.from == poses.to
                        && !swept.as_ref().is_some_and(|(motion, _)| motion.contained)
                    {
                        if max == 0.0 {
                            continue;
                        } // A stationary point has no static entering face.
                        let Some((distance, normal)) = collider.cast_ray(
                            poses.to.position,
                            poses.to.rotation,
                            origin + offset,
                            *dir,
                            max,
                            true,
                        ) else {
                            continue;
                        };
                        (
                            motion::ContactMotion {
                                fraction: sweep.from + (sweep.to - sweep.from) * distance / max,
                                point: origin + offset + dir * distance,
                                velocity: Vec3::ZERO,
                                contained: false,
                            },
                            avian3d::prelude::RayHitData {
                                entity,
                                distance,
                                normal,
                            },
                        )
                    } else {
                        let Some((contact, normal)) = swept else {
                            continue;
                        };
                        let part = if contact.contained {
                            0.0
                        } else {
                            (contact.fraction - sweep.from) / (sweep.to - sweep.from)
                        };
                        (
                            contact,
                            avian3d::prelude::RayHitData {
                                entity,
                                distance: part * max,
                                normal: normal.normalize(),
                            },
                        )
                    };
                    contact.point -= offset;
                    if nearest.as_ref().is_none_or(|(best, _)| {
                        hit.distance < best.distance
                            || (hit.distance == best.distance && entity < best.entity)
                    }) {
                        nearest = Some((hit, contact));
                    }
                }
                true
            },
        );
        world
            .contact_motion
            .set(nearest.as_ref().map(|(_, motion)| *motion));
        nearest.map(|(hit, _)| hit)
    };
    let terrain = world
        .grid
        .as_deref()
        .and_then(|grid| grid.cast_ray(origin, Vec3::from(dir), max))
        .map(|hit| (hit.t, hit.normal));
    match (armor, terrain) {
        (Some(armor), Some((distance, normal))) if distance < armor.distance => {
            Some(SegmentHit::Terrain { distance, normal })
        }
        (Some(armor), _) => Some(SegmentHit::Armor(armor)),
        (None, Some((distance, normal))) => Some(SegmentHit::Terrain { distance, normal }),
        (None, None) => None,
    }
}

/// A test's rays through the live march's first-contact query ([`cast_disc_segment`]), each a
/// disc of `radius` laid as a fresh shell's ([`resolve::layout`]): what [`probe_march`] records
/// per run is the armour volume each ray reached first, `None` for open air or the ground.
#[cfg(test)]
#[derive(Resource, Default)]
pub(crate) struct MarchProbe {
    pub(crate) radius: f32,
    pub(crate) rays: Vec<(Vec3, Dir3, f32)>,
    pub(crate) reached: Vec<Vec<Option<Entity>>>,
}

#[cfg(test)]
pub(crate) use march_tests::assert_occupied_matter_queries;

/// The game's damage model: mount [`probe_march`] after completed collider placement, beside
/// the production walk, with an empty [`MarchProbe`].
#[cfg(test)]
pub(crate) fn mount_march_probe(app: &mut App) {
    app.init_resource::<MarchProbe>().add_systems(
        FixedPostUpdate,
        probe_march
            .after(avian3d::prelude::PhysicsSystems::Writeback)
            .after(SimPhase::ColliderPoses)
            .before(SimPhase::ProjectileMarch)
            .before(crate::construction::breakage::LawSet),
    );
}

/// Casts every [`MarchProbe`] ray through the march's query against the world as a march in the
/// same fixed tick reads it.
#[cfg(test)]
fn probe_march(world: ProjectileMarchWorld, mut probe: ResMut<MarchProbe>) {
    let reached = probe
        .rays
        .iter()
        .map(|&(origin, dir, max)| {
            let frame = walk::DiscFrame::anchored(Vec3::from(dir)).expect("a probe's direction");
            let offsets = resolve::layout(&frame, probe.radius);
            match cast_disc_segment(&world, &offsets, origin, dir, max, &|_| true) {
                Some(SegmentHit::Armor(hit)) => world.armour(hit.entity).map(|(volume, _)| volume),
                _ => None,
            }
        })
        .collect();
    probe.reached.push(reached);
}

/// Sandbox inspection keeps a spent shell's mesh, tracer and marks frozen; the game despawns it.
#[derive(Resource, Default)]
pub struct RetainSpentShells(pub bool);

/// Firing tank and weapon slot. Included in the initial shell bundle for self-exclusion and server
/// fire attribution.
#[derive(Clone, Copy, Component)]
pub struct ShotSource {
    /// The tank root the shell was fired from, or the [`crate::FreeShooter`] that fired it.
    pub tank: Entity,
    /// The firing weapon's slot in `TankSim::weapons` — its spawn-time `WeaponIndex`.
    pub weapon: usize,
    /// The fired round: its index in the firing weapon's `Weapon::rounds`, which every role reads
    /// from the same tank data (the fire event carries this index, not the round).
    pub round: u8,
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

/// What a keyed shot did at each resolved face, in the game's damage model: the struck volume,
/// the travel and calibre beside the verdict. Independent of the first-terminal receipt, since
/// the authority can reach further vehicles afterward. Raised wherever the march runs; the
/// authority's cosmetic record (`hull_marks`) is its one reader, and the simulation never reads it.
#[derive(Event, Clone, Copy, Debug)]
pub(crate) struct ShellStrike {
    pub feedback: crate::substances::SurfaceFeedback,
    pub shot: ShotId,
    /// The strike's ordinal in [`PenetrationMarks::log`], so no two resolved faces of one shot
    /// share it, including faces reached after the first terminal.
    pub sequence: u32,
    /// The volume owning the struck face.
    pub struck: Entity,
    /// Where the round met the face (WORLD): a bounce's contact point, a terminal's entrance — the
    /// face, never an embed point inside the material.
    pub at: Vec3,
    /// The struck face's outward normal (WORLD).
    pub normal: Vec3,
    /// The round's direction of travel as it met the face (WORLD, unit).
    pub incoming: Vec3,
    /// The incoming flying body's calibre (m), retained for the scripted battle's strike log.
    pub caliber: f32,
    /// The calibre (m) of what marked the face ([`face_contact_caliber`]): the hull mark's
    /// footprint.
    pub contact_caliber: f32,
    /// What the round did at the face: `Ricochet` for a bounce, `Stopped` or `Through` for a
    /// terminal.
    pub verdict: Verdict,
}

/// Authority-only report that a keyed shot first took a shown HP point from a tank's pool
/// ([`ShotDose::shown_losses`]), so the shooter's hit marker follows what the struck tank shows.
/// [`DamageReport`] emits it at most once; the private wire receipt intentionally carries no HP
/// amount.
#[derive(Event)]
pub(crate) struct ShellDamage {
    pub shot: ShotId,
    /// The shown HP points this step took from the tank's pools.
    pub amount: u32,
}

/// The game's damage model: one line contribution delivered at its resolved path moment.
/// Its momentum reads the physical exchange, independently of retained energy and HP dose.
#[derive(Debug, Clone)]
pub(crate) struct DeliveredLine {
    pub(crate) crossing: VolumeCrossing,
    /// The game's damage model: exposed sample chords in WORLD, clipped at rest; none for a skid,
    /// whose line has no length. Construction allocation reads these; the crossing's axis
    /// endpoints retain the shared rest point.
    pub(crate) contacts: Vec<(Vec3, Vec3)>,
    pub(crate) momentum: Vec3,
    pub(crate) entrance_momentum: Vec3,
    /// The game's damage model: entrance-only work (J) included in the crossing's retained energy.
    pub(crate) entrance_energy: f32,
}

/// One authority march step of `shell` that logged armour marks, threw spall cones or burst,
/// raised in the tick it marched, before the shell can despawn: its marks from `marks_from` and
/// its cones from `cones_from` are this step's, and `burst` is the burst it set off, if it did.
/// The constructions' fracture laws read their deposits off it (`construction::breakage`).
#[derive(Event, Debug, Clone)]
pub(crate) struct ShellMarched {
    pub(crate) shell: Entity,
    pub(crate) marks_from: usize,
    pub(crate) cones_from: usize,
    pub(crate) lines: Vec<DeliveredLine>,
    pub(crate) burst: Option<burst::Burst>,
}

/// Trigger-agnostic shell spawn seam used by guns and sandbox tools.
#[derive(Event)]
pub struct FireShell {
    /// The game's damage model: physical birth; the march and replica begin here.
    pub birth: Vec3,
    /// The visual muzzle, retained for the flash, report and shot explanation.
    pub origin: Vec3,
    pub direction: Dir3,
    pub speed: f32,
    /// The flying body's calibre (m): the shell's view and its impacts read it.
    pub caliber: f32,
    /// The flying body's mass (kg): the momentum a hit or a bounce delivers reads it.
    pub mass: f32,
    /// The round's quadratic-drag coefficient (1/m) ([`crate::rounds::Round::drag_k`]): what the
    /// flight, the catch-up and the range table step with.
    pub drag_k: f32,
    /// Nose factor (dimensionless, `0 < N ≤ 1`) of the round and its penetrator, read by the
    /// capability law.
    pub nose_factor: f32,
    /// The penetrator's calibre (m) ([`crate::rounds::Round::penetrator`]), no greater than
    /// `caliber`: what meets the plate reads it — the bound mechanism, the disc, contact and the
    /// cone's hole. The whole round's for a full-calibre round.
    pub penetrator_caliber: f32,
    /// The penetrator's mass (kg), no greater than `mass`; the bound mechanism reads it.
    pub penetrator_mass: f32,
    /// Crew line rate: reference-mm of dose per metre of chord through a witness (ADR-0040).
    pub wound: f32,
    /// The round's burst ([`crate::rounds::Round::burst`]); `None` for an inert round.
    pub burst: Option<burst::Burst>,
    pub fuze_mode: u8,
    pub(crate) contact: crate::rounds::ContactPreset,
    /// The game's damage-model view: emissions authored by the round, never inferred from calibre.
    pub(crate) effects: crate::rounds::FlightEffects,
    /// The shot's muzzle emission ([`crate::rounds::Round::muzzle`]), view only.
    pub(crate) muzzle: crate::guns::MuzzleEmission,
    /// Fire mechanism at the source.
    pub mechanism: crate::spec::FireMechanism,
    /// Firing source for self-exclusion and authority attribution.
    pub shooter: Option<ShotSource>,
    /// Whether this round has a tracer visual; flight and collision are unaffected.
    pub tracer: bool,
    pub shot_origin: FireShellOrigin,
    /// Free-flight ticks to apply at spawn for a reconstructed remote shell.
    pub catch_up_ticks: u32,
    /// Network identity; `None` for a shell fired where no shot clock runs.
    pub shot: Option<ShotId>,
}

impl FireShell {
    /// The game's damage model: physical `birth` and visual muzzle `origin` are shot facts.
    /// Maps `round` data to a locally authored shell at its own muzzle speed, with no catch-up.
    /// A reconstructed shell overrides the origin kind and catch-up after supplying both facts.
    #[expect(
        clippy::too_many_arguments,
        reason = "the shot states both physical birth and visual muzzle"
    )]
    pub(crate) fn of_round(
        round: &crate::rounds::Round,
        birth: Vec3,
        origin: Vec3,
        direction: Dir3,
        mechanism: crate::spec::FireMechanism,
        tracer: bool,
        shooter: Option<ShotSource>,
        shot: Option<ShotId>,
    ) -> Self {
        Self {
            birth,
            origin,
            direction,
            speed: round.muzzle_speed,
            caliber: round.flying.caliber,
            mass: round.flying.mass,
            drag_k: round.drag_k,
            nose_factor: round.flying.nose_factor,
            penetrator_caliber: round.penetrator.caliber,
            penetrator_mass: round.penetrator.mass,
            wound: round.wound,
            contact: round.contact,
            burst: round.burst,
            fuze_mode: round.fuze_mode,
            effects: round.effects,
            muzzle: round.muzzle,
            mechanism,
            tracer,
            shooter,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks: 0,
            shot,
        }
    }
}

/// The game's damage model: what flies between contacts. A resisting bite leaves the core and
/// a burst leaves its nose. This type owns their drag rebuild; contact discovery stores its
/// result and playback presents it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FlyingBody {
    mass: f32,
    caliber: f32,
    /// Quadratic drag (1/m), proportional to form times calibre squared over mass.
    drag_k: f32,
}

impl FlyingBody {
    fn with_body(self, body: RoundBody) -> Self {
        Self {
            mass: body.mass,
            caliber: body.caliber,
            drag_k: self.drag_k * self.mass / body.mass * (body.caliber / self.caliber).powi(2),
        }
    }

    fn after(self, decision: &resolve::Decision, core: RoundBody) -> Self {
        if matches!(decision, resolve::Decision::Bite(_)) {
            self.with_body(core)
        } else {
            self
        }
    }
}

/// A shell in flight. Kinematic — integrated by hand, no physics engine.
#[derive(Component)]
#[require(ShotDose)]
pub(crate) struct Projectile {
    impact_style: crate::rounds::ImpactStyle,
    velocity: Vec3,
    /// The mechanism of what meets the plate, the round's penetrator, bound at spawn from its fire
    /// data and the substance registry ([`mechanism::SolidShot::bind`]) — data, never an asset.
    /// The disc, contact, the force law and the cone's hole read its body.
    round: mechanism::SolidShot,
    contact: crate::rounds::ContactPreset,
    flying: FlyingBody,
    /// Crew-line coefficient: disc-mean chord times this times [`HP_PER_REFERENCE_MM`] gives HP dose.
    /// Read only by the witness pass; the hard walk never sees it.
    wound: f32,
    /// The round's fuze, built at spawn from its fire data ([`burst::Fuze::of`]) and advanced by
    /// the authority's march alone: SIM state, never on the wire.
    fuze: burst::Fuze,
    /// Authority's §13.5 sampling frame: sim state deciding hits. Anchored from the fire direction
    /// at spawn ([`DiscFrame::anchored`]) and parallel-transported through gravity and ricochets;
    /// rebuilding from current travel would snap the pattern across the anchoring branch.
    /// Replicas consume the exported path without using this frame.
    disc: walk::DiscFrame,
    /// The game's damage model: where every sample stands as the last step ended, anchor-relative.
    /// Discovery retains these reached positions, including a skid's disc or perforated hand-offs,
    /// and reads any material newly holding them in the current scene.
    ///
    /// `None` at spawn: authority discovery lays its birth disc at the shell's translation.
    /// Replicas carry no discovery starts.
    starts: Option<resolve::Interior>,
    /// The game's damage model: immutable crossings awaiting their real path moments. Empty
    /// at spawn; authority playback advances its cursor across fixed-tick boundaries.
    playback: std::collections::VecDeque<resolve::Playback>,
    path_sequence: u32,
    replica: Option<path::Replica>,
    /// The game's damage model: tanks touched by this round, for first-contact statistics.
    contacted: Vec<Entity>,
    /// The game's damage model: source clearance ends permanently at the first box exit.
    source_cleared: bool,
}

impl Projectile {
    pub(crate) fn impact_style(&self) -> crate::rounds::ImpactStyle {
        self.impact_style
    }

    /// The flying body's caliber (m), read by the geometric views.
    pub(crate) fn caliber(&self) -> f32 {
        self.flying.caliber
    }

    /// What meets the plate: the penetrator's body, which the capability law, the disc and
    /// contact read.
    fn body(&self) -> RoundBody {
        self.round.body()
    }

    /// The penetrator's calibre (m): the diameter of the hole a perforation cuts.
    pub(crate) fn penetrator_caliber(&self) -> f32 {
        self.body().caliber
    }

    /// The round's current speed (m/s) — read by view code that scales an effect off a shell in
    /// flight (the tracer streak's nominal length).
    pub(crate) fn speed(&self) -> f32 {
        self.velocity.length()
    }

    /// Whether this tank armed the fuze; its recorder keeps that attribution encounter open.
    pub(crate) fn armed_by(&self, tank: Entity) -> bool {
        self.fuze
            .armed()
            .is_some_and(|(_, armed)| armed == Some(tank))
    }

    /// Stationary fixture for view tests needing a projectile entity; never marched.
    #[cfg(test)]
    pub(crate) fn view_test_round(caliber: f32) -> Self {
        Self {
            impact_style: crate::rounds::ImpactStyle::fixture(crate::rounds::ImpactPreset::Compact),
            velocity: Vec3::ZERO,
            round: bound(RoundBody {
                mass: 1.0,
                caliber,
                nose_factor: 0.3,
            }),
            flying: FlyingBody {
                caliber,
                mass: 1.0,
                drag_k: 0.0,
            },
            contact: crate::rounds::ContactPreset::fixture(),
            wound: 30.0,
            fuze: burst::Fuze::Inert,
            disc: walk::DiscFrame {
                u: Vec3::X,
                v: Vec3::Y,
            },
            starts: None,
            playback: Default::default(),
            path_sequence: 0,
            replica: None,
            contacted: Vec::new(),
            source_cleared: false,
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

/// Solid taxing the penetrator over geometric line-of-sight distance, for plates and modules
/// alike (§2), on `Armor`. At extraction, `bake` resolves primitive material names against
/// `assets/materials/materials.ron` (§12): no per-query string lookup or per-node authored number.
#[derive(Component)]
pub struct BallisticVolume {
    /// The substance's identity and its §13.2 field value (reference-mm of armour per metre of
    /// chord).
    pub material: walk::VolumeMaterial,
    /// The substance's registry name — identity, for diagnostics and inspection. Never parsed for
    /// behaviour.
    #[allow(
        dead_code,
        reason = "carried from the bake for the x-ray readout; the walk reads only the factor, \
                  deliberately"
    )]
    pub substance: String,
}

/// Crew HP pool at a station (ADR-0040), dosed by what the hard world sends through it ([`wound`]).
/// It supplies no armour layer, resistance, shadowing or spall. Colliders carry [`WitnessSurface`]
/// on [`crate::Layer::Witness`]; the node carries [`ComponentHealth`].
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

/// The game's damage model: HP pool for a crew station, module or ammunition.
/// Data authors whole HP points; holdings use [`POOL_UNITS_PER_HP`] units per point. [`ShotDose`]
/// converts once per box per shot, combining penetrator energy loss (crew: chord × crew line rate),
/// bounce shock and entering cone directions (§§5–6). Sub-point doses accumulate; holdings stop
/// at zero. The owner decides cook-off or station/module knockout. Views read only
/// [`Self::shown`].
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComponentHealth {
    /// Pool units held, at most `points × POOL_UNITS_PER_HP`.
    held: u32,
    /// The pool's size, in whole HP points, at most [`MAX_POOL_POINTS`].
    points: u32,
}

impl ComponentHealth {
    /// A full pool of `points` whole HP points; a size past [`MAX_POOL_POINTS`] (which every parse
    /// refuses) saturates there.
    pub fn full(points: u32) -> Self {
        let points = points.min(MAX_POOL_POINTS);
        Self {
            held: points * POOL_UNITS_PER_HP,
            points,
        }
    }

    /// A pool of `points` whole HP points holding `held` pool units, at most full.
    #[cfg(test)]
    pub fn holding(points: u32, held: u32) -> Self {
        let mut pool = Self::full(points);
        pool.set_held(held);
        pool
    }

    /// The pool's size, in whole HP points.
    pub fn points(&self) -> u32 {
        self.points
    }

    /// The pool units it holds.
    pub fn held(&self) -> u32 {
        self.held
    }

    /// The pool units it holds when full.
    pub fn capacity(&self) -> u32 {
        self.points * POOL_UNITS_PER_HP
    }

    /// It holds nothing: the HP pool emptied, its station or module knocked out.
    pub fn is_empty(&self) -> bool {
        self.held == 0
    }

    /// The HP points it shows ([`shown_points`]): whole points, rounded up.
    pub fn shown(&self) -> u32 {
        shown_points(self.held)
    }

    /// Take up to `units` pool units, stopping at empty; returns the pool units taken.
    pub fn take(&mut self, units: u32) -> u32 {
        let taken = units.min(self.held);
        self.held -= taken;
        taken
    }

    pub fn drain(&mut self) {
        self.held = 0;
    }

    pub fn refill(&mut self) {
        self.held = self.capacity();
    }

    /// Hold `held` pool units, at most full: a replica writing the authority's state.
    pub fn set_held(&mut self, held: u32) {
        self.held = held.min(self.capacity());
    }
}

/// One crossing of a ballistic volume by the penetrator: where it entered and exited the solid.
/// `(exit - entry).length()` is the geometric line-of-sight thickness — slope captured by geometry,
/// no cosine term (design doc §2).
#[cfg(any(feature = "dev_ui", test))]
pub struct PenetrationEvent {
    pub entry: Vec3,
    pub exit: Vec3,
}

/// One skid of a shell's. The strike and the axis's turn are different points (a skid turns a body
/// radius off the face), so what follows the path reads the turn by its place, never by coordinate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RicochetMark {
    /// Where the round struck the face.
    pub at: Vec3,
    /// The axis node the skid turned at, in the shell's [`ShellPath::points`].
    pub path_index: usize,
}

/// The volume crossings a shell has made this flight — what the sandbox draws to inspect the march.
/// Public, like `ShellPath`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct PenetrationMarks {
    #[cfg(any(feature = "dev_ui", test))]
    pub events: Vec<PenetrationEvent>,
    /// Each skid off a too-oblique face (deflected, did not enter), in order.
    pub ricochets: Vec<RicochetMark>,
    /// Every armour event the authority's march resolved, in march order: the whole path a pen cam
    /// record is read from. Replicas decide no armour; their ricochet points come from public paths.
    pub log: Vec<ArmourMark>,
    /// The game's damage model: immutable sampled hull frames, addressed by mark and owner.
    pub(crate) frames:
        std::collections::BTreeMap<(usize, Entity), crate::damage_cam::record::HullFrame>,
    /// The game's damage model: each tank a crossing reached, as `(crossing mark, tank)`, in the
    /// order the round reached them. A crossing shared by overlapping tanks reaches a later tank
    /// after its entrance, possibly in a later tick; the recorder opens that tank's encounter then.
    pub(crate) reached: Vec<(usize, Entity)>,
}

/// One armour event of a shell's flight, as the authority's march resolved it. Positions WORLD,
/// speeds m/s.
#[derive(Clone, Debug)]
pub enum ArmourMark {
    /// The game's damage model: slid off a face or was held there, without entering it.
    Bounce {
        /// The game's damage model: the axis at the exchange, distinct from its contact point.
        at: Vec3,
        position: Vec3,
        normal: Vec3,
        incoming: Vec3,
        outgoing: Vec3,
        speed_in: f32,
        speed_out: f32,
        /// The volume owning the struck face.
        struck: Entity,
        /// The dose the bounce jarred into `struck`, joules: the kinetic energy the round lost at
        /// the bounce. Armour has no pool and takes none.
        shock: f32,
    },
    /// Entered the material: came to rest inside it or out the far side.
    Crossing {
        entrance: Vec3,
        normal: Vec3,
        incoming: Vec3,
        speed_in: f32,
        /// Every volume the line put a chord into, in volume order.
        volumes: Vec<VolumeCrossing>,
        end: CrossingEnd,
        /// The spall cones this crossing threw: the next `cones` of the shell's [`SpallMarks`].
        cones: usize,
    },
    /// The round's fuze ran out at `at` (design doc §5a, "The burst"), the round travelling at
    /// `speed` (zero for a round at rest). `nose` is the mass (kg) of the nose piece, which flies
    /// on from `at` unless the round was at rest.
    Burst {
        at: Vec3,
        /// The game's environment airblast origin: the authored filler centre, behind the tip.
        field_origin: Vec3,
        speed: f32,
        /// Seconds from the round reaching `at` to the burst: the delay's remainder for a round at
        /// rest, zero in flight. The march settles the field in the tick the round stopped; the
        /// record times it here.
        after: f32,
        /// The burst's greatest sideways speed `v_s` (m/s), its round's ([`burst::Burst`]).
        sideways: f32,
        nose: f32,
        /// The tank whose body armed or last restarted the fuze, for attribution only.
        tank: Option<Entity>,
    },
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

#[derive(Clone, Copy, Debug)]
pub enum CrossingEnd {
    Rest { at: Vec3 },
    Exit { at: Vec3, speed: f32 },
}

/// How far a crossing's line got through one volume.
#[derive(Clone, Copy, Debug)]
pub struct VolumeCrossing {
    pub volume: Entity,
    /// The game's damage model records the first entry and last exit (or rest) of its dosed,
    /// exposed portions on the crossing's axis (WORLD); hidden contacts do not extend them.
    pub entry: Vec3,
    pub end: Vec3,
    /// Disc-mean chord (m).
    pub chord: f32,
    /// The game's damage model: hard matter's reference-mm readout, chord × factor. A witness
    /// carries its `chord × wound` dose here, from the round's crew line rate
    /// ([`ArmourMark::Wound`]).
    pub cost: f32,
    /// The game's damage model: retained work (J), after outgoing debris, from the physical
    /// exchange. Zero for a witness, which takes no energy and is dosed by `cost`.
    pub energy: f32,
    /// The whole disc traversed its exposed portions; it may still occupy a hidden portion.
    pub perforated: bool,
}

/// One direction of a spall cone, as marched: where it stopped (where it entered the first hard
/// body it met, or the struck tank's horizon when it met none), and the boxes it dosed.
#[derive(Clone)]
pub struct SpallRay {
    /// The game's damage model: the emitted direction remains known when a ray stops at its
    /// origin. Environment fragment momentum follows this direction, including contact stops.
    pub direction: Dir3,
    pub end: Vec3,
    /// The strength (J) the direction was thrown with: the cone's `S` times its profile. It and
    /// its doses are the field's energy, never HP: a construction crushes with `strength / N`, a box's
    /// pool reads a dose through its field's unit ([`field_units`]).
    pub strength: f32,
    /// The doses it addressed (volume, J), in entry order.
    pub doses: Vec<(Entity, f32)>,
}

/// The game's damage model: one emitted spall field, with its origin, axis, descriptor, total
/// strength and marched directions. The recorder reads its strength to reconstruct the field.
#[derive(Clone)]
pub struct SpallCone {
    /// The game's damage-model source volumes, retained for authority-local effect attribution.
    #[cfg(any(test, feature = "dev_ui"))]
    pub(crate) exited: Vec<Entity>,
    pub origin: Vec3,
    pub axis: Dir3,
    /// The field law and ratio its directions were weighted by: its exit layer's substance's.
    /// Its rim, the cone's half-angle, is [`spall_cone::FieldDescriptor::half_angle`].
    pub field: spall_cone::FieldDescriptor,
    /// The strength `S` (J) the directions were thrown with ([`walk::SpallSource::strength`]):
    /// direction `k` carries `S·f_k`, and the cone emits `S/2` in all, the energy its round lost
    /// across the body times the debris share `κ` and the exit material's strength ratio `r`.
    pub strength: f32,
    pub rays: Vec<SpallRay>,
}

impl SpallCone {
    /// The game's damage model records one admitted, nonempty direction set per exit field.
    /// Each direction represents this share of the emitted field.
    pub(crate) fn ray_share(&self) -> f32 {
        assert!(
            !self.rays.is_empty(),
            "a recorded cone has its admitted nonempty direction set"
        );
        1.0 / self.rays.len() as f32
    }
}

/// A burst's field (design doc §5a, "The burst"): thrown from the burst point about the round's
/// flight direction at the carried speed; `rays` are its directions as marched, in stratum order.
pub struct BurstField {
    pub origin: Vec3,
    pub axis: Dir3,
    /// The burst law and ratio its directions were weighted by: the ring at the carried speed (its
    /// [`ArmourMark::Burst`]'s) over the burst's greatest sideways speed.
    pub field: spall_cone::FieldDescriptor,
    /// The strength `S` (J) the directions were thrown with: direction `k` carries `S·f_k`, and
    /// the field emits `S/2` in all, its wall's driven energy ([`spall_cone::field_strength`]).
    pub strength: f32,
    pub rays: Vec<SpallRay>,
}

/// The fields a shell has thrown this flight — one cone per body exit it reached, and its burst.
/// Public like `PenetrationMarks`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct SpallMarks {
    pub cones: Vec<SpallCone>,
    /// The round's burst field, once it has burst and thrown one.
    pub burst: Option<BurstField>,
}

/// Catch-up age beyond which muzzle effects and fire audio are suppressed; authority dose is
/// unaffected.
pub(crate) const STALE_FIRE_TICKS: u32 = authored_constants::STALE_FIRE_TICKS;

/// This round carries a tracer — a fact of the BELT it came out of ([`FireShell::tracer`]), and so a
/// simulation fact: it says what the round IS, not that anything is drawn. Flight and collision are
/// identical with or without it; only [`view`] reads it, to decide whether to hang a streak.
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct TracerRound;

/// The resolved surface: terrain, or the struck substance's complete presentation response.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImpactSurface {
    Terrain,
    Material(crate::substances::SurfaceFeedback),
}

/// The game's damage model: the public outcome at a surface. `Ricochet` is a resolved
/// deflection; `Stopped` entered material and ended inside it; `Through` left its far side.
/// `Struck` is a surface contact without a material crossing, as at terrain.
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

/// The game's damage-model view: the calibre (m) of the body that marks a struck face. A
/// perforation's hole is cut by the penetrator (`penetrator`, the body the cone's hole reads); a
/// bounce or a stop is the flying assembly (`flying`, a kept carrier with its core) meeting the
/// face. Equal for a full-calibre round and for a discarded carrier, whose flying body is its
/// penetrator.
pub(crate) fn face_contact_caliber(verdict: Verdict, flying: f32, penetrator: f32) -> f32 {
    match verdict {
        Verdict::Through => penetrator,
        Verdict::Ricochet | Verdict::Stopped | Verdict::Struck => flying,
    }
}

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
    /// Finite nonzero outward surface normal from the certified geometry or checked server path.
    pub(crate) normal: Vec3,
    /// Authored contact dressing, bound with the round at spawn.
    pub(crate) style: crate::rounds::ImpactStyle,
    /// Surface class resolved from volume ancestry.
    pub(crate) surface: ImpactSurface,
    /// What the round did at the struck surface; [`ImpactOutcome::Struck`] for terrain.
    pub(crate) outcome: ImpactOutcome,
}

/// Tags a debug impact marker for view observers.
#[derive(Component)]
pub struct ImpactMarker;

/// Flight, armour resolution and impact events. Spawns bare sim entities: no assets, meshes,
/// materials or `Visibility`. Client roots dress them through [`view::plugin`].
pub fn sim_plugin(app: &mut App) {
    // Validate the game's damage-model laws, pools and field units before play.
    spall_cone::ConeShape::shipped();
    spall_cone::cone_unit();
    spall_cone::BurstFields::shipped();
    box_classes::BoxClasses::shipped();
    energy_dose::joules_per_hp();
    physical_laws::PhysicalLaws::shipped();
    app.init_resource::<crate::map::MapManifest>();
    app.configure_sets(
        FixedPostUpdate,
        (
            avian3d::prelude::PhysicsSystems::Writeback,
            SimPhase::ColliderPoses,
            SimPhase::ProjectileMarch,
            DamageConsequences,
            SimPhase::RecordedConsequences,
            crate::construction::breakage::LawSet,
            SimPhase::Publish,
        )
            .chain(),
    )
    .init_resource::<PathClock>()
    .add_systems(
        PostUpdate,
        path::present
            .in_set(path::PathPresent)
            .before(bevy::transform::TransformSystems::Propagate),
    )
    .init_resource::<RetainSpentShells>()
    .add_observer(on_fire_shell)
    // The game's damage model walks the completed physics interval. Its impulses affect
    // the next physics step; all consequences and publications follow in the phase chain.
    .add_systems(
        FixedPostUpdate,
        integrate_projectiles
            .in_set(GameplaySet)
            .in_set(SimPhase::ProjectileMarch)
            .before(DamageConsequences),
    );
    mount_collider_motion(app);
}

/// The game's damage model keeps collider history only for authority resolution ([`motion`]).
/// [`integrate_projectiles`] returns on [`ClientReplica`]; clients replay public paths (ADR-0037).
fn mount_collider_motion(app: &mut App) {
    app.init_resource::<motion::ColliderMotion>()
        .add_observer(motion::forget_collider)
        .add_observer(motion::forget_body)
        .add_systems(
            FixedPostUpdate,
            motion::seed_births
                .run_if(not(resource_exists::<ClientReplica>))
                .after(avian3d::prelude::PhysicsSystems::Prepare)
                .before(avian3d::prelude::PhysicsSystems::StepSimulation),
        )
        .add_systems(
            FixedPostUpdate,
            (
                motion::place_children,
                avian3d::collider_tree::update_moved_collider_aabbs::<Collider>
                    .run_if(resource_exists::<avian3d::collider_tree::ColliderTrees>),
                motion::snapshot,
            )
                .chain()
                .run_if(not(resource_exists::<ClientReplica>))
                .in_set(SimPhase::ColliderPoses),
        );
}

/// The game's damage model: construct a complete flight from fire data. Replicas never query geometry.
#[expect(
    clippy::too_many_arguments,
    reason = "birth binds fire data, clocks, recording and weapon attribution"
)]
fn on_fire_shell(
    fire: On<FireShell>,
    fixed_time: Res<Time<Fixed>>,
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
    let now = shot_clock.as_deref().map_or(0, |clock| clock.0.raw());
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
    let drag = fire.drag_k;
    let dt = fixed_time.timestep().as_secs_f32();
    let (position, velocity, points) = fast_forward_shell(
        fire.birth,
        fire.direction * fire.speed,
        drag,
        dt,
        fire.catch_up_ticks,
    );

    // Travel direction after any catch-up (gravity/drag bend it); fall back to the bore for a
    // degenerate zero velocity so a spent-to-rest catch-up never trips `Dir3`.
    let travel = Dir3::new(velocity).unwrap_or(fire.direction);
    // Tracer presence never changes flight or collision: non-tracer rounds still resolve impacts.
    // Dead-reckoned streaks were rejected; client-only `ballistics::view` reads `TracerRound`.
    let penetrator = RoundBody {
        mass: fire.penetrator_mass,
        caliber: fire.penetrator_caliber,
        nose_factor: fire.nose_factor,
    };
    let shell_base = (
        fire.effects,
        Projectile {
            impact_style: crate::rounds::ImpactStyle::of(
                fire.effects.impact,
                fire.mass,
                fire.speed,
            ),
            velocity,
            round: bound(penetrator),
            contact: fire.contact,
            flying: FlyingBody {
                caliber: fire.caliber,
                mass: fire.mass,
                drag_k: drag,
            },
            wound: fire.wound,
            fuze: burst::Fuze::of(fire.burst),
            // From the BORE, not from the post-catch-up travel direction: a shell reconstructed with
            // catch-up must anchor its ring exactly where the locally fired one did.
            disc: walk::DiscFrame::anchored(Vec3::from(fire.direction))
                .expect("a checked fire bore anchors a finite sampling frame"),
            starts: None,
            playback: Default::default(),
            path_sequence: 0,
            contacted: Vec::new(),
            source_cleared: false,
            replica: replica.is_some().then(|| {
                path::Replica::born(
                    fire.shot
                        .map_or(now.wrapping_sub(fire.catch_up_ticks), |shot| shot.fire_tick),
                    fire.birth,
                    fire.direction * fire.speed,
                )
            }),
        },
        DamageReport::default(),
        TerminalReport::default(),
        ShellPath {
            points,
            segment_starts: Vec::new(),
        },
        PenetrationMarks::default(),
        SpallMarks::default(),
        Transform::from_translation(position).looking_to(travel, Vec3::Y),
    );

    // One spawn transaction lets `view`'s dressing observer read a complete shell: Bevy 0.19
    // writes the bundle before `Add` observers; a late insert would be incomplete. `Option<T>`
    // is not a Bundle, so branch explicitly; `spawn_shell` folds in tracer and recording.
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
    match (fire.shot, fire.shooter) {
        (Some(shot), Some(source)) => {
            spawn_shell(&mut commands, (shell_base, Shot(shot), source), born)
        }
        (Some(shot), None) => spawn_shell(&mut commands, (shell_base, Shot(shot)), born),
        (None, Some(source)) => spawn_shell(&mut commands, (shell_base, source), born),
        (None, None) => spawn_shell(&mut commands, shell_base, born),
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

/// Optional tracer and recording facts included in the spawn transaction.
struct Born {
    tracer: bool,
    recording: Option<ShotRecording>,
}

/// Fold [`TracerRound`] and [`ShotRecording`] into any caller bundle without multiplying its
/// identity/source branches; see [`on_fire_shell`] for the complete-spawn invariant.
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
    shot_clock: Option<Res<'w, ShotClock>>,
}

/// Armour geometry, hit ownership and deterministic ground shared by march phases as one
/// [`SystemParam`].
#[derive(SystemParam)]
struct ProjectileMarchWorld<'w, 's> {
    motion: Option<Res<'w, motion::ColliderMotion>>,
    collider_bodies: Query<'w, 's, &'static avian3d::prelude::ColliderOf>,
    body_colliders: Query<'w, 's, &'static avian3d::prelude::RigidBodyColliders>,
    layers: Query<'w, 's, &'static avian3d::prelude::CollisionLayers>,
    rigs: Query<'w, 's, (Entity, &'static crate::tank::Rig)>,
    locals: Query<'w, 's, &'static Transform, Without<Projectile>>,
    moment: Local<'s, std::cell::Cell<Option<f32>>>,
    sweep: Local<'s, std::cell::Cell<Option<motion::Sweep>>>,
    contact_motion: Local<'s, std::cell::Cell<Option<motion::ContactMotion>>>,
    geometry: Query<
        'w,
        's,
        (
            &'static Position,
            &'static Rotation,
            &'static Collider,
            Option<&'static BallisticSurfaces>,
        ),
    >,
    spatial: SpatialQuery<'w, 's>,
    // Each collider's broad-phase bounds, the ones `spatial`'s AABB queries test a reached leaf
    // against: a field throw reads its candidates from them ([`resolve::FieldReach`]).
    aabbs: Query<'w, 's, &'static ColliderAabb>,
    volumes: Query<'w, 's, &'static BallisticVolume>,
    owners: Query<'w, 's, &'static VolumeOf>,
    parents: Query<'w, 's, &'static ChildOf>,
    // The heightmap ground, when the heightmap world is live: the march's terrain contacts read
    // the exact deterministic caster (see [`cast_disc_segment`]). Geometry-only fixtures may
    // omit the ground.
    grid: Option<Res<'w, HeightGrid>>,
    // Instrument-only alternatives; shipped marches read their one loaded law and work bound.
    #[cfg(test)]
    spall_law: Option<Res<'w, spall_cone::SpallLaw>>,
    #[cfg(test)]
    burst_law: Option<Res<'w, spall_cone::BurstLaw>>,
    #[cfg(test)]
    step_crossings: Option<Res<'w, resolve::StepCrossings>>,
    #[cfg(test)]
    contact_log: Option<Res<'w, contact_classes::ContactLog>>,
    // Witness volumes (ADR-0040) and their colliders: read by the witness pass, excluded from every
    // armour classification.
    witnesses: Query<'w, 's, &'static WitnessVolume>,
    witness_surfaces: Query<'w, 's, (), With<WitnessSurface>>,
}

impl ProjectileMarchWorld<'_, '_> {
    fn source_box(&self, tank: Entity) -> Option<SourceBox> {
        // Tank-less tool fire and a despawned shooter have no source body to clear.
        let colliders = self.body_colliders.get(tank).ok()?;
        let history = self
            .motion
            .as_ref()
            .expect("the authority captures source-body motion before marching its rounds");
        let body = *history
            .bodies
            .get(&tank)
            .expect("a source rigid body has captured endpoint poses before projectile march");
        let mut bounds: Option<SourceBox> = None;
        for entity in colliders.iter() {
            let (_, _, shape, _) = self.geometry.get(entity).expect(
                "a source body's registered collider has prepared geometry before the march",
            );
            let poses = *history
                .colliders
                .get(&entity)
                .expect("a source body's registered collider has captured endpoint poses");
            for fraction in [0.0, 1.0] {
                let root = body.at(fraction);
                let part = poses.at(fraction);
                let aabb = shape.aabb(
                    root.local(part.position),
                    root.rotation.inverse() * part.rotation,
                );
                bounds = Some(match bounds {
                    None => SourceBox {
                        min: aabb.min,
                        max: aabb.max,
                        body,
                    },
                    Some(bounds) => SourceBox {
                        min: bounds.min.min(aabb.min),
                        max: bounds.max.max(aabb.max),
                        body,
                    },
                });
            }
        }
        bounds
    }

    fn pose(&self, entity: Entity) -> Option<motion::Pose> {
        let fraction = self.moment.get()?;
        let motion = self.motion.as_ref()?; // Static query fixtures carry no interval history.
        Some(motion.colliders.get(&entity).expect(
            "game's damage model: a frozen-scene candidate belongs to the captured collider interval"
        ).at(fraction))
    }

    fn hull_frames(&self) -> Vec<(Entity, crate::damage_cam::record::HullFrame)> {
        let (Some(fraction), Some(history)) = (self.moment.get(), self.motion.as_ref()) else {
            return Vec::new(); // Standalone static fixtures have no captured interval.
        };
        self.rigs
            .iter()
            .map(|(tank, rig)| {
                let pose = history
                    .bodies
                    .get(&tank)
                    .expect("game's damage model: a tank rig has a captured body pose")
                    .at(fraction);
                let (at, turn) = crate::tank::rig_world_pose(
                    rig.hull,
                    tank,
                    pose.position,
                    pose.rotation,
                    &self.parents,
                    &self.locals,
                )
                .expect("game's damage model: the hull belongs to its rig hierarchy");
                (tank, crate::damage_cam::record::HullFrame::posed(at, turn))
            })
            .collect()
    }

    fn body_primitives(&self, primitive: Entity) -> Vec<(Entity, Entity)> {
        let body = self
            .collider_bodies
            .get(primitive)
            .expect("game's damage model: a prepared collider belongs to a physics body")
            .body;
        let parts = self
            .body_colliders
            .get(body)
            .expect("game's damage model: ColliderOf populates its body's collider relationship");
        let mut parts: Vec<_> = parts
            .iter()
            .filter_map(|part| self.armour(part).map(|(node, _)| (node, part)))
            .collect();
        parts.sort();
        parts
    }

    fn motion_padding(&self) -> f32 {
        if self.moment.get().is_some() || self.sweep.get().is_some() {
            self.motion
                .as_ref()
                .map_or(0.0, |motion| motion.largest_motion)
        } else {
            0.0
        }
    }

    /// The armour volume a collider belongs to: its nearest [`BallisticVolume`] ancestor. `None`
    /// for terrain and for a witness's collider, whatever node a witness is parented under — a
    /// witness is never a layer of the walk (ADR-0040).
    fn armour(&self, collider: Entity) -> Option<(Entity, &BallisticVolume)> {
        if self.witness_surfaces.contains(collider) {
            return None;
        }
        let volume = hit_ancestor(collider, &self.volumes, &self.parents);
        if volume.is_some() {
            assert!(
                self.layers
                    .get(collider)
                    .is_ok_and(|layers| layers.memberships.has_all(Layer::Armor)),
                "game's damage model: a BallisticVolume collider requires Armor membership"
            );
        }
        volume
    }

    /// The volume node a collider belongs to, of either kind: a witness's own node for a witness
    /// collider, the armour volume otherwise. A field admits matter independently of ownership.
    fn volume_of(&self, collider: Entity) -> Option<Entity> {
        if self.witness_surfaces.contains(collider) {
            hit_ancestor(collider, &self.witnesses, &self.parents).map(|(node, _)| node)
        } else {
            self.armour(collider).map(|(node, _)| node)
        }
    }

    /// The game's damage model: one step's seconds under this world's crossing ceiling.
    fn step_budget(&self, seconds: f64) -> resolve::Budget {
        let ceiling = resolve::MAX_STEP_CROSSINGS;
        #[cfg(test)]
        let ceiling = self
            .step_crossings
            .as_ref()
            .map_or(ceiling, |chosen| chosen.0);
        resolve::Budget::new(seconds, ceiling)
    }
}

/// One march phase's shell pose, flight state and records. [`Mut`] wrappers preserve change
/// detection: a phase that writes nothing marks nothing changed.
struct MarchingShell<'a, 'w> {
    transform: &'a mut Mut<'w, Transform>,
    projectile: &'a mut Mut<'w, Projectile>,
    terminal_report: &'a mut Mut<'w, TerminalReport>,
    path: &'a mut Mut<'w, ShellPath>,
    marks: &'a mut Mut<'w, PenetrationMarks>,
    spall: &'a mut Mut<'w, SpallMarks>,
    dose: &'a mut Mut<'w, ShotDose>,
    /// The shell's network identity, when it has one: `None` where no shot clock runs (the
    /// sandboxes); single-player keys its rounds (`damage_cam::delivery::offline_plugin`).
    shot: Option<&'a Shot>,
    /// Firing source for launch clearance and hit attribution.
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
    /// The flight ended and the shell retires.
    stopped: bool,
    /// Pool units the authority actually removed this step, across every crossing and every spall
    /// fragment.
    damage_dealt: u64,
    /// The game's damage model's line effects delivered this tick, for fracture consumers.
    lines: Vec<DeliveredLine>,
}

/// The game's damage model: the authority's post-physics march and consequences.
#[expect(
    clippy::too_many_arguments,
    reason = "the march system: shells, march world, colliders, bodies, health, net state, two recorders and a cost sink"
)]
fn integrate_projectiles(
    mut projectiles: Query<(
        Entity,
        &mut Transform,
        &mut Projectile,
        &mut DamageReport,
        &mut TerminalReport,
        &mut ShellPath,
        &mut PenetrationMarks,
        &mut SpallMarks,
        &mut ShotDose,
        // A network identity is absent on tank-less sandbox fire.
        Option<&Shot>,
        // The tank that fired this shell, when it was fired BY one — `on_fire_shell` attaches it from
        // `FireShell::shooter` on every attributed shot. The authority reads it for initial source
        // clearance and hit confirmation attribution; a replica marches
        // nothing. `None` for the sandbox's tank-less camera fire, which excludes nothing.
        Option<&ShotSource>,
        // The pen cam recording of a recorded round (authority main-gun rounds only).
        Option<&mut ShotRecording>,
    )>,
    world: ProjectileMarchWorld,
    map: Res<crate::map::MapManifest>,
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
    // Optional fixed-tick cost sink (`SPIKE_COST_TRACE` or server stats), covering the whole march.
    // Unarmed runs pay only the Option check.
    mut cost: Option<ResMut<crate::cost::TickClock>>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let ProjectileMarchNet {
        replica,
        shot_clock,
    } = net;
    // No clock read unless the cost sink is armed.
    let march_t0 = cost.as_ref().map(|_| Instant::now());
    let dt = time.delta_secs();
    // Replicas present public paths; only authority constructs and executes a march tick.
    if replica.is_some() {
        return;
    }

    // The published authority tick stamps immutable paths. It is absent on tank-less fixtures.
    let present = shot_clock.map(|clock| clock.0.raw());
    // Publication tick; clockless fixtures use zero.
    let now = present.unwrap_or(0);

    // The game's damage model: shared HP, impulses and attribution commit by stable source,
    // weapon and fire tick, retaining each shot's internal order. Tank-less unkeyed instruments
    // follow keyed fire in local entity order; they have no network scoring identity.
    let mut ordered: Vec<_> = projectiles.iter_mut().collect();
    ordered.sort_unstable_by_key(|shell| match shell.9 {
        Some(Shot(id)) => (0, id.combatant.0, id.weapon, id.fire_tick, 0),
        None => (1, 0, 0, 0, shell.0.to_bits()),
    });

    for (
        entity,
        mut transform,
        mut projectile,
        mut damage_report,
        mut terminal_report,
        mut path,
        mut marks,
        mut spall,
        mut dose,
        shot,
        source,
        recording,
    ) in ordered
    {
        let mut shell = MarchingShell {
            transform: &mut transform,
            projectile: &mut projectile,
            terminal_report: &mut terminal_report,
            path: &mut path,
            marks: &mut marks,
            spall: &mut spall,
            dose: &mut dose,
            shot,
            source,
        };

        let mut tick = MarchTick {
            now,
            dt,
            commands: &mut commands,
        };
        let (marks_from, emptied_from) = (shell.marks.log.len(), shell.dose.emptied().len());
        let reached_from = shell.marks.reached.len();
        let (cones_from, burst_before) = (shell.spall.cones.len(), shell.projectile.fuze.burst());
        let shooter = shell.source.map(|source| source.tank);
        let owned_before = owned_losses(shell.dose, &world.owners, shooter);
        let step = march_shell_step(
            &mut shell,
            &world,
            &colliders,
            &mut health,
            &mut bodies,
            &mut tick,
        );
        recorder.note(
            entity,
            &mut shell.projectile.contacted,
            shot.map(|shot| shot.0),
            recording,
            shell.marks,
            (marks_from, reached_from),
            shell.dose,
            emptied_from,
            &world.owners,
            &health,
        );
        // A fuze holds its burst until it bursts, and only bursting leaves it inert.
        let burst = burst_before.filter(|_| shell.projectile.fuze.burst().is_none());
        if shell.marks.log.len() > marks_from
            || shell.spall.cones.len() > cones_from
            || burst.is_some()
            || !step.lines.is_empty()
        {
            commands.trigger(ShellMarched {
                shell: entity,
                marks_from,
                cones_from,
                lines: step.lines,
                burst,
            });
        }

        shell.transform.translation = step.position;
        shell.transform.look_to(step.direction, Vec3::Y);
        shell.path.points.push(step.position);

        // HP snapshots lose shot count: route a discrete fact reliably to the fire-time owner
        // through `net::shot_transport`. `owned_losses` keeps its marker aligned with tank displays.
        if step.damage_dealt > 0
            && !damage_report.0
            && let Some(shot) = shot
        {
            let amount = owned_losses(shell.dose, &world.owners, shooter) - owned_before;
            if amount > 0 {
                commands.trigger(ShellDamage {
                    shot: shot.0,
                    amount,
                });
                damage_report.0 = true;
            }
        }

        if step.stopped {
            if retain.0 {
                // Sandbox inspection: remove live flight but retain mesh, path and marks;
                // its `c` command clears them.
                commands.entity(entity).remove::<Projectile>();
            } else {
                commands.entity(entity).despawn();
            }
        } else if step.position.y < map.play_area.floor_m() {
            // Below the world floor: bound a shell that never hits terrain. No impact to inspect,
            // so even the sandbox despawns it.
            if let Some(shot) = shot {
                commands.trigger(FlightOutcome {
                    shot: shot.0,
                    sequence: shell.projectile.path_sequence,
                    tick: now.wrapping_add(1),
                    fraction: 0.0,
                    spans: Vec::new(),
                    carrier: Vec3::ZERO,
                    resume: step.position,
                    velocity: Vec3::ZERO,
                    stopped: true,
                    impact: None,
                    body: shell.projectile.body(),
                    drag_k: shell.projectile.flying.drag_k,
                });
            }
            commands.entity(entity).despawn();
        } else {
            shell.projectile.velocity = Vec3::from(step.direction) * step.speed;
        }
    }

    if let (Some(cost), Some(t0)) = (cost.as_mut(), march_t0) {
        cost.record_march(t0.elapsed().as_secs_f64() * 1.0e6);
    }
}

/// The game's damage model: one authority tick's publication stamp, duration and command sink,
/// shared by its shells.
struct MarchTick<'a, 'w, 's> {
    now: u32,
    dt: f32,
    commands: &'a mut Commands<'w, 's>,
}

/// AUTHORITY: the witness pass over `legs` (ADR-0040, [`wound::line`]), through the witnesses
/// `admit` keeps. Doses each witness the line put a chord into, logs one [`ArmourMark::Wound`] and
/// settles it at its own index; returns the pool units removed.
///
/// The hard world is already resolved when this runs and nothing here reaches back into it, so a
/// contradiction in the witness walk is a bug and panics in every build.
fn wound_legs(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    legs: &[resolve::Leg],
    speed: f32,
    health: &mut Query<&mut ComponentHealth>,
) -> u64 {
    let (volumes, incoming) = wound::line(context, admit, legs, shell.projectile.wound)
        .unwrap_or_else(|error| error.panic());
    let Some(incoming) = incoming else {
        return 0; // No witness chord lies on these resolved legs.
    };
    shell.dose.add_wounds(&volumes);
    shell.marks.log.push(ArmourMark::Wound {
        incoming: Vec3::from(incoming),
        speed,
        volumes,
    });
    let mark = shell.marks.log.len() - 1;
    shell.dose.settle(health, mark)
}

/// Ray-march one tick's travel budget for a single shell, resolving every surface it reaches.
///
/// Free flight until a surface, then resolve it — terrain stops the shell; a ballistic volume
/// ricochets (too oblique) or is crossed (charge resistance → spend potency → perforate or embed) — and keep
/// marching the leftover budget along the new direction. The game's damage model treats an
/// instant at rest as a state: preserve its heading, advance its fuze clock and let the next
/// gravity step move it. A non-finite flight state is a bug.
#[expect(
    clippy::too_many_lines,
    reason = "one ordered loop: open air, ground, an impact fuze, the crossing and its continuation \
              share the flight and the step's budget"
)]
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
    tick: &mut MarchTick,
) -> MarchStep {
    let dt = tick.dt;
    world.moment.set(Some(0.0));
    world.sweep.set(None);
    world.contact_motion.set(None);
    // Accumulate the authority's actual HP decrease across every direct crossing and spall fragment
    // this step. The caller's per-shell latch turns the aggregate into at most one discrete confirm.
    let mut damage_dealt = 0;
    let mut lines = Vec::new();
    // The game's damage model gives only the initial path inside the source's box clearance.
    // The latch survives later tick boundaries and bounces; source identity still attributes dose.
    let shooter = shell.source.map(|source| source.tank);
    // A cleared round never reads its source's box again, so it builds none.
    let source_box = shooter
        .filter(|_| !shell.projectile.source_cleared)
        .and_then(|source| world.source_box(source));
    // A burst in flight swaps the round for its nose piece mid-step ([`burst_here`]); the disc keeps
    // its radius, the nose being the round's calibre.
    let radius = resolve::disc_radius(shell.projectile.body().caliber);
    // Clearance admits the source on the very starts the step resumes from ([`launch`]).
    let laid = launch_starts(shell.projectile, shell.transform.translation, radius);
    let ignored_source = std::cell::Cell::new(shooter.filter(|_| {
        !shell.projectile.source_cleared
            && source_box.is_some_and(|bounds| {
                // The game's damage model: playback has no discovery disc. Its captured
                // admission remains in force until it returns certified sample starts.
                !shell.projectile.playback.is_empty() || bounds.holds_any(&laid, 0.0)
            })
    }));
    let resting = std::cell::RefCell::new(Vec::new());
    let not_own = |entity: Entity| {
        !resting.borrow().contains(&entity)
            && ignored_source.get().is_none_or(|source| {
                hit_ancestor(entity, &world.owners, &world.parents)
                    .is_none_or(|(_, owner)| owner.tank() != source)
            })
    };
    // Gravity and drag on the velocity through the free-flight kernel the FireEvent catch-up folds
    // ([`fast_forward_shell`]). An unbent open-air step flies exactly `stepped × dt`. The march may
    // *bend* the direction (ricochet), so we carry direction + speed and rebuild the
    // velocity at the end rather than assuming a straight step.
    assert!(
        shell.projectile.velocity.is_finite(),
        "the game's damage model carries finite flight velocity between steps"
    );
    let previous = if shell.projectile.velocity == Vec3::ZERO {
        Dir3::new(shell.projectile.disc.u.cross(shell.projectile.disc.v))
            .expect("the game's damage model retains a nondegenerate sampling frame at world rest")
    } else {
        Dir3::new(shell.projectile.velocity)
            .expect("the game's damage model carries a representable flight direction")
    };
    let mut playback = std::mem::take(&mut shell.projectile.playback);
    let stepped = if playback.is_empty() {
        freeflight_step(
            shell.projectile.velocity,
            shell.projectile.flying.drag_k,
            dt,
        )
    } else {
        shell.projectile.velocity
    };
    let speed = stepped.length();
    assert!(
        speed.is_finite(),
        "the game's damage model integrates a finite flight speed from validated state"
    );
    // At the apex the integrator can return exactly zero: no displacement this step, with the
    // incoming heading retained until gravity supplies a new direction.
    let dir = playback.front().map_or_else(
        || Dir3::new(stepped).unwrap_or(previous),
        |playing| playing.flight().direction,
    );
    if playback.is_empty() {
        shell.projectile.fuze.at_speed(speed);
    }
    let mut budget = world.step_budget(f64::from(dt));
    let mut stopped = false;
    // Whether the march has bent the shell off its original free-flight segment. Until it does,
    // an open-air fly-out moves the axis by exactly `stepped × dt` (the catch-up kernel's step);
    // after a bend the leftover budget flies along the new direction instead.
    let mut bent = !playback.is_empty();
    // AUTHORITY: whether this shell's ONE terminal (`ShellTerminal` — embed/perforation) has been
    // emitted. A perforated shell can keep marching through the interior across fixed ticks, so
    // this spawn-time latch supplies both the same-tick and cross-tick halves of the invariant
    // without destroying `Shot`, which remains the damage-attribution identity. It also mutes
    // post-terminal `ShellRicochet`s: the client's cosmetic shell ended at the terminal, so an
    // interior bounce after it must not ride the wire.
    let mut terminal_emitted = shell.terminal_report.0;

    // The disc's sampling basis, carried through this tick's gravity bend so the ring never re-rolls
    // (§13.5: the frame is spawn-anchored and transported, never rebuilt from the current direction).
    // Transported from the direction the frame was LAST left on — the pre-step velocity, which is
    // also the direction the last step's sweep ran along — not from the shell's visual forward:
    // `looking_to` is degenerate for a round travelling straight down, and the sampling basis is sim
    // state that must not read a view detail.
    let context = resolve::ResolveContext {
        world,
        colliders,
        laws: walk::WalkLaws::default(),
    };
    // The game's damage model discovers only outside a pending crossing. A carried result
    // supplies its own world path and speed until playback reaches its end.
    let mut flight = match playback.front() {
        Some(playing) => playing.flight(),
        None => launch(shell, &context, &not_own, previous, dir, speed, radius),
    };

    loop {
        if playback.is_empty()
            && ignored_source.get().is_some()
            && source_box.is_some_and(|bounds| {
                !bounds.holds_any(
                    &flight.sample_starts(radius),
                    (1.0 - budget.remaining() as f32 / dt).clamp(0.0, 1.0),
                )
            })
        {
            ignored_source.set(None);
        }
        if let Some(mut playing) = playback.pop_front() {
            let (spent, complete, removed) = playing.advance(
                budget.remaining(),
                shell,
                &mut terminal_emitted,
                health,
                bodies,
                tick.commands,
                &mut lines,
            );
            damage_dealt += removed;
            budget.fly(spent);
            flight = playing.flight();
            resting.borrow_mut().clear();
            bent = true;
            if !complete {
                playback.push_front(playing);
                break;
            }
            let before_round = shell.projectile.round;
            let (ended, removed) = follow_crossing(
                shell,
                &context,
                &mut flight,
                playing.into_decision(),
                health,
            );
            damage_dealt += removed;
            if shell.projectile.round != before_round {
                publish_path_state(shell, tick, &budget, &flight, ended, None);
            }
            if ended {
                stopped = true;
                playback.clear();
                break;
            }
            continue;
        }
        let from = (1.0 - budget.remaining() as f32 / dt).clamp(0.0, 1.0);
        world.moment.set(Some(from));
        // The game's damage model reads unread starts (a skid's footprint, a newly placed
        // disc) before the step may end: material holding one is resolved this tick, and the
        // starts carried across the boundary are read ones.
        flight
            .settle(&context, &not_own, radius)
            .unwrap_or_else(|error| error.panic());
        if budget.remaining() <= 0.0 && !flight.holds_material() {
            break;
        }
        let origin = flight.sweep_origin();
        let dir = flight.direction;
        let remaining = budget.remaining() as f32;
        let exit = ignored_source.get().and_then(|_| {
            source_box
                .expect("source clearance carries its box")
                .clearance_seconds(
                    &flight.sample_starts(radius),
                    dir * flight.speed,
                    from,
                    remaining,
                )
        });
        let scan_seconds = exit.unwrap_or(remaining);
        let leaving_source = scan_seconds < remaining;
        let within = scan_seconds * flight.speed;
        let to = if leaving_source {
            from + scan_seconds / dt
        } else {
            1.0
        };
        world.sweep.set(Some(motion::Sweep { from, to, dt }));
        world.contact_motion.set(None);
        let reach = within;
        let ahead = match next_surface(world, &flight, radius, reach, &not_own) {
            Ahead::Armor(hit) if hit.distance > within => Ahead::Open,
            Ahead::Terrain { distance, .. } if distance > within => Ahead::Open,
            ahead => ahead,
        };
        // A stationary round can wait until a moving body's contact or the tick boundary. Its
        // fuze still consumes those seconds; distance alone would freeze an armed count here.
        if flight.speed == 0.0 {
            let wait = world
                .contact_motion
                .get()
                .map_or(scan_seconds, |moving| (moving.fraction - from) * dt);
            if shell.projectile.fuze.wait_at_rest(wait) {
                let after = shell.projectile.fuze.delay_left().expect(
                    "only an armed fuze expires while the game's damage model waits at rest",
                );
                budget.fly(f64::from(after));
                damage_dealt += burst_here(shell, &context, &flight, 0.0, health).1;
                publish_path_state(shell, tick, &budget, &flight, true, None);
                stopped = true;
                break;
            }
        }
        // The game's damage model spends only the remaining seconds in open air. A discovery
        // interval bounds the scan and physical displacement. An unbent segment
        // retains the catch-up kernel's exact displacement.
        let displacement = if bent || leaving_source {
            dir * within
        } else {
            stepped * dt
        };
        // An armed round whose delay runs out short of what lies ahead bursts in the open.
        let open = ahead.open_metres(displacement.length());
        if let Some(at) = open.and_then(|open| shell.projectile.fuze.due_within(open)) {
            let (ended, removed) = burst_ahead(
                shell,
                &context,
                &not_own,
                &mut flight,
                &mut budget,
                at,
                None,
                health,
            );
            damage_dealt += removed;
            publish_path_state(shell, tick, &budget, &flight, ended, None);
            if ended {
                stopped = true;
                break;
            }
            bent = true;
            continue;
        }
        let hit = match ahead {
            Ahead::Open => {
                shell.projectile.fuze.flown(displacement.length());
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
                if leaving_source {
                    budget.fly(f64::from(scan_seconds));
                    let fraction = (1.0 - budget.remaining() as f32 / dt).clamp(0.0, 1.0);
                    // Every start has left the box, so no membership the sweep read with the
                    // source transparent can miss a source volume: the starts carry on as swept.
                    if source_box.is_some_and(|bounds| {
                        !bounds.holds_any(&flight.sample_starts(radius), fraction)
                    }) {
                        ignored_source.set(None);
                    }
                    bent = true;
                    continue;
                }
                budget.fly(budget.remaining());
                break;
            }
            Ahead::Terrain { distance, normal } => {
                let entry = origin + dir * distance;
                let leg = resolve::Leg::new(origin, dir, distance, &flight.sample_offsets(radius));
                damage_dealt += wound_legs(shell, &context, &not_own, &[leg], speed, health);
                // AUTHORITY: the ground stops the round dead, with no chord to resist it over, so
                // its resisting force is DERIVED unbounded: it functions every impact fuze.
                shell
                    .projectile
                    .fuze
                    .entered(f64::INFINITY, speed, None, distance);
                strike_ground(shell, entry, normal, tick);
                budget.fly(f64::from(distance / flight.speed));
                stop_at(shell, &mut flight, entry, distance);
                publish_path_state(
                    shell,
                    tick,
                    &budget,
                    &flight,
                    true,
                    Some(path::PathImpact::Ground { at: entry, normal }),
                );
                stopped = true;
                break;
            }
            Ahead::Armor(hit) => Some(hit),
            // A held continuation with no surface in reach: its material's exit is still owed, and
            // the continuation alone knows where it starts.
            Ahead::Held => None,
        };
        let mut contact = hit.map_or(0.0, |hit| hit.distance);
        let moving = world.contact_motion.take();
        world.sweep.set(None);
        let mut carrier = Vec3::ZERO;
        if let Some(moving) = moving {
            world.moment.set(Some(moving.fraction));
            // The game's damage model clocks contact by fraction even when world speed is zero:
            // a moving body can still enter a stationary round's position.
            let seconds = (moving.fraction - from) * dt;
            // No relative traversal: there is no frame to decide a crossing in.
            let opened = (flight.direction * flight.speed != moving.velocity).then(|| {
                resolve::at_first_touch(
                    &context,
                    &not_own,
                    &shell.projectile.round,
                    shell.projectile.contact,
                    &flight,
                    moving,
                    seconds,
                    dt,
                )
                .unwrap_or_else(|error| error.panic())
            });
            // Seconds of the way to first touch the opened flight still has to fly, in its frame.
            let short = opened
                .as_ref()
                .map_or(0.0, |(opened, stand_off)| stand_off / opened.speed);
            let flown = (contact - flight.speed * short).max(0.0);
            let leg = resolve::Leg::new(origin, dir, flown, &flight.sample_offsets(radius));
            damage_dealt += wound_legs(shell, &context, &not_own, &[leg], flight.speed, health);
            budget.fly(f64::from((seconds - short).max(0.0)));
            shell.projectile.fuze.flown(flown);
            let Some((opened, stand_off)) = opened else {
                // The game's damage model: other bodies can still meet the world path during the
                // remaining seconds. Rediscover without this resting material until another
                // crossing changes the velocity; the next tick reads all membership again.
                resting.borrow_mut().push(
                    hit.expect("relative-rest contact was discovered on an addressed collider")
                        .entity,
                );
                flight.position = moving.point;
                flight.continuation = None;
                bent = true;
                continue;
            };
            carrier = moving.velocity;
            flight = opened;
            // The crossing is decided in the struck body's frame, where the round flies at its
            // relative speed: an armed `Impact` count keeps its seconds there too.
            shell.projectile.fuze.at_speed(flight.speed);
            contact = stand_off;
            bent = true;
        }
        if let Some((ended, removed)) = function_on_contact(
            shell,
            &context,
            &not_own,
            &mut flight,
            &mut budget,
            contact,
            moving,
            health,
        ) {
            damage_dealt += removed;
            if ended {
                publish_path_state(shell, tick, &budget, &flight, true, None);
                stopped = true;
                break;
            }
            let velocity = flight.direction * flight.speed + carrier;
            if let Ok(direction) = Dir3::new(velocity) {
                flight.frame = flight.frame.transport(*flight.direction, *direction);
                flight.direction = direction;
            } // At world rest the heading is retained.
            flight.speed = velocity.length();
            publish_path_state(shell, tick, &budget, &flight, false, None);
            bent = true;
            continue;
        }

        let cleared;
        (playback, cleared) = decide_playback(
            shell,
            &context,
            &not_own,
            &flight,
            contact,
            hit.map(|hit| hit.entity),
            source_box.filter(|_| ignored_source.get().is_some()),
            health,
            &mut budget,
        );
        // The decided chain carries the admission it was planned under; playback reads none.
        if cleared {
            ignored_source.set(None);
        }
        let mut start = 1.0 - budget.remaining() / f64::from(dt);
        let mut ordinal = shell.marks.ricochets.len() as u32;
        for playing in &mut playback {
            playing.in_moving_frame(carrier);
            if let Some(shot) = shell.shot {
                let whole = start.floor();
                let outcome = playing.public_path(
                    shot.0,
                    shell.projectile.path_sequence,
                    tick.now.wrapping_add(whole as u32),
                    start - whole,
                    ordinal,
                );
                if matches!(outcome.impact, Some(path::PathImpact::Bounce { .. })) {
                    ordinal += 1;
                }
                tick.commands.trigger(outcome);
                shell.projectile.path_sequence += 1;
            }
            start += playing.seconds() / f64::from(dt);
        }
    }
    // An armed round that stopped short of its delay bursts where it lies, at rest, when the
    // delay's remainder runs out (its mark's `after`); its field settles this tick.
    if stopped {
        damage_dealt += burst_here(shell, &context, &flight, 0.0, health).1;
    }
    // The game's damage model carries an immutable playback cursor across ticks, or its clear
    // reached sample starts once its discovery chain has finished.
    shell.projectile.source_cleared = ignored_source.get().is_none()
        || (playback.is_empty()
            && source_box.is_none_or(|bounds| {
                !bounds.holds_any(
                    &flight.sample_starts(radius),
                    (1.0 - budget.remaining() as f32 / dt).clamp(0.0, 1.0),
                )
            }));
    shell.projectile.disc = flight.frame;
    shell.projectile.starts = flight.continuation.clone();
    shell.projectile.playback = playback;

    world.moment.set(None);
    world.sweep.set(None);
    MarchStep {
        position: flight.position,
        direction: flight.direction,
        speed: flight.speed,
        stopped,
        damage_dealt,
        lines,
    }
}

/// The game's damage model: publish a discrete state change at the seconds budget's cursor.
fn publish_path_state(
    shell: &mut MarchingShell,
    tick: &mut MarchTick,
    budget: &resolve::Budget,
    flight: &resolve::Flight,
    stopped: bool,
    impact: Option<path::PathImpact>,
) {
    let Some(shot) = shell.shot else { return }; // Tank-less sandbox shots have no network audience.
    let at = 1.0 - budget.remaining() / f64::from(tick.dt);
    let whole = at.floor();
    tick.commands.trigger(FlightOutcome {
        shot: shot.0,
        sequence: shell.projectile.path_sequence,
        tick: tick.now.wrapping_add(whole as u32),
        fraction: at - whole,
        spans: Vec::new(),
        carrier: Vec3::ZERO,
        resume: flight.position,
        velocity: flight.direction * flight.speed,
        stopped,
        impact,
        body: shell.projectile.body(),
        drag_k: shell.projectile.flying.drag_k,
    });
    shell.projectile.path_sequence += 1;
}

/// The game's damage model: the WORLD sample starts [`launch`] resumes the step from: the
/// carried starts as the last step left them, or the disc a newly placed round lays on its
/// carried frame at `position`. Launch clearance admits the source on exactly these.
fn launch_starts(projectile: &Projectile, position: Vec3, radius: f32) -> Vec<Vec3> {
    match &projectile.starts {
        Some(starts) => starts
            .seeds
            .iter()
            .map(|seed| starts.world_origin() + seed.offset)
            .collect(),
        None => resolve::layout(&projectile.disc, radius)
            .into_iter()
            .map(|offset| position + offset)
            .collect(),
    }
}

/// The game's damage model: the step's discovery samples. Each start a sweep reached is
/// certified in the previous scene. Geometry born or moved over it contacts immediately in the
/// new scene and seeds the same material walk. One no sweep has reached is
/// placed, not moved, into the scene: a round's birth at the muzzle or a catch-up's end.
/// Its membership is read where it stands ([`resolve::Flight::born`]),
/// so a muzzle inside a foreign body starts the round inside that body's material (MEASURED in a
/// 30-bot run: tanks pressed together, one gun's muzzle in the other's turret side), and the walk
/// carries it out like any held start.
fn launch(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    previous: Dir3,
    dir: Dir3,
    speed: f32,
    radius: f32,
) -> resolve::Flight {
    let position = shell.transform.translation;
    let frame = shell.projectile.disc;
    match shell.projectile.starts.take() {
        Some(starts) => resolve::Flight::launched(previous, dir, speed, frame, starts),
        None => resolve::Flight::born(
            context, not_own, position, previous, dir, speed, frame, radius,
        )
        .unwrap_or_else(|error| error.panic()),
    }
}

/// The game's damage model freezes each discovered crossing before playback starts. Held
/// samples finish in that scene. Discovery under launch clearance ends at the decision after which
/// the source is solid, and says so: every start is outside the source box with none held, or
/// a skid strikes outside it. A continuing skid returns to relative discovery after playback
/// spends its flight time; a held contact ends the round. No tick re-collects a pending crossing.
#[expect(
    clippy::too_many_arguments,
    reason = "the immutable planner also closes the struck physical body's actual geometry"
)]
fn decide_playback(
    shell: &MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    flight: &resolve::Flight,
    contact: f32,
    struck: Option<Entity>,
    clearance: Option<SourceBox>,
    health: &Query<&mut ComponentHealth>,
    budget: &mut resolve::Budget,
) -> (std::collections::VecDeque<resolve::Playback>, bool) {
    let mut out = std::collections::VecDeque::new();
    let mut cleared = false;
    let mut flight = flight.clone();
    let mut fuze = shell.projectile.fuze;
    let mut flying = shell.projectile.flying;
    let radius = resolve::disc_radius(shell.projectile.body().caliber);
    let mut contact = Some(contact);
    // The game's damage model freezes geometry at contact. Discovery using launch clearance
    // ends at this box's exit; playback then returns to the live march with the source solid.
    let clearance = clearance.map(|bounds| {
        let pose = bounds.body.at(context
            .world
            .moment
            .get()
            .expect("the authority plans a crossing in its captured contact scene"));
        SourceBox {
            body: motion::Poses {
                from: pose,
                to: pose,
            },
            ..bounds
        }
    });
    loop {
        flight
            .settle(context, admit, radius)
            .unwrap_or_else(|error| error.panic());
        let mut barrier = None;
        let mut entered = None;
        let decision = if let Some(distance) = contact.take() {
            if let burst::Fuze::Unarmed(burst::Burst {
                trigger: crate::rounds::fuze::Trigger::Impact { .. },
                ..
            }) = fuze
                && let Some((force, owner)) = resolve::entry_force(
                    context,
                    admit,
                    &shell.projectile.round,
                    shell.projectile.contact,
                    &flight,
                    distance,
                )
                .unwrap_or_else(|error| error.panic())
            {
                fuze.entered(force, flight.speed, owner, distance);
                entered = Some((force, owner, flight.speed, distance));
            }
            if let Some(at) = fuze.due_within(distance) {
                resolve::clear_path(context, admit, &shell.projectile.round, &flight, at)
                    .unwrap_or_else(|error| error.panic())
                    .decision
            } else {
                let decided = resolve::decide(
                    context,
                    admit,
                    &shell.projectile.round,
                    flying.mass,
                    shell.projectile.contact,
                    &flight,
                    distance,
                )
                .unwrap_or_else(|error| error.panic());
                #[cfg(test)]
                contact_classes::note(
                    context,
                    admit,
                    &shell.projectile.round,
                    shell.projectile.contact,
                    &flight,
                    distance,
                    &decided,
                );
                decided
            }
        } else {
            let reach = if flight.holds_material() {
                resolve::FIELD_REACH
            } else if let Some(struck) = struck {
                resolve::body_completion(context, struck, admit, &flight, radius)
                    .unwrap_or_else(|error| error.panic())
            } else {
                break;
            };
            if reach == 0.0 {
                break;
            }
            let reach = clearance
                .and_then(|bounds| {
                    bounds.clearance_seconds(
                        &flight.sample_starts(radius),
                        *flight.direction,
                        0.0,
                        reach,
                    )
                })
                .unwrap_or(reach);
            match next_surface(context.world, &flight, radius, reach, admit) {
                Ahead::Armor(hit) => {
                    contact = Some(hit.distance);
                    continue;
                }
                Ahead::Held => {
                    contact = Some(0.0);
                    continue;
                }
                ahead => {
                    let distance = ahead
                        .open_metres(reach)
                        .expect("an unheld discovery result has an open approach");
                    let distance = if let Some(at) = fuze.due_within(distance) {
                        at
                    } else {
                        distance
                    };
                    if !matches!(ahead, Ahead::Open) {
                        barrier = Some(distance);
                    }
                    let span = resolve::clear_path(
                        context,
                        admit,
                        &shell.projectile.round,
                        &flight,
                        distance,
                    )
                    .unwrap_or_else(|error| error.panic());
                    if let Some(distance) = span.contact {
                        contact = Some(distance);
                        continue;
                    }
                    span.decision
                }
            }
        };
        budget.charge().unwrap_or_else(|error| error.panic());
        let mut playing = resolve::Playback::new(
            decision.clone(),
            &flight,
            shell.projectile.round,
            flying,
            context,
            admit,
            shell.projectile.wound,
            &|volume| health.contains(volume),
        );
        if let Some((force, owner, speed, approach)) = entered {
            playing.on_entry(force, owner, speed, approach);
        }
        flying = playing.outgoing_body();
        out.push_back(playing);
        let reached_barrier = barrier.is_some_and(|distance| {
            matches!(&decision,
            resolve::Decision::Clear { travel, .. } if *travel >= distance)
        });
        // A skid off a face outside the box ends clearance: the axis turns a body radius short of
        // that face, so where the axis turned can still stand inside the box.
        let bounced_out = clearance.is_some_and(|bounds| match &decision {
            resolve::Decision::Bounce(bounce) => !bounds.holds_any(&[bounce.position], 0.0),
            _ => false,
        });
        // The game's damage model: a skid finishes this immutable contact. Playback
        // spends its approach before discovery can reach another face.
        let bounced = matches!(decision, resolve::Decision::Bounce(_));
        let stopped = follow_fuzed(&mut fuze, &mut flight, decision);
        cleared = bounced_out
            || clearance.is_some_and(|bounds| {
                !flight.holds_material() && !bounds.holds_any(&flight.sample_starts(radius), 0.0)
            });
        if bounced || stopped || reached_barrier || cleared || fuze.due_within(0.0).is_some() {
            break;
        }
    }
    (out, cleared)
}

/// AUTHORITY: an unarmed `Impact` fuze reads the body the flight meets `contact` metres ahead
/// ([`resolve::entry_force`]) and functions on it ([`burst::Fuze::entered`]). A delay that runs out
/// by the contact bursts there, before the crossing ([`burst_ahead`]). Returns, when it burst,
/// whether the round ended and the pool units removed; `None` leaves the crossing to the walk.
#[expect(
    clippy::too_many_arguments,
    reason = "the reached contact supplies its frame and clock to burst delivery"
)]
fn function_on_contact(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    flight: &mut resolve::Flight,
    budget: &mut resolve::Budget,
    contact: f32,
    moving: Option<motion::ContactMotion>,
    health: &mut Query<&mut ComponentHealth>,
) -> Option<(bool, u64)> {
    let burst::Fuze::Unarmed(burst::Burst {
        trigger: crate::rounds::fuze::Trigger::Impact { .. },
        ..
    }) = shell.projectile.fuze
    else {
        return None;
    };
    // The game's damage model reads the same certified entrance as the crossing driver.
    let (force, owner) = resolve::entry_force(
        context,
        not_own,
        &shell.projectile.round,
        shell.projectile.contact,
        flight,
        contact,
    )
    .unwrap_or_else(|error| error.panic())?;
    shell
        .projectile
        .fuze
        .entered(force, flight.speed, owner, contact);
    shell.projectile.fuze.due_within(contact)?;
    Some(burst_ahead(
        shell, context, not_own, flight, budget, contact, moving, health,
    ))
}

/// AUTHORITY: advance the fuze over a decided crossing (design doc §5a, "The trigger"). The count
/// runs over every metre of the crossing's path. A corridor found clear is open path. A bounce
/// flies its way in, then meets a body: an armed fuze restarts from it. A bite flies from the
/// flight's position to each body exit it reached in turn ([`resolve::Bite::path_to`]), which arms
/// the fuze at the threshold or restarts an armed one, then on to where the flight resumes or the
/// round came to rest. A delay that runs out on the way is restarted by nothing after it
/// ([`burst::Fuze::exited`]): it is due where the crossing hands the flight back.
fn advance_fuze(fuze: &mut burst::Fuze, decision: &resolve::Decision) {
    match decision {
        resolve::Decision::Clear { travel, .. } => fuze.flown(*travel),
        resolve::Decision::Bounce(bounce) => {
            fuze.flown(bounce.travel);
            fuze.bounced(bounce.speed, bounce.owner);
        }
        resolve::Decision::Bite(bite) => {
            let mut flown = 0.0;
            for source in &bite.spall {
                let at = bite.path_to(source.position).max(flown);
                fuze.flown(at - flown);
                flown = at;
                fuze.exited(source.plate_met, source.exit_speed, bite.owner);
            }
            fuze.flown((bite.travel - flown).max(0.0));
        }
    }
}

/// The round stops at `entry`, `approach` metres of path from the flight's sweep origin, with no
/// crossing resolved: the fuze flies them first, so an armed round waits out only what is left of
/// its delay where it stops ([`burst_here`]).
fn stop_at(shell: &mut MarchingShell, flight: &mut resolve::Flight, entry: Vec3, approach: f32) {
    shell.projectile.fuze.flown(approach);
    flight.position = entry;
}

/// AUTHORITY: the one step that carries a fuze and a flight over a decided crossing, shared by
/// playback ([`follow_crossing`]) and discovery ([`decide_playback`]): advance the fuze along the
/// crossing's path ([`advance_fuze`]), follow it, and rescale an armed `Impact` count to the speed
/// the crossing hands back ([`burst::Fuze::at_speed`]). Discovery relies on the rescale to end its
/// immutable chain where playback's fuze runs out: without it a round slowed hard inside a body
/// keeps its entry-speed metres, and the next frozen span carries the burst past its point.
/// Returns whether the round came to rest.
fn follow_fuzed(
    fuze: &mut burst::Fuze,
    flight: &mut resolve::Flight,
    decision: resolve::Decision,
) -> bool {
    advance_fuze(fuze, &decision);
    if flight.follow(decision) {
        return true;
    }
    fuze.at_speed(flight.speed);
    false
}

/// AUTHORITY: advance the fuze ([`advance_fuze`]) and the flight over a decided crossing. A delay
/// that ran out inside the crossing bursts where the crossing hands the flight back, at the speed it
/// hands back, whatever is left of the step's budget ([`burst_here`]). Returns whether the round
/// ended, at rest or in the burst, and the pool units the burst removed.
fn follow_crossing(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    flight: &mut resolve::Flight,
    decision: resolve::Decision,
    health: &mut Query<&mut ComponentHealth>,
) -> (bool, u64) {
    if follow_fuzed(&mut shell.projectile.fuze, flight, decision) {
        return (true, 0);
    }
    if shell.projectile.fuze.due_within(0.0).is_some() {
        return burst_here(shell, context, flight, flight.speed, health);
    }
    (false, 0)
}

/// AUTHORITY: fly the round `metres` of open path, where its fuze runs out, with the witness pass
/// over that leg, and burst there at the flight's speed ([`burst_here`]). Returns whether the round
/// ended, and the pool units removed.
#[expect(
    clippy::too_many_arguments,
    reason = "a contact burst carries the struck frame to its world endpoint"
)]
fn burst_ahead(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    flight: &mut resolve::Flight,
    budget: &mut resolve::Budget,
    metres: f32,
    moving: Option<motion::ContactMotion>,
    health: &mut Query<&mut ComponentHealth>,
) -> (bool, u64) {
    let radius = resolve::disc_radius(shell.projectile.body().caliber);
    let leg = resolve::Leg::new(
        flight.sweep_origin(),
        flight.direction,
        metres,
        &flight.sample_offsets(radius),
    );
    let wounded = wound_legs(shell, context, admit, &[leg], flight.speed, health);
    flight.fly(flight.direction * metres);
    let seconds = metres / flight.speed;
    if let Some(moving) = moving {
        // The game's damage model: the burst belongs to the reached world contact. Carrier
        // displacement is bounded by |velocity| times the remaining tick, not by a fixed length.
        flight.fly(moving.velocity * seconds);
        context.world.moment.set(Some(moving.fraction));
    }
    budget.fly(f64::from(seconds));
    let (ended, removed) = burst_here(shell, context, flight, flight.speed, health);
    (ended, wounded + removed)
}

/// AUTHORITY: burst an armed round where `flight` stands, travelling at `speed` (zero at rest, the
/// delay's remainder then waited out where it lies): log its [`ArmourMark::Burst`] and settle its
/// doses at that mark. Its nose piece flies on as the shell's round, inert, unless the round is at
/// rest, which ends it. Returns whether the round ended and the pool units removed; an unarmed
/// fuze does nothing.
fn burst_here(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    flight: &resolve::Flight,
    speed: f32,
    health: &mut Query<&mut ComponentHealth>,
) -> (bool, u64) {
    let Some((burst, tank)) = shell.projectile.fuze.armed() else {
        return (false, 0);
    };
    let after = if speed > 0.0 {
        0.0
    } else {
        shell.projectile.fuze.delay_left().unwrap_or(0.0)
    };
    shell.projectile.fuze = burst::Fuze::Inert;
    let nose = burst.nose;
    shell.marks.log.push(ArmourMark::Burst {
        at: flight.position,
        field_origin: burst.filler_center(flight.position, flight.direction),
        speed,
        after,
        sideways: burst.sideways_speed,
        nose: nose.mass,
        tank,
    });
    shell.path.points.push(flight.position);
    throw_burst(shell, context, &burst, flight, speed, &|volume| {
        health.contains(volume)
    });
    let removed = shell.dose.settle(health, shell.marks.log.len() - 1);
    let ended = if speed > 0.0 {
        let projectile = &mut **shell.projectile;
        projectile.round = bound(nose);
        projectile.flying = projectile.flying.with_body(nose);
        false
    } else {
        true
    };
    (ended, removed)
}

/// The ground stops the authority's shell at `entry`, publishing the struck terrain impact.
/// Client lifecycle recording belongs to public-path presentation. The caller stops the flight.
fn strike_ground(shell: &MarchingShell, entry: Vec3, normal: Vec3, tick: &mut MarchTick) {
    tick.commands.trigger(Impact {
        position: entry,
        normal,
        style: shell.projectile.impact_style(),
        surface: ImpactSurface::Terrain,
        outcome: ImpactOutcome::Struck,
    });
}

/// What lies ahead of a flight, for the next crossing.
enum Ahead {
    Open,
    Terrain {
        distance: f32,
        normal: Vec3,
    },
    Armor(avian3d::prelude::RayHitData),
    /// The game's damage model: held material belongs to the frozen crossing, never discovery.
    Held,
}

impl Ahead {
    /// Metres of open path the flight has before what lies ahead: `open`, its open-air leg, when
    /// nothing does; the distance to a surface. None for a held continuation, inside material.
    fn open_metres(&self, open: f32) -> Option<f32> {
        match self {
            Ahead::Open => Some(open),
            Ahead::Terrain { distance, .. } => Some(*distance),
            Ahead::Armor(hit) => Some(hit.distance),
            Ahead::Held => None,
        }
    }
}

/// The next surface a flight meets within `budget` metres — shared by the runtime march and the
/// §13.6 gate, so both cut successive crossings at the same places.
///
/// Cast from every sample's own start ([`resolve::Flight::sample_offsets`]), not from a disc
/// re-laid at the axis: a sample's start is the only point its membership is known at.
///
/// The game's damage model closes a held sample's material while deciding the immutable
/// crossing. Playback then carries its world path across ticks without rediscovering that exit.
fn next_surface(
    world: &ProjectileMarchWorld,
    flight: &resolve::Flight,
    radius: f32,
    budget: f32,
    not_own: &dyn Fn(Entity) -> bool,
) -> Ahead {
    if flight.holds_material() {
        return Ahead::Held;
    }
    match cast_disc_segment(
        world,
        &flight.sample_offsets(radius),
        flight.sweep_origin(),
        flight.direction,
        budget,
        not_own,
    ) {
        Some(SegmentHit::Armor(hit)) => Ahead::Armor(hit),
        Some(SegmentHit::Terrain { distance, normal }) => Ahead::Terrain { distance, normal },
        None => Ahead::Open,
    }
}

/// AUTHORITY: throw an armed round's burst field (design doc §5a, "The burst") from where `flight`
/// stands, about its direction, at the carried `speed`, recording it as the shell's
/// [`SpallMarks::burst`] and adding its doses to the shot's [`ShotDose`]. Its strength `S` (J)
/// carries the wall's driven energy ([`spall_cone::field_strength`]), its doses converting at the
/// burst's unit ([`spall_cone::BurstFields::unit`]): nothing the round lost reaching `flight`
/// enters it. Its field is the ring at ratio `speed / v_s`
/// ([`spall_cone::BurstFields::field`]), walked through every spatially reached volume, leaving
/// none. Every nonzero field contributes to the shot tally before pool rounding. The game's
/// damage model requires a derived field to stay inside the
/// law's domain: the round and law are validated before play, so a contradiction is a bug.
fn throw_burst(
    shell: &mut MarchingShell,
    context: &resolve::ResolveContext<'_, '_, '_>,
    burst: &burst::Burst,
    flight: &resolve::Flight,
    speed: f32,
    is_box: &dyn Fn(Entity) -> bool,
) {
    let fields = spall_cone::BurstFields::shipped();
    #[cfg(test)]
    let fields = context
        .world
        .burst_law
        .as_deref()
        .map_or(fields, |law| &law.0);
    let throw = FieldThrow {
        origin: flight.position,
        axis: flight.direction,
        strength: spall_cone::field_strength(burst.energy),
        unit: fields.unit(),
        exited: &[],
    };
    assert!(
        throw.strength.is_finite() && throw.strength >= 0.0,
        "the game's damage model derives finite nonnegative burst strength from validated energy",
    );
    if throw.strength == 0.0 {
        return; // A field carrying no energy has nothing for a pool or a construction.
    }
    let (field, law) = fields.field(speed / burst.sideways_speed);
    let spread = law.spread(field).expect(
        "the game's damage model derives a burst field from a validated round, finite speed and \
         configured field law; it must remain inside that law's domain",
    );
    let rays = walk_field(&throw, &spread, shell.dose, context, is_box);
    shell.spall.burst = Some(BurstField {
        origin: throw.origin,
        axis: throw.axis,
        field,
        strength: throw.strength,
        rays,
    });
}

/// The game's damage model: one field's origin, axis, strength (J), its mechanism's unit (the HP
/// its doses are worth per joule), and the volumes it leaves (a body exit's), which a direction
/// starting inside one does not enter while leaving. Re-entry can dose it again
/// ([`resolve::field_direction`]).
struct FieldThrow<'a> {
    origin: Vec3,
    axis: Dir3,
    strength: f32,
    unit: spall_cone::FieldUnit,
    exited: &'a [Entity],
}

/// Walk every direction of `spread` about `throw`'s axis from its origin, direction `k` at
/// strength `S·f_k`, standing for `1/N` of the field, through every spatially reached volume
/// ([`resolve::field_direction`]), adding its doses to `dose`; returns the rays in stratum order.
/// Every contribution reaches the shot tally before its accumulated total rounds to pool units.
/// Ownership is attribution only: neighboring tanks and unowned environment matter resist and
/// receive the same field. Material presence and occlusion decide where each direction stops.
/// The game's damage model requires every direction and witness pass to pair; a contradiction
/// panics in every build.
fn walk_field(
    throw: &FieldThrow<'_>,
    spread: &spall_cone::Spread,
    dose: &mut ShotDose,
    context: &resolve::ResolveContext<'_, '_, '_>,
    is_box: &dyn Fn(Entity) -> bool,
) -> Vec<SpallRay> {
    let world = context.world;
    let weight = 1.0 / spread.directions_count() as f32;
    let within = |collider: Entity| world.volume_of(collider).is_some();
    let reach = resolve::FieldReach::of_throw(context, &within, throw.origin);
    let mut rays = Vec::with_capacity(spread.directions_count());
    for (ray_dir, profile) in spread.directions(throw.axis) {
        let thrown = throw.strength * profile;
        let walked = resolve::field_direction(
            context,
            &reach,
            is_box,
            throw.exited,
            throw.origin,
            ray_dir,
            thrown,
            weight,
        );
        let ray = walked.unwrap_or_else(|error| error.panic());
        dose.add_field(&ray.doses, throw.unit);
        let doses = ray
            .doses
            .iter()
            .map(|dose| (dose.address, dose.amount))
            .collect();
        rays.push(SpallRay {
            direction: ray_dir,
            end: ray.end,
            strength: thrown,
            doses,
        });
    }
    rays
}

/// Throw the spall cone of one body exit, appending it to the shell's record and its doses to the
/// shot's [`ShotDose`] (design doc §5). Returns whether it was thrown.
///
/// The game's damage-model strength `S` is [`walk::SpallSource::strength`] (J): the cone carries
/// its share of the energy its round lost across the body ([`walk::SpallSource::carried`]), and
/// its doses convert at the cone's unit ([`spall_cone::cone_unit`]). Every nonzero cone is walked,
/// recorded and tallied, including contributions below the pool quantum. The cone is spread
/// by the ball law at the source's ratio ([`walk::SpallSource::debris_ratio`]): direction `k` of
/// the spread ([`spall_cone::Spread`]) carries `S·f_k` and stands for `1/N` of the cone, walked
/// through every spatially reached volume ([`walk_field`]). Directions still leaving the source
/// have not entered it; genuine re-entry can return carried energy to it. The game's damage model requires the
/// source ratio and direction to remain inside the law's domain: substances are validated at
/// load and the crossing derives the source from a resolved flight.
fn throw_spall_cone(
    spall: &mut SpallMarks,
    dose: &mut ShotDose,
    source: &walk::SpallSource,
    context: &resolve::ResolveContext<'_, '_, '_>,
    is_box: &dyn Fn(Entity) -> bool,
) -> bool {
    let shape = spall_cone::ConeShape::shipped();
    #[cfg(test)]
    let shape = context
        .world
        .spall_law
        .as_deref()
        .map_or(shape, |law| &law.0);
    let strength = source.strength();
    assert!(
        strength.is_finite() && strength >= 0.0,
        "the game's damage model derives finite nonnegative spall strength from crossing energy",
    );
    let direction = Dir3::new(source.direction).expect(
        "the game's damage model derives a spall source from the resolved flight direction",
    );
    let throw = FieldThrow {
        origin: source.position,
        axis: shape.axis(source.face, direction),
        strength,
        unit: spall_cone::cone_unit(),
        exited: &source.exited,
    };
    if strength == 0.0 {
        return false; // A cone carrying no energy has nothing for a pool or a construction.
    }
    let spread = shape
        .spread(spall_cone::FieldDescriptor::ball(source.debris_ratio))
        .expect(
            "the game's damage model takes a spall source's ratio from its load-validated \
             material; the derived field must stay inside the configured law's domain",
        );
    let rays = walk_field(&throw, &spread, dose, context, is_box);
    spall.cones.push(SpallCone {
        #[cfg(any(test, feature = "dev_ui"))]
        exited: source.exited.clone(),
        origin: source.position,
        axis: throw.axis,
        field: spread.field(),
        strength,
        rays,
    });
    true
}

/// Apply a crossing's momentum share to the struck body. The declared `Forces` query keeps this
/// immediate velocity write visible to Bevy's scheduler; static or non-rigid owners do not match.
fn apply_hit_impulse(bodies: &mut Query<Forces>, body: Entity, impulse: Vec3, point: Vec3) {
    if let Ok(forces) = bodies.get_mut(body) {
        crate::track::sim::apply_explicit_impulse(forces, impulse, point);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// The game's damage model rounds quantized dose to the nearest pool unit, ties away from
    /// zero. Pool settlement relies on this independently of authored crew-line and energy rates.
    #[test]
    fn a_dose_converts_to_the_nearest_pool_unit() {
        assert_eq!(pool_units_of(0), 0);
        assert_eq!(pool_units_of(DOSE_UNITS_PER_POOL_UNIT / 2 - 1), 0);
        assert_eq!(
            pool_units_of(DOSE_UNITS_PER_POOL_UNIT / 2),
            1,
            "a tie rounds away from zero"
        );
        assert_eq!(
            pool_units_of(30 * DOSE_UNITS_PER_HP),
            30 * POOL_UNITS_PER_HP
        );
    }

    /// The game's damage model never turns a broken deposit into zero through a saturating cast.
    /// Pool settlement and its recorded dose rely on both conversion edges rejecting invalid input.
    #[test]
    fn invalid_dose_and_energy_are_bugs_before_quantization() {
        for invalid in [-1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert!(std::panic::catch_unwind(|| dose_units(invalid)).is_err());
            assert!(std::panic::catch_unwind(|| energy_units(invalid)).is_err());
        }
        assert_eq!(dose_units(0.0), 0);
        assert_eq!(energy_units(0.0), 0);
    }

    /// A box loses its shot's dose converted once on its total: doses each below half a pool unit
    /// add up before they are converted, a settle mid-shot takes only what the converted total has
    /// gained since the last, and the pool stops at zero. Fixture doses are fractions of pool units.
    #[test]
    fn a_shots_dose_is_converted_once_per_box() {
        let mut world = World::new();
        let target = world.spawn(ComponentHealth::full(30)).id();
        world.spawn(ShotDose::default());
        let dose = |world: &mut World, amounts: &[u64]| -> u64 {
            let amounts = amounts.to_vec();
            world
                .run_system_once(
                    move |mut shot: Query<&mut ShotDose>,
                          mut health: Query<&mut ComponentHealth>| {
                        let mut shot = shot.single_mut().unwrap();
                        for amount in &amounts {
                            shot.add_units(target, *amount);
                        }
                        shot.settle(&mut health, 0)
                    },
                )
                .unwrap()
        };
        let full = 30 * POOL_UNITS_PER_HP;
        assert_eq!(
            dose(&mut world, &[DOSE_UNITS_PER_POOL_UNIT * 4 / 10; 10]),
            4,
            "ten 0.4-unit doses take 4 units"
        );
        assert_eq!(
            dose(&mut world, &[DOSE_UNITS_PER_POOL_UNIT * 3 / 10]),
            0,
            "4.3 units converts to the 4 taken"
        );
        assert_eq!(
            dose(&mut world, &[DOSE_UNITS_PER_POOL_UNIT * 3 / 10]),
            1,
            "4.6 units converts to 5"
        );
        assert_eq!(
            world.get::<ComponentHealth>(target).unwrap().held(),
            full - 5
        );
        assert_eq!(
            dose(&mut world, &[100 * DOSE_UNITS_PER_HP]),
            u64::from(full - 5),
            "the pool stops at zero"
        );
        assert!(world.get::<ComponentHealth>(target).unwrap().is_empty());
    }

    /// The game's damage model: `ShellDamage` confirms only shown points lost by another tank.
    /// Fields reach the shooter's pools too; counting every owned pool would confirm self-dose.
    /// FIXTURE DATA: three 30-point pools, 1 reference-mm each.
    #[test]
    fn a_hit_confirmation_never_counts_the_shooters_own_pools() {
        let mut world = World::new();
        let (shooter, struck) = (world.spawn_empty().id(), world.spawn_empty().id());
        let own = world
            .spawn((ComponentHealth::full(30), VolumeOf(shooter)))
            .id();
        let theirs = world
            .spawn((ComponentHealth::full(30), VolumeOf(struck)))
            .id();
        let unowned = world.spawn(ComponentHealth::full(30)).id();
        world.spawn(ShotDose::default());
        let confirmed = |world: &mut World, boxes: Vec<Entity>| -> (u32, u32) {
            world
                .run_system_once(
                    move |mut shot: Query<&mut ShotDose>,
                          mut health: Query<&mut ComponentHealth>,
                          owners: Query<&VolumeOf>| {
                        let mut shot = shot.single_mut().unwrap();
                        for &address in &boxes {
                            shot.add(address, 1.0);
                        }
                        shot.settle(&mut health, 0);
                        (
                            owned_losses(&shot, &owners, Some(shooter)),
                            owned_losses(&shot, &owners, None),
                        )
                    },
                )
                .unwrap()
        };
        let lost = shown_lost(30 * POOL_UNITS_PER_HP, pool_units(1.0));
        assert_eq!(
            confirmed(&mut world, vec![own, unowned]),
            (0, lost),
            "a self-dose and an unowned pool's dose confirm no hit for the shooter"
        );
        assert_eq!(
            confirmed(&mut world, vec![theirs]).0,
            lost,
            "another tank's lost points confirm the hit"
        );
    }

    /// The game's damage model: `MarchStep::damage_dealt` needs a wider sum than one pool.
    /// Fixture: two [`MAX_POOL_POINTS`] pools each lose just over half the u32 range, summing to
    /// one unit beyond u32 (DERIVED). A u32 sum panics under overflow checks and wraps in release.
    #[test]
    fn a_settle_can_remove_more_units_than_one_pool_can_hold() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let targets = [
            world.spawn(ComponentHealth::full(MAX_POOL_POINTS)).id(),
            world.spawn(ComponentHealth::full(MAX_POOL_POINTS)).id(),
        ];
        let each = u32::MAX / 2 + 1;
        let removed = world
            .run_system_once(move |mut health: Query<&mut ComponentHealth>| {
                let mut shot = ShotDose::default();
                for target in targets {
                    shot.add_units(target, u64::from(each) * DOSE_UNITS_PER_POOL_UNIT);
                }
                shot.settle(&mut health, 0)
            })
            .unwrap();
        assert_eq!(removed, 2 * u64::from(each));
        for target in targets {
            assert_eq!(
                world.get::<ComponentHealth>(target).unwrap().held(),
                MAX_POOL_POINTS * POOL_UNITS_PER_HP - each
            );
        }
    }

    /// The game's damage model accumulates sub-half-point shots; every view relies on
    /// [`ComponentHealth::shown`] showing full until a whole point is lost and zero only when empty.
    /// Fixture: separate [`ShotDose`]s of 0.47 HP into a 3-point pool.
    /// Whole-point dose rounding never empties it; rounding the display down or to nearest shows
    /// loss after the first or second shot. Emptying before the full pool is taken also fails.
    #[test]
    fn doses_below_half_a_point_accumulate_and_show_only_whole_points() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let target = world.spawn(ComponentHealth::full(3)).id();
        let shoot = |world: &mut World| -> bool {
            world
                .run_system_once(move |mut health: Query<&mut ComponentHealth>| {
                    let mut shot = ShotDose::default();
                    shot.add_units(target, 47 * DOSE_UNITS_PER_HP / 100);
                    shot.settle(&mut health, 0);
                    shot.emptied().contains(&target)
                })
                .unwrap()
        };
        // 0.47 points a shot: 3 / 0.47 = 6.4, so the seventh empties it (DERIVED).
        let shown = [3, 3, 2, 2, 1, 1, 0];
        for (shot, shown) in shown.into_iter().enumerate() {
            let emptied = shoot(&mut world);
            let pool = *world.get::<ComponentHealth>(target).unwrap();
            assert_eq!(pool.shown(), shown, "after shot {}", shot + 1);
            assert_eq!(emptied, shot == 6, "shot {} empties it", shot + 1);
            assert_eq!(pool.is_empty(), shot == 6, "after shot {}", shot + 1);
        }
        let short = ComponentHealth::holding(3, 3 * POOL_UNITS_PER_HP - 999);
        assert_eq!(
            (short.shown(), short.points()),
            (3, 3),
            "0.999 points lost shows undamaged"
        );
    }

    /// What a box's pool loses depends only on the multiset of its contributions, never their
    /// order: several cones' rays summed forward, reversed, sorted either way, interleaved and
    /// shuffled take the same pool units, including totals beside a conversion tie. Fixture doses
    /// (reference-mm), each cone 256 equal rays.
    #[test]
    fn a_boxs_hp_lost_is_independent_of_the_order_of_its_doses() {
        let target = Entity::from_raw_u32(1).unwrap();
        let cone = |total: f32| vec![total / 256.0; 256];
        let lost = |amounts: &[f32]| -> u32 {
            let mut shot = ShotDose::default();
            for amount in amounts {
                shot.add(target, *amount);
            }
            pool_units_of(shot.tallies[0].units)
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
            pool_units_of(3 * DOSE_UNITS_PER_POOL_UNIT / 2),
            2,
            "1.5 units, a tie, rounds away from zero"
        );
        assert_eq!(pool_units_of(3 * DOSE_UNITS_PER_POOL_UNIT / 2 - 1), 1);
    }

    /// The least shown dose is the display's own edge: a full pool dosed that share of one HP point
    /// shows a lost point, and a dose unit less shows full. The pen cam's floor band
    /// (`pen_cam::potency::Potency::floor`) is that dose over the crew face and relies on it; a
    /// floor read as the sim's quantum, as half a point, or as a whole point the conversion's half
    /// unit undercuts, fails here. Fixture pool: 30 HP points.
    #[test]
    fn the_least_shown_hp_is_the_displays_edge() {
        let units = (least_shown_hp() * DOSE_UNITS_PER_HP as f64).round() as u64;
        let shown_after = |units: u64| {
            let mut pool = ComponentHealth::full(30);
            pool.take(pool_units_of(units));
            pool.shown()
        };
        assert_eq!(shown_after(units), 29);
        assert_eq!(shown_after(units - 1), 30);
    }

    /// EACH DOSE CONVERTS AT ITS SOURCE'S RATE: a hard box's line deposit and a bounce's shock,
    /// joules, at the one energy rate ([`energy_dose::joules_per_hp`]); a field deposit, joules
    /// too, at its own mechanism's unit ([`field_units`]), never at the energy rate; a witness's
    /// chord dose at [`HP_PER_REFERENCE_MM`]; mixed deposits sum as whole dose units, in either
    /// order, before the one conversion. The shot tally and the pen cam's reading of which
    /// boxes a field knocks out rely on every field deposit converting at its field's unit, and
    /// every hard pool's line dose at the one energy rate. Fixture doses: three HP points' worth of
    /// joules in the field at a fixture unit of 1/700 HP per joule (0.7 HP at the shipped energy
    /// rate, so the wrong rate shows 1), two on the line and one in the shock at the energy rate,
    /// and 1 reference-mm of chord dose.
    #[test]
    fn each_dose_converts_at_its_sources_rate() {
        use mechanism::{Dose, DoseSource};
        let target = Entity::from_raw_u32(1).unwrap();
        let unit = spall_cone::FieldUnit::fixture(1.0 / 700.0);
        let field = |amount| Dose {
            address: target,
            amount,
            source: DoseSource::Field,
        };
        let points = |fields: &[Dose], lines: &[f32], shocks: &[f32], chords: &[f32]| -> u32 {
            let mut shot = ShotDose::default();
            for line in lines {
                shot.add_units(target, energy_units(*line));
            }
            shot.add_field(fields, unit);
            for shock in shocks {
                shot.add_energy(target, *shock);
            }
            for chord in chords {
                shot.add(target, *chord);
            }
            shown_points(pool_units_of(shot.tallies[0].units))
        };
        let joules = |points: f64| (points * energy_dose::joules_per_hp()) as f32;
        let field_joules = |points: f64| (points / unit.hp_per_joule()) as f32;
        let chord_rate = u32::try_from(HP_PER_REFERENCE_MM).unwrap();
        let three = [field(field_joules(3.0))];
        assert_eq!(points(&three, &[], &[], &[]), 3, "a field deposit");
        assert_eq!(points(&[], &[joules(2.0)], &[], &[]), 2, "a line deposit");
        assert_eq!(points(&[], &[], &[joules(1.0)], &[]), 1, "a shock");
        assert_eq!(points(&[], &[], &[], &[1.0]), chord_rate, "a chord dose");
        assert_eq!(
            points(&three, &[joules(2.0)], &[joules(1.0)], &[1.0]),
            3 + 2 + 1 + chord_rate
        );
        let mut shot = ShotDose::default();
        shot.add(target, 1.0);
        shot.add_energy(target, joules(1.0));
        shot.add_field(&three, unit);
        shot.add_units(target, energy_units(joules(2.0)));
        assert_eq!(
            shown_points(pool_units_of(shot.tallies[0].units)),
            3 + 2 + 1 + chord_rate,
            "in the other order"
        );
    }

    /// The game's damage model: a field's HP unit scales crew dose, never crushed volume.
    /// Construction reads `S*f/N` joules through `Load::field`; crew pools read those joules
    /// through the field's unit in `ShotDose`. A unit in strength would move crushed volume;
    /// a tally using the line's energy rate would miss crew dose. Both consumers rely on this.
    /// Fixture: 2.7 MJ lost, exit ratios 1 and 0.2, lambda 2.4, 64 directions,
    /// crushing density 1.35e8 J/m³, and a crew station reached by every direction.
    /// Units: 1.5e-4 and 3.0e-4 HP/J, distinct from the line's 1/3000 HP/J.
    #[test]
    fn a_fields_hp_unit_moves_crew_dose_and_never_crushed_volume() {
        const CRUSH: f32 = 1.35e8; // Fixture data: J/m³.
        let spread = spall_cone::ConeShape::new(spall_cone::AxisRule::ExitDirection, 64)
            .unwrap()
            .spread(spall_cone::FieldDescriptor::ball(2.4))
            .unwrap();
        let share = 1.0 / spread.directions_count() as f32;
        for ratio in [1.0_f32, 0.2] {
            let source = walk::SpallSource {
                t: 0.0,
                position: Vec3::ZERO,
                face: None,
                direction: Vec3::NEG_Z,
                exited: Vec::new(),
                plate_met: 0.0,
                emitted: 2_700_000.0 * (spall_cone::cone_energy().debris_share() * ratio),
                debris_ratio: 2.4,
                strength_ratio: ratio,
                exit_speed: 0.0,
            };
            let strength = source.strength();
            let doses: Vec<f32> = spread
                .strata()
                .map(|[.., f]| strength * f * share)
                .collect();
            let hits: Vec<(usize, Vec3, f32, Vec3)> = doses
                .iter()
                .map(|&energy| (0, Vec3::ZERO, energy, Vec3::ZERO))
                .collect();
            let crushed: f64 = crate::construction::fracture::Load::field(&hits, |_| CRUSH)
                .iter()
                .map(|load| f64::from(load.crushed))
                .sum();
            let share = spall_cone::cone_energy().debris_share();
            let carried = f64::from(2_700_000.0 * (share * ratio));
            assert!(
                (crushed - carried / f64::from(CRUSH)).abs() <= 1e-6 * crushed,
                "ratio {ratio}: crushed {crushed} m³ for {carried} J carried"
            );
            let station = Entity::from_raw_u32(1).unwrap();
            for unit in [1.5e-4, 3.0e-4] {
                let fixture = spall_cone::FieldUnit::fixture(unit);
                let mut shot = ShotDose::default();
                let field: Vec<mechanism::Dose> = doses
                    .iter()
                    .map(|&amount| mechanism::Dose {
                        address: station,
                        amount,
                        source: mechanism::DoseSource::Field,
                    })
                    .collect();
                shot.add_field(&field, fixture);
                let hp = hp_before_rounding(shot.tallies[0].units) as f64;
                let joules: f64 = doses.iter().map(|&dose| f64::from(dose)).sum();
                assert!(
                    (hp - joules * unit).abs() <= 1e-5 * joules * unit,
                    "ratio {ratio}: {hp} HP from {joules} J at {unit} HP per joule"
                );
            }
        }
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

    /// The game's damage model: measurement tool, not a firing-record acceptance test.
    /// Fixture: the record's 15 lb, 3 in body, using each existing hit-nose proxy.
    #[test]
    #[ignore = "measurement tool: prints limit speeds without firing-row pass conditions"]
    fn measure_plate_limit_speeds() {
        for nose_factor in [NoseShape::Ogive { crh: 1.0 }.factor(), 0.5] {
            let body = RoundBody {
                mass: 15.0 * 0.453_592_37,
                caliber: 3.0 * 0.0254,
                nose_factor,
            };
            for ratio in [
                0.1_f32, 0.2, 0.24, 0.25, 0.3, 0.5, 0.65, 0.7, 0.75, 0.9, 0.99, 1.0,
            ] {
                let depth = body.caliber * ratio * 1000.0;
                let speed = speed_for(body, depth);
                let f = speed / 0.3048 * 15.0_f32.sqrt() / (0.25 * (ratio * 0.25).sqrt());
                let charge = physical_laws::PhysicalLaws::shipped()
                    .plate_capacity
                    .charge(f64::from(ratio));
                let after = speed_for(body, (f64::from(depth) * charge) as f32);
                let after_f = after / 0.3048 * 15.0_f32.sqrt() / (0.25 * (ratio * 0.25).sqrt());
                println!(
                    "MEASURED nose={nose_factor:.6} t/d={ratio:.2} before={speed:.6} F={f:.1} after={after:.6} F={after_f:.1}"
                );
            }
        }
        for (gun, key, round) in crate::guns::GunRegistry::shipped().fired_rounds() {
            let body = round.penetrator;
            for ratio in [0.1_f32, 0.5, 0.9] {
                let thickness = body.caliber * ratio;
                let bulk = speed_for(body, thickness * 1000.0);
                let share = physical_laws::PhysicalLaws::shipped()
                    .plate_capacity
                    .charge(f64::from(ratio));
                let thin = speed_for(body, (f64::from(thickness) * 1000.0 * share) as f32);
                println!(
                    "MEASURED shot {gun}/{key} plate_mm={:.6} old_limit={bulk:.6} new_limit={thin:.6}",
                    thickness * 1000.0
                );
            }
        }
    }

    /// The game's damage model: plate/ammunition authors rely on rising limit speed, a continuous
    /// one-calibre join, unchanged bulk and mass/nose/calibre ordering. Catches thin share above
    /// one calibre or reversed thickness/calibre. Composes the inverse; walk tests own wiring.
    /// Fixture thickness sweep; shipped rounds supply their bodies and hit-nose proxies.
    #[test]
    fn plate_limit_speeds_keep_bulk_and_round_ordering() {
        let capacity = &physical_laws::PhysicalLaws::shipped().plate_capacity;
        for (_, _, round) in crate::guns::GunRegistry::shipped().fired_rounds() {
            let body = round.penetrator;
            let limit = |body: RoundBody, thickness: f32| {
                speed_for(
                    body,
                    (f64::from(thickness)
                        * 1000.0
                        * capacity.charge(f64::from(thickness) / f64::from(body.caliber)))
                        as f32,
                )
            };
            let mut previous = 0.0;
            for i in 1..=200 {
                let thickness = body.caliber * i as f32 / 100.0;
                let speed = limit(body, thickness);
                assert!(speed > previous);
                previous = speed;
                if thickness >= body.caliber {
                    assert_eq!(
                        speed,
                        speed_for(body, (f64::from(thickness) * 1000.0) as f32)
                    );
                }
                assert!(
                    limit(
                        RoundBody {
                            mass: body.mass * 2.0,
                            ..body
                        },
                        thickness
                    ) < speed
                );
                for widen in [1.001, 1.01, 1.02, 2.0] {
                    assert!(
                        limit(
                            RoundBody {
                                caliber: body.caliber * widen,
                                ..body
                            },
                            thickness
                        ) >= speed,
                        "widening at fixed mass cannot lower the plate limit beyond rounding"
                    );
                }
                let flatter = RoundBody {
                    nose_factor: 1.0,
                    ..body
                };
                assert!(limit(flatter, thickness) >= speed);
            }
            for epsilon in [1.0e-3_f32, 1.0e-4] {
                let low = limit(body, body.caliber * (1.0 - epsilon));
                let high = limit(body, body.caliber * (1.0 + epsilon));
                assert!(low < high && (high / low - 1.0) < 20.0 * epsilon);
            }
        }
    }

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

    /// `speed_for` inverts `capability` across the speed range a round meets, and spends compose
    /// through it: spending `k1` then `k2` through two inversions lands on the same speed as
    /// spending `k1 + k2` in one.
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

    /// The local speed exponent `d ln C / d ln v` is the dimensionless law's own,
    /// `2x / ((1 + x) ln(1 + x))` with `x = k v² / a`: 2 in the limit `x → 0`, and positive and
    /// strictly below 2 at every positive speed, for every geometry, so capability strictly
    /// increases with speed.
    ///
    /// Swept over the round fixtures and every nose family across its parameter range, from 1 m/s
    /// to 2 km/s. The exponent is read as a central difference of `ln C` over `v (1 ± h)`: with
    /// `h = 1e-2` the truncation error is `O(h²)` (≈ 1e-4, DERIVED) and f32 rounding of `C`
    /// contributes `ε / h` (≈ 1e-5, DERIVED), so a tolerance of 1e-3 cannot hide a departure from
    /// the law and cannot be tripped by arithmetic.
    #[test]
    fn capability_law_speed_exponent_is_the_dimensionless_law_s() {
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
                        0.0 < law && law < 2.0,
                        "{round:?} at {speed} m/s: the law's exponent {law}"
                    );
                    assert!(
                        measured > 0.0,
                        "{round:?} at {speed} m/s: capability falls with speed, exponent {measured}"
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

    /// The drag law is stated on its two data parameters: the anchor nose flies on the anchor form
    /// to the bit, so a round whose flight nose is that nose flies on it; and the base share is
    /// the floor a nose approaches as its factor goes to zero. Parameters that state no law (a
    /// base that is not positive, an anchor at or under the base) are refused. A mutant that moves
    /// the nose share off the anchor, drops the base share or admits a negative nose share fails
    /// here. FIXTURE DATA: a 2-crh anchor on 0.3 over a base drag coefficient of 0.25.
    #[test]
    fn the_drag_law_is_stated_on_its_anchors() {
        let anchor = NoseShape::Ogive { crh: 2.0 };
        let law = DragLaw::stated(0.25, anchor, 0.3).expect("fixture parameters state a law");
        assert_eq!(law.form(anchor).to_bits(), 0.3_f32.to_bits());
        let base = 0.25 * form_per_drag_coefficient();
        let needle = law.form(NoseShape::Ogive { crh: 1.0e6 });
        assert!(
            (needle as f64 / base - 1.0).abs() < 1.0e-5,
            "a 1e6-crh ogive flies on {needle}, the base share is {base}"
        );
        for (coefficient, form) in [
            (0.0, 0.3),
            (f64::NAN, 0.3),
            (0.25, base),
            (0.25, f64::INFINITY),
        ] {
            assert!(
                DragLaw::stated(coefficient, anchor, form).is_err(),
                "{coefficient} / {form}"
            );
        }
    }

    /// Drag only slows a shell — never speeds it up or reverses it — and gravity always pulls the
    /// vertical component down. Guards the analytic drag step against a sign or stability slip.
    #[test]
    fn freeflight_step_bleeds_speed_and_falls() {
        let v0 = Vec3::new(700.0, 0.0, 0.0);
        let v1 = freeflight_step(v0, fixture_drag_k(0.088, 10.2), 0.01);
        assert!(v1.length() < v0.length(), "drag must reduce speed");
        assert!(v1.y < 0.0, "gravity must pull the shell down");
    }

    /// The game's damage model uses one integrator: catch-up velocity equals N [`freeflight_step`]s,
    /// never a separate closed-form trajectory. `march_tests::catch_up_lands_where_the_march_flies`
    /// owns positional agreement. Zero catch-up preserves origin, launch velocity and a one-point
    /// trail, as local fire does.
    #[test]
    fn fast_forward_matches_single_tick_advances() {
        let origin = Vec3::new(1.0, 50.0, -3.0);
        let v0 = Vec3::new(600.0, 20.0, 0.0);
        let k = fixture_drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        for n in [0, 7] {
            let mut vel = v0;
            for _ in 0..n {
                vel = freeflight_step(vel, k, dt);
            }

            let (ff_pos, ff_vel, path) = fast_forward_shell(origin, v0, k, dt, n);
            assert_eq!(ff_vel, vel, "fast-forward velocity == {n} single steps");
            // One point per stepped tick plus the origin, and the trail starts AT the muzzle
            // (requirement: the tracer trail must not start 64 m behind the shell).
            assert_eq!(path.len(), n as usize + 1);
            assert_eq!(path[0], origin, "the trail starts at the muzzle");
            assert_eq!(*path.last().unwrap(), ff_pos, "the trail ends at the shell");
        }
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

/// The game's damage model: Avian-backed [`integrate_projectiles`] tests capturing real impacts
/// and surface classification. Plate fixtures follow `sandbox::spawn_targets`.
#[cfg(test)]
pub(crate) mod march_tests {
    use std::time::Duration;

    use avian3d::prelude::{
        AngularInertia, AngularVelocity, Collider, CollisionLayers, GravityScale, LayerMask,
        LinearVelocity, Mass, NoAutoAngularInertia, NoAutoMass, PhysicsPlugins, RigidBody,
    };
    use bevy::time::TimeUpdateStrategy;

    use super::*;

    /// Anchor fixture sampling on its fire direction, as [`on_fire_shell`] does. Another direction
    /// gives the ring a longitudinal component, putting samples ahead of and behind the disc.
    fn test_disc(direction: Vec3) -> walk::DiscFrame {
        walk::DiscFrame::anchored(direction).expect("a fixture fires along a real direction")
    }

    /// One captured impact — the fields the armor read branches on.
    #[derive(Clone, Copy)]
    struct Captured {
        style: crate::rounds::ImpactStyle,
        position: Vec3,
        normal: Vec3,
        surface: ImpactSurface,
        verdict: Verdict,
        deflection: Option<Vec3>,
    }

    /// The capture sink: every `Impact` the march fires lands here (view-only observer stand-in).
    #[derive(Resource, Default)]
    struct ImpactLog(Vec<Captured>);

    fn capture_impact(impact: On<Impact>, mut log: ResMut<ImpactLog>) {
        log.0.push(Captured {
            style: impact.style,
            position: impact.position,
            normal: impact.normal,
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
    /// Fixture crew line rate (reference-mm per metre of witness chord) for [`fixture_round`]: the
    /// main-gun value the round files (`assets/rounds/`) tune, a labelled fixture number.
    pub(super) const FIXTURE_WOUND: f32 = 30.0;

    /// The game's damage model: full-calibre fixture flight state, bound as the real spawn binds
    /// it. [`fixture_round_with`] also supports a distinct penetrator.
    pub(super) fn fixture_round(body: RoundBody, velocity: Vec3) -> Projectile {
        fixture_round_with(body, body, velocity)
    }

    /// The game's damage model: fixture state flying as `body`, meeting matter as `penetrator`.
    /// Its drag is fixture data ([`fixture_drag_k`]); the mechanism binds only the penetrator.
    pub(super) fn fixture_round_with(
        body: RoundBody,
        penetrator: RoundBody,
        velocity: Vec3,
    ) -> Projectile {
        Projectile {
            impact_style: crate::rounds::ImpactStyle::fixture(crate::rounds::ImpactPreset::Compact),
            velocity,
            round: bound(penetrator),
            flying: FlyingBody {
                caliber: body.caliber,
                mass: body.mass,
                drag_k: fixture_drag_k(body.caliber, body.mass),
            },
            contact: crate::rounds::ContactPreset::fixture(),
            wound: FIXTURE_WOUND,
            fuze: burst::Fuze::Inert,
            disc: test_disc(velocity.normalize()),
            starts: None,
            playback: Default::default(),
            path_sequence: 0,
            replica: None,
            contacted: Vec::new(),
            source_cleared: false,
        }
    }

    /// Settled Avian world with real march/impact capture and one static steel `Armor` plate
    /// (full extents `size`, centre `at`, facing ±Z). Sim-only: shells are bare; drawing tests
    /// mount the presentation plugin.
    pub(super) fn world_with_plate(size: Vec3, at: Vec3) -> App {
        plate_world(size, at, false)
    }

    fn plate_world(size: Vec3, at: Vec3, view: bool) -> App {
        let mut app = bare_world(view);
        let collider = Collider::cuboid(size.x, size.y, size.z);
        app.world_mut().spawn((
            Transform::from_translation(at),
            RigidBody::Static,
            collider,
            CollisionLayers::new([Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "RHA".to_string(),
            },
        ));
        settle(&mut app);
        app
    }

    /// Let Avian register the static colliders and build the spatial-query pipeline before a shell
    /// is marched against them.
    fn settle(app: &mut App) {
        for _ in 0..8 {
            app.update();
        }
    }

    /// The march's world with nothing in it: Avian, the transform hierarchy, the asset system,
    /// the manual clock and the real march, every plugin
    /// mounted, finished and cleaned up. Callers spawn their geometry and [`settle`].
    fn bare_world(view: bool) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            // Avian's collider cache reads `AssetEvent<Mesh>`, so the asset system must be present
            // even though these cuboid colliders carry no mesh handle.
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
            16,
        )))
        // Fixture: map bounds enclose this geometry-only instrument's flight space.
        .insert_resource(crate::map::tests::fixture_manifest(4096.0, -1024.0, 2048.0))
        .init_resource::<RetainSpentShells>()
        .init_resource::<ImpactLog>()
        .add_observer(capture_impact)
        // The game's damage-model march on a fixture-controlled clock: each update advances
        // the manual virtual duration independently of the production schedule.
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
        app
    }

    /// The game's damage model: authority resolution alone keeps collider history; replicas
    /// present public paths. FIXTURE DATA: a static hull with one child collider.
    #[test]
    fn only_the_authority_keeps_collider_history() {
        for replica in [false, true] {
            let mut app = App::new();
            app.add_plugins((
                MinimalPlugins,
                bevy::transform::TransformPlugin,
                AssetPlugin::default(),
                PhysicsPlugins::default(),
            ))
            .init_asset::<Mesh>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
                16,
            )));
            if replica {
                app.insert_resource(ClientReplica);
            }
            mount_collider_motion(&mut app);
            while app.plugins_state() == bevy::app::PluginsState::Adding {
                std::thread::sleep(Duration::from_millis(1));
            }
            app.finish();
            app.cleanup();
            let hull = app
                .world_mut()
                .spawn((Transform::from_xyz(0.0, 1.0, 0.0), RigidBody::Static))
                .id();
            app.world_mut().spawn((
                ChildOf(hull),
                Transform::from_xyz(0.0, 0.5, 0.0),
                Collider::cuboid(1.0, 1.0, 1.0),
            ));
            for _ in 0..4 {
                app.update();
            }
            let kept = app
                .world()
                .resource::<motion::ColliderMotion>()
                .colliders
                .len();
            assert_eq!(
                kept > 0,
                !replica,
                "only the role that walks keeps collider history (replica: {replica})"
            );
        }
    }

    /// Spawn an 88 round at `origin` travelling `dir` (unit) at `speed`, then march until an impact is
    /// captured (or the bound trips). Returns every impact fired.
    fn fire_and_capture(app: &mut App, origin: Vec3, dir: Vec3, speed: f32) -> Vec<Captured> {
        fire_with(app, FIXTURE_88, FIXTURE_88, origin, dir, speed)
    }

    /// [`fire_and_capture`] of a round that flies as `body` and meets the plate as `penetrator`.
    fn fire_with(
        app: &mut App,
        body: RoundBody,
        penetrator: RoundBody,
        origin: Vec3,
        dir: Vec3,
        speed: f32,
    ) -> Vec<Captured> {
        app.world_mut().spawn((
            fixture_round_with(body, penetrator, dir * speed),
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

    /// The game's damage model: armour-layer geometry requires ballistic-volume ancestry.
    /// The authority must refuse missing material data rather than treating the collider as ground.
    #[test]
    #[should_panic(expected = "an Armor collider requires BallisticVolume ancestry")]
    fn armour_without_a_ballistic_volume_is_a_wiring_bug() {
        let mut app = world_with_plate(Vec3::splat(1.0), Vec3::new(0.0, 2.0, 0.0));
        let plate = app
            .world_mut()
            .query_filtered::<Entity, With<BallisticVolume>>()
            .single(app.world())
            .expect("fixture plate");
        app.world_mut()
            .entity_mut(plate)
            .remove::<BallisticVolume>();
        fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
    }

    /// The game's damage model admits ballistic ancestry only with discovery membership.
    /// Corridors and fields must fail on a miswired solid instead of disagreeing with flight.
    #[test]
    #[should_panic(expected = "a BallisticVolume collider requires Armor membership")]
    fn a_ballistic_volume_without_armour_membership_is_a_wiring_bug() {
        use bevy::ecs::system::RunSystemOnce;
        let mut app = world_with_plate(Vec3::ONE, Vec3::Y * 2.0);
        let plate = app
            .world_mut()
            .query_filtered::<Entity, With<BallisticVolume>>()
            .single(app.world())
            .unwrap();
        app.world_mut()
            .entity_mut(plate)
            .insert(CollisionLayers::new([Layer::Debris], LayerMask::ALL));
        app.world_mut()
            .run_system_once(move |world: ProjectileMarchWorld| {
                world.armour(plate);
            })
            .unwrap();
    }

    /// The game's damage model: a kept carrier delivers the assembly's momentum and a discarded
    /// one only the penetrator's. Hull shock relies on the flying mass at the fire-to-march seam.
    /// Both fixture cores stop in the plate, so a mutant retaining the discarded carrier's mass
    /// delivers four times the expected impulse (DERIVED from the fixture masses).
    #[test]
    fn a_hit_delivers_only_the_flying_body_s_momentum() {
        const SPEED: f32 = 900.0;
        const BODY_MASS: f32 = 100.0;
        let core = r#", penetrator: Some((mass: (1.25, Fixture("fixture")),
            caliber: (0.02, Fixture("fixture"))))"#;
        for (carrier, flying_mass) in [("Kept", 5.0), ("Discarded", 1.25)] {
            let mut round = crate::rounds::tests::of_type(carrier, "5.0", "0.05", core);
            round.muzzle_speed = SPEED;
            // Fixture plate: thicker than the core's capability at the fixture muzzle speed.
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.4), Vec3::new(0.0, 2.0, 0.0));
            app.add_observer(on_fire_shell);
            let body = app
                .world_mut()
                .spawn((
                    RigidBody::Dynamic,
                    Transform::default(),
                    Mass(BODY_MASS),
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
            app.world_mut().trigger(FireShell::of_round(
                &round,
                Vec3::new(0.0, 2.0, 2.0),
                Vec3::new(0.0, 2.0, 2.0),
                Dir3::NEG_Z,
                crate::spec::FireMechanism::Single,
                true,
                None,
                None,
            ));
            for _ in 0..2 {
                app.update();
            }
            let delivered = app
                .world()
                .get::<LinearVelocity>(body)
                .expect("dynamic body has linear velocity")
                .0
                .length()
                * BODY_MASS;
            let expected = flying_mass * SPEED;
            assert!(
                (delivered / expected - 1.0).abs() < 0.02,
                "{carrier}: delivered {delivered} N·s against the flying body's {expected}"
            );
        }
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
            assert_eq!(
                impacts.len(),
                usize::from(!replica),
                "only the authority tests the owned plate"
            );
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

    /// Regression: cosmetic termination does not end authority damage or its one-shot report latch.
    #[test]
    fn damage_confirmation_survives_cosmetic_terminal_and_latches_first_positive_step() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.01), Vec3::new(0.0, 2.0, 0.0));
        app.init_resource::<DamageLog>()
            .add_observer(capture_damage);

        // The components belong to a tank: only HP taken from a tank's pools confirms a hit.
        let tank = app.world_mut().spawn_empty().id();
        let spawn_component = |app: &mut App, z: f32, held: u32| {
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
                    ComponentHealth::holding(1000, held),
                    VolumeOf(tank),
                ))
                .id()
        };
        let spent = spawn_component(&mut app, -20.0, 0);
        let first_live = spawn_component(&mut app, -40.0, 1000 * POOL_UNITS_PER_HP);
        let second_live = spawn_component(&mut app, -60.0, 1000 * POOL_UNITS_PER_HP);
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
        let health = |entity| app.world().get::<ComponentHealth>(entity).unwrap().shown();
        assert_eq!(health(spent), 0, "zero HP cannot produce a fake decrease");
        assert!(
            health(first_live) < 1000,
            "the first live component shows damage"
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
    /// a replica's fail-closed armour contact — is `Struck`. The public verdict names all three
    /// armour outcomes from the real resolution: a graze is one armour `Ricochet` deflected back off
    /// the face, a round that bites a plate thicker than its capability is `Stopped`, and a clean
    /// perforation is one `Through` impact read at the entry face. Plate thicknesses and the 75°
    /// graze are fixture data.
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
        assert_eq!(
            read(&bounce),
            [(
                ImpactSurface::Material(crate::substances::SurfaceFeedback::fixture()),
                Verdict::Ricochet
            )]
        );
        let deflect = bounce[0]
            .deflection
            .expect("a ricochet carries its outgoing direction");
        assert!(
            deflect.z.abs() < 1.0e-3 && deflect.x > 0.99,
            "the round leaves along the face (normal +Z), got {deflect:?}"
        );
        assert_eq!(
            stopped
                .iter()
                .filter(|hit| matches!(hit.surface, ImpactSurface::Material(_)))
                .map(|hit| hit.verdict)
                .collect::<Vec<_>>(),
            [Verdict::Stopped],
        );
        assert_eq!(
            read(&through),
            [(
                ImpactSurface::Material(crate::substances::SurfaceFeedback::fixture()),
                Verdict::Through
            )],
            "a clean perforation fires exactly one impact"
        );
        assert!(
            (through[0].position.z - 0.025).abs() < 0.05,
            "the impact reads at the entry face, got z={}",
            through[0].position.z
        );
        assert!(replica.is_empty(), "a replica never tests the plate");
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
    fn fire_shell_catch_up(
        app: &mut App,
        catch_up_ticks: u32,
        preset: crate::rounds::ImpactPreset,
    ) -> Vec<Captured> {
        app.add_observer(on_fire_shell);
        app.world_mut().trigger(FireShell {
            effects: crate::rounds::FlightEffects {
                impact: preset,
                ..crate::rounds::FlightEffects::fixture_traced()
            },
            muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
            birth: Vec3::new(0.0, 2.0, 2.0),
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            drag_k: fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            nose_factor: FIXTURE_88.nose_factor,
            penetrator_caliber: FIXTURE_88.caliber,
            penetrator_mass: FIXTURE_88.mass,
            contact: crate::rounds::ContactPreset::fixture(),
            wound: 30.0,
            burst: None,
            fuze_mode: 0,
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

    /// The game's damage-model view relies on contact dressing surviving complete spawn and
    /// authority resolution. Equal projectile bodies may carry either authored art recipe, and the
    /// read's scale is the fired round's muzzle energy.
    #[test]
    fn fired_round_preserves_its_authored_contact_recipe_at_impact() {
        use crate::rounds::{ImpactPreset, ImpactStyle};
        for preset in [ImpactPreset::Compact, ImpactPreset::Layered] {
            let style = ImpactStyle::of(preset, FIXTURE_88.mass, 800.0);
            let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
            fire_shell_catch_up(&mut app, 0, preset);
            for _ in 0..8 {
                app.update();
            }
            let hits = &app.world().resource::<ImpactLog>().0;
            assert!(
                !hits.is_empty(),
                "the fixture shot must exercise contact dressing"
            );
            assert!(hits.iter().all(|hit| hit.style == style));
        }
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
            effects: crate::rounds::FlightEffects::fixture_traced(),
            muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
            birth: Vec3::new(0.0, 2.0, 2.0),
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            drag_k: fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            nose_factor: FIXTURE_88.nose_factor,
            penetrator_caliber: FIXTURE_88.caliber,
            penetrator_mass: FIXTURE_88.mass,
            contact: crate::rounds::ContactPreset::fixture(),
            wound: 30.0,
            burst: None,
            fuze_mode: 0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: Some(ShotSource {
                tank: shooter,
                weapon: shot.weapon as usize,
                round: 0,
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

    /// The game's damage model: [`on_fire_shell`] binds the penetrator's mechanism and muzzle
    /// capability; disc, contact and cone hole read its calibre. Flight uses the fire's drag and
    /// views use its flying-body size (`rounds` owns flying-body selection). All roles spawn here.
    /// Fixture core and drag catch binding the flying body or rebuilding drag from either body.
    #[test]
    fn a_fired_shell_flies_as_its_body_and_meets_the_plate_as_its_penetrator() {
        const CORE: RoundBody = RoundBody {
            mass: 2.0,
            caliber: 0.04,
            nose_factor: FIXTURE_88.nose_factor,
        };
        const SPEED: f32 = 900.0;
        /// Neither body's drag on the fixture form: fixture data.
        const DRAG: f32 = 3.0e-4;
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, -50.0));
        app.add_observer(on_fire_shell);
        app.world_mut().trigger(FireShell {
            effects: crate::rounds::FlightEffects::fixture_traced(),
            muzzle: crate::guns::MuzzleEmission::fixture_cannon(),
            birth: Vec3::new(0.0, 2.0, 2.0),
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: SPEED,
            caliber: FIXTURE_88.caliber,
            mass: FIXTURE_88.mass,
            drag_k: DRAG,
            nose_factor: FIXTURE_88.nose_factor,
            penetrator_caliber: CORE.caliber,
            penetrator_mass: CORE.mass,
            contact: crate::rounds::ContactPreset::fixture(),
            wound: FIXTURE_WOUND,
            burst: None,
            fuze_mode: 0,
            mechanism: crate::spec::FireMechanism::Single,
            shooter: None,
            tracer: true,
            shot_origin: FireShellOrigin::Local,
            catch_up_ticks: 0,
            shot: None,
        });
        app.world_mut().flush();

        let world = app.world_mut();
        let mut shells = world.query::<&Projectile>();
        let projectile = shells.single(world).expect("one shell is born");
        assert_eq!(
            projectile.body(),
            CORE,
            "the mechanism binds the penetrator"
        );
        assert_eq!(projectile.speed(), SPEED);
        assert_eq!(
            capability(projectile.body(), projectile.speed()),
            capability(CORE, SPEED)
        );
        assert_ne!(
            capability(projectile.body(), projectile.speed()),
            capability(FIXTURE_88, SPEED)
        );
        assert_eq!(
            projectile.flying.drag_k, DRAG,
            "the shell flies on its fire's drag"
        );
        for body in [FIXTURE_88, CORE] {
            assert_ne!(DRAG, fixture_drag_k(body.caliber, body.mass));
        }
        assert_eq!(projectile.caliber(), FIXTURE_88.caliber);
    }

    /// Every `FireShell` producer shares the allocation boundary, not only network receive. An
    /// oversized catch-up therefore fails closed before it can materialize a shell or path.
    #[test]
    fn oversized_fire_shell_catch_up_fails_closed_before_spawning() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
        let hits = fire_shell_catch_up(
            &mut app,
            MAX_COSMETIC_CATCH_UP_TICKS + 1,
            crate::rounds::ImpactPreset::Layered,
        );

        assert!(hits.is_empty(), "a rejected catch-up produces no impact");
        let mut projectiles = app.world_mut().query_filtered::<Entity, With<Projectile>>();
        assert!(
            projectiles.iter(app.world()).next().is_none(),
            "a rejected catch-up creates no projectile"
        );
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

    fn a_shot() -> ShotId {
        ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 100,
        }
    }

    /// Where the uncertifiable fixture sits: far from the plate, in open air. Fixture data.
    const UNCERTIFIABLE_AT: Vec3 = Vec3::new(0.0, 20.0, 5.0);

    /// The game's damage model: a compound, neither a certified trimesh nor convex, around
    /// [`UNCERTIFIABLE_AT`]. Discovery refuses geometry outside the collector's shape contract.
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

    /// A pen cam opens only for a hit on armour that resolved one of its verdicts, read
    /// from the shell's whole logged path: the ricochet, the stop and the perforation each open
    /// one; a ground hit opens none. Plate thicknesses and the 75°
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
    }

    /// The game's damage model requires the same certified-shape invariant on the authority.
    #[test]
    #[should_panic(expected = "spawn constructs certified trimesh or convex ballistic geometry")]
    fn the_authority_panics_on_an_unreadable_ballistic_shape() {
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::new(0.0, 2.0, 0.0));
        spawn_uncertifiable(&mut app);
        spawn_free_shell(&mut app, UNCERTIFIABLE_AT, Vec3::Z, 800.0, a_shot());
        app.update();
    }

    // --- Terminal confirms (first-crossing receipt) ---------------------------------------------

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

    /// Position is a law input: f32 spacing grows with distance, so near-origin fixtures alone
    /// cannot exercise combat-range tolerances.
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
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            impacts.len() >= 2,
            "the authority resolves the perforation and then the backstop embed (got {})",
            impacts.len()
        );
    }

    /// The game's cosmetic hull marks belong to every vehicle whose face the shot reaches.
    /// The shot's first-terminal receipt must not suppress a later vehicle's mark, even across
    /// ticks. Distinct fixture part names also exercise the recorder's data-driven part lookup.
    #[test]
    fn hull_marks_reach_a_second_vehicle_after_the_first_terminal() {
        use crate::damage::VolumeOf;
        use crate::hull_marks::{HullMarks, MarkKind};
        use crate::tank::SimParts;

        for distance in [-2.0, -30.0] {
            for (yaw, thickness, second_kind) in [
                (0.0, 0.5, MarkKind::Dent),
                (75f32.to_radians(), 0.1, MarkKind::Scratch),
            ] {
                // Fixture data: a thin first face, then a thick face, square or at a ricochet angle.
                let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.05), Vec3::new(0.0, 2.0, 0.0));
                crate::hull_marks::offline_plugin(&mut app);
                app.init_resource::<TerminalLog>()
                    .add_observer(capture_terminal);
                let first = app
                    .world_mut()
                    .query_filtered::<Entity, With<BallisticVolume>>()
                    .single(app.world())
                    .unwrap();
                let second = app
                    .world_mut()
                    .spawn((
                        Transform::from_xyz(0.0, 2.0, distance)
                            .with_rotation(Quat::from_rotation_y(yaw)),
                        RigidBody::Static,
                        Collider::cuboid(3.0, 3.0, thickness),
                        CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                        BallisticVolume {
                            material: walk::VolumeMaterial::fixture(STEEL),
                            substance: "RHA".to_string(),
                        },
                    ))
                    .id();
                let mut roots = Vec::new();
                for (part, name) in [(first, "Face_A"), (second, "Panel_Z")] {
                    let root = app
                        .world_mut()
                        .spawn((
                            Transform::IDENTITY,
                            Position(Vec3::ZERO),
                            Rotation::IDENTITY,
                            HullMarks::default(),
                            SimParts(std::collections::HashMap::from([(name.to_owned(), part)])),
                        ))
                        .id();
                    app.world_mut().entity_mut(part).insert((
                        Name::new(name),
                        VolumeOf(root),
                        ChildOf(root),
                    ));
                    roots.push(root);
                }
                settle(&mut app);
                spawn_headon_shell(&mut app, a_shot());
                for _ in 0..8 {
                    app.update();
                }
                assert_eq!(app.world().resource::<TerminalLog>().0.len(), 1);
                for (root, kind) in roots.into_iter().zip([MarkKind::Hole, second_kind]) {
                    let record = app.world().get::<HullMarks>(root).unwrap();
                    let marks: Vec<_> = record
                        .iter()
                        .map(|(_, mark)| (mark.part, mark.kind))
                        .collect();
                    assert_eq!(
                        marks,
                        [(0, kind)],
                        "each reached vehicle keeps its own face mark"
                    );
                }
            }
        }
    }

    /// A flat [`HeightGrid`] at `height` metres spanning the world square (raw meters, like the
    /// production resource).
    pub(super) fn flat_grid(height: f32) -> HeightGrid {
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
        assert_eq!(
            armor.surface,
            ImpactSurface::Material(crate::substances::SurfaceFeedback::fixture()),
            "the nearer plate wins"
        );
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

    // §13.1 end-to-end counterparts of the core's synthetic-hit laws: real trimeshes and live march.

    /// Production-style box trimesh and shell table: counter-clockwise faces from outside.
    /// The collector reads winding because parry flips normals against the ray, losing entry/exit
    /// orientation; inward winding would invert crossings. One closed box gives every face shell 0.
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

    /// The game's damage model: outward-wound trimesh plates of one substance, each a static body.
    pub(super) fn world_with_trimesh_plates(plates: &[(Vec3, Vec3)]) -> App {
        let mut app = world_with_plate(Vec3::splat(0.01), Vec3::new(0.0, -500.0, 0.0));
        for (size, at) in plates {
            let (collider, surfaces) = box_trimesh(*size);
            app.world_mut().spawn((
                Transform::from_translation(*at),
                RigidBody::Static,
                collider,
                surfaces,
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

    /// The game's damage model charges a union once; see §13.1 for the rejected serial resolver.
    /// Fixture capability ≈121 reference-mm (DERIVED): 160 mm union stops it though the first
    /// 70 mm plate cannot; 115 mm union passes it though the 140 mm sum cannot. Abutting totals
    /// straddle the same limit, catching a shared-face hop that skips the second plate.
    #[test]
    fn overlapping_trimesh_plates_charge_their_union_once() {
        // Fixture: 160 mm union tests undercharge.
        assert!(
            perforates(&[slab(0.07, 0.025)]),
            "the first plate alone does not defeat the round",
        );
        assert!(
            !perforates(&[slab(0.07, 0.025), slab(0.10, -0.05)]),
            "the overlapped second plate is charged, not crossed for free",
        );

        // Fixture: 115 mm union, 140 mm sum tests overcharge.
        assert!(
            perforates(&[slab(0.07, 0.0225), slab(0.07, -0.0225)]),
            "the shared 25 mm is charged ONCE; summing it would have defeated the round",
        );
        assert!(
            !perforates(&[slab(0.14, 0.0)]),
            "140 mm really is beyond it — the control that makes the line above mean something",
        );

        // Abutment. Totals of 115 and 160 mm straddle the capability.
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

    /// The game's damage model: rim-only engagement deflects less than full engagement (§13.5).
    /// An axis-only model misses this plate; deflection must scale with engagement.
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
    ///
    /// The same pair is fired again out where the map ends. Position is a LAW input: f32 spacing
    /// coarsens with distance, so at 2.4 km the transit handoff lands ~1.5e-4 m off the face it was
    /// computed from — four orders coarser than the 2.4e-8 two metres from the world origin, and the
    /// reason the prune margin scales instead of being a constant. It is also where the margin's
    /// CEILING starts to bind: the unclamped scaling term reaches the march's own boundary nudge at
    /// 2.5 km, and a margin that large re-collects the exit face of the plate just perforated.
    ///
    /// HONEST SCOPE of the far arm: it is an end-to-end guard, not the deterministic statement of
    /// either bound. Which side of the plane the handoff rounds to is a property of the coordinates,
    /// and a sweep of 480 far-map arrangements found it landing SHORT every time — so this arm would
    /// survive the margin being deleted. The bounds themselves are pinned in the collector:
    /// `a_face_the_origin_sits_a_hair_past_is_still_collected` walks the ULP ladder at this same
    /// distance, and `the_face_the_march_stepped_past_is_not_re_collected` holds the ceiling at
    /// 2.5 km.
    #[test]
    fn plates_across_a_900_mm_gap_are_two_crossings_and_both_perforate() {
        // (round origin, first plate's centre along -Z from it); the second sits 950 mm further.
        for (origin, first) in [
            (Vec3::new(0.0, 2.0, 2.0), -1.5),
            (Vec3::new(2400.0, 2.0, 2400.0), -2.0),
        ] {
            let plate = |z: f32| (Vec3::new(3.0, 3.0, 0.05), origin + Vec3::new(0.0, 0.0, z));
            let mut app = world_with_trimesh_plates(&[plate(first), plate(first - 0.95)]);
            app.init_resource::<TerminalLog>();
            app.add_observer(capture_terminal);
            spawn_headon_shell_at(&mut app, a_shot(), origin);
            for _ in 0..8 {
                app.update();
            }

            let impacts = app.world().resource::<ImpactLog>().0.clone();
            assert_eq!(
                impacts.len(),
                2,
                "two plates, two armour reads — the gap merges nothing and hides nothing \
                 (origin {origin}): {:?}",
                impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
            );
            let faces = [origin.z + first + 0.025, origin.z + first - 0.925];
            for (impact, face) in impacts.iter().zip(faces) {
                assert!(matches!(impact.surface, ImpactSurface::Material(_)));
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
            // No event in air: a fabricated weld merges both crossings; a fabricated surface
            // adds a third. Face reads keep the same 1 mm allowance as above.
            let air = faces[1] + 1.0e-3..=faces[0] - 0.05 - 1.0e-3;
            assert!(
                impacts
                    .iter()
                    .all(|impact| !air.contains(&impact.position.z)),
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
                (terminals[0].position.z - faces[0]).abs() < 1.0e-3,
                "the terminal reads at the FIRST plate's entry face, got z={}",
                terminals[0].position.z,
            );
        }
    }

    /// The game's damage model: seeds and resumed corridor share one anchor-relative point,
    /// [`MARCH_EPS`] past the perforation exit ([`resolve::Interior::seeds`]). Rebuilding that
    /// point in world f32 (2.44e-4 m spacing past 2048 m, DERIVED) can double-own a nearby entry
    /// (`UnexpectedEntry`) or leave it unowned (`UnexpectedExit`); either is a walk error.
    /// Near the origin, a resumed crossing also seeds presence at its own handoff origin
    /// ([`walk::ShellPresence::spans`]). Fixture: a thin plate followed by a non-welding tilted
    /// volume, its entry swept across the resumption plane below world spacing, near and far.
    /// A sub-disc round isolates the axis sample; every arrangement must resolve without error.
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
            // One world per origin: the plate stays, and each arrangement swaps in its own tilted
            // volume and re-spawns the round.
            let mut app = world_with_trimesh_plates(&[(plate, far + Vec3::new(0.0, 0.0, -1.0))]);
            let mut failed = Vec::new();
            for step in -20i32..=20 {
                // `ahead > 0`: the tilted volume's entry lies BEFORE the resumption plane, so the
                // seed owns it; `< 0`: after it, so the next corridor does.
                let ahead = step as f64 * 2.0e-5;
                let entry = resume + ahead;
                // Its +Z face, rotated by TILT about Y, crosses the axis `half / cos(TILT)` in front
                // of its centre.
                let half = ((entry - centre_z as f64) * (TILT as f64).to_radians().cos()) as f32;
                let volume = app
                    .world_mut()
                    .spawn((
                        Transform::from_translation(Vec3::new(far.x, far.y, centre_z))
                            .with_rotation(Quat::from_rotation_y(TILT.to_radians())),
                        RigidBody::Static,
                        box_trimesh(Vec3::new(tilted.x, tilted.y, half * 2.0)),
                        CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                        BallisticVolume {
                            material: walk::VolumeMaterial::fixture(STEEL * 0.01),
                            substance: "soft".to_string(),
                        },
                    ))
                    .id();
                for _ in 0..8 {
                    app.update();
                }
                let origin = far + Vec3::new(0.0, 0.0, 2.0);
                let shell = spawn_round(&mut app, body, origin, Vec3::NEG_Z * 800.0);
                for _ in 0..4 {
                    app.update();
                }
                let impacts = std::mem::take(&mut app.world_mut().resource_mut::<ImpactLog>().0);
                let _ = app.world_mut().try_despawn(shell);
                app.world_mut().despawn(volume);
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
                    matches!(impact.surface, ImpactSurface::Material(_))
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

    /// The game's damage model: a real first-contact event for surface-mark consumer tests.
    /// All construction, mass and speed choices below are fixture data; the core stays generic.
    pub(crate) fn first_contact_strike(
        flying: RoundBody,
        core: RoundBody,
        verdict: Verdict,
    ) -> ShellStrike {
        #[derive(Resource, Default)]
        struct Strikes(Vec<ShellStrike>);
        let thickness = if verdict == Verdict::Stopped {
            1.0
        } else if verdict == Verdict::Ricochet {
            0.4
        } else {
            0.05
        };
        let mut app =
            world_with_trimesh_plates(&[(Vec3::new(12.0, thickness, 3.0), Vec3::Y * 50.0)]);
        if verdict == Verdict::Ricochet {
            let plate = app
                .world_mut()
                .query::<(Entity, &Transform)>()
                .iter(app.world())
                .find(|(_, pose)| pose.translation.y == 50.0)
                .unwrap()
                .0;
            app.world_mut()
                .get_mut::<Transform>(plate)
                .unwrap()
                .rotation = Quat::from_rotation_z(75.0_f32.to_radians());
            settle(&mut app);
        }
        app.init_resource::<Strikes>().add_observer(
            |event: On<ShellStrike>, mut strikes: ResMut<Strikes>| {
                strikes.0.push(*event);
            },
        );
        let shell = spawn_round(&mut app, core, Vec3::Y * 51.0, Vec3::NEG_Y * 800.0);
        *app.world_mut().get_mut::<Projectile>(shell).unwrap() =
            fixture_round_with(flying, core, Vec3::NEG_Y * 800.0);
        app.world_mut()
            .entity_mut(shell)
            .insert(Shot(crate::ShotId {
                combatant: crate::CombatantId(7),
                weapon: 0,
                fire_tick: 100,
            }));
        app.update();
        let strike = *app
            .world()
            .resource::<Strikes>()
            .0
            .first()
            .expect("the fixture reaches its first face");
        assert_eq!(
            strike.verdict, verdict,
            "the fixture resolves its requested contact class"
        );
        strike
    }

    /// The game's damage model spends a tick's seconds at the speed each stretch actually has.
    /// The consumer is open flight after a perforation: allocating initial-speed metres there
    /// runs ahead. DERIVED counterexample: a zero-length change from 1000 to 200 m/s halfway
    /// through a 1/64 s tick runs ahead by (1000 - 200)/128 = 6.25 m. The fixture below includes
    /// the plate's finite force-law time as well. All dimensions and speeds are fixture data.
    #[test]
    fn a_perforation_ends_the_tick_where_its_clock_says() {
        use mechanism::Mechanism;
        let dt = 1.0 / 64.0;
        let velocity = Vec3::NEG_Y * 1000.0;
        let speed = freeflight_step(
            velocity,
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            dt,
        )
        .length();
        let thickness = (capability(FIXTURE_88, speed) - capability(FIXTURE_88, 200.0)) / STEEL;
        let centre = Vec3::Y * 50.0;
        let top = centre.y + thickness * 0.5;
        let approach = speed * dt * 0.5;
        let origin = Vec3::Y * (top + approach);
        let mut app = world_with_trimesh_plates(&[(Vec3::new(3.0, thickness, 3.0), centre)]);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            f64::from(dt),
        )));
        app.update();
        let shell = spawn_round(&mut app, FIXTURE_88, origin, velocity);
        app.update();
        let projectile = app.world().get::<Projectile>(shell).unwrap();
        let exit_speed = projectile.speed();
        assert!(
            (exit_speed - 200.0).abs() < 0.1,
            "the fixture slows to its chosen exit speed: {exit_speed}"
        );
        let material_time = projectile.round.flight_time(
            projectile.round.at(speed),
            projectile.round.at(exit_speed),
            f64::from(thickness),
        );
        let expected = origin.y
            - approach
            - thickness
            - exit_speed * (dt - approach / speed - material_time as f32);
        let actual = app.world().get::<Transform>(shell).unwrap().translation.y;
        let metre_budget_position = origin.y - speed * dt;
        assert!(
            (actual - expected).abs() < 1.0e-3,
            "clock expects y={expected}, actual y={actual}"
        );
        assert!(
            actual - metre_budget_position > 6.0,
            "the counterexample exposes metres allocated at the old speed"
        );
    }

    /// The game's damage model exports a bounce's work and momentum even without tank ownership.
    /// Environment construction readers rely on the same exchange as owned rigid bodies.
    /// Fixture: an inert round meets a broad plate at grazing incidence.
    #[test]
    fn a_ricochet_delivers_its_exchange_to_unowned_construction() {
        #[derive(Resource, Default)]
        struct Lines(Vec<DeliveredLine>);
        let mut app = world_with_trimesh_plates(&[(Vec3::new(100.0, 0.1, 100.0), Vec3::Y * 50.0)]);
        app.init_resource::<Lines>().add_observer(
            |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
                lines.0.extend_from_slice(&event.lines);
            },
        );
        let direction = Vec3::new(1.0, -0.1, 0.0).normalize();
        let shell = spawn_round(&mut app, FIXTURE_88, Vec3::Y * 50.3, direction * 800.0);
        app.update();
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        let ArmourMark::Bounce {
            incoming,
            outgoing,
            speed_in,
            speed_out,
            shock,
            struck,
            position,
            ..
        } = marks.log[0]
        else {
            panic!("the grazing fixture must bounce");
        };
        let lines: Vec<_> = app
            .world()
            .resource::<Lines>()
            .0
            .iter()
            .filter(|line| {
                line.crossing.volume == struck
                    && line.crossing.entry == position
                    && line.crossing.end == position
            })
            .collect();
        assert_eq!(lines.len(), 1, "the bounce must publish one exchange");
        let expected = FIXTURE_88.mass * (incoming * speed_in - outgoing * speed_out);
        assert!((lines[0].momentum - expected).length() < expected.length() * 1e-5);
        assert_eq!(lines[0].crossing.volume, struck);
        assert_eq!(lines[0].crossing.entry, position);
        assert_eq!(lines[0].crossing.end, position);
        assert_eq!(lines[0].crossing.energy, shock);
        assert_eq!(lines[0].entrance_energy, shock);
        assert_eq!(lines[0].entrance_momentum, lines[0].momentum);
    }

    /// The game's damage-model inspector relies on the real throw retaining its emitting solids.
    /// Fixture: one perforated plate; replacing source identity with an empty list fails.
    #[test]
    fn a_live_exit_cone_retains_its_emitting_volume() {
        let mut app = world_with_plate(Vec3::new(3.0, 0.05, 3.0), Vec3::Y * 50.0);
        let plate = app
            .world_mut()
            .query_filtered::<Entity, With<BallisticVolume>>()
            .single(app.world())
            .unwrap();
        let shell = spawn_round(&mut app, FIXTURE_88, Vec3::Y * 51.0, Vec3::NEG_Y * 800.0);
        app.update();
        let cones = &app.world().get::<SpallMarks>(shell).unwrap().cones;
        assert!(
            !cones.is_empty(),
            "the fixture must emit a field through real playback"
        );
        assert!(cones.iter().all(|cone| cone.exited.contains(&plate)));
    }

    /// The game's damage model must deliver the whole resolved momentum exchange independently
    /// of its HP dose. Constructions and freed trunks read these lines. Fixture data: head-on and
    /// oblique perforations of fixture plates, overlapping or spaced on one body or two; entry
    /// and final exit states supply the independent whole-chain budget.
    #[test]
    fn a_perforation_delivers_its_whole_exchange_independently_of_hp() {
        #[derive(Resource, Default)]
        struct Lines(Vec<DeliveredLine>);
        let oblique = Vec3::new(0.4, -1.0, 0.0).normalize();
        for (direction, overlapped, spaced, shared_body) in [
            (Vec3::NEG_Y, false, false, false),
            (oblique, false, false, false),
            (oblique, true, false, false),
            (Vec3::NEG_Y, false, true, false),
            (Vec3::NEG_Y, false, true, true),
            (oblique, false, true, false),
            (oblique, false, true, true),
        ] {
            let plate = (Vec3::new(3.0, 0.05, 3.0), Vec3::Y * 50.0);
            let plates = if spaced {
                vec![plate, (plate.0, plate.1 - Vec3::Y * 0.4)]
            } else if overlapped {
                vec![plate, plate]
            } else {
                vec![plate]
            };
            let mut app = world_with_trimesh_plates(&plates);
            if shared_body {
                let body = app
                    .world_mut()
                    .spawn((RigidBody::Static, Transform::default()))
                    .id();
                let parts: Vec<_> = app
                    .world_mut()
                    .query::<(Entity, &Transform)>()
                    .iter(app.world())
                    .filter(|(_, pose)| pose.translation.y > 40.0)
                    .map(|(entity, _)| entity)
                    .collect();
                for part in parts {
                    app.world_mut()
                        .entity_mut(part)
                        .remove::<RigidBody>()
                        .insert(ChildOf(body));
                }
                settle(&mut app);
            }
            if overlapped {
                // Fixture: the first cofacial volume is wholly hidden by the stronger one.
                let weak = app
                    .world_mut()
                    .query_filtered::<Entity, With<BallisticVolume>>()
                    .iter(app.world())
                    .min()
                    .unwrap();
                app.world_mut()
                    .get_mut::<BallisticVolume>(weak)
                    .unwrap()
                    .material = walk::VolumeMaterial::fixture(1.0);
            }
            app.init_resource::<Lines>().add_observer(
                |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
                    lines.0.extend_from_slice(&event.lines);
                },
            );
            let shell = spawn_round(&mut app, FIXTURE_88, Vec3::Y * 51.0, direction * 800.0);
            // Fixture: the carrier has three times the core mass.
            let flying_mass = 4.0 * FIXTURE_88.mass;
            {
                let mut projectile = app.world_mut().get_mut::<Projectile>(shell).unwrap();
                projectile.flying.mass = flying_mass;
                projectile.flying.drag_k = 0.0;
            }
            app.update();
            let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
            let (speed_in, incoming) = match &marks.log[0] {
                ArmourMark::Crossing {
                    speed_in, incoming, ..
                } => (*speed_in, *incoming),
                _ => panic!("the fixture enters material"),
            };
            let speed = match marks.log.last().unwrap() {
                ArmourMark::Crossing {
                    end: CrossingEnd::Exit { speed, .. },
                    ..
                } => *speed,
                _ => panic!("the fixture exits its final plate: {:?}", marks.log),
            };
            assert_eq!(marks.log.len(), if spaced { 2 } else { 1 });
            let lost = 0.5
                * (f64::from(flying_mass) * f64::from(speed_in).powi(2)
                    - f64::from(FIXTURE_88.mass) * f64::from(speed).powi(2));
            let lines = &app.world().resource::<Lines>().0;
            let axis = (lines.last().unwrap().crossing.end - lines.last().unwrap().crossing.entry)
                .normalize();
            let expected = flying_mass * incoming * speed_in - FIXTURE_88.mass * axis * speed;
            let retained: f64 = lines
                .iter()
                .map(|line| f64::from(line.crossing.energy))
                .sum();
            let carried: f64 = app
                .world()
                .get::<SpallMarks>(shell)
                .unwrap()
                .cones
                .iter()
                .map(|cone| f64::from(cone.strength * spall_cone::EMITTED_SHARE))
                .sum();
            let momentum: Vec3 = lines.iter().map(|line| line.momentum).sum();
            eprintln!(
                "MEASURED fixture perforation (spaced={spaced}, shared_body={shared_body}): lost={lost:.6} J, retained={retained:.6} J, debris={carried:.6} J; momentum={momentum:?} N s, expected={expected:?} N s"
            );
            assert_eq!(
                app.world().get::<Projectile>(shell).unwrap().flying.mass,
                FIXTURE_88.mass
            );
            assert!(((retained + carried) / lost - 1.0).abs() < 1.0e-5);
            assert!((momentum - expected).length() / expected.length() < 1.0e-4);
        }
    }

    /// The game's damage model leaves only the core after the first resisting contact. Hull
    /// impulse consumers rely on a later bounce in the same frozen chain reading that mass.
    /// Fixture: a thin entrance plate and a thick face at 75 degrees on one inertial body.
    #[test]
    fn a_chained_bounce_transfers_only_the_core_momentum() {
        let mut app = world_with_trimesh_plates(&[]);
        let mass = 50_000.0;
        let body = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Transform::default(),
                Mass(mass),
                AngularInertia::new(Vec3::splat(1.0e5)),
                NoAutoMass,
                NoAutoAngularInertia,
                GravityScale(0.0),
            ))
            .id();
        for (size, pose) in [
            (
                Vec3::new(3.0, 0.05, 3.0),
                Transform::from_translation(Vec3::Y * 50.0),
            ),
            (
                Vec3::new(12.0, 0.4, 3.0),
                Transform::from_translation(Vec3::Y * 45.0)
                    .with_rotation(Quat::from_rotation_z(75.0_f32.to_radians())),
            ),
        ] {
            let (collider, surfaces) = box_trimesh(size);
            app.world_mut().spawn((
                pose,
                ChildOf(body),
                collider,
                surfaces,
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                VolumeOf(body),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "fixture".into(),
                },
            ));
        }
        settle(&mut app);
        let shell = spawn_round(&mut app, FIXTURE_88, Vec3::Y * 51.0, Vec3::NEG_Y * 800.0);
        let flying_mass = 4.0 * FIXTURE_88.mass;
        {
            let mut projectile = app.world_mut().get_mut::<Projectile>(shell).unwrap();
            projectile.flying.mass = flying_mass;
            projectile.flying.drag_k = 0.0;
        }
        app.update();
        let marks = &app.world().get::<PenetrationMarks>(shell).unwrap().log;
        let [
            ArmourMark::Crossing {
                incoming, speed_in, ..
            },
            ArmourMark::Bounce {
                outgoing,
                speed_out,
                ..
            },
        ] = marks.as_slice()
        else {
            panic!("the fixture perforates and then bounces: {marks:?}");
        };
        let expected =
            flying_mass * *incoming * *speed_in - FIXTURE_88.mass * *outgoing * *speed_out;
        let actual = app.world().get::<LinearVelocity>(body).unwrap().0 * mass;
        assert!(
            (actual - expected).length() / expected.length() < 1e-4,
            "the frozen chain transfers {actual:?} N s, expected {expected:?}"
        );
    }

    /// The game's damage model routes each crossed volume's momentum once. A hull sharing a
    /// crossing with unowned matter receives only its line, regardless of entrance ownership.
    /// Fixture: two identical overlapping plates and an inertial body with no external forces.
    #[test]
    fn each_crossed_body_receives_only_its_own_line() {
        #[derive(Resource, Default)]
        struct Lines(Vec<DeliveredLine>);
        for owned_index in [0, 1] {
            let plate = (Vec3::new(3.0, 0.05, 3.0), Vec3::Y * 50.0);
            let mut app = world_with_trimesh_plates(&[plate, plate]);
            app.init_resource::<Lines>().add_observer(
                |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
                    lines.0.extend_from_slice(&event.lines);
                },
            );
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
            let mut plates: Vec<_> = app
                .world_mut()
                .query_filtered::<Entity, With<BallisticVolume>>()
                .iter(app.world())
                .collect();
            plates.sort();
            let owned = plates[owned_index];
            app.world_mut().entity_mut(owned).insert(VolumeOf(body));
            app.update();
            spawn_round(&mut app, FIXTURE_88, Vec3::Y * 51.0, Vec3::NEG_Y * 800.0);
            app.update();
            let lines = &app.world().resource::<Lines>().0;
            assert_eq!(lines.len(), 2);
            let expected = lines
                .iter()
                .find(|line| line.crossing.volume == owned)
                .unwrap()
                .momentum;
            let delivered = app.world().get::<LinearVelocity>(body).unwrap().0 * 100.0;
            assert!(expected.length() > 0.0);
            assert!(
                (delivered - expected).length() / expected.length() < 0.001,
                "owned index {owned_index}: delivered {delivered:?}, own line {expected:?}"
            );
        }
    }

    /// The game's damage model decides a crossing once. The path and addressed dose survive
    /// both a different tick partition and motion of the original geometry after discovery.
    /// The runtime and destruction consumers rely on effects arriving at their path moments,
    /// with integer conversion performed once. All dimensions, rates and speeds are fixture data.
    #[test]
    fn immutable_playback_preserves_path_and_dose_across_tick_partitions() {
        #[derive(Resource, Default)]
        struct Lines(Vec<DeliveredLine>);
        fn begin() -> (App, Entity, Entity) {
            let mut app = world_with_trimesh_plates(&[]);
            app.init_resource::<Lines>().add_observer(
                |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
                    lines.0.extend_from_slice(&event.lines);
                },
            );
            let plate = app
                .world_mut()
                .spawn((
                    Transform::from_translation(Vec3::Y * 50.0),
                    RigidBody::Static,
                    box_trimesh(Vec3::new(3.0, 20.0, 3.0)),
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material: walk::VolumeMaterial::fixture(0.1),
                        substance: "fixture".into(),
                    },
                    ComponentHealth::full(1000),
                ))
                .id();
            settle(&mut app);
            let shell = spawn_round(&mut app, FIXTURE_88, Vec3::Y * 60.01, Vec3::NEG_Y * 200.0);
            app.update();
            assert!(
                !app.world()
                    .get::<Projectile>(shell)
                    .unwrap()
                    .playback
                    .is_empty()
            );
            assert_eq!(
                app.world().get::<ComponentHealth>(plate).unwrap().held(),
                ComponentHealth::full(1000).held(),
                "the line's end moment has not arrived"
            );
            assert!(
                app.world().resource::<Lines>().0.is_empty(),
                "fracture receives no future line at the entrance mark"
            );
            (app, shell, plate)
        }
        fn advance(app: &mut App, seconds: f64) {
            app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
                seconds,
            )));
            app.update();
        }
        let (mut coarse, a, plate_a) = begin();
        let (mut fine, b, plate_b) = begin();
        for (app, plate, x) in [(&mut coarse, plate_a, 100.0), (&mut fine, plate_b, 200.0)] {
            let at = Vec3::new(x, 50.0, 0.0);
            app.world_mut()
                .entity_mut(plate)
                .insert((Transform::from_translation(at), Position(at)));
        }
        for _ in 0..2 {
            advance(&mut coarse, 0.02);
        }
        for _ in 0..8 {
            advance(&mut fine, 0.005);
        }
        let at = |app: &App, shell| app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            at(&coarse, a).distance(at(&fine, b)) < 1.0e-4,
            "equal elapsed time reaches the same point inside the immutable path"
        );
        assert_eq!(
            coarse
                .world()
                .get::<ComponentHealth>(plate_a)
                .unwrap()
                .held(),
            ComponentHealth::full(1000).held(),
            "the later line dose is still pending"
        );
        assert!(coarse.world().resource::<Lines>().0.is_empty());
        assert!(fine.world().resource::<Lines>().0.is_empty());
        advance(&mut coarse, 0.16);
        advance(&mut fine, 0.16);
        assert!(
            coarse
                .world()
                .get::<Projectile>(a)
                .unwrap()
                .playback
                .is_empty()
        );
        assert!(
            fine.world()
                .get::<Projectile>(b)
                .unwrap()
                .playback
                .is_empty()
        );
        assert!(
            at(&coarse, a).distance(at(&fine, b)) < 1.0e-4,
            "the path ends at the same world point"
        );
        let lines_a = &coarse.world().resource::<Lines>().0;
        let lines_b = &fine.world().resource::<Lines>().0;
        assert!(
            !lines_a.is_empty(),
            "fracture receives the line when its dose arrives"
        );
        assert_eq!(lines_a.len(), 1, "the fixture has one decided volume line");
        assert_eq!(
            lines_a.iter().map(|line| line.momentum).sum::<Vec3>(),
            lines_b.iter().map(|line| line.momentum).sum::<Vec3>(),
            "the delivered impulse share is partition independent"
        );
        assert_eq!(lines_a.len(), lines_b.len());
        assert_eq!(
            lines_a.iter().map(|line| line.crossing.energy).sum::<f32>(),
            lines_b.iter().map(|line| line.crossing.energy).sum::<f32>()
        );
        let units = |app: &App, shell| {
            app.world()
                .get::<ShotDose>(shell)
                .unwrap()
                .tallies
                .iter()
                .map(|tally| tally.units)
                .sum::<u64>()
        };
        assert_eq!(
            units(&coarse, a),
            units(&fine, b),
            "tick partition cannot re-round a decided dose"
        );
        assert!(
            units(&coarse, a) > 0,
            "the fixture produces a measurable integer dose"
        );
        assert_eq!(
            coarse
                .world()
                .get::<ComponentHealth>(plate_a)
                .unwrap()
                .held(),
            fine.world().get::<ComponentHealth>(plate_b).unwrap().held()
        );
        assert!(
            coarse
                .world()
                .get::<ComponentHealth>(plate_a)
                .unwrap()
                .held()
                < ComponentHealth::full(1000).held(),
            "the addressed dose arrives even after its geometry moved"
        );
    }

    /// The game's damage model closes held material in discovery beyond this tick's travel;
    /// playback retains the immutable continuation until its path moment. Fixture: tick travel
    /// ends half a nudge past a plate exit, inside a tilted volume whose entry precedes the
    /// resumption plane. The seed owns it even though the tick ends before its exit. Discarding
    /// that presence would leave an unowned exit (`UnexpectedExit`) and panic ([`next_surface`]).
    #[test]
    fn a_held_samples_drain_closes_its_material_past_the_travel() {
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
        let stepped = freeflight_step(velocity, fixture_drag_k(body.caliber, body.mass), DT);
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
            .starts
            .as_ref()
            .map(resolve::Interior::world_origin)
            .expect("an open-air flight carries its axis start across the tick");
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

    /// The game's damage model: native flight and [`fast_forward_shell`] agree within one world
    /// rounding over equal steps, at any map coordinate. Per-step world rounding would drift;
    /// both carry anchor plus offset.
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
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
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

    /// The game's damage model: a skid's exchange reaches the matter it struck. A construction or trunk
    /// reads only `ShellMarched::lines`, so a skid that published none would spend its work and
    /// momentum on nothing. Plate, speed and the 75° graze are fixture data. Mutant: no line for a
    /// skid.
    #[test]
    fn a_skid_delivers_its_exchange_as_a_line_on_the_face() {
        #[derive(Resource, Default)]
        struct Lines(Vec<DeliveredLine>);
        let graze = Vec3::new(
            75.0_f32.to_radians().sin(),
            0.0,
            -75.0_f32.to_radians().cos(),
        );
        let mut app = world_with_plate(Vec3::new(3.0, 3.0, 0.10), Vec3::Y * 2.0);
        app.init_resource::<Lines>().add_observer(
            |event: On<ShellMarched>, mut lines: ResMut<Lines>| {
                lines.0.extend_from_slice(&event.lines);
            },
        );
        let shell = spawn_round(
            &mut app,
            FIXTURE_88,
            Vec3::new(-1.0, 2.0, 0.6),
            graze * 800.0,
        );
        app.update();
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        let ArmourMark::Bounce {
            position,
            incoming,
            outgoing,
            speed_in,
            speed_out,
            struck,
            shock,
            ..
        } = marks.log[0]
        else {
            panic!("the fixture skids: {:?}", marks.log);
        };
        let lines = &app.world().resource::<Lines>().0;
        assert_eq!(lines.len(), 1, "one contact, one line");
        let line = &lines[0];
        assert_eq!(line.crossing.volume, struck);
        assert_eq!(
            (line.crossing.entry, line.crossing.end),
            (position, position)
        );
        assert!(shock > 0.0, "the fixture skid does work");
        assert_eq!(line.crossing.energy, shock);
        assert_eq!(line.entrance_energy, shock);
        let exchanged = FIXTURE_88.mass * (incoming * speed_in - outgoing * speed_out);
        assert!(
            line.momentum.distance(exchanged) < exchanged.length() * 1.0e-5,
            "the line carries the skid's impulse: {} of {exchanged}",
            line.momentum
        );
        assert_eq!(line.entrance_momentum, line.momentum);
    }

    /// An 88 grazing a thick plate at 75°, and a second plate square to the grazing ray's mirror,
    /// its face offset one calibre radius from the struck plane and 10 mm along that mirror,
    /// extending from 40 mm below the mirror's line to 300 mm above it: the impacts the round made.
    /// FIXTURE DATA.
    fn graze_and_plate() -> Vec<Captured> {
        let incidence = 75.0_f32.to_radians();
        let incoming = Vec3::new(incidence.sin(), 0.0, -incidence.cos());
        let normal = Vec3::Z;
        let mirror = incoming - 2.0 * incoming.dot(normal) * normal;
        let first = (Vec3::new(3.0, 3.0, 0.1), Vec3::new(0.0, 2.0, 0.0));
        let bounce = Vec3::new(0.0, 2.0, 0.05);
        let radius = FIXTURE_88.caliber * 0.5;
        let layout_anchor = bounce + normal * radius;
        // The second plate's frame: local +Z faces back up the mirror, local +Y leans away from the
        // first plate.
        let back = -mirror;
        let up = (normal - mirror * normal.dot(mirror)).normalize();
        let side = up.cross(back);
        let rotation = Quat::from_mat3(&Mat3::from_cols(side, up, back));
        let thickness = 0.05;
        let (low, high) = (-0.040, 0.300);
        let centre = layout_anchor + mirror * (0.010 + thickness * 0.5) + up * ((low + high) * 0.5);
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
        app.world().resource::<ImpactLog>().0.clone()
    }

    /// The game's damage model admits a round's source on the starts its step resumes from.
    /// Fixture: tanks pressed together, the round perforating a foreign plate at its own muzzle at
    /// 60 degrees, leave its starts staggered along the travel by up to `r tan 60°` = 76 mm
    /// (DERIVED). Its trailing start can stand in its source's box while the square disc at the
    /// axis lies clear of it; a clearance reading that square disc turns the source solid under the
    /// trailing start, whose corridor then meets an exit it never entered. Positions are fixture data.
    #[test]
    fn launch_clearance_reads_the_starts_launch_resumes() {
        let radius = resolve::disc_radius(FIXTURE_88.caliber);
        let travel = Vec3::X;
        let mut projectile = fixture_round(FIXTURE_88, travel * 700.0);
        let position = Vec3::new(10.0, 2.0, -3.0);
        let tilt = 60.0_f32.to_radians().tan();
        let square = resolve::layout(&projectile.disc, radius);
        let staggered: Vec<Vec3> = square
            .iter()
            .map(|&lateral| lateral + travel * (lateral.dot(Vec3::Y) * tilt))
            .collect();
        let fresh = launch_starts(&projectile, position, radius);
        projectile.starts = Some(resolve::Interior::laid(position, staggered));
        let carried = launch_starts(&projectile, position, radius);
        let frame = projectile.disc;
        let starts = projectile
            .starts
            .take()
            .expect("the fixture carries its starts");
        let dir = Dir3::new(travel).expect("a direction");
        let flight = resolve::Flight::launched(dir, dir, 700.0, frame, starts);
        let resumed = flight.sample_starts(radius);
        assert_eq!(carried.len(), resumed.len());
        for (admitted, swept) in carried.iter().zip(&resumed) {
            assert!(
                admitted.distance(*swept) < 1.0e-6,
                "clearance admits {admitted} where launch resumes {swept}"
            );
        }
        let trailing = carried
            .iter()
            .map(|start| (*start - position).dot(travel))
            .fold(f32::INFINITY, f32::min);
        assert!(
            trailing < -0.05
                && fresh
                    .iter()
                    .all(|start| (*start - position).dot(travel).abs() < 1.0e-6),
            "the staggered starts trail the square disc by {trailing} m"
        );
    }

    /// A GRAZE AND A PLATE THE DISC MEETS WITHIN THE GRAZE'S DEPTH ARE ONE CROSSING, AND IT BITES.
    ///
    /// [`graze_and_plate`]: the second plate reaches into the incoming disc's path, so the disc's
    /// upper samples meet its face before they would meet the grazed plate's, inside the graze's
    /// depth, and both pieces are one crossing (design doc §13.5, the hand-off) with the entrance
    /// reading both surfaces. The second face, square to the mirror, tilts the read's normal toward
    /// the round, so the graze that ricochets alone bites:
    /// one entry, its normal leaning toward the second face, and no bounce or walk contradiction.
    #[test]
    fn a_graze_and_a_plate_struck_together_bite_as_one_crossing() {
        let impacts = graze_and_plate();
        let read: Vec<(Vec3, Verdict)> = impacts.iter().map(|i| (i.position, i.verdict)).collect();
        assert!(
            impacts
                .first()
                .is_some_and(|impact| impact.verdict.entered()),
            "the round bit at first contact: {read:?}",
        );
        // The grazed face's normal is +Z and the second face's leans back along -X, toward the round.
        assert!(
            impacts[0].normal.x < -0.01,
            "the entry reads the second plate's face as well as the grazed one: {:?}",
            impacts[0].normal,
        );
        assert!(
            impacts
                .iter()
                .all(|impact| impact.verdict != Verdict::Ricochet),
            "nothing bounced: {read:?}",
        );
    }

    /// Fly `shell`, launched within one march nudge of a face, for eight steps and assert it
    /// reported the face: impacts, all on armour, the first an entry, and exactly one terminal. The
    /// logs are emptied and the round despawned after, so the world takes the next launch clean.
    fn assert_face_reported(app: &mut App, shell: Entity, at: &str) {
        for _ in 0..8 {
            app.update();
        }
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert!(
            !impacts.is_empty(),
            "the round met the plate {at} and something was reported for it — a silent \
             pass-through is free armour",
        );
        assert!(
            impacts
                .iter()
                .all(|impact| matches!(impact.surface, ImpactSurface::Material(_))),
            "and what was reported is the plate, not the ground beyond it: {impacts:?}",
            impacts = impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );
        // With the face collected the crossing resolves honestly: an 88 through 50 mm.
        assert!(
            impacts[0].verdict.entered(),
            "{at} the crossing resolved: {impacts:?}",
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
        app.world_mut().resource_mut::<ImpactLog>().0.clear();
        app.world_mut().resource_mut::<TerminalLog>().0.clear();
        let _ = app.world_mut().try_despawn(shell);
    }

    /// The game's damage model starts each sweep where membership is known. A dropped transit
    /// entry gives `UnexpectedExit` and panics; a dropped entrance silently gives [`walk::Begin::Miss`]
    /// and free passage with no dose or `Impact` — no log trace. A boundary nudge skips faces
    /// within [`MARCH_EPS`]; re-laying a gravity-bent disc advances samples by up to `r·sin θ`
    /// (DERIVED 9 µm for an 88 at 800 m/s after a 16 ms bend), putting them inside unowned material.
    /// Fixture level launches span a few ULP to half a nudge, and one nudge ± 4 ULP, from the face.
    /// Vertical launches at one nudge ± 4 ULP keep the disc flush: gravity cannot tilt travel,
    /// so every sample meets at the same `t`. All crossings must resolve. One unchanged plate per
    /// direction; only the round respawns.
    #[test]
    fn a_round_launched_just_off_a_face_never_starts_a_sample_inside_it() {
        let face = 0.525_f32;
        let off = |clearance: f32, step: i32| {
            f32::from_bits((face + clearance).to_bits().wrapping_add_signed(step))
        };

        let mut level = world_with_trimesh_plates(&[slab(0.05, 0.5)]);
        level.init_resource::<TerminalLog>();
        level.add_observer(capture_terminal);
        for (clearance, steps) in [
            (0.0, 0..=2),
            (1.0e-6, 0..=2),
            (4.0e-6, 0..=2),
            (8.0e-6, 0..=2),
            (2.0e-5, 0..=2),
            (0.5 * MARCH_EPS, 0..=2),
            (MARCH_EPS, -4..=4),
        ] {
            for step in steps {
                let at = Vec3::new(0.0, 2.0, off(clearance, step));
                let shell = spawn_headon_shell_at(&mut level, a_shot(), at);
                assert_face_reported(
                    &mut level,
                    shell,
                    &format!("level, {clearance:e} m + {step} ULP off it"),
                );
            }
        }

        let mut falling =
            world_with_trimesh_plates(&[(Vec3::new(3.0, 0.05, 3.0), Vec3::new(0.0, 0.5, 0.0))]);
        falling.init_resource::<TerminalLog>();
        falling.add_observer(capture_terminal);
        for step in -4i32..=4 {
            let shell = spawn_round(
                &mut falling,
                FIXTURE_88,
                Vec3::new(0.1, off(MARCH_EPS, step), -0.2),
                Vec3::NEG_Y * 800.0,
            );
            falling.world_mut().entity_mut(shell).insert(Shot(a_shot()));
            assert_face_reported(
                &mut falling,
                shell,
                &format!("falling, one nudge + {step} ULP off it"),
            );
        }
    }

    /// A step's flight launched in open air: [`resolve::Flight::launched`] on an axis start at
    /// `position` with no rounding carried, in a world with no armour near.
    fn launched_in_open_air(
        position: Vec3,
        swept: Dir3,
        stepped: Vec3,
        frame: walk::DiscFrame,
    ) -> resolve::Flight {
        resolve::Flight::launched(
            swept,
            Dir3::new(stepped).expect("a direction"),
            stepped.length(),
            frame,
            resolve::Interior::laid(
                position,
                resolve::layout(&frame, resolve::disc_radius(FIXTURE_88.caliber)),
            ),
        )
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
        let stepped = freeflight_step(
            velocity,
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            DT,
        );
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
        let stepped = freeflight_step(
            velocity,
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            DT,
        );
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

    /// The game's damage model carries an immutable oblique crossing through any tick boundary.
    /// Every stand-off reads the plate once; playback must not lay a new disc inside it. Angles,
    /// dimensions, speeds and stand-off sampling below are fixture data.
    #[test]
    fn an_oblique_perforation_that_ends_a_tick_reads_its_plate_once() {
        const DT: f32 = 0.016;
        const STANDOFFS: usize = 200;
        let incidence = 60.0_f32.to_radians();
        let dir = Vec3::new(incidence.sin(), 0.0, -incidence.cos());
        let velocity = dir * 800.0;
        let stepped = freeflight_step(
            velocity,
            fixture_drag_k(FIXTURE_88.caliber, FIXTURE_88.mass),
            DT,
        );
        let budget = stepped.length() * DT;
        let face = Vec3::new(0.0, 2.0, 0.51);
        // The plate never changes, so each stand-off re-spawns only the round.
        let (size, centre) = slab(0.02, 0.5);
        let mut app = world_with_trimesh_plates(&[(size, centre)]);
        let mut misread = Vec::new();
        for index in 0..STANDOFFS {
            // The face lies this far short of the tick's travel: the crossing ends the tick for
            // stand-offs whose perforation spends what is left of it.
            let short = 0.02 - 0.04 * index as f32 / STANDOFFS as f32;
            let shell = spawn_round(
                &mut app,
                FIXTURE_88,
                face - dir * (budget - short),
                velocity,
            );
            for _ in 0..3 {
                app.update();
            }
            let impacts = std::mem::take(&mut app.world_mut().resource_mut::<ImpactLog>().0);
            let reads: Vec<Verdict> = impacts
                .iter()
                .filter(|impact| matches!(impact.surface, ImpactSurface::Material(_)))
                .map(|impact| impact.verdict)
                .collect();
            if reads.len() != 1 {
                misread.push((short, reads));
            }
            let _ = app.world_mut().try_despawn(shell);
        }
        assert!(
            misread.is_empty(),
            "{} of {STANDOFFS} stand-offs did not read the plate exactly once, with the face this \
             far short of the tick's travel: {misread:?}",
            misread.len(),
        );
    }

    /// The game's damage model charges both the incoming approach and the bent transit in
    /// seconds. The axis hand-off can trail the nearest sample, so charging only first contact
    /// grants free approach time. The observer is open flight after the perforation. All speeds,
    /// angles and dimensions below are fixture data.
    #[test]
    fn a_crossing_charges_the_whole_path_it_flew() {
        let start = Vec3::new(0.0, 2.0, 2.0);
        let dt = 0.016_f32;
        // The budget the march works from: this tick's velocity after gravity and drag, times dt.
        let stepped = freeflight_step(Vec3::NEG_Z * 800.0, fixture_drag_k(0.088, 10.2), dt);
        let incoming_speed = stepped.length();

        for degrees in [0.0_f32, 40.0, 60.0] {
            let mut app = world_with_trimesh_plates(&[]);
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(0.0, 2.0, 0.0))
                    .with_rotation(Quat::from_rotation_y(degrees.to_radians())),
                RigidBody::Static,
                // Fixture: each tested angle bites, isolating the path-distance accounting.
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
            use mechanism::Mechanism;
            let projectile = app.world().get::<Projectile>(shell).unwrap();
            let outgoing_speed = projectile.speed();
            let material_seconds = projectile.round.flight_time(
                projectile.round.at(incoming_speed),
                projectile.round.at(outgoing_speed),
                f64::from(through),
            );
            let flown_seconds =
                f64::from(approach / incoming_speed + onward / outgoing_speed) + material_seconds;
            assert!(
                (flown_seconds - f64::from(dt)).abs() < 2.0e-6,
                "{degrees} degrees: path spent {flown_seconds} s, tick has {dt} s"
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

    /// The game's damage model: a `Miss` flies the examined corridor, not just to first contact.
    /// Otherwise repeated casts creep by boundary nudges, spending a tick's roughly twelve-metre
    /// flight within millimetres of the plate. Fixture: zero-factor `BallisticVolume` geometry
    /// guarantees a cast hit but no positive union field, avoiding a geometric knife-edge.
    /// It must emit no impact and land at the free-flight endpoint.
    #[test]
    fn a_miss_flies_the_whole_corridor_it_examined() {
        let start = Vec3::new(0.0, 2.0, 2.0);
        let dt = 0.016_f32;
        let (free_flight, _, _) = fast_forward_shell(
            start,
            Vec3::NEG_Z * 800.0,
            fixture_drag_k(0.088, 10.2),
            dt,
            1,
        );

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
        // A direct creep witness complements the free-flight comparison.
        assert!(
            end.z < -10.0,
            "the round did not creep at the surface it examined: z = {}",
            end.z,
        );
    }

    /// The game's damage model: MEASURED counterexample at `(2499.9, 924.963, 1524.939)`, 38°
    /// incidence. Independent world-f32 rounding (0.24 mm spacing) puts a handoff 0.3848 mm past
    /// its plane, beyond the prune margin's 0.25 mm ceiling, pruning an entry inside the playable
    /// envelope and leaving an unmatched exit. No margin can satisfy both bounds. Anchoring takes
    /// world-scale subtraction once and keeps handoff and face offsets on one origin, so error
    /// stays corridor-scale.
    #[test]
    fn a_crossing_resolves_at_the_far_corner_of_the_map() {
        let at = Vec3::new(2499.9, 924.963, 1524.939);
        let mut app = world_with_trimesh_plates(&[]);
        // Two spaced plates exercise re-anchoring after the first crossing bends the round,
        // catching compounded handoff error.
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
            "two plates, each with its own resolved read: {:?}",
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

    /// The game's damage model: every occupied convex volume must be readable by flight,
    /// corridor, field and aim queries at its current pose. Callers supply isolated fixture matter
    /// whose AABB centre is inside its solid, so the test ray crosses it independently of a view.
    pub(crate) fn assert_occupied_matter_queries(app: &mut App, volume: Entity) {
        use bevy::ecs::system::RunSystemOnce;
        app.world_mut()
            .run_system_once(move |world: ProjectileMarchWorld| {
                let (position, rotation, collider, _) = world.geometry.get(volume).unwrap();
                let bounds = collider.aabb(position.0, *rotation);
                let centre = (bounds.min + bounds.max) * 0.5;
                // Fixture query margin, m: start outside the convex solid and finish beyond it.
                let margin = 1.0;
                let origin = centre + Vec3::Z * ((bounds.max.z - bounds.min.z) * 0.5 + margin);
                let length = bounds.max.z - bounds.min.z + 2.0 * margin;
                let admits = |entity| entity == volume;
                let hit =
                    cast_disc_segment(&world, &[Vec3::ZERO], origin, Dir3::NEG_Z, length, &admits);
                assert!(
                    matches!(hit, Some(SegmentHit::Armor(hit)) if hit.entity == volume),
                    "flight must meet occupied matter"
                );
                let context = resolve::ResolveContext {
                    world: &world,
                    colliders: &world.geometry,
                    laws: walk::WalkLaws::default(),
                };
                let corridor = resolve::build_corridor(
                    &context,
                    &admits,
                    origin,
                    Vec3::ZERO,
                    Vec3::NEG_Z,
                    length,
                    walk::DiscFrame::anchored(Vec3::NEG_Z).unwrap(),
                    0.0,
                    &[],
                )
                .unwrap();
                assert!(
                    corridor.samples[0]
                        .hits
                        .iter()
                        .any(|hit| hit.volume == volume),
                    "corridor must meet occupied matter"
                );
                let reach = resolve::FieldReach::of_throw(&context, &admits, origin);
                let (_, field) = resolve::field_presence(&context, &reach, origin, Dir3::NEG_Z)
                    .unwrap()
                    .unwrap();
                assert!(
                    field
                        .layers
                        .iter()
                        .any(|layer| layer.present.iter().any(|present| present.volume == volume)),
                    "field must meet occupied matter"
                );
                let aimed = crate::aim::aim_distance(
                    &world.spatial,
                    Ray3d::new(origin, Dir3::NEG_Z),
                    length,
                    Entity::PLACEHOLDER,
                    &world.owners,
                    &world.parents,
                );
                assert!(aimed < length, "aim must meet occupied matter");
            })
            .unwrap();
    }

    /// A shipped house, `house_1`, standing with its ground point at the origin in a bare march
    /// world: every element a collider of its own substance, as the scatter spawns it.
    fn house_world() -> App {
        let mut app = bare_world(false);
        let kind = crate::construction::tests::shipped_house();
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        crate::construction::spawn(
            &mut commands,
            &kind,
            crate::scatter::MapInstance {
                id: "fixture_house".to_owned(),
                ordinal: 0,
            },
            Isometry3d::IDENTITY,
        );
        queue.apply(world);
        settle(&mut app);
        app
    }

    /// A BRICK WALL IS MATTER THE EXISTING WALK CROSSES (the game's environment destruction
    /// model): a fixture 88 at 800 m/s, square onto the shipped house's east wall through a pier
    /// 1 m over the ground, enters at the wall's outer face and comes out the far side, while the
    /// same round at 100 m/s comes to rest inside the wall. The wall stops or passes a round by its chord
    /// and the shipped brick row alone; a wall the march ignored (no armour layer, no volume)
    /// gives no impact at all, and one charged as steel stops the fast round too.
    #[test]
    fn a_brick_wall_is_perforated_by_a_fast_round_and_stops_a_slow_one() {
        // The east wall's outer face is x = 2 m; its first pier spans z 2 to 3 m (house_1's spec).
        let origin = Vec3::new(10.0, 1.0, 2.5);
        // The wall's inner face is x = 1.75 m.
        let fast = fire_and_capture(&mut house_world(), origin, Vec3::NEG_X, 800.0);
        let through = fast.first().expect("800 m/s: the wall was never struck");
        assert!(matches!(through.surface, ImpactSurface::Material(_)));
        assert_eq!(through.verdict, Verdict::Through);
        assert!(
            (through.position.x - 2.0).abs() < 0.01,
            "800 m/s: struck at the outer face, not {}",
            through.position
        );
        let slow = fire_and_capture(&mut house_world(), origin, Vec3::NEG_X, 100.0);
        let stopped = slow.first().expect("100 m/s: the wall was never struck");
        assert!(matches!(stopped.surface, ImpactSurface::Material(_)));
        assert_eq!(stopped.verdict, Verdict::Stopped);
        assert!(
            stopped.position.x > 1.75 && stopped.position.x < 2.0,
            "100 m/s: at rest inside the wall, not at {}",
            stopped.position
        );
    }
}

#[cfg(test)]
mod fuze_tests;

#[cfg(test)]
pub(crate) fn path_fixture(shot: crate::ShotId, tick: u32) -> FlightOutcome {
    FlightOutcome {
        shot,
        sequence: 0,
        tick,
        fraction: 0.0,
        spans: Vec::new(),
        carrier: Vec3::ZERO,
        resume: Vec3::ZERO,
        velocity: Vec3::X * 500.0,
        stopped: false,
        impact: None,
        body: march_tests::FIXTURE_88,
        drag_k: fixture_drag_k(0.088, 10.2),
    }
}
