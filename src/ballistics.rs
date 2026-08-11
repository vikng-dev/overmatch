//! Shared shell flight, collision queries, penetration, and impacts.

use std::sync::Arc;
use std::time::Instant;

use avian3d::prelude::{Collider, Forces, Position, Rotation, SpatialQuery, SpatialQueryFilter};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
// `shot_trace::record` only evaluates its closure when tracing is armed.
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::damage::{DamageConsequences, VolumeOf, hit_ancestor};
use crate::state::{GameplaySet, SimPhase};
use crate::terrain_grid::HeightGrid;
use crate::{ClientReplica, Layer, PredictedPresent, Replaying, ShotId};

/// Server-sanctioned outcome bookkeeping: the client's reconciliation buffer and the bounded
/// catch-up that replays it into a drawable flight. Netcode, not ballistics — the flight,
/// penetration and spall math below never reads it.
mod sanctioned;

/// The §13 union field walk — the pure resolution core (overlaps charge their union, micro-gaps
/// ε-weld, the shell samples the world as a caliber-wide disc). NOT WIRED: the march below still
/// runs the serial [`resolve_armor_crossing`]; slice 2 of the arc replaces it with this. Everything
/// in there is a pure function over pre-collected hit lists, which is what makes the §13.6
/// invariants testable to the bit.
#[allow(
    dead_code,
    reason = "slice-1 pure core; slice 2 wires it into the march and retires the serial resolver"
)]
mod walk;

/// The §13 walk driven against the live world: corridor collection, the staged `begin`/`finish`, and
/// the mapping from its declarative plan onto this module's impacts, spall, damage and impulses.
mod resolve;

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

pub(crate) use sanctioned::{
    SanctionedBounce, SanctionedBounceInsert, SanctionedShots, SanctionedTerminal,
};
use sanctioned::{SanctionedCatchUp, catch_up_sanctioned_chain};

/// The march's boundary nudge: every segment cast starts this far along the direction so a shell
/// does not immediately re-touch the face it is leaving.
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

/// One open-air step shared by the live march and network catch-up. Collision handling belongs to
/// the caller.
pub(crate) fn advance_shell(position: Vec3, velocity: Vec3, drag_k: f32, dt: f32) -> (Vec3, Vec3) {
    let velocity = freeflight_step(velocity, drag_k, dt);
    (position + velocity * dt, velocity)
}

/// Free-flight catch-up from the muzzle. Returns the caught-up state plus its path; callers must
/// resolve any armor crossed by the skipped chords.
pub(crate) fn fast_forward_shell(
    origin: Vec3,
    velocity: Vec3,
    drag_k: f32,
    dt: f32,
    ticks: u32,
) -> (Vec3, Vec3, Vec<Vec3>) {
    let mut pos = origin;
    let mut vel = velocity;
    let mut points = Vec::with_capacity(ticks as usize + 1);
    points.push(pos);
    for _ in 0..ticks {
        (pos, vel) = advance_shell(pos, vel, drag_k, dt);
        points.push(pos);
    }
    (pos, vel, points)
}

/// Wrapping elapsed ticks on the same half-range rule Lightyear's `Tick - Tick -> i32` uses, kept
/// net-neutral for the ballistics layer. Returns `None` when `then` is actually ahead of `now`; a
/// genuine elapsed interval may cross `u32::MAX` and still returns its small positive distance.
fn elapsed_ticks(now: u32, then: u32) -> Option<u32> {
    let elapsed = now.wrapping_sub(then);
    (elapsed <= i32::MAX as u32).then_some(elapsed)
}

/// DERIVED from the client's default 100-tick rollback window: cosmetic recovery never integrates
/// farther than simulation can reconcile. A larger authority interval fails closed rather than
/// drawing a shortened, invented trajectory.
pub(crate) const MAX_COSMETIC_CATCH_UP_TICKS: u32 = 100;

/// Reference-mm penetration capability using a DeMarre-shaped mass and speed curve.
const PEN_K: f32 = 0.005_8;
const PEN_N: f32 = 1.43;
/// Tunable projectile-mass exponent.
const MASS_EXP: f32 = 0.5;

/// Reference-mm a projectile of `mass` kg can defeat at `speed` m/s.
fn capability(mass: f32, speed: f32) -> f32 {
    PEN_K * mass.powf(MASS_EXP) * speed.powf(PEN_N)
}

/// Inverse of [`capability`] for a fixed projectile: the speed carrying `capability` reference-mm at
/// this `mass`. Spending cost then inverting is the Lambert–Jonas residual-velocity shape —
/// barely-penetrate exits slow, big overmatch barely slows (design doc §3).
fn speed_for(mass: f32, capability: f32) -> f32 {
    (capability / (PEN_K * mass.powf(MASS_EXP))).powf(1.0 / PEN_N)
}

/// Deterministic spall-cone directions with normalized polar position `t` in `[0, 1]`.
fn spall_directions(axis: Dir3, half_angle: f32, n: usize) -> Vec<(Dir3, f32)> {
    let z = Vec3::from(axis);
    let up = if z.y.abs() > 0.99 { Vec3::X } else { Vec3::Y };
    let x = z.cross(up).normalize();
    let y = z.cross(x);
    const GOLDEN: f32 = 2.399_963_2;
    (0..n)
        .filter_map(|k| {
            let t = (k as f32 + 0.5) / n as f32;
            let polar = half_angle * t;
            let az = k as f32 * GOLDEN;
            let local = z * polar.cos() + (x * az.cos() + y * az.sin()) * polar.sin();
            Dir3::new(local).ok().map(|d| (d, t))
        })
        .collect()
}

/// DERIVED max RHA-mm for an on-axis fragment: the upper endpoint of the 3–30 mm reference range
/// recorded in `.agents/docs/design/armor-penetration-and-damage.md`.
const FRAG_PEN_MAX: f32 = 30.0;
/// Fragment air drag (1/m).
const FRAG_DRAG: f32 = 0.6;
/// HP a fragment deposits per RHA-mm of its current penetration at the moment of impact.
const FRAG_DMG_PER_MM: f32 = 0.12;

/// March one spall fragment through ballistic volumes and return its visual trace.
fn cast_spall_fragment(
    origin: Vec3,
    dir: Dir3,
    mut pen: f32,
    mut range: f32,
    spatial: &SpatialQuery,
    volumes: &Query<&BallisticVolume>,
    parents: &Query<&ChildOf>,
    health: &mut Query<&mut ComponentHealth>,
    filter: &SpatialQueryFilter,
    // Only authority writes HP; replicas retain the visual trace.
    deposit: bool,
) -> (SpallFragment, f32) {
    const EPS: f32 = 1.0e-3;
    const PROBE: f32 = 50.0;
    let mut pos = origin;
    let mut deposited = false;
    let mut damage_dealt = 0.0;
    while range > EPS {
        let Some(hit) = spatial.cast_ray(pos, dir, range, true, filter) else {
            pos += Vec3::from(dir) * range; // flew the rest, hit nothing
            break;
        };
        let at = pos + Vec3::from(dir) * hit.distance;
        pen = (pen / (1.0 + FRAG_DRAG * hit.distance)).max(0.0); // drag over the gap
        let node = hit_ancestor(hit.entity, volumes, parents).map(|(e, v)| (e, v.material_factor));
        let Some((node_entity, factor)) = node else {
            pos = at;
            break;
        };
        if let Ok(mut hp) = health.get_mut(node_entity) {
            if deposit {
                let before = hp.current;
                hp.current = (before - pen * FRAG_DMG_PER_MM).max(0.0);
                damage_dealt += before - hp.current;
            }
            deposited = true;
        }
        let span = spatial
            .cast_ray_predicate(
                at + Vec3::from(dir) * EPS,
                dir,
                PROBE,
                false,
                filter,
                &|e| e == hit.entity,
            )
            .map(|exit| EPS + exit.distance)
            .unwrap_or(0.0);
        let cost = span * factor;
        if pen > cost {
            pen -= cost;
            pos = at + Vec3::from(dir) * (span + EPS);
            range -= hit.distance + span + EPS;
        } else {
            pos = at + Vec3::from(dir) * span * (pen / cost.max(EPS));
            break;
        }
    }
    (
        SpallFragment {
            end: pos,
            deposited,
        },
        damage_dealt,
    )
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
/// The axis alone is not first contact (codex finding 5): §13.5's shell meets the world as a disc, so
/// a round whose centre threads a gap while its rim clips a plate MUST resolve — otherwise the whole
/// η law is unreachable in the live march and every graze silently becomes a clean miss.
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
    frame: &walk::DiscFrame,
    radius: f32,
    origin: Vec3,
    dir: Dir3,
    max: f32,
    not_own: &dyn Fn(Entity) -> bool,
) -> Option<SegmentHit> {
    let armor = if radius <= 0.0 {
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
            collect::swept_aabb(origin, Vec3::from(dir), max, radius),
            |entity| {
                near_armor = hit_ancestor(entity, &world.volumes, &world.parents).is_some()
                    && not_own(entity);
                // Stop at the first candidate: this asks IF, not WHICH.
                !near_armor
            },
        );
        if near_armor {
            let mut nearest: Option<avian3d::prelude::RayHitData> = None;
            for offset in walk::disc_offsets(frame, radius, walk::DEFAULT_RING) {
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
/// Invariant: re-age from `PredictedPresent - bounce_tick` when available; `waited` counts only
/// forward ticks after this client created the hold. See ADR-0021.
#[derive(Component)]
struct Held {
    /// Fixed ticks spent actually waiting for a verdict after this client created the hold.
    waited: u32,
    /// Re-age fallback when [`PredictedPresent`] is unavailable.
    age: u32,
    /// Local contact normal for the first sanctioned bounce after this hold re-seeds.
    normal: Vec3,
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
    /// The combatant whose body took this bounce's impulse, if the struck volume belongs to one.
    /// The SAME body [`apply_hit_impulse`] armed, so a client can tell whether a spark it draws is
    /// one of the hits an arriving [`HullShock`] episode is made of.
    pub victim: Option<crate::CombatantId>,
}

/// Authority armor terminal for a keyed shot.
#[derive(Event)]
pub(crate) struct ShellTerminal {
    pub shot: ShotId,
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal.
    pub normal: Vec3,
    /// The server's penetration verdict (gates the flame lick on the client, as in the local read).
    pub penetrated: bool,
    /// Ricochets resolved before this terminal.
    pub after_bounces: u32,
    /// The combatant whose body took this terminal's impulse — see [`ShellRicochet::victim`].
    pub victim: Option<crate::CombatantId>,
}

/// Authority-only report that a keyed shot first lowered an HP pool. [`DamageReport`] emits it at
/// most once; the private wire receipt intentionally carries no HP amount.
#[derive(Event)]
pub(crate) struct ShellDamage {
    pub shot: ShotId,
    pub amount: f32,
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
pub(crate) struct Projectile {
    velocity: Vec3,
    caliber: f32,
    mass: f32,
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
    /// The §13.2 field value: reference-mm of armour per metre of chord.
    pub material_factor: f32,
    /// The substance's registry name — identity, for diagnostics/inspection and the paint livery.
    /// Never parsed for behaviour.
    #[allow(
        dead_code,
        reason = "carried from the bake for the x-ray readout and the paintable-livery pass; the \
                  walk reads only the factor, deliberately"
    )]
    pub substance: String,
}

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

#[derive(Component)]
pub struct ComponentVolume;

/// A component's HP pool (crew/module/ammo). A spall fragment deposits 1; the main penetrator
/// transiting deposits many (scaled by the cost it paid crossing — design §6). `current` clamps at
/// 0; the *consequences* of reaching 0 (cookoff, crew death, knock-out) are later increments (§§7–8).
#[derive(Component)]
pub struct ComponentHealth {
    pub current: f32,
    pub max: f32,
}

/// What the authority was resolving when it applied an external impulse to a hull.
///
/// It names the episode for the owner; it is deliberately NOT a magnitude. No force, direction, or
/// application point ever rides [`HullShock`].
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ShockCause {
    /// No shock has ever landed on this hull ([`HullShock::count`] is still zero).
    #[default]
    None,
    /// A round deflected off a face and kicked the hull normal-ward.
    Ricochet,
    /// A round was defeated by the plate and dumped all its remaining momentum into the hull.
    Embed,
    /// A round punched through, leaving behind the momentum it spent crossing.
    Perforation,
}

impl ShockCause {
    /// Which cause survives when several resolutions land inside one open episode: the most severe.
    /// A perforation and a graze in the same window are one episode, and the player is owed the
    /// perforation's name for it.
    const fn severity(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Ricochet => 1,
            Self::Embed => 2,
            Self::Perforation => 3,
        }
    }
}

/// Owner-private PROOF that the authority applied an impulse this hull could not have predicted.
///
/// The client cannot predict being shot, so its own copy never moves on its own. Registered
/// `.replicate().predict()` with a permanently INERT rollback condition (`net::protocol`, REV 25):
/// the arrival of a bumped `count` triggers nothing by itself — `net::adoption` observes the
/// mismatch on the confirmed histories, holds it for its own drawn impact, and orders the
/// rollback that restores every replicated predicted component at `tick`, hull velocity included.
/// THAT is how the shove is delivered; this component carries none of it. A hit's Δv (~0.14 m/s)
/// is an order of magnitude under the velocity rollback gate, so without that forced rollback the
/// authoritative velocity is compared, judged close enough, and discarded — the reason this
/// component has to exist at all.
///
/// `count` is MONOTONIC because replication carries STATE, not TRANSITIONS: a bump-and-restore
/// inside one send window is never observed as two events, only as a final value. A counter that
/// only moves forward makes "have I already realized this?" a comparison rather than an event
/// subscription that can miss.
#[derive(Component, Clone, Copy, Default, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct HullShock {
    /// Episodes the authority has applied to this hull. Monotonic (wrapping); zero = never hit.
    pub count: u32,
    /// The authority tick this episode's counter changed — the tick whose restored hull state
    /// carries the episode's accumulated shove, and the tick a replay must be able to reach.
    pub tick: u32,
    /// The authority tick this episode OPENED on: the tick of the FIRST impulse it is made of.
    ///
    /// `[opened, tick]` is the episode's own set of impulse ticks, and it is CARRIED rather than
    /// derived because no arithmetic over `tick` alone reproduces it. `close_episode` publishes the
    /// first impulse a fresh [`HullShockLedger`] ever sees IMMEDIATELY (there is no open episode to
    /// defer behind), so that episode spans a single tick, while a deferred one can span up to
    /// `SHOCK_EPISODE_TICKS − 1`. A fixed-width window ending at `tick` would therefore claim
    /// fifteen ticks a fresh hull's first episode never covered — including, because a respawn keeps
    /// the combatant identity and only replaces the entity, ticks belonging to the hull's PREVIOUS
    /// life. `net::adoption` matches sparks against this range; see [`HullShockLedger::arm`].
    pub opened: u32,
    pub cause: ShockCause,
}

/// One episode [`HullShockLedger::close_episode`] just published: what caused it, and the tick of
/// the first impulse it is made of.
///
/// Returned as one value because the two are only meaningful together — [`HullShock::opened`] is
/// what makes `[opened, tick]` the episode's exact span, and a caller that could write one without
/// the other could publish a span the ledger never observed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ClosedEpisode {
    pub(crate) cause: ShockCause,
    pub(crate) opened: u32,
}

/// Local, never-replicated bookkeeping kept beside [`HullShock`]. Its two halves belong to
/// different composition roles and never both run on one peer.
///
/// AUTHORITY half ([`HullShockLedger::arm`] / [`HullShockLedger::close_episode`]) implements the
/// episode rule. OWNER half ([`HullShockLedger::realize`]) is the monotonic "last realized" mark
/// that makes re-application during replay idempotent: the component is registered for local
/// rollback, so a rollback rewinds it with the rest of the tick's state and replay re-realizes the
/// shock against the restored history.
#[derive(Component, Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct HullShockLedger {
    pending: Option<ShockCause>,
    /// The tick the OPEN (unpublished) group of impulses started on — see [`Self::close_episode`],
    /// which is what stamps it. `None` exactly when `pending` is `None`.
    opened_at: Option<u32>,
    last_bump_tick: Option<u32>,
    applied_count: u32,
}

impl HullShockLedger {
    /// AUTHORITY: record that an external impulse just landed on this hull. It is armed, not
    /// published — [`Self::close_episode`] decides which tick the episode closes on, and stamps the
    /// tick it opened on.
    pub(crate) fn arm(&mut self, cause: ShockCause) {
        if self
            .pending
            .is_none_or(|open| cause.severity() > open.severity())
        {
            self.pending = Some(cause);
        }
    }

    /// AUTHORITY: take the armed cause iff no episode has opened within `episode_ticks` of `now`.
    ///
    /// While an episode is open the armed cause is DEFERRED, not dropped — it closes the moment the
    /// window expires. Dropping would silently lose the hit that matters most (a main-gun round
    /// arriving a few ticks behind an MG pellet), because nothing later would ever mention it.
    ///
    /// WHY THE OPEN TICK IS STAMPED HERE and not in [`Self::arm`]. `net::protocol` runs this EVERY
    /// authority tick, after the march that arms the ledger
    /// (`close_hull_shock_episodes.after(SimPhase::ProjectileMarch)`), so the first tick this sees a
    /// pending cause IS the tick that cause was armed on. Stamping it in `arm` would mean plumbing a
    /// tick through the whole ballistic march for a value the once-per-tick caller already holds.
    ///
    /// The stamp gives [`HullShock::opened`] two properties by CONSTRUCTION, and the ordering rule in
    /// `net::adoption` rests on both:
    ///
    /// - `opened ≤ tick`, because it is written on a tick at or before the one that publishes;
    /// - `opened` of the NEXT episode is strictly greater than this one's `tick`, because publishing
    ///   clears it and only a later call can write it again. Consecutive episodes on one hull
    ///   therefore span DISJOINT tick ranges, with no appeal to `episode_ticks` arithmetic — which is
    ///   what makes a fresh ledger's single-tick first episode as exact as a deferred one's.
    ///
    /// Both are claims about PLAIN numeric order and are NOT wrap-general — the assumption belongs in
    /// the open, because the deferral test one line below deliberately IS wrap-aware
    /// (`now.wrapping_sub(last)`) and the two could be mistaken for the same discipline. The tick
    /// counter these spans live in is lightyear 0.28's `Tick`: a u32 compared with plain `u32::cmp`
    /// and advanced with SATURATING arithmetic, on that crate's documented assumption that a session
    /// never reaches the ~828-day boundary. At saturation the timeline freezes, so a pending episode
    /// stalls rather than wrapping into a span that falsely covers an older spark — the direction
    /// that is safe. Anything that made the counter genuinely wrap invalidates the ordering argument,
    /// not the deferral one.
    ///
    /// If this were ever NOT called on some tick, the stamp lands late and the published span is
    /// NARROWER than the truth. That direction is safe: a claim can only fail to match a spark it
    /// owns, never match one it does not.
    pub(crate) fn close_episode(&mut self, now: u32, episode_ticks: u32) -> Option<ClosedEpisode> {
        let cause = self.pending?;
        let opened = *self.opened_at.get_or_insert(now);
        if self
            .last_bump_tick
            .is_some_and(|last| now.wrapping_sub(last) < episode_ticks)
        {
            return None;
        }
        self.pending = None;
        self.opened_at = None;
        self.last_bump_tick = Some(now);
        Some(ClosedEpisode { cause, opened })
    }

    /// OWNER: whether this monotonic count has already been realized on this timeline.
    pub(crate) fn is_realized(&self, count: u32) -> bool {
        self.applied_count == count
    }

    /// OWNER: mark a count realized. Rewound by local rollback, so replay re-realizes it.
    pub(crate) fn realize(&mut self, count: u32) {
        self.applied_count = count;
    }

    #[cfg(test)]
    pub(crate) fn pending(&self) -> Option<ShockCause> {
        self.pending
    }

    #[cfg(test)]
    pub(crate) fn applied(&self) -> u32 {
        self.applied_count
    }
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
}

/// A single spall fragment's trace: where it stopped, and whether it deposited HP (hit a component)
/// or merely shadowed / flew on (hit armor or air). Carries 1 HP; no penetration of its own (§5).
pub struct SpallFragment {
    pub end: Vec3,
    pub deposited: bool,
}

/// One spall event — the cone thrown from a perforation exit. Origin + axis + half-angle describe
/// the fixed-shape cone; `fragments` are the resolved rays the sandbox draws.
pub struct SpallBurst {
    pub origin: Vec3,
    pub axis: Dir3,
    pub half_angle: f32,
    pub fragments: Vec<SpallFragment>,
}

/// The spall a shell has thrown this flight — one burst per perforation exit. Public like
/// `PenetrationMarks`; freed when the shell despawns.
#[derive(Component, Default)]
pub struct SpallMarks {
    pub bursts: Vec<SpallBurst>,
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

/// View-only tracer streak child. The view layer clamps it to travel since the latest anchor.
#[derive(Component)]
pub struct TracerStreak {
    pub nominal_len: f32,
}

impl TracerStreak {
    /// Child transform for a streak that has travelled `flown` metres from its current anchor.
    ///
    /// Invariant: both spawn and view maintenance use this function, so the tail never precedes
    /// the muzzle or latest ricochet.
    pub(crate) fn drawn_transform(&self, flown: f32) -> Transform {
        let len = self.nominal_len.min(flown).max(0.0);
        Transform {
            translation: Vec3::Z * (len * 0.5),
            rotation: Quat::from_rotation_arc(Vec3::Y, Vec3::NEG_Z),
            scale: Vec3::new(1.0, len, 1.0),
        }
    }
}

/// Preloaded tracer-streak view assets (mesh + emissive material), built once so a tracer round clones
/// handles rather than rebuilding them per shot — the streak twin of [`ProjectileAssets`].
#[derive(Resource)]
struct TracerAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

/// Preloaded shell scene, cloned per shot rather than loaded each time.
#[derive(Resource)]
struct ProjectileAssets {
    scene: Handle<WorldAsset>,
}

/// The impact surface class, resolved from ballistic-volume ancestry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ImpactSurface {
    Terrain,
    Armor,
}

/// The authority's own identity for an impact this client is only RE-DRAWING.
///
/// A replica never resolves an armor outcome itself: its cosmetic shell freezes at contact and the
/// spark is drawn from the authority's sanctioned bounce or terminal (see
/// [`resolve_replica_armor_contact`]). Those facts name the tick the authority resolved the impact
/// on and the body it gave the impulse to, so a spark can be matched to the hits an arriving
/// [`HullShock`] episode is made of instead of to "some armor impact, somewhere, recently".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct AuthorityImpact {
    /// The SERVER tick the impact resolved on — the same tick [`HullShockLedger::arm`] ran on.
    pub(crate) tick: u32,
    /// The combatant whose body took the impulse, if the struck volume belongs to one.
    pub(crate) victim: Option<crate::CombatantId>,
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
    /// Whether the round entered armor rather than ricocheting or striking terrain.
    pub(crate) penetrated: bool,
    /// Deflected direction for a ricochet; absent for other impacts.
    pub(crate) deflection: Option<Vec3>,
    /// The authority fact this spark re-draws, when it is re-drawing one. `None` when the impact is
    /// the resolver's OWN — the authority's live march, or an unkeyed sandbox/singleplayer shell —
    /// which is exactly the case where there is no remote fact to correlate it with.
    pub(crate) authority: Option<AuthorityImpact>,
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

pub fn plugin(app: &mut App) {
    app.init_resource::<RetainSpentShells>()
        .init_resource::<MarchMode>()
        .insert_resource(MgShortCircuit(crate::env_flag(
            "SPIKE_MG_SHORTCIRCUIT",
            false,
        )))
        .add_observer(on_fire_shell)
        .add_observer(on_impact)
        .add_systems(Startup, setup_assets)
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

fn setup_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Preload once; firing clones the handle rather than hitting the asset server per shot.
    commands.insert_resource(ProjectileAssets {
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb")),
    });
    // The tracer streak: a thin UNIT capsule authored along its local +Y. The per-shot child
    // transform (`on_fire_shell`) rotates that axis onto the shell's local −Z (its travel axis — the
    // projectile `Transform` is kept `look_to(velocity)` by `integrate_projectiles`) and scales the
    // Scale the capsule in the per-shot child transform.
    let mesh = meshes.add(Capsule3d::new(0.018, 1.0));
    // The EMISSIVE IS THE WHOLE VISUAL: black base + zero reflectance kill every lit contribution,
    // so the streak renders exactly its emissive — which rides far above 1.0 in linear space, where
    // the HDR camera's `Bloom` (camera.rs) halos it and the tonemapper rolls the over-bright core to
    // white-hot for free. Do NOT set `unlit: true` here: StandardMaterial's unlit path outputs
    // `base_color` alone and IGNORES `emissive`, which rendered the old streak as a flat sRGB
    // "square sausage" that bloom never caught. Warm orange; magnitude tunes against bloom intensity.
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        reflectance: 0.0,
        emissive: LinearRgba::rgb(30.0, 12.0, 3.0),
        ..default()
    });
    commands.insert_resource(TracerAssets { mesh, material });
}

/// Spawn a shell from `FireShell`, using fixed-tick catch-up when requested.
///
/// Invariant: each skipped chord is checked. Terrain may end locally; a keyed replica armor contact
/// becomes a hidden hold until an authority outcome arrives. Replica ballistics never decides armor.
fn on_fire_shell(
    fire: On<FireShell>,
    assets: Res<ProjectileAssets>,
    tracer_assets: Res<TracerAssets>,
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
    // The net client's predicted present `P` — the tick every cosmetic shell lives at, and the one
    // the shot-lifecycle recorder stamps its rows with. Absent on the authority (server / SP /
    // sandbox), where an OBSERVER shell (the only kind that carries `fire.shot` here) never exists.
    present: Option<Res<PredictedPresent>>,
    // The server has no `PredictedPresent`, but the shared network protocol gives it this
    // net-neutral tick so locally authored lifecycle rows retain their actual fire time.
    shot_clock: Option<Res<crate::ShotClock>>,
    // The shot-lifecycle recorder (`SPIKE_SHOT_TRACE`): absent unless armed, so an unrecorded run pays
    // one `Option` check per shot. `FireShellOrigin` preserves local-vs-reconstructed attribution;
    // `ClientReplica` distinguishes the two locally authored roles (`own` vs `auth`).
    replica: Option<Res<ClientReplica>>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut commands: Commands,
) {
    let now = present
        .as_deref()
        .map_or_else(|| shot_clock.as_deref().map_or(0, |clock| clock.0), |p| p.0);
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
                        normal,
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
                        penetrated: false,
                        deflection: None,
                        authority: None,
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
    // visible (a non-tracer MG round still bounces, ricochets, and lands; dead-reckoned streaks were
    // rejected). Only the ATTACHED VISUAL differs, gated below at the RENDER layer.
    let visibility = if catch_up_hold.is_some() {
        Visibility::Hidden
    } else {
        Visibility::default()
    };
    let shell_base = (
        Projectile {
            velocity,
            caliber: fire.caliber,
            mass: fire.mass,
            drag_k: drag,
            // From the BORE, not from the post-catch-up travel direction: a shell reconstructed with
            // catch-up must anchor its ring exactly where the locally fired one did.
            disc: walk::DiscFrame::anchored(Vec3::from(fire.direction)).unwrap_or(
                walk::DiscFrame {
                    u: Vec3::X,
                    v: Vec3::Y,
                },
            ),
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
            capability: capability(fire.mass, speed),
        },
        // Root visibility so an attached streak child inherits it (harmless on the shell-scene path).
        visibility,
        Transform::from_translation(position).looking_to(travel, Vec3::Y),
    );

    // Every sim-affecting component is in this ONE spawn transaction. Bevy 0.19 bundles have a
    // 15-element tuple limit, so use explicit branches rather than a late `.insert`: an `Option<T>`
    // is not a Bundle, and inserting `Shot`/`ShotSource` after `Projectile` lets lifecycle
    // observers see a logically incomplete shell.
    let mut shell = match (fire.shot, fire.shooter, catch_up_hold) {
        (Some(shot), Some(source), Some(held)) => {
            commands.spawn((shell_base, Shot(shot), source, held))
        }
        (Some(shot), Some(source), None) => commands.spawn((shell_base, Shot(shot), source)),
        (Some(shot), None, Some(held)) => commands.spawn((shell_base, Shot(shot), held)),
        (Some(shot), None, None) => commands.spawn((shell_base, Shot(shot))),
        (None, Some(source), Some(held)) => commands.spawn((shell_base, source, held)),
        (None, Some(source), None) => commands.spawn((shell_base, source)),
        (None, None, Some(held)) => commands.spawn((shell_base, held)),
        (None, None, None) => commands.spawn(shell_base),
    };

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

    // Visual policy: main-gun scene, MG tracer streak, or invisible non-tracer MG round.
    if fire.caliber >= TRACER_MAX_CALIBER {
        shell.insert(WorldAssetRoot(assets.scene.clone()));
    } else if fire.tracer {
        // Scale with travel speed, with a floor for slow rounds.
        let streak = TracerStreak {
            nominal_len: (speed * 0.018).max(2.0),
        };
        // Seed clamped: an observer may be born after the per-frame maintainer has run.
        let flown = position.distance(fire.origin);
        let transform = streak.drawn_transform(flown);
        shell.with_child((
            Mesh3d(tracer_assets.mesh.clone()),
            MeshMaterial3d(tracer_assets.material.clone()),
            transform,
            // A light streak neither casts nor receives shadow — without this the sun dragged a
            // long capsule shadow across the terrain under every tracer. World geometry otherwise:
            // it is a child of a SHELL, never of a tank, so it is drawn in every view.
            crate::render_policy::VisualScope::WORLD_EFFECT,
            streak,
        ));
    }
}

/// Optional multiplayer state used by the shared projectile march. Grouping it keeps the march's
/// simulation queries visible without exceeding Bevy's system-parameter arity.
#[derive(SystemParam)]
struct ProjectileMarchNet<'w> {
    replica: Option<Res<'w, ClientReplica>>,
    sanctioned: Option<Res<'w, SanctionedShots>>,
    replaying: Option<Res<'w, Replaying>>,
    present: Option<Res<'w, PredictedPresent>>,
}

/// The world one shell march probes: armor geometry, the ownership hierarchy that classifies a hit,
/// and the deterministic ground. Grouped as a [`SystemParam`] so the march's phase helpers share one
/// parameter instead of five, exactly as [`ProjectileMarchNet`] groups the net state.
#[derive(SystemParam)]
struct ProjectileMarchWorld<'w, 's> {
    spatial: SpatialQuery<'w, 's>,
    volumes: Query<'w, 's, &'static BallisticVolume>,
    owners: Query<'w, 's, &'static VolumeOf>,
    /// The struck body's match-local identity, when it has one. Read only to NAME the victim on the
    /// authority facts a client re-draws; nothing in the march branches on it.
    combatants: Query<'w, 's, &'static crate::CombatantId>,
    parents: Query<'w, 's, &'static ChildOf>,
    // The heightmap ground, when the heightmap world is live: the march's terrain contacts read
    // the exact deterministic caster, not parry (see [`cast_march_segment`]). Absent on the flat
    // fallback world, where the parry terrain cast remains.
    grid: Option<Res<'w, HeightGrid>>,
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
    damage_dealt: f32,
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
    )>,
    world: ProjectileMarchWorld,
    // Collider poses + shapes, for the §13 corridor collector: it walks each candidate's geometry
    // itself rather than asking Avian for a nearest hit (see `ballistics::collect`).
    colliders: Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static BallisticSurfaces>,
    )>,
    mut bodies: Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
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
        replaying,
        present,
    } = net;
    // F1 (rollback-safe cosmetics): on a net client, lightyear replays FixedMain N times per
    // rollback. Every shell this system marches is VIEW-ONLY (`deposit == false` — HP and impulse are
    // the server's authority; see `ClientReplica`) and its picture must advance exactly ONE step per
    // FORWARD tick. Re-marching on each replayed tick would teleport every in-flight shell forward by
    // the rollback depth (with duplicate `ShellPath` points) and age every `Held` shell one extra
    // tick per replay — burning the grace window in a single frame and corrupting the
    // `present − bounce_tick` re-seed arithmetic that `Held` depends on. So skip the whole march on a
    // replayed tick; the shells resume untouched on the next forward tick. The DETERMINISTIC sim state
    // a rollback exists to correct (`TankSim`, physics) is not here — it re-runs in `GameplaySet`
    // normally. The authority (server/SP/sandbox) never sets `Replaying`, so it is never skipped.
    if replaying.is_some_and(|r| r.0) {
        return;
    }
    // March-cost attribution timer (`SPIKE_COST_TRACE`): only sampled when the recorder is armed, so an
    // unmeasured run never touches the clock. Covers the whole march (query iteration + every cast).
    let march_t0 = cost.as_ref().map(|_| Instant::now());
    let dt = time.delta_secs();
    // Authority = not a replica: only then does a hit actually mutate health here.
    let deposit = replica.is_none();
    let sanctioned = sanctioned.as_deref();
    // F3: the predicted present tick, if this is a net client — the clock the overdue-consumption
    // check below compares each sanctioned outcome's server tick against. Absent on the authority.
    let present = present.map(|p| p.0);
    // The tick every shot-lifecycle row this march writes is stamped with: the predicted present (the
    // tick each cosmetic shell lives at). Never read on the authority — every row site is `!deposit`.
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
        shot,
        held,
        source,
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

        // Reorient the shell to its travel direction so the mesh follows the (gravity-curved,
        // ricochet-bent) path instead of holding its launch heading.
        shell.transform.translation = step.position;
        shell.transform.look_to(step.direction, Vec3::Y);
        shell.path.points.push(step.position);

        // A state snapshot cannot preserve how many shots caused its resulting HP. Raise one discrete,
        // shot-attributed fact; `net::shot_transport` routes it reliably to the fire-time owner.
        if deposit
            && step.damage_dealt > 0.0
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
            shell.readout.capability = capability(shell.projectile.mass, step.speed);
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
                normal: if index == 0 {
                    held.normal
                } else {
                    segment.bounce.direction
                },
                caliber: shell.projectile.caliber,
                surface: ImpactSurface::Armor,
                penetrated: false,
                deflection: Some(segment.bounce.direction),
                authority: Some(AuthorityImpact {
                    tick: segment.bounce.bounce_tick,
                    victim: segment.bounce.victim,
                }),
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
                || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated, "via": "chain" }),
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

        resume_from_catch_up(shell, &caught_up);
        // Un-hide (the hold's invisible-stop) and resume marching next tick.
        commands
            .entity(entity)
            .remove::<Held>()
            .insert(Visibility::Inherited);
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
                    "pen": terminal.penetrated,
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
                    caliber: shell.projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: Some(segment.bounce.direction),
                    authority: Some(AuthorityImpact {
                        tick: segment.bounce.bounce_tick,
                        victim: segment.bounce.victim,
                    }),
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
                    || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated, "via": "chain" }),
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
            resume_from_catch_up(shell, &caught_up);
            return true;
        }
    } else if let Some(terminal) = buf.terminal(shot.0, consumed)
        && let Some(late) = elapsed_ticks(present, terminal.impact_tick)
        && late > OVERDUE_MARGIN_TICKS
    {
        // Finalize at the server's read — position, normal, and the `penetrated` verdict
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
            || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated }),
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
/// — position, struck-face normal, and the `penetrated` verdict that gates the flame lick.
///
/// The server's read is spatially disconnected from wherever this client's shell actually was, so
/// the trail begins a new view segment rather than inventing a correction chord to it.
fn finish_at_sanctioned_terminal(
    shell: &mut MarchingShell,
    terminal: &SanctionedTerminal,
    commands: &mut Commands,
) {
    let caliber = shell.projectile.caliber;
    shell.path.begin_segment();
    shell.path.points.push(terminal.position);
    commands.trigger(Impact {
        position: terminal.position,
        normal: terminal.normal,
        caliber,
        surface: ImpactSurface::Armor,
        penetrated: terminal.penetrated,
        deflection: None,
        authority: Some(AuthorityImpact {
            tick: terminal.impact_tick,
            victim: terminal.victim,
        }),
    });
}

/// Re-anchor a cosmetic shell on the state its authority catch-up ended at, and refresh the readout
/// the sandbox's info layer reads.
fn resume_from_catch_up(shell: &mut MarchingShell, caught_up: &SanctionedCatchUp) {
    shell.transform.translation = caught_up.position;
    if let Ok(direction) = Dir3::new(caught_up.velocity) {
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
    shell.projectile.velocity = caught_up.velocity;
    shell.readout.speed = caught_up.velocity.length();
    shell.readout.capability = capability(shell.projectile.mass, caught_up.velocity.length());
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
    bodies: &mut Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
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
    let mut damage_dealt = 0.0;
    // SHOOTER SELF-EXCLUSION (see [`not_own_volume`]): this round is transparent to the tank that
    // fired it, for every cast below. Identical on the authority and on a replica — the one place
    // both ends must agree about the shooter's own geometry, or the server's damage model and the
    // client's cosmetic model describe different worlds.
    let shooter = shell.source.map(|source| source.tank);
    let not_own = |entity: Entity| not_own_volume(entity, shooter, &world.owners, &world.parents);
    // The march casts against terrain (which stops the shell) and ballistic volumes (which it
    // crosses) — split per [`cast_march_segment`], nearer hit wins. This filter serves the
    // interior probes (thickness / exit faces / spall), which are armor-only by construction.
    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    // Nudge past each boundary we resolve so we don't immediately re-hit it.
    const EPS: f32 = 1.0e-3;

    // Advance free-flight (gravity + drag on the velocity, then the position step) through the
    // shared per-tick kernel — the SAME [`advance_shell`] the FireEvent catch-up folds, so a
    // caught-up shell and a natively-flown one can't diverge. `freeflight_pos` is this tick's
    // free-flight landing point; the ray-march below overrides it only where the segment hits
    // something. The march may *bend* the direction (normalization / ricochet), so we carry
    // direction + speed and rebuild the velocity at the end rather than assuming a straight step.
    let (freeflight_pos, stepped) = advance_shell(
        shell.transform.translation,
        shell.projectile.velocity,
        shell.projectile.drag_k,
        dt,
    );
    let Ok(mut dir) = Dir3::new(stepped) else {
        return None;
    };
    let mut speed = stepped.length();
    let mut pos = shell.transform.translation;
    let mut remaining = speed * dt;
    let mut stopped = false;
    // Set when an observer shell freezes at armor contact to await its bounce keyframe (see the
    // `!deposit` branch): the shell keeps its entity and velocity, and the `Held` handler above
    // drives the wait on subsequent ticks — distinct from `stopped` (which despawns).
    let mut holding = false;
    // Whether the march has bent the shell off its original free-flight segment. Until it does,
    // an open-air fly-out lands exactly on `freeflight_pos` (the shared advance); after a bend the
    // leftover budget flies along the new direction instead.
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
    // Transported from the direction the frame was LAST left on — the pre-step velocity — not from
    // the shell's visual forward: `looking_to` is degenerate for a round travelling straight down,
    // and the sampling basis is sim state that must not read a view detail.
    let previous = Dir3::new(shell.projectile.velocity).unwrap_or(dir);
    let mut frame = shell
        .projectile
        .disc
        .transport(Vec3::from(previous), Vec3::from(dir));
    let radius = if shell.projectile.caliber >= resolve::DISC_MIN_CALIBER {
        shell.projectile.caliber * 0.5
    } else {
        0.0
    };
    // Which primitives each disc sample is still inside. Non-empty only between the crossings of one
    // multi-plate step; the resolver hands it forward so the next corridor never has to infer it.
    let mut interior: Vec<walk::SampleSeed> = Vec::new();

    while remaining > EPS {
        let origin = pos + dir * EPS;
        let Some(step_hit) =
            cast_disc_segment(world, &frame, radius, origin, dir, remaining, &not_own)
        else {
            // Open air — fly out the rest of the step. On the original (unbent) segment this is
            // exactly the shared `advance_shell` landing point; a `continue` past this point only
            // ever follows a bend, so `bent` is the exact discriminant.
            pos = if bent {
                pos + dir * remaining
            } else {
                freeflight_pos
            };
            break;
        };
        // The ground stops the shell: same `Impact` read whether or not the MG short-circuit
        // is armed (the B-arm's terrain stop was already identical); only the lifecycle row
        // differs — the short-circuit records nothing, exactly as before.
        let hit = match step_hit {
            SegmentHit::Terrain { distance, normal } => {
                let entry = origin + dir * distance;
                let shorted =
                    shortcircuit && shell.projectile.caliber < MG_SHORTCIRCUIT_CALIBER_MAX;
                commands.trigger(Impact {
                    position: entry,
                    normal,
                    caliber: shell.projectile.caliber,
                    surface: ImpactSurface::Terrain,
                    penetrated: false,
                    deflection: None,
                    authority: None,
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
                pos = entry;
                stopped = true;
                break;
            }
            SegmentHit::Armor(hit) => hit,
        };
        let entry = origin + dir * hit.distance;
        let travelled = EPS + hit.distance;

        // EXPERIMENTAL B-arm (`SPIKE_MG_SHORTCIRCUIT`): a sub-20 mm round stops dead at the first
        // surface, skipping the entire penetration-resolution march below (thickness/span probes,
        // ricochet, spall, HP). Population-preserving — same despawn-on-contact as the live path —
        // so the A−B tick-cost delta isolates the resolution machinery. Default off (see the type).
        if shortcircuit && shell.projectile.caliber < MG_SHORTCIRCUIT_CALIBER_MAX {
            // Classify the first surface from the hit's volume ancestry (the same `hit_ancestor`
            // rule the full march uses just below) so the read is honest even in the B-arm. The
            // short-circuit stops dead without resolving penetration, so `penetrated: false`.
            let surface = if hit_ancestor(hit.entity, &world.volumes, &world.parents).is_some() {
                ImpactSurface::Armor
            } else {
                ImpactSurface::Terrain
            };
            commands.trigger(Impact {
                position: entry,
                normal: hit.normal,
                caliber: shell.projectile.caliber,
                surface,
                penetrated: false,
                deflection: None,
                authority: None,
            });
            pos = entry;
            stopped = true;
            break;
        }

        // The struck `BallisticVolume` sits on the hit's ancestry (`hit_ancestor`, the shared
        // hierarchy-resolution rule), keeping the node entity so transit damage and spall can
        // address the component. No volume in the ancestry ⇒ classified terrain (an
        // Armor-layer entity without ownership — same rule as before the cast split).
        let resolved = hit_ancestor(hit.entity, &world.volumes, &world.parents)
            .map(|(node, volume)| (node, volume.material_factor));
        let Some((node_entity, factor)) = resolved else {
            // Terrain-classified: stop here.
            commands.trigger(Impact {
                position: entry,
                normal: hit.normal,
                caliber: shell.projectile.caliber,
                surface: ImpactSurface::Terrain,
                penetrated: false,
                deflection: None,
                authority: None,
            });
            // A terrain stop is the one shot terminal that needs NO confirm: static, pose-independent
            // geometry, so both ends already agree (ADR-0021's invariant). Recorded on the client so
            // the analyzer can close the lifecycle of a shot that simply never reached armor, instead
            // of filing it as never-consumed.
            if !deposit && let Some(shot) = shot {
                crate::shot_trace::record(
                    shot_trace,
                    "end",
                    now,
                    shot.0,
                    || json!({ "why": "terrain" }),
                );
            }
            pos = entry;
            stopped = true;
            break;
        };

        // Replica armor state machine: consume a buffered bounce/terminal, otherwise hold a keyed
        // shell hidden; an unkeyed replica shell ends locally. INVARIANT: every replica arm below
        // continues or breaks, so the physical armor resolution after it is authority-only.
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
                    if let Some(new_dir) = direction {
                        // Carry the sampling basis through the sanctioned bend, exactly as the
                        // authority's own ricochet arm does below. A replica that re-seeds on
                        // server truth without transporting its frame keeps a basis built for the
                        // INCOMING heading — one whose vectors have a component along the new travel
                        // axis, so its ring samples sit in front of and behind the disc rather than
                        // across it.
                        frame = frame.transport(Vec3::from(dir), Vec3::from(new_dir));
                        dir = new_dir;
                    }
                    speed = sanctioned_speed;
                    // Lift the shell's BODY clear of the face it bounced off, exactly as the
                    // authority's own ricochet does. At oblique incidence a disc's lateral offsets
                    // lie mostly along the struck surface's normal, so resuming at the contact point
                    // leaves half the ring behind the face and first contact fires again on the very
                    // surface just left — which on a replica means holding for a keyframe that was
                    // already consumed. The RECORDED bounce point is untouched; only where the march
                    // resumes moves.
                    pos = origin
                        + Vec3::new(hit_normal.x, hit_normal.y, hit_normal.z).normalize_or_zero()
                            * radius;
                    bent = true;
                    remaining -= travelled;
                    continue;
                }
                ReplicaArmorContact::Stopped => {
                    pos = entry;
                    stopped = true;
                    break;
                }
                ReplicaArmorContact::Holding => {
                    pos = entry;
                    holding = true;
                    break;
                }
            }
        }

        // The §13 union walk: collect an atomic corridor from here, integrate the field over
        // EVERYTHING in it, and apply the plan it returns. `node_entity`/`factor` above are no longer
        // the resolution's inputs — the corridor finds every volume for itself — but they still
        // classify armour from terrain, which is why the read stays.
        let _ = (node_entity, factor);
        let resolved = resolve::resolve_crossing(
            shell,
            &resolve::ResolveContext {
                world,
                colliders,
                armor: &armor,
                deposit,
                laws: walk::WalkLaws::default(),
            },
            origin,
            dir,
            speed,
            hit.distance,
            &interior,
            &mut terminal_emitted,
            health,
            bodies,
            &not_own,
            commands,
        );
        // FAIL CLOSED. A structured walk error means the corridor could not be resolved honestly —
        // unpairable topology, an unprobeable collider, a crossing that would not close inside 50 m.
        // The round stops where it was: no perforation, no spall, no transit damage. Free penetration
        // is the one outcome worse than a stopped shell, because it is indistinguishable from armour
        // that was never modelled.
        let crossing = match resolved {
            Ok(crossing) => crossing,
            Err(error) => {
                warn_once!(
                    "ballistics: union walk failed, stopping the round at contact: {error:?}"
                );
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: shell.projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: None,
                    authority: None,
                });
                pos = entry;
                stopped = true;
                break;
            }
        };
        damage_dealt += crossing.damage;
        interior = crossing.seeds;
        let crossing_resume = crossing.resume;
        // What the crossing actually flew, summed over its segments by the only code that knows them
        // (see `resolve::Crossing::travel`). `travelled` — the distance to the NEAREST sample's
        // contact — is the driver's own cast talking, and at oblique incidence that ray leads the
        // axis handoff by `r·tan(incidence)`.
        let flown = crossing.travel;
        // Every outcome leaves the round off its original free-flight segment (see the open-air
        // break above), whether it bounced off the face or bent into the plate.
        bent = true;
        match crossing.outcome {
            ArmorCrossing::Ricochet {
                direction,
                speed: bled,
            } => {
                frame = frame.transport(Vec3::from(dir), Vec3::from(direction));
                dir = direction;
                speed = bled;
                pos = crossing_resume.unwrap_or(entry);
                remaining -= EPS + flown;
                continue;
            }
            ArmorCrossing::Embedded { at } => {
                pos = at;
                stopped = true;
                break;
            }
            ArmorCrossing::Perforated {
                exit,
                direction,
                speed: residual,
            } => {
                frame = frame.transport(Vec3::from(dir), Vec3::from(direction));
                dir = direction;
                speed = residual;
                pos = exit;
                remaining -= EPS + flown;
            }
        }
    }
    // Carry the (possibly re-rolled) basis back onto the shell so the next tick transports from
    // where this one left off rather than re-anchoring.
    shell.projectile.disc = frame;

    Some(MarchStep {
        position: pos,
        direction: dir,
        speed,
        stopped,
        holding,
        damage_dealt,
    })
}

/// How one authority-resolved crossing of a ballistic volume ended (design doc §§2–6).
enum ArmorCrossing {
    /// Too oblique and not overmatched: deflected off the face — no entry, no spall.
    Ricochet { direction: Dir3, speed: f32 },
    /// Defeated by the plate: buried partway through, where the march ends.
    Embedded { at: Vec3 },
    /// Punched through: the march resumes at the exit face carrying the residual speed.
    ///
    /// How much of the tick's budget the crossing cost is NOT here: it is [`resolve::Crossing::travel`],
    /// because a crossing is two segments and only the resolver knows both.
    Perforated {
        exit: Vec3,
        direction: Dir3,
        speed: f32,
    },
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
    let caliber = shell.projectile.caliber;
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
            penetrated: false,
            deflection: Some(bounce.direction),
            authority: Some(AuthorityImpact {
                tick: bounce.bounce_tick,
                victim: bounce.victim,
            }),
        });
        shell.path.begin_segment();
        shell.path.points.push(bounce.origin);
        shell.marks.ricochets.push(bounce.origin);
        // Trace a buffered bounce separately from a shell that never contacted.
        if let Some(shot) = shot {
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
                || json!({ "res": "pre_term", "it": terminal.impact_tick, "pen": terminal.penetrated }),
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
        // Hold a keyed shell hidden until the authority outcome or expiry.
        commands.entity(entity).insert((
            Held {
                waited: 0,
                age: 0,
                normal: hit_normal,
            },
            Visibility::Hidden,
        ));
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
        penetrated: false,
        deflection: None,
        authority: None,
    });
    ReplicaArmorContact::Stopped
}

/// Throw the spall cone a perforation's exit face makes, appending one burst to the shell's record.
///
/// The *count* comes from the material chewed (`cost`) and the hole size (`caliber`) — the fragment
/// supply; each fragment's *energy* comes from the shot's residual (v_res²) and its position in the
/// cone (on-axis strongest). So a thin/soft body throws few fragments and a barely-through round
/// throws weak ones — both extremes low (design §5). Each fragment then penetrates per its energy.
///
/// Returns the HP the authority actually removed; a replica resolves the identical geometry and the
/// identical visual trace, and deposits nothing.
fn throw_spall_burst(
    spall: &mut Mut<SpallMarks>,
    exit: Vec3,
    dir: Dir3,
    cost: f32,
    caliber: f32,
    speed: f32,
    world: &ProjectileMarchWorld,
    health: &mut Query<&mut ComponentHealth>,
    armor: &SpatialQueryFilter,
    // Only authority writes HP; replicas retain the visual trace.
    deposit: bool,
) -> f32 {
    // Nudge past the exit face so a fragment's first cast doesn't re-touch it — the same 1 mm
    // boundary nudge the march itself uses.
    const EPS: f32 = 1.0e-3;
    // Spall (design §5). The fragment COUNT is supply only: (material chewed / ref) × (caliber /
    // ref), capped. Residual energy is deliberately NOT a factor here — it is applied per fragment
    // below as `shot_energy`, so a barely-through round throws its full complement of fragments and
    // throws them weakly, rather than throwing fewer. The cone's shape is fixed; only density scales.
    const SPALL_MAX_FRAGMENTS: usize = 24;
    const SPALL_COST_REF: f32 = 100.0; // ref-mm (≈ a 100 mm steel plate)
    const SPALL_VRES_REF: f32 = 500.0; // m/s
    const SPALL_CALIBER_REF: f32 = 0.088; // m (the 88)
    const SPALL_HALF_ANGLE: f32 = 0.35; // rad (~20°)
    const SPALL_RANGE: f32 = 6.0; // m — fragments are short-range
    let mut damage_dealt = 0.0;
    let count_f =
        SPALL_MAX_FRAGMENTS as f32 * (cost / SPALL_COST_REF) * (caliber / SPALL_CALIBER_REF);
    let count = (count_f.round() as i32).clamp(0, SPALL_MAX_FRAGMENTS as i32) as usize;
    if count > 0 {
        // Residual energy sets how hard each fragment is thrown (full at the reference exit
        // speed); the on-axis fragments (`t→0`) keep the most of it.
        let shot_energy = (speed / SPALL_VRES_REF).powi(2).clamp(0.0, 1.0);
        let mut burst = SpallBurst {
            origin: exit,
            axis: dir,
            half_angle: SPALL_HALF_ANGLE,
            fragments: Vec::with_capacity(count),
        };
        for (fdir, t) in spall_directions(dir, SPALL_HALF_ANGLE, count) {
            let birth_pen = FRAG_PEN_MAX * shot_energy * (1.0 - t);
            let (fragment, fragment_damage) = cast_spall_fragment(
                exit + Vec3::from(fdir) * EPS,
                fdir,
                birth_pen,
                SPALL_RANGE,
                &world.spatial,
                &world.volumes,
                &world.parents,
                health,
                armor,
                deposit,
            );
            damage_dealt += fragment_damage;
            burst.fragments.push(fragment);
        }
        spall.bursts.push(burst);
    }
    damage_dealt
}

/// Apply a crossing's momentum share to the struck body. The declared `Forces` query keeps this
/// immediate velocity write visible to Bevy's scheduler; static or non-rigid owners do not match.
///
/// The same match arms the body's [`HullShockLedger`], so the owner is told an unpredictable
/// impulse happened exactly when one actually reached a body — never on a resolution the physics
/// itself skipped. The ledger records only that it happened and why; the shove reaches the owner as
/// part of the state the resulting rollback restores (see [`HullShock`]).
fn apply_hit_impulse(
    bodies: &mut Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
    body: Entity,
    impulse: Vec3,
    point: Vec3,
    cause: ShockCause,
) {
    if let Ok((forces, wake, ledger)) = bodies.get_mut(body) {
        crate::track::sim::apply_explicit_impulse(
            forces,
            wake,
            crate::track::sim::ExplicitImpulse::AtPoint { impulse, point },
        );
        if let Some(mut ledger) = ledger {
            ledger.arm(cause);
        }
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

    /// The "one integrator" property: fast-forwarding a shell N ticks lands it in the SAME state as
    /// N single-tick advances. `fast_forward_shell` folds the shared `advance_shell` — the exact
    /// per-tick kernel the live march steps in open air — so a caught-up shell can't diverge from a
    /// natively integrated one. Guards against re-deriving the catch-up as a closed-form trajectory.
    #[test]
    fn fast_forward_matches_single_tick_advances() {
        let origin = Vec3::new(1.0, 50.0, -3.0);
        let v0 = Vec3::new(600.0, 20.0, 0.0);
        let k = drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        let n = 7;

        // N single-tick advances by hand.
        let (mut pos, mut vel) = (origin, v0);
        for _ in 0..n {
            (pos, vel) = advance_shell(pos, vel, k, dt);
        }

        let (ff_pos, ff_vel, path) = fast_forward_shell(origin, v0, k, dt, n);
        assert_eq!(ff_pos, pos, "fast-forward position == N single advances");
        assert_eq!(ff_vel, vel, "fast-forward velocity == N single advances");
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
    use bevy::prelude::*;
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
        penetrated: bool,
        deflection: Option<Vec3>,
    }

    /// The capture sink: every `Impact` the march fires lands here (view-only observer stand-in).
    #[derive(Resource, Default)]
    struct ImpactLog(Vec<Captured>);

    fn capture_impact(impact: On<Impact>, mut log: ResMut<ImpactLog>) {
        log.0.push(Captured {
            position: impact.position,
            surface: impact.surface,
            penetrated: impact.penetrated,
            deflection: impact.deflection,
        });
    }

    #[derive(Resource, Default)]
    struct DamageLog(Vec<(ShotId, f32)>);

    fn capture_damage(damage: On<ShellDamage>, mut log: ResMut<DamageLog>) {
        log.0.push((damage.shot, damage.amount));
    }

    /// Steel: reference-mm of armor per metre of material, so a plate's cost ≈ its thickness in mm
    /// (matches `sandbox::spawn_targets`).
    const STEEL: f32 = 1000.0;

    /// Build an Avian world with one static steel plate (full extents `size`, centred at `at`, facing
    /// ±Z) on the `Armor` layer, register the real march + the impact capture, and settle the physics
    /// so the spatial-query pipeline includes the plate before any shell is marched.
    fn world_with_plate(size: Vec3, at: Vec3) -> App {
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

        // Drive plugin finish/cleanup by hand (a bare `update()` loop skips it) — Avian registers its
        // diagnostics resources in `Plugin::finish`, and the spatial-query systems require them.
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
                material_factor: STEEL,
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
            Projectile {
                velocity: dir * speed,
                caliber: 0.088,
                mass: 10.2,
                drag_k: drag_k(0.088, 10.2),
                // Anchored on THIS fixture's fire direction, exactly as `on_fire_shell` would.
                disc: test_disc(dir),
            },
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![origin],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            ShellReadout {
                speed,
                capability: capability(10.2, speed),
            },
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

    /// The same crossing that moves the authority body ARMS its hull-shock ledger, and a replica
    /// arms nothing — the mirror of [`hit_impulse_changes_only_the_authority_body`] on the fact the
    /// owner is owed rather than on the momentum itself.
    ///
    /// Only ARMING is asserted here: publishing is the episode rule's decision (`net::protocol`),
    /// and this world has no timeline to decide it on.
    #[test]
    fn hit_impulse_arms_the_hull_shock_ledger_only_on_the_authority() {
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
                    HullShockLedger::default(),
                ))
                .id();
            let mut plates = app
                .world_mut()
                .query_filtered::<Entity, With<BallisticVolume>>();
            let plate = plates.single(app.world()).expect("one plate");
            app.world_mut().entity_mut(plate).insert(VolumeOf(body));
            app.update();

            assert_eq!(
                app.world().get::<HullShockLedger>(body).unwrap().pending(),
                None,
                "an unstruck hull owes its owner nothing",
            );
            let impacts = fire_and_capture(&mut app, Vec3::new(0.0, 2.0, 2.0), Vec3::NEG_Z, 800.0);
            assert_eq!(impacts.len(), 1, "the owned plate was crossed once");
            let pending = app.world().get::<HullShockLedger>(body).unwrap().pending();

            if replica {
                assert_eq!(
                    pending, None,
                    "a replica never authors the fact it was shot — it is told",
                );
            } else {
                assert!(
                    pending.is_some(),
                    "the crossing that moved the authority body must owe its owner a shock",
                );
            }
        }
    }

    /// The episode rule, on the ledger that implements it: the first shock closes immediately, a
    /// shock landing inside the open window is DEFERRED rather than dropped, and it closes the tick
    /// the window expires. The deferral is the whole correctness argument — a main-gun round
    /// arriving behind an MG pellet must not be silently swallowed.
    #[test]
    fn a_shock_inside_an_open_episode_is_deferred_not_dropped() {
        const WINDOW: u32 = 16;
        let mut ledger = HullShockLedger::default();

        assert_eq!(ledger.close_episode(100, WINDOW), None, "nothing armed");
        ledger.arm(ShockCause::Ricochet);
        assert_eq!(
            ledger.close_episode(100, WINDOW),
            Some(ClosedEpisode {
                cause: ShockCause::Ricochet,
                opened: 100,
            }),
            "an isolated shock closes in its own tick — no latency the player can feel, and it \
             spans that ONE tick, not a window's worth",
        );
        assert_eq!(
            ledger.close_episode(101, WINDOW),
            None,
            "nothing left armed"
        );

        // A burst: every pellet inside the open window coalesces into ONE episode, and the most
        // severe cause is the one the owner is told about.
        for tick in 104..=112 {
            ledger.arm(ShockCause::Ricochet);
            assert_eq!(
                ledger.close_episode(tick, WINDOW),
                None,
                "tick {tick} is inside the open episode",
            );
        }
        ledger.arm(ShockCause::Perforation);
        ledger.arm(ShockCause::Ricochet);
        assert_eq!(ledger.close_episode(115, WINDOW), None);
        assert_eq!(
            ledger.close_episode(116, WINDOW),
            Some(ClosedEpisode {
                cause: ShockCause::Perforation,
                opened: 104,
            }),
            "the deferred episode closes the tick its window expires, naming its worst hit and the \
             tick its FIRST hit landed on",
        );
        assert_eq!(ledger.close_episode(117, WINDOW), None);
    }

    /// THE SPAN IS EXACT FOR EVERY EPISODE, INCLUDING THE FIRST — the property `net::adoption`'s
    /// spark correlation rests on, and the one a `close − SHOCK_EPISODE_TICKS` window did not have.
    ///
    /// A fresh ledger has no open episode to defer behind, so its first hit publishes on its own
    /// tick and spans exactly that tick. Every later episode opens strictly after the previous one
    /// closed. So the spans of consecutive episodes on one hull never overlap, and a fresh hull's
    /// first episode never reaches back over ticks it did not cover — which, because a respawn
    /// keeps the combatant identity, is where a PREVIOUS life's hits live.
    #[test]
    fn episode_spans_never_overlap_and_a_fresh_ledgers_first_spans_one_tick() {
        const WINDOW: u32 = 16;
        let mut ledger = HullShockLedger::default();
        let mut spans: Vec<(u32, u32)> = Vec::new();

        // ONE monotonic clock, exactly as `close_hull_shock_episodes` sees it — every tick, after
        // the march that arms. An isolated hit at 100, a burst at 104/110, an isolated one at 140.
        const HITS: [u32; 4] = [100, 104, 110, 140];
        for now in 100..=160 {
            if HITS.contains(&now) {
                ledger.arm(ShockCause::Embed);
            }
            if let Some(episode) = ledger.close_episode(now, WINDOW) {
                spans.push((episode.opened, now));
            }
        }

        assert_eq!(
            spans,
            vec![(100, 100), (104, 116), (140, 140)],
            "the first hit publishes alone on its own tick; the burst coalesces into one episode \
             spanning its first hit to the tick the window expired; the last is isolated again",
        );
        for pair in spans.windows(2) {
            let ((_, closed), (opened, _)) = (pair[0], pair[1]);
            // PLAIN `>`, deliberately, and the assumption is the one lightyear already makes: the
            // authority tick counter is a u32 with saturating arithmetic and plain ordering, so it
            // does not wrap inside any session this game can have (~828 days at 64 Hz). This is NOT
            // a wrap-general proof of disjointness and must not be read as one.
            assert!(
                opened > closed,
                "episode spans must be disjoint: {:?} then {:?}",
                pair[0],
                pair[1],
            );
        }
        assert!(
            spans
                .iter()
                .all(|(opened, closed)| closed - opened < WINDOW),
            "no episode can span more than its window: {spans:?}",
        );
    }

    /// The owner half is a monotonic comparison, not an event subscription: replication carries
    /// STATE, so a count that was bumped and restored inside one send window is only ever seen as a
    /// final value, and rewinding the mark (what local rollback does) must re-arm realization.
    #[test]
    fn realization_is_a_rewindable_comparison_not_an_event() {
        let mut ledger = HullShockLedger::default();
        assert!(ledger.is_realized(0), "a never-shot hull owes nothing");
        assert!(!ledger.is_realized(7));

        ledger.realize(7);
        assert!(ledger.is_realized(7), "realizing twice is a no-op");

        // What a rollback does to this component: restore the pre-shock value from history.
        let rewound = HullShockLedger::default();
        assert!(!rewound.is_realized(7), "replay must re-realize the shock");
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
                    Projectile {
                        velocity: dir * 755.0,
                        caliber: 0.0079,
                        mass: 0.0118,
                        drag_k: drag_k(0.0079, 0.0118),
                        disc: test_disc(dir),
                    },
                    DamageReport::default(),
                    TerminalReport::default(),
                    ShellPath {
                        points: vec![origin],
                        segment_starts: Vec::new(),
                    },
                    PenetrationMarks::default(),
                    SpallMarks::default(),
                    ShellReadout {
                        speed: 755.0,
                        capability: capability(0.0118, 755.0),
                    },
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

        let spawn_component = |app: &mut App, z: f32, current: f32| {
            app.world_mut()
                .spawn((
                    Transform::from_translation(Vec3::new(0.0, 2.0, z)),
                    RigidBody::Static,
                    Collider::cuboid(3.0, 3.0, 0.10),
                    CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                    BallisticVolume {
                        material_factor: 1.0,
                        substance: "RHA".to_string(),
                    },
                    ComponentHealth {
                        current,
                        max: 100.0,
                    },
                ))
                .id()
        };
        let spent = spawn_component(&mut app, -20.0, 0.0);
        let first_live = spawn_component(&mut app, -40.0, 100.0);
        let second_live = spawn_component(&mut app, -60.0, 100.0);
        for _ in 0..8 {
            app.update();
        }

        let shot = a_shot();
        let origin = Vec3::new(0.0, 2.0, 2.0);
        let shell = app
            .world_mut()
            .spawn((
                Projectile {
                    velocity: Vec3::NEG_Z * 800.0,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(Vec3::NEG_Z),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed: 800.0,
                    capability: capability(10.2, 800.0),
                },
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
        assert_eq!(health(spent), 0.0, "zero HP cannot produce a fake decrease");
        assert!(
            health(first_live) < 100.0,
            "the first live component took damage"
        );
        assert!(
            health(second_live) < 100.0,
            "the penetrator kept damaging later geometry"
        );
        assert_eq!(
            app.world().resource::<DamageLog>().0.len(),
            1,
            "one damaging shot produces one discrete confirmation across all later deposits"
        );
        assert_eq!(app.world().resource::<DamageLog>().0[0].0, shot);
        assert!(app.world().resource::<DamageLog>().0[0].1 > 0.0);
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
        assert!(
            hit.penetrated,
            "a clean perforation is a penetration (flame lick earned)"
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
        assert!(!hit.penetrated, "a ricochet bit no steel — no flame lick");
        let deflect = hit
            .deflection
            .expect("a ricochet carries its outgoing direction");
        assert!(
            deflect.z > 0.0,
            "the bounce deflects back off the face (+Z), got {deflect:?}"
        );
    }

    /// Raise an unkeyed catch-up shell and return its fallback impacts.
    fn fire_shell_catch_up(app: &mut App, catch_up_ticks: u32) -> Vec<Captured> {
        app.insert_resource(ProjectileAssets {
            scene: Handle::default(),
        });
        app.insert_resource(TracerAssets {
            mesh: Handle::default(),
            material: Handle::default(),
        });
        app.add_observer(on_fire_shell);
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
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
        app.insert_resource(ProjectileAssets {
            scene: Handle::default(),
        });
        app.insert_resource(TracerAssets {
            mesh: Handle::default(),
            material: Handle::default(),
        });
        app.init_resource::<SpawnedShot>()
            .add_observer(on_fire_shell)
            .add_observer(capture);

        let shot = a_shot();
        let shooter = Entity::PLACEHOLDER;
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
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
        app.insert_resource(crate::PredictedPresent(120));
        app.insert_resource(ProjectileAssets {
            scene: Handle::default(),
        });
        app.insert_resource(TracerAssets {
            mesh: Handle::default(),
            material: Handle::default(),
        });
        app.add_observer(on_fire_shell);

        let shot = a_shot();
        app.world_mut().trigger(FireShell {
            origin: Vec3::new(0.0, 2.0, 2.0),
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
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
            .expect("the armor catch-up keeps one hidden, keyed shell waiting for authority");
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Hidden),
            "the candidate shell is invisible while it awaits authority"
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
                victim: None,
            },
        );
        app.update();

        assert!(
            app.world().get::<Held>(shell).is_none(),
            "the keyframe releases the held catch-up shell"
        );
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "the sanctioned keyframe makes the continued shell visible again"
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
        assert!(!hits[0].penetrated, "no flame lick — neutral spark");
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
        assert!(hits[0].penetrated, "authority perforates (flame lick)");
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
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(dir),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
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
                Projectile {
                    velocity: dir.normalize() * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(dir.normalize()),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
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
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
                victim: None,
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
                // Inert in these tests (no `PredictedPresent` resource → the F3 overdue path is off);
                // the hold/pre-armed paths under test don't read it.
                bounce_tick: 0,
                sequence: 0,
                victim: None,
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
        // HELD_EXTRA ticks — the exact shared integrator, so it is consistent with the present timeline.
        let dt = 0.016; // the test world's ManualDuration
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "resumed position = sanctioned bounce fast-forwarded by the hold duration (got {pos}, want {expected_pos})",
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

    /// Set the replica world's replay flag.
    fn set_replaying(app: &mut App, replaying: bool) {
        app.insert_resource(crate::Replaying(replaying));
    }

    /// Regression: replayed ticks do not advance a cosmetic shell.
    #[test]
    fn rollback_replay_freezes_the_cosmetic_march() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        let origin = Vec3::new(0.0, 2.0, 5.0);
        let dir = Vec3::Z;
        let speed = 800.0;
        let shell = app
            .world_mut()
            .spawn((
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(dir),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id();

        app.update();
        assert!(
            app.world().get::<Held>(shell).is_none(),
            "baseline: the shell is still free-flying, not yet at contact",
        );
        let pos_before = app.world().get::<Transform>(shell).unwrap().translation;
        let vel_before = app.world().get::<Projectile>(shell).unwrap().velocity;
        let points_before = app.world().get::<ShellPath>(shell).unwrap().points.len();

        set_replaying(&mut app, true);
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Transform>(shell).unwrap().translation,
            pos_before,
            "a replayed tick must not advance the shell (no double-march teleport)",
        );
        assert_eq!(
            app.world().get::<Projectile>(shell).unwrap().velocity,
            vel_before,
            "a replayed tick must not integrate the shell's velocity",
        );
        assert_eq!(
            app.world().get::<ShellPath>(shell).unwrap().points.len(),
            points_before,
            "a replayed tick must not append duplicate ShellPath points",
        );
        assert!(
            app.world().resource::<ImpactLog>().0.is_empty(),
            "a replayed tick fires no impact",
        );

        set_replaying(&mut app, false);
        app.update();
        assert_ne!(
            app.world().get::<Transform>(shell).unwrap().translation,
            pos_before,
            "a forward tick resumes the march",
        );
    }

    /// Regression: replayed ticks do not age a hold or its re-seed.
    #[test]
    fn rollback_replay_does_not_age_the_hold_and_reseed_stays_exact() {
        let shot = a_shot();
        let bounce = authority_bounce(shot);
        let mut app = replica_world(SanctionedShots::default());
        let shell = spawn_oblique_shell(&mut app, shot);

        for _ in 0..8 {
            app.update();
            if app.world().get::<Held>(shell).is_some() {
                break;
            }
        }
        assert!(app.world().get::<Held>(shell).is_some(), "held at contact");

        const HELD_FWD: u32 = 4;
        for _ in 0..HELD_FWD {
            app.update();
        }
        assert_eq!(app.world().get::<Held>(shell).unwrap().waited, HELD_FWD);

        set_replaying(&mut app, true);
        for _ in 0..8 {
            app.update();
        }
        assert_eq!(
            app.world().get::<Held>(shell).unwrap().waited,
            HELD_FWD,
            "a replay must not age the hold window (it would burn the grace window and over-age the re-seed)",
        );

        set_replaying(&mut app, false);
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce.origin,
                direction: bounce.direction,
                speed: bounce.speed,
                bounce_tick: 0,
                sequence: 0,
                victim: None,
            },
        );
        app.update();
        let dt = 0.016;
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_FWD,
        );
        let pos = app.world().get::<Transform>(shell).unwrap().translation;
        assert!(
            pos.distance(expected_pos) < 1.0e-3,
            "re-seed re-aged by the TRUE hold count, not the storm's replays (got {pos}, want {expected_pos})",
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
            victim: None,
        };
        let b1 = SanctionedBounce {
            origin: Vec3::new(0.0, 2.0, 0.05),
            direction: Vec3::Z,
            speed: 300.0,
            bounce_tick: 0,
            sequence: 1,
            victim: None,
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
        assert!(
            froze,
            "the own shell holds at armor contact like any Shot-carrying shell"
        );
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Hidden),
            "the hold is an INVISIBLE stop — no frozen round hanging on the plate",
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
                victim: None,
            },
        );
        app.update();

        assert!(app.world().get::<Held>(shell).is_none(), "hold cleared");
        assert_eq!(
            app.world().get::<Visibility>(shell),
            Some(&Visibility::Inherited),
            "the re-seeded shell is shown again",
        );
        let marks = app.world().get::<PenetrationMarks>(shell).unwrap();
        assert_eq!(
            marks.ricochets.len(),
            1,
            "the sanctioned bounce re-anchored the clamp"
        );
        // Re-aged by the held ticks — the same present − bounce_tick arithmetic as an observer shell.
        let dt = 0.016;
        let (expected_pos, _, _) = fast_forward_shell(
            bounce.origin,
            bounce.direction.normalize() * bounce.speed,
            drag_k(0.088, 10.2),
            dt,
            HELD_EXTRA,
        );
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
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 20));
        let bounce_origin = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: bounce_origin,
                direction: Vec3::Z,
                speed: 500.0,
                bounce_tick: shot.fire_tick + 5,
                sequence: 0,
                victim: None,
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
    /// armor read (position, normal, `penetrated`) rather than holding for a contact that never comes.
    #[test]
    fn overdue_terminal_finalizes_a_pose_divergent_miss() {
        let shot = a_shot();
        let mut app = replica_world(SanctionedShots::default());
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 20));
        let impact_pos = Vec3::new(1.0, 2.0, 3.0);
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert_terminal(
                shot,
                SanctionedTerminal {
                    position: impact_pos,
                    normal: Vec3::Z,
                    penetrated: true,
                    impact_tick: shot.fire_tick + 5,
                    after_bounces: 0,
                    victim: None,
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
            impacts[0].penetrated,
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
        app.insert_resource(crate::PredictedPresent(shot.fire_tick + 5));
        app.world_mut().resource_mut::<SanctionedShots>().insert(
            shot,
            SanctionedBounce {
                origin: Vec3::new(1.0, 2.0, 3.0),
                direction: Vec3::Z,
                speed: 500.0,
                // present(105) − bounce_tick(104) = 1 <= OVERDUE_MARGIN_TICKS — inside the margin.
                bounce_tick: shot.fire_tick + 4,
                sequence: 0,
                victim: None,
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
        penetrated: bool,
        after_bounces: u32,
    }

    #[derive(Resource, Default)]
    struct TerminalLog(Vec<CapturedTerminal>);

    fn capture_terminal(terminal: On<ShellTerminal>, mut log: ResMut<TerminalLog>) {
        log.0.push(CapturedTerminal {
            position: terminal.position,
            penetrated: terminal.penetrated,
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
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(dir),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![origin],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed,
                    capability: capability(10.2, speed),
                },
                Transform::from_translation(origin).looking_to(dir, Vec3::Y),
                Shot(shot),
            ))
            .id()
    }

    /// AUTHORITY EMISSION — embed. A 500 mm plate defeats the 88 head-on (cost 500 > ~263 cap): the
    /// march raises exactly ONE `ShellTerminal`, penetrated (an embed bit steel), zero prior bounces.
    #[test]
    fn authority_embed_emits_one_penetrated_terminal() {
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
        assert!(terminals[0].penetrated, "an embed bit steel — flame lick");
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
                material_factor: STEEL,
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
        assert!(terminals[0].penetrated, "a perforation breached the plate");
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
    /// feedback (flame lick via `penetrated: true`), typically ≈(P−S)+OWL after contact instead of
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
                    penetrated: true,
                    impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                    after_bounces: 0,
                    victim: None,
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
            impacts[0].penetrated,
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
                penetrated: true,
                impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                after_bounces: 0,
                victim: None,
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
        assert!(impacts[0].penetrated, "server verdict rides through");
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
                penetrated: false,
                impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                after_bounces: 1,
                victim: None,
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
                bounce_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                sequence: 0,
                victim: None,
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
                    penetrated: true,
                    impact_tick: 0, // inert (no `PredictedPresent` — F3 overdue path off)
                    after_bounces: 0,
                    victim: None,
                },
            );
        app.update();

        assert!(
            app.world().get_entity(shell).is_err(),
            "the shooter's own shell resolves on its confirm — the honest read on their own hit",
        );
        let impacts = app.world().resource::<ImpactLog>().0.clone();
        assert_eq!(impacts.len(), 1);
        assert!(impacts[0].penetrated, "own hit shows the flame lick in MP");
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
                penetrated: true,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 0,
                victim: None,
            },
        );
        // Even a corrupt divergent duplicate must not replace the first terminal.
        buf.insert_terminal(
            shot,
            SanctionedTerminal {
                position: Vec3::Y * 99.0,
                normal: Vec3::Z,
                penetrated: false,
                impact_tick: 0, // inert (dedup test — buffer semantics, not the march)
                after_bounces: 3,
                victim: None,
            },
        );
        let stored = buf.terminal(shot, 0).expect("terminal stored");
        assert_eq!(stored.position, Vec3::X, "first insert wins");
        assert!(stored.penetrated, "first insert's verdict kept");
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
        assert!(!hit.penetrated, "terrain never reads as a penetration");
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
        assert!(armor.penetrated, "an 88 perforates 50 mm flat");
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
    fn box_trimesh(size: Vec3) -> (Collider, BallisticSurfaces) {
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
    fn world_with_trimesh_plates(plates: &[(Vec3, Vec3)]) -> App {
        let mut app = world_with_plate(Vec3::splat(0.01), Vec3::new(0.0, -500.0, 0.0));
        for (size, at) in plates {
            app.world_mut().spawn((
                Transform::from_translation(*at),
                RigidBody::Static,
                box_trimesh(*size),
                CollisionLayers::new([Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material_factor: STEEL,
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
                Projectile {
                    velocity: Vec3::NEG_Z * 800.0,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
                    disc: test_disc(Vec3::NEG_Z),
                },
                DamageReport::default(),
                TerminalReport::default(),
                ShellPath {
                    points: vec![Vec3::new(0.0, 2.0, 2.0)],
                    segment_starts: Vec::new(),
                },
                PenetrationMarks::default(),
                SpallMarks::default(),
                ShellReadout {
                    speed: 800.0,
                    capability: capability(10.2, 800.0),
                },
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
    /// Two arrangements, each pinning one direction against the 88's capability (~261 reference-mm):
    /// a union of 325 mm must defeat the round even though its first plate alone (150 mm) would not,
    /// and a union of 250 mm must let it through even though the two plates SUMMED (300 mm) would
    /// not.
    #[test]
    fn overlapping_trimesh_plates_charge_their_union_once() {
        // Undercharge — the defect that shipped. Union 325 mm.
        assert!(
            perforates(&[slab(0.15, 0.05)]),
            "the first plate alone does not defeat the round",
        );
        assert!(
            !perforates(&[slab(0.15, 0.05), slab(0.20, -0.10)]),
            "the overlapped second plate is charged, not crossed for free",
        );

        // Overcharge — War Thunder's failure mode. Union 250 mm, sum 300 mm.
        assert!(
            perforates(&[slab(0.15, 0.05), slab(0.15, -0.05)]),
            "the shared 50 mm is charged ONCE; summing it would have defeated the round",
        );
        assert!(
            !perforates(&[slab(0.30, 0.0)]),
            "300 mm really is beyond it — the control that makes the line above mean something",
        );
    }

    /// SEAM INVISIBILITY as an outcome: plates meeting exactly resolve as the one plate they add up
    /// to, on BOTH sides of the capability. Under the serial resolver the 1 mm nudge hopped the
    /// shared face and the second plate was free, so perfect abutment bought nothing — which is why
    /// the authoring standard and the resolver had to be redesigned together.
    #[test]
    fn exactly_abutting_trimesh_plates_resolve_as_one_plate() {
        for (total, split) in [
            (0.25f32, [slab(0.125, 0.0625), slab(0.125, -0.0625)]),
            (0.325, [slab(0.15, 0.0875), slab(0.175, -0.0875)]),
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
                    material_factor: STEEL,
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
    /// through — no terminal, no perforation, one bare `penetrated: false` spark.
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
                impact.penetrated,
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
        // between them.
        assert!(
            impacts
                .iter()
                .all(|impact| !(-0.425..=0.475).contains(&impact.position.z)),
            "no armour was invented in 900 mm of air: {impacts:?}",
            impacts = impacts.iter().map(|i| i.position).collect::<Vec<_>>(),
        );

        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "one terminal per shot");
        assert!(terminals[0].penetrated, "the terminal breached the plate");
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
            assert!(impact.penetrated, "an 88 still goes through 50 mm out here");
            assert!(
                (impact.position.z - (far.z + face)).abs() < 1.0e-3,
                "the read is at the plate's own entry face (expected z={}, got {})",
                far.z + face,
                impact.position.z,
            );
        }
        let terminals = app.world().resource::<TerminalLog>().0.clone();
        assert_eq!(terminals.len(), 1, "one terminal per shot");
        assert!(terminals[0].penetrated);
    }

    /// FREE PENETRATION IS THE ONE OUTCOME WORSE THAN A STOPPED SHELL.
    ///
    /// Every observed instance of the prune defect landed on a TRANSIT corridor, where a dropped
    /// entry leaves a bare exit and the walk shouts `UnexpectedExit` — wrong, but loud, and the
    /// driver fails closed on it. The same dropped face on an ENTRANCE corridor is silent: `begin`
    /// sees no material, returns [`walk::Begin::Miss`], and `resolve_crossing` flies the round
    /// through the whole examined corridor with zero cost, zero damage and NO `Impact` at all. That
    /// is armour that was never modelled, which is precisely the defect class §13 exists to kill,
    /// and it would leave no trace in any log.
    ///
    /// So: put the entrance corridor's origin exactly on a plate's face, and a few ULP either side
    /// of it, by spawning the shell one march nudge in front. Whatever else happens, the round may
    /// not pass through unremarked.
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

            let impacts = app.world().resource::<ImpactLog>().0.clone();
            assert!(
                !impacts.is_empty(),
                "the round met the plate at {step} ULP and something was reported for it — a \
                 silent pass-through is free armour",
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
                impacts[0].penetrated,
                "at {step} ULP the crossing resolved, rather than failing closed",
            );
            assert_eq!(
                app.world().resource::<TerminalLog>().0.len(),
                1,
                "and it reported its one terminal",
            );
        }
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
            victim: None,
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
            !impacts[0].penetrated,
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
        let (_, stepped) = advance_shell(start, Vec3::NEG_Z * 800.0, drag_k(0.088, 10.2), dt);
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
                    material_factor: STEEL,
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
        let (free_flight, _) = advance_shell(start, Vec3::NEG_Z * 800.0, drag_k(0.088, 10.2), dt);

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
                material_factor: 0.0,
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
                    material_factor: STEEL,
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
            impacts.iter().all(|impact| impact.penetrated),
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
