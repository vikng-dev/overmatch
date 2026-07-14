//! Shared shell flight, collision queries, penetration, and impacts.

use std::time::Instant;

use avian3d::prelude::{Forces, LayerMask, SpatialQuery, SpatialQueryFilter, WriteRigidBodyForces};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
// `shot_trace::record` only evaluates its closure when tracing is armed.
use serde_json::json;

use crate::damage::{VolumeOf, hit_ancestor};
use crate::state::GameplaySet;
use crate::{ClientReplica, Layer, PredictedPresent, Replaying, ShotId};

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

/// A server-sanctioned ricochet consumed by a client cosmetic shell.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedBounce {
    /// The exact server bounce point — where the re-seeded shell restarts.
    pub origin: Vec3,
    /// The post-bounce travel direction (unit; the receiver guards it before use).
    pub direction: Vec3,
    /// The post-bounce speed (m/s).
    pub speed: f32,
    /// Server tick where this bounce resolved.
    pub bounce_tick: u32,
    /// Zero-based ordinal, consumed strictly in order.
    pub sequence: u32,
}

/// A server-sanctioned armor terminal consumed by a client cosmetic shell.
#[derive(Clone, Copy)]
pub(crate) struct SanctionedTerminal {
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal, straight from the server's raycast.
    pub normal: Vec3,
    /// The server's penetration verdict — gates the flame lick, exactly as the authority's read did.
    pub penetrated: bool,
    /// Server tick where this terminal resolved.
    pub impact_tick: u32,
    /// Required number of prior bounces before this terminal may be consumed.
    pub after_bounces: u32,
}

/// Per-shot sanctioned state: ordered bounces + the (at most one) terminal, plus an age for expiry.
struct SanctionedShot {
    bounces: Vec<SanctionedBounce>,
    terminal: Option<SanctionedTerminal>,
    /// Seconds since last touched — evicted once it outlives any shell that could still consume it.
    age: f32,
}

/// Bounded client buffer of server-sanctioned outcomes, keyed by [`ShotId`].
#[derive(Resource, Default)]
pub(crate) struct SanctionedShots {
    shots: std::collections::HashMap<ShotId, SanctionedShot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SanctionedBounceInsert {
    Inserted,
    Duplicate,
    Capacity,
}

impl SanctionedShots {
    /// Configured expiry for unconsumed authority outcomes; recorded in trace metadata.
    pub(crate) const MAX_AGE_SECS: f32 = 3.0;
    /// DERIVED backstop: `30 combatants * 2 weapons * 750 rounds/min * 3 s / 60 = 2,250` shots;
    /// 4,096 is the next power of two.
    const MAX_SHOTS: usize = 4_096;
    /// DERIVED: a shot cannot consume more bounces than the cosmetic segment-work horizon.
    pub(crate) const MAX_BOUNCES_PER_SHOT: usize = MAX_COSMETIC_CATCH_UP_TICKS as usize;

    /// Stable tie-break among already-buffered entries: the greatest tuple is evicted.
    fn eviction_key(shot: ShotId) -> (u64, u8, u32) {
        (shot.combatant.0, shot.weapon, shot.fire_tick)
    }

    /// This shot's entry, fresh-touched, with the over-cap eviction applied.
    fn entry(&mut self, shot: ShotId) -> &mut SanctionedShot {
        if self.shots.len() >= Self::MAX_SHOTS && !self.shots.contains_key(&shot) {
            // Capacity overflow evicts one oldest entry; normal removal remains time-based. Equal
            // ages use the stable ShotId-derived order so cosmetic traces reproduce across runs.
            if let Some(oldest) = self
                .shots
                .iter()
                .max_by(|(a_shot, a), (b_shot, b)| {
                    a.age.total_cmp(&b.age).then_with(|| {
                        Self::eviction_key(**a_shot).cmp(&Self::eviction_key(**b_shot))
                    })
                })
                .map(|(k, _)| *k)
            {
                self.shots.remove(&oldest);
            }
        }
        let entry = self.shots.entry(shot).or_insert_with(|| SanctionedShot {
            bounces: Vec::new(),
            terminal: None,
            age: 0.0,
        });
        entry.age = 0.0;
        entry
    }

    /// Insert a server-sanctioned bounce idempotently by `(shot, sequence)`.
    pub(crate) fn insert(
        &mut self,
        shot: ShotId,
        bounce: SanctionedBounce,
    ) -> SanctionedBounceInsert {
        let entry = self.entry(shot);
        if entry.bounces.iter().any(|b| b.sequence == bounce.sequence) {
            return SanctionedBounceInsert::Duplicate;
        }
        if entry.bounces.len() >= Self::MAX_BOUNCES_PER_SHOT {
            return SanctionedBounceInsert::Capacity;
        }
        entry.bounces.push(bounce);
        SanctionedBounceInsert::Inserted
    }

    /// Record a shot's terminal, idempotently by [`ShotId`].
    ///
    /// INVARIANT: [`TerminalReport`] permits at most one authority terminal, so first insert wins.
    pub(crate) fn insert_terminal(&mut self, shot: ShotId, terminal: SanctionedTerminal) -> bool {
        let entry = self.entry(shot);
        if entry.terminal.is_some() {
            return false;
        }
        entry.terminal = Some(terminal);
        true
    }

    /// Whether anything is buffered under this exact [`ShotId`].
    #[cfg(test)]
    pub(crate) fn has_shot(&self, shot: ShotId) -> bool {
        self.shots.contains_key(&shot)
    }

    /// The next ordered bounce, if it has arrived.
    fn next(&self, shot: ShotId, consumed: usize) -> Option<SanctionedBounce> {
        self.shots
            .get(&shot)
            .and_then(|e| e.bounces.iter().find(|b| b.sequence as usize == consumed))
            .copied()
    }

    /// The terminal only after all of its preceding bounces have been consumed.
    fn terminal(&self, shot: ShotId, consumed: usize) -> Option<SanctionedTerminal> {
        self.shots
            .get(&shot)
            .and_then(|e| e.terminal)
            .filter(|t| t.after_bounces as usize == consumed)
    }

    /// Age every tracked shot and evict those past [`Self::MAX_AGE_SECS`]. Driven by `net::client`.
    pub(crate) fn age(&mut self, dt: f32) {
        for entry in self.shots.values_mut() {
            entry.age += dt;
        }
        self.shots.retain(|_, e| e.age <= Self::MAX_AGE_SECS);
    }
}

/// One authority-bounded free-flight segment beginning at a sanctioned bounce.
struct SanctionedFlightSegment {
    bounce: SanctionedBounce,
    points: Vec<Vec3>,
}

/// A client catch-up through every already-buffered authority outcome up to `present`.
struct SanctionedCatchUp {
    segments: Vec<SanctionedFlightSegment>,
    position: Vec3,
    velocity: Vec3,
    terminal: Option<SanctionedTerminal>,
}

/// Why an authority-outcome chain cannot be reconstructed safely on this client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SanctionedCatchUpReject {
    IntervalBeyondCosmeticHorizon,
    ChainBeyondCosmeticHorizon,
}

impl SanctionedCatchUpReject {
    fn trace_reason(self) -> &'static str {
        match self {
            Self::IntervalBeyondCosmeticHorizon => "interval_beyond_cosmetic_horizon",
            Self::ChainBeyondCosmeticHorizon => "chain_beyond_cosmetic_horizon",
        }
    }
}

/// Reserve one disconnected authority segment before integrating it.
///
/// INVARIANT: one cosmetic reconstruction may integrate at most
/// [`MAX_COSMETIC_CATCH_UP_TICKS`] total steps and materialize at most that many authority
/// segments. The segment limit also bounds same-tick bounce chains, whose elapsed steps are zero.
fn reserve_sanctioned_catch_up_work(
    integrated_ticks: &mut u32,
    segments: &mut u32,
    steps: u32,
) -> Result<(), SanctionedCatchUpReject> {
    if *segments >= MAX_COSMETIC_CATCH_UP_TICKS
        || steps > MAX_COSMETIC_CATCH_UP_TICKS.saturating_sub(*integrated_ticks)
    {
        return Err(SanctionedCatchUpReject::ChainBeyondCosmeticHorizon);
    }
    *integrated_ticks += steps;
    *segments += 1;
    Ok(())
}

/// Fast-forward authority outcomes without integrating through a known later outcome.
///
/// INVARIANT: outcome ingress may retain early or out-of-order facts, but this is the sole path to
/// [`fast_forward_shell`] for a sanctioned chain; every segment reserves bounded work before it can
/// allocate or integrate.
fn catch_up_sanctioned_chain(
    shot: ShotId,
    consumed: usize,
    first: SanctionedBounce,
    present: Option<u32>,
    fallback_age: u32,
    sanctioned: &SanctionedShots,
    fallback_velocity: Vec3,
    drag_k: f32,
    dt: f32,
) -> Result<SanctionedCatchUp, SanctionedCatchUpReject> {
    enum NextOutcome {
        Bounce(SanctionedBounce, u32),
        Terminal(SanctionedTerminal, u32),
    }

    let mut segments = Vec::new();
    let mut bounce = first;
    let mut seed_velocity =
        Dir3::new(bounce.direction).map_or(fallback_velocity, |dir| Vec3::from(dir) * bounce.speed);
    let mut consumed = consumed + 1;
    let mut integrated_ticks = 0;
    let mut segment_count = 0;
    loop {
        let next = present.and_then(|present| {
            let due_bounce = sanctioned.next(shot, consumed).and_then(|next| {
                elapsed_ticks(present, next.bounce_tick)?;
                let gap = elapsed_ticks(next.bounce_tick, bounce.bounce_tick)?;
                Some((next, gap))
            });
            let due_terminal = sanctioned.terminal(shot, consumed).and_then(|terminal| {
                elapsed_ticks(present, terminal.impact_tick)?;
                let gap = elapsed_ticks(terminal.impact_tick, bounce.bounce_tick)?;
                Some((terminal, gap))
            });
            match (due_bounce, due_terminal) {
                (Some((next, bounce_gap)), Some((terminal, terminal_gap))) => {
                    if terminal_gap <= bounce_gap {
                        Some(NextOutcome::Terminal(terminal, terminal_gap))
                    } else {
                        Some(NextOutcome::Bounce(next, bounce_gap))
                    }
                }
                (Some((next, gap)), None) => Some(NextOutcome::Bounce(next, gap)),
                (None, Some((terminal, gap))) => Some(NextOutcome::Terminal(terminal, gap)),
                (None, None) => None,
            }
        });

        match next {
            Some(NextOutcome::Bounce(next, gap)) => {
                if gap > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                let steps = gap.saturating_sub(1);
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, steps)?;
                let (_, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, steps);
                segments.push(SanctionedFlightSegment { bounce, points });
                bounce = next;
                seed_velocity = Dir3::new(bounce.direction)
                    .map_or(velocity, |dir| Vec3::from(dir) * bounce.speed);
                consumed += 1;
            }
            Some(NextOutcome::Terminal(terminal, gap)) => {
                if gap > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                let steps = gap.saturating_sub(1);
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, steps)?;
                let (_, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, steps);
                segments.push(SanctionedFlightSegment { bounce, points });
                return Ok(SanctionedCatchUp {
                    segments,
                    position: terminal.position,
                    velocity,
                    terminal: Some(terminal),
                });
            }
            None => {
                let age = present
                    .and_then(|present| elapsed_ticks(present, bounce.bounce_tick))
                    .unwrap_or(fallback_age);
                if age > MAX_COSMETIC_CATCH_UP_TICKS {
                    return Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon);
                }
                reserve_sanctioned_catch_up_work(&mut integrated_ticks, &mut segment_count, age)?;
                let (position, velocity, points) =
                    fast_forward_shell(bounce.origin, seed_velocity, drag_k, dt, age);
                segments.push(SanctionedFlightSegment { bounce, points });
                return Ok(SanctionedCatchUp {
                    segments,
                    position,
                    velocity,
                    terminal: None,
                });
            }
        }
    }
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
    /// The server's penetration verdict (gates the flame lick on the client, as in the local read).
    pub penetrated: bool,
    /// Ricochets resolved before this terminal.
    pub after_bounces: u32,
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
/// On the `Armor` layer. `material_factor` (density/hardness → reference-mm per metre) is authored;
/// the march doesn't spend it yet — that is the next increment.
#[derive(Component)]
pub struct BallisticVolume {
    pub material_factor: f32,
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
}

/// Momentum absorbed by a volume crossing, applied at its entry point.
#[derive(Event)]
struct HitImpulse {
    body: Entity,
    impulse: Vec3,
    point: Vec3,
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
        .insert_resource(MgShortCircuit(
            std::env::var("SPIKE_MG_SHORTCIRCUIT").is_ok(),
        ))
        .add_observer(on_fire_shell)
        .add_observer(on_impact)
        .add_observer(on_hit_impulse)
        .add_systems(Startup, setup_assets)
        // The same march, integrated on whichever clock the mode selects: `Real` on the fixed
        // server step (`Res<Time>` is `Time<Fixed>` here), `Demo` per-frame on virtual time
        // (`Res<Time>` is `Time<Virtual>` here). One reads as the true sim, the other as smooth.
        .add_systems(
            FixedUpdate,
            integrate_projectiles.in_set(GameplaySet).run_if(march_real),
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
        let filter = SpatialQueryFilter::from_mask(
            LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
        );
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
            if let Some(hit) = spatial.cast_ray_predicate(
                segment[0] + Vec3::from(dir) * EPS,
                dir,
                reach,
                true,
                &filter,
                &not_own,
            ) {
                let contact = segment[0] + Vec3::from(dir) * (EPS + hit.distance);
                let surface = if hit_ancestor(hit.entity, &volumes, &parents).is_some() {
                    ImpactSurface::Armor
                } else {
                    ImpactSurface::Terrain
                };

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
                        normal: hit.normal,
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
                        normal: hit.normal,
                        caliber: fire.caliber,
                        surface,
                        penetrated: false,
                        deflection: None,
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
            // A light streak neither casts nor receives shadow — without these the sun dragged a
            // long capsule shadow across the terrain under every tracer.
            NotShadowCaster,
            NotShadowReceiver,
            streak,
        ));
    }
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
    volumes: Query<&BallisticVolume>,
    owners: Query<&VolumeOf>,
    mut health: Query<&mut ComponentHealth>,
    parents: Query<&ChildOf>,
    retain: Res<RetainSpentShells>,
    // Present only on the net client (a replica): shells still fly, raycast, spark, and despawn, but
    // HP deposition and hit impulse are the server's authority. Absent in SP / sandbox / server.
    replica: Option<Res<ClientReplica>>,
    // The observer's server-sanctioned bounce buffer (`net::client` fills it). Present only on a net
    // client; absent in SP/sandbox/server, where the authority march resolves bounces for real and
    // never consults it. An observer shell at armor contact re-seeds from here or holds for it.
    sanctioned: Option<Res<SanctionedShots>>,
    // F1: set (to `true`) only while lightyear is REPLAYING a rollback on a net client. The whole
    // cosmetic march below is skipped on a replayed tick — see the early return. Absent on the
    // authority (never rolls back).
    replaying: Option<Res<Replaying>>,
    // F3: this net client's predicted present tick `P` (every cosmetic shell lives at `P`). Read to
    // consume an OVERDUE sanctioned outcome for a shell that MISSED the plate the server resolved on.
    // Present only on a net client; absent on the authority (it resolves shots for real).
    present: Option<Res<PredictedPresent>>,
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
    spatial: SpatialQuery,
    time: Res<Time>,
    mut commands: Commands,
) {
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
    // F3: the predicted present tick, if this is a net client — the clock the overdue-consumption
    // check below compares each sanctioned outcome's server tick against. Absent on the authority.
    let present = present.map(|p| p.0);
    // The tick every shot-lifecycle row this march writes is stamped with: the predicted present (the
    // tick each cosmetic shell lives at). Never read on the authority — every row site is `!deposit`.
    let now = present.unwrap_or(0);
    // The march casts against terrain (which stops the shell) and ballistic volumes (which it
    // crosses); the struck entity being a `BallisticVolume` is what tells the two apart.
    let world = SpatialQueryFilter::from_mask(
        LayerMask::from(Layer::Terrain) | LayerMask::from(Layer::Armor),
    );
    let armor = SpatialQueryFilter::from_mask(Layer::Armor);
    // Nudge past each boundary we resolve so we don't immediately re-hit it.
    const EPS: f32 = 1.0e-3;
    // How far ahead to search for a volume's far face — its full geometric thickness, even past the
    // end of this step (thin plates resolve well within it).
    const PROBE: f32 = 50.0;
    // Steeper than this from the surface normal, an un-overmatched round ricochets (rad, ~70°).
    const RICOCHET_ANGLE: f32 = 1.221;
    // Speed retained through a ricochet.
    const RICOCHET_BLEED: f32 = 0.6;
    // Shock a glancing bounce jars into an *exposed component* (not armor): scaled by impact energy
    // (capability) × squareness (cos incidence). A graze chips structural integrity without one-
    // shotting; a faint graze barely registers; small arms barely scratch. Armor has no HP → shrugs.
    const SHOCK_K: f32 = 0.045;
    // Share of the impact angle the round straightens toward the normal on entry (normalization).
    const NORMALIZATION: f32 = 0.2;
    // Overmatch when calibre ≥ this × the plate's thickness: ricochet suppressed, slope cancelled.
    const OVERMATCH_RATIO: f32 = 3.0;
    // Spall (design §5). Budget = (material chewed / ref) × (residual energy / ref) × (caliber /
    // ref), capped — both a fragment supply (cost) and a push (v_res²) are needed, so a thin/soft
    // body or a barely-through round throws little. The cone's shape is fixed; only density scales.
    const SPALL_MAX_FRAGMENTS: usize = 24;
    const SPALL_COST_REF: f32 = 100.0; // ref-mm (≈ a 100 mm steel plate)
    const SPALL_VRES_REF: f32 = 500.0; // m/s
    const SPALL_CALIBER_REF: f32 = 0.088; // m (the 88)
    const SPALL_HALF_ANGLE: f32 = 0.35; // rad (~20°)
    const SPALL_RANGE: f32 = 6.0; // m — fragments are short-range
    // Main-penetrator transit damage = cost paid crossing the component × this (design §6).
    const TRANSIT_K: f32 = 1.0;

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
        // Accumulate the authority's actual HP decrease across every direct crossing and spall fragment
        // this step. The per-shell latch below turns the aggregate into at most one discrete confirm.
        let mut damage_dealt = 0.0;
        // SHOOTER SELF-EXCLUSION (see [`not_own_volume`]): this round is transparent to the tank that
        // fired it, for every cast below. Identical on the authority and on a replica — the one place
        // both ends must agree about the shooter's own geometry, or the server's damage model and the
        // client's cosmetic model describe different worlds.
        let shooter = source.map(|source| source.tank);
        let not_own = |entity: Entity| not_own_volume(entity, shooter, &owners, &parents);
        // NET-CLIENT HOLD: a `Shot`-carrying shell — an observer's replica OR the shooter's own
        // predicted round — frozen (and hidden, see the hold insert below) at armor contact, waiting
        // the grace window for the server's verdict on this contact. It does NOT free-flight while
        // held — it resolves from whichever sanctioned outcome arrives (a bounce keyframe → re-seed
        // and continue; a terminal confirm → the full honest armor read at the server's position) or,
        // past the window, degrades to fail-closed truncation. The authority never holds (it resolves
        // shots for real), and a shell with no `Shot` never enters this state (it fail-closes on
        // first contact below).
        if let Some(mut held) = held {
            // The bounce we are waiting on (the next unconsumed ordinal for this shot), if it arrived.
            let arrived = shot.zip(sanctioned.as_ref()).and_then(|(shot, buf)| {
                buf.next(shot.0, marks.ricochets.len())
                    .map(|bounce| (shot.0, buf.as_ref(), bounce))
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
                    marks.ricochets.len(),
                    first_bounce,
                    present,
                    held.age,
                    sanctioned,
                    projectile.velocity,
                    projectile.drag_k,
                    dt,
                ) {
                    Ok(caught_up) => caught_up,
                    Err(reject) => {
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "overdue",
                            now,
                            shot_id,
                            || json!({ "res": "reject", "why": reject.trace_reason() }),
                        );
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "end",
                            now,
                            shot_id,
                            || json!({ "why": "catch_up_reject" }),
                        );
                        commands.entity(entity).despawn();
                        continue;
                    }
                };
                for (index, segment) in caught_up.segments.iter().enumerate() {
                    // The candidate contact and every later authority correction can be spatially
                    // displaced. Each server origin begins a disconnected view segment, never a
                    // fictional correction chord.
                    path.begin_segment();
                    path.points.extend(segment.points.iter().copied());
                    marks.ricochets.push(segment.bounce.origin);
                    commands.trigger(Impact {
                        position: segment.bounce.origin,
                        normal: if index == 0 {
                            held.normal
                        } else {
                            segment.bounce.direction
                        },
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: false,
                        deflection: Some(segment.bounce.direction),
                    });
                    if index == 0 {
                        crate::shot_trace::record(&mut shot_trace, "hold", now, shot_id, || {
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
                            &mut shot_trace,
                            "overdue",
                            now,
                            shot_id,
                            || json!({ "res": "bounce", "late": late, "seq": segment.bounce.sequence, "via": "chain" }),
                        );
                    }
                }

                if let Some(terminal) = caught_up.terminal {
                    path.begin_segment();
                    path.points.push(terminal.position);
                    commands.trigger(Impact {
                        position: terminal.position,
                        normal: terminal.normal,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: terminal.penetrated,
                        deflection: None,
                    });
                    let late = present
                        .and_then(|present| elapsed_ticks(present, terminal.impact_tick))
                        .unwrap_or_default();
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "overdue",
                        now,
                        shot_id,
                        || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated, "via": "chain" }),
                    );
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "end",
                        now,
                        shot_id,
                        || json!({ "why": "terminal" }),
                    );
                    commands.entity(entity).despawn();
                    continue;
                }

                transform.translation = caught_up.position;
                if let Ok(direction) = Dir3::new(caught_up.velocity) {
                    transform.look_to(direction, Vec3::Y);
                }
                projectile.velocity = caught_up.velocity;
                readout.speed = caught_up.velocity.length();
                readout.capability = capability(projectile.mass, caught_up.velocity.length());
                // Un-hide (the hold's invisible-stop) and resume marching next tick.
                commands
                    .entity(entity)
                    .remove::<Held>()
                    .insert(Visibility::Inherited);
                continue;
            }
            // Consume a terminal only after every preceding bounce; re-anchor at server truth.
            let terminal = shot
                .zip(sanctioned.as_ref())
                .and_then(|(s, buf)| buf.terminal(s.0, marks.ricochets.len()));
            if let Some(terminal) = terminal {
                path.begin_segment();
                path.points.push(terminal.position);
                commands.trigger(Impact {
                    position: terminal.position,
                    normal: terminal.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: terminal.penetrated,
                    deflection: None,
                });
                if let Some(shot) = shot {
                    crate::shot_trace::record(&mut shot_trace, "hold", now, shot.0, || {
                        json!({
                            "held": held.waited,
                            "res": "terminal",
                            "it": terminal.impact_tick,
                            "pen": terminal.penetrated,
                        })
                    });
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "end",
                        now,
                        shot.0,
                        || json!({ "why": "terminal" }),
                    );
                }
                commands.entity(entity).despawn();
                continue;
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
                        &mut shot_trace,
                        "hold",
                        now,
                        shot.0,
                        || json!({ "held": held.waited, "res": "expired" }),
                    );
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "end",
                        now,
                        shot.0,
                        || json!({ "why": "bounce_dissolve" }),
                    );
                }
                commands.entity(entity).despawn();
            }
            continue;
        }

        // Replica fallback: consume a known outcome by its tick when interpolated geometry missed it.
        if !deposit
            && let (Some(shot), Some(buf), Some(present)) = (shot, sanctioned.as_ref(), present)
        {
            let consumed = marks.ricochets.len();
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
                        buf.as_ref(),
                        projectile.velocity,
                        projectile.drag_k,
                        dt,
                    ) {
                        Ok(caught_up) => caught_up,
                        Err(reject) => {
                            crate::shot_trace::record(
                                &mut shot_trace,
                                "overdue",
                                now,
                                shot.0,
                                || json!({ "res": "reject", "why": reject.trace_reason() }),
                            );
                            crate::shot_trace::record(
                                &mut shot_trace,
                                "end",
                                now,
                                shot.0,
                                || json!({ "why": "catch_up_reject" }),
                            );
                            commands.entity(entity).despawn();
                            continue;
                        }
                    };
                    for segment in &caught_up.segments {
                        path.begin_segment();
                        path.points.extend(segment.points.iter().copied());
                        marks.ricochets.push(segment.bounce.origin);
                        commands.trigger(Impact {
                            position: segment.bounce.origin,
                            // The keyframe does not carry the surface normal; preserve the existing
                            // overdue-path approximation from its sanctioned outgoing direction.
                            normal: segment.bounce.direction,
                            caliber: projectile.caliber,
                            surface: ImpactSurface::Armor,
                            penetrated: false,
                            deflection: Some(segment.bounce.direction),
                        });
                        let late =
                            elapsed_ticks(present, segment.bounce.bounce_tick).unwrap_or_default();
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "overdue",
                            now,
                            shot.0,
                            || json!({ "res": "bounce", "late": late, "seq": segment.bounce.sequence }),
                        );
                    }
                    if let Some(terminal) = caught_up.terminal {
                        path.begin_segment();
                        path.points.push(terminal.position);
                        commands.trigger(Impact {
                            position: terminal.position,
                            normal: terminal.normal,
                            caliber: projectile.caliber,
                            surface: ImpactSurface::Armor,
                            penetrated: terminal.penetrated,
                            deflection: None,
                        });
                        let late = elapsed_ticks(present, terminal.impact_tick).unwrap_or_default();
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "overdue",
                            now,
                            shot.0,
                            || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated, "via": "chain" }),
                        );
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "end",
                            now,
                            shot.0,
                            || json!({ "why": "terminal" }),
                        );
                        commands.entity(entity).despawn();
                        continue;
                    }
                    transform.translation = caught_up.position;
                    if let Ok(direction) = Dir3::new(caught_up.velocity) {
                        transform.look_to(direction, Vec3::Y);
                    }
                    projectile.velocity = caught_up.velocity;
                    readout.speed = caught_up.velocity.length();
                    readout.capability = capability(projectile.mass, caught_up.velocity.length());
                    continue;
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
                path.begin_segment();
                path.points.push(terminal.position);
                commands.trigger(Impact {
                    position: terminal.position,
                    normal: terminal.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: terminal.penetrated,
                    deflection: None,
                });
                crate::shot_trace::record(
                    &mut shot_trace,
                    "overdue",
                    now,
                    shot.0,
                    || json!({ "res": "terminal", "late": late, "pen": terminal.penetrated }),
                );
                crate::shot_trace::record(
                    &mut shot_trace,
                    "end",
                    now,
                    shot.0,
                    || json!({ "why": "terminal" }),
                );
                commands.entity(entity).despawn();
                continue;
            }
        }

        // Advance free-flight (gravity + drag on the velocity, then the position step) through the
        // shared per-tick kernel — the SAME [`advance_shell`] the FireEvent catch-up folds, so a
        // caught-up shell and a natively-flown one can't diverge. `freeflight_pos` is this tick's
        // free-flight landing point; the ray-march below overrides it only where the segment hits
        // something. The march may *bend* the direction (normalization / ricochet), so we carry
        // direction + speed and rebuild the velocity at the end rather than assuming a straight step.
        let (freeflight_pos, stepped) = advance_shell(
            transform.translation,
            projectile.velocity,
            projectile.drag_k,
            dt,
        );
        let Ok(mut dir) = Dir3::new(stepped) else {
            continue;
        };
        let mut speed = stepped.length();
        let mut pos = transform.translation;
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
        let mut terminal_emitted = terminal_report.0;

        // Ray-march the step: free flight until a surface, then resolve it — terrain stops the
        // shell; a ballistic volume ricochets (too oblique) or is crossed (normalize → spend cost →
        // perforate or embed) — and keep marching the leftover budget along the new direction.
        while remaining > EPS {
            let origin = pos + dir * EPS;
            let Some(hit) =
                spatial.cast_ray_predicate(origin, dir, remaining, true, &world, &not_own)
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
            let entry = origin + dir * hit.distance;
            let travelled = EPS + hit.distance;

            // EXPERIMENTAL B-arm (`SPIKE_MG_SHORTCIRCUIT`): a sub-20 mm round stops dead at the first
            // surface, skipping the entire penetration-resolution march below (thickness/span probes,
            // ricochet, spall, HP). Population-preserving — same despawn-on-contact as the live path —
            // so the A−B tick-cost delta isolates the resolution machinery. Default off (see the type).
            if shortcircuit.0 && projectile.caliber < MG_SHORTCIRCUIT_CALIBER_MAX {
                // Classify the first surface from the hit's volume ancestry (the same `hit_ancestor`
                // rule the full march uses just below) so the read is honest even in the B-arm. The
                // short-circuit stops dead without resolving penetration, so `penetrated: false`.
                let surface = if hit_ancestor(hit.entity, &volumes, &parents).is_some() {
                    ImpactSurface::Armor
                } else {
                    ImpactSurface::Terrain
                };
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface,
                    penetrated: false,
                    deflection: None,
                });
                pos = entry;
                stopped = true;
                break;
            }

            // The struck `BallisticVolume` sits on the hit's ancestry (`hit_ancestor`, the shared
            // hierarchy-resolution rule), keeping the node entity so transit damage and spall can
            // address the component. No volume in the ancestry ⇒ terrain.
            let resolved = hit_ancestor(hit.entity, &volumes, &parents)
                .map(|(node, volume)| (node, volume.material_factor));
            let Some((node_entity, factor)) = resolved else {
                // Terrain: stop here.
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Terrain,
                    penetrated: false,
                    deflection: None,
                });
                // A terrain stop is the one shot terminal that needs NO confirm: static, pose-independent
                // geometry, so both ends already agree (ADR-0021's invariant). Recorded on the client so
                // the analyzer can close the lifecycle of a shot that simply never reached armor, instead
                // of filing it as never-consumed.
                if !deposit && let Some(shot) = shot {
                    crate::shot_trace::record(
                        &mut shot_trace,
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
            // shell hidden; an unkeyed replica shell ends locally. Authority resolves armor below.
            if !deposit {
                let next_bounce = shot
                    .zip(sanctioned.as_ref())
                    .and_then(|(s, buf)| buf.next(s.0, marks.ricochets.len()));
                if let Some(bounce) = next_bounce {
                    // Buffered bounce: re-seed from authority truth and keep the remaining step.
                    commands.trigger(Impact {
                        position: bounce.origin,
                        normal: hit.normal,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: false,
                        deflection: Some(bounce.direction),
                    });
                    if let Ok(new_dir) = Dir3::new(bounce.direction) {
                        dir = new_dir;
                    }
                    speed = bounce.speed;
                    pos = bounce.origin;
                    bent = true;
                    path.begin_segment();
                    path.points.push(bounce.origin);
                    marks.ricochets.push(bounce.origin);
                    remaining -= travelled;
                    // Trace a buffered bounce separately from a shell that never contacted.
                    if let Some(shot) = shot {
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "contact",
                            now,
                            shot.0,
                            || json!({ "res": "pre_bounce", "seq": bounce.sequence, "bt": bounce.bounce_tick }),
                        );
                    }
                    continue;
                }
                // Buffered terminal: resolve at the authority read without a hold.
                let terminal = shot
                    .zip(sanctioned.as_ref())
                    .and_then(|(s, buf)| buf.terminal(s.0, marks.ricochets.len()));
                if let Some(terminal) = terminal {
                    path.begin_segment();
                    path.points.push(terminal.position);
                    commands.trigger(Impact {
                        position: terminal.position,
                        normal: terminal.normal,
                        caliber: projectile.caliber,
                        surface: ImpactSurface::Armor,
                        penetrated: terminal.penetrated,
                        deflection: None,
                    });
                    if let Some(shot) = shot {
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "contact",
                            now,
                            shot.0,
                            || json!({ "res": "pre_term", "it": terminal.impact_tick, "pen": terminal.penetrated }),
                        );
                        crate::shot_trace::record(
                            &mut shot_trace,
                            "end",
                            now,
                            shot.0,
                            || json!({ "why": "terminal" }),
                        );
                    }
                    pos = entry;
                    stopped = true;
                    break;
                }
                if let Some(shot) = shot {
                    // Hold a keyed shell hidden until the authority outcome or expiry.
                    commands.entity(entity).insert((
                        Held {
                            waited: 0,
                            age: 0,
                            normal: hit.normal,
                        },
                        Visibility::Hidden,
                    ));
                    // The corresponding `hold` row closes this trace interval.
                    crate::shot_trace::record(
                        &mut shot_trace,
                        "contact",
                        now,
                        shot.0,
                        || json!({ "res": "hold" }),
                    );
                    pos = entry;
                    holding = true;
                    break;
                }
                // 3. No identity — fail closed immediately (pre-slice behaviour).
                commands.trigger(Impact {
                    position: entry,
                    normal: hit.normal,
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: None,
                });
                pos = entry;
                stopped = true;
                break;
            }

            // Momentum bookkeeping for this crossing: the incoming velocity (before any bend/bleed)
            // and the body that owns the struck volume. Each resolution branch below hands the body
            // its share of the shell's momentum, `m·(v_in − v_out)` — a shell that stops dumps it all,
            // a perforation less (it carries momentum out), a ricochet a partial normal-ward kick.
            let v_in = Vec3::from(dir) * speed;
            let body = owners.get(node_entity).ok().map(|owner| owner.tank());

            // Outward surface normal; angle of incidence is measured from it (0 = head-on).
            let normal = Dir3::new(hit.normal).unwrap_or(-dir);
            let incidence = Vec3::from(dir).angle_between(-Vec3::from(normal));

            // Plate thickness *along its normal* (perpendicular, face to face) — the overmatch test:
            // a round whose calibre dwarfs the plate cannot be deflected by it.
            let thickness = spatial
                .cast_ray_predicate(
                    entry - Vec3::from(normal) * EPS,
                    -normal,
                    PROBE,
                    false,
                    &armor,
                    &|e| e == hit.entity,
                )
                .map(|back| EPS + back.distance)
                .unwrap_or(0.0);
            let overmatched = thickness > 0.0 && projectile.caliber >= OVERMATCH_RATIO * thickness;

            // Ricochet: too oblique → deflect off the face (no entry, no spall) — unless overmatch
            // suppresses it (design §4).
            if !overmatched && incidence > RICOCHET_ANGLE {
                // Shock: even a deflected hit jars an exposed component (barrel, optic) — scaled by
                // impact energy (capability) and how square the graze was. Armor has no HP, so it
                // shrugs the bounce off; a fragile module loses integrity without being one-shot.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    let shock = SHOCK_K * capability(projectile.mass, speed) * incidence.cos();
                    let before = hp.current;
                    hp.current = (before - shock).max(0.0);
                    damage_dealt += before - hp.current;
                }
                dir = reflect(dir, normal);
                bent = true; // off the original free-flight segment (see the open-air break)
                speed *= RICOCHET_BLEED;
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                        point: entry,
                    });
                }
                // The bounce reads on the struck face: a hard bright spark fan, biased along the
                // deflected (outgoing) direction — a ricochet throws its sparks the way it kicked off.
                // It bit no steel, so `penetrated: false` (no flame lick — a bounce doesn't ignite).
                commands.trigger(Impact {
                    position: entry,
                    normal: Vec3::from(normal),
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: false,
                    deflection: Some(Vec3::from(dir)),
                });
                marks.ricochets.push(entry);
                path.points.push(entry);
                // AUTHORITY → CLIENTS: replicate this sanctioned bounce as the cause (ADR-0016) so
                // every client re-seeds its cosmetic shell from truth rather than improvising. Only
                // for a net-attributed shell (`Shot` present, stamped by the shared protocol stamp);
                // SP/sandbox shells carry none and raise nothing. Muted after this shot's terminal
                // (`!terminal_emitted`) — an interior bounce past a perforation is invisible to the
                // client shell, which ended at the terminal. `dir`/`speed` are already
                // post-reflect/bleed (the outgoing state); `entry` is the bounce point.
                // `net::shot_transport` observes this and stamps the bounce tick. The ordinal is this
                // ricochet's 0-based index — the SAME count a client shell derives from its own
                // `ricochets` — so multi-bounce shots re-seed in order. (Only reachable on the
                // authority: the client `!deposit` branch above already broke before here.)
                if let Some(shot) = shot
                    && !terminal_emitted
                {
                    commands.trigger(ShellRicochet {
                        shot: shot.0,
                        origin: entry,
                        direction: Vec3::from(dir),
                        speed,
                        sequence: (marks.ricochets.len() - 1) as u32,
                    });
                }
                pos = entry;
                remaining -= travelled;
                continue;
            }

            // Normalize: a modest bend toward the inward normal as the round bites in (shortens the
            // path it cuts and nudges the exit). Overmatch does NOT bend it further — the round drives
            // through in roughly the same direction; overmatch instead cancels the *slope cost* below.
            dir = bend_toward(dir, -normal, NORMALIZATION * incidence);
            bent = true; // off the original free-flight segment (see the open-air break)
            let span = spatial
                .cast_ray_predicate(entry + dir * EPS, dir, PROBE, false, &armor, &|e| {
                    e == hit.entity
                })
                .map(|exit| EPS + exit.distance)
                .unwrap_or(0.0);

            // Cost = effective metres × the material's reference-mm-per-metre. An overmatched plate
            // can't present its oblique line-of-sight to a round that dwarfs it, so it charges only
            // the perpendicular thickness; otherwise the full slope span.
            let cap = capability(projectile.mass, speed);
            let effective = if overmatched { thickness } else { span };
            let cost = effective * factor;
            if cap <= cost {
                // Defeated: embed partway through (depth scaled by the capability it could pay).
                let embed = entry + dir * span * (cap / cost);
                marks.events.push(PenetrationEvent {
                    entry,
                    exit: embed,
                    overmatched,
                });
                path.points.push(embed);
                // It buried itself here, spending all it had (`cap`) — deposit that as transit damage
                // if the volume is a damageable component (design §6). No exit, so no spall.
                if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                    let before = hp.current;
                    hp.current = (before - cap * TRANSIT_K).max(0.0);
                    damage_dealt += before - hp.current;
                }
                // The embed's visible face is the ENTRY surface — its normal is what sparks kick
                // off of (the embed point itself is inside the plate). The round buried itself in the
                // steel (`penetrated: true`): the hot-metal signature earns the brief flame lick, even
                // though the plate ultimately defeated it — it bit in, it didn't bounce.
                commands.trigger(Impact {
                    position: embed,
                    normal: Vec3::from(normal),
                    caliber: projectile.caliber,
                    surface: ImpactSurface::Armor,
                    penetrated: true,
                    deflection: None,
                });
                // AUTHORITY → CLIENTS: the shot's TERMINAL (`ImpactConfirm` on the wire) — the same
                // read the `Impact` above showed locally, so every client renders the identical honest
                // embed (position/normal/flame lick). The `!terminal_emitted` guard covers the
                // perforate-then-embed-same-tick chain (the perforation already terminated the shot);
                // no latch write is needed here — the embedded shell stops and despawns, so nothing
                // after this can emit.
                if let Some(shot) = shot
                    && !terminal_emitted
                {
                    commands.trigger(ShellTerminal {
                        shot: shot.0,
                        position: embed,
                        normal: Vec3::from(normal),
                        penetrated: true,
                        after_bounces: marks.ricochets.len() as u32,
                    });
                }
                // Stopped: the body absorbs the full remaining momentum (v_out = 0).
                if let Some(body) = body {
                    commands.trigger(HitImpulse {
                        body,
                        impulse: projectile.mass * v_in,
                        point: entry,
                    });
                }
                pos = embed;
                stopped = true;
                break;
            }

            // Perforate: spend the cost (residual speed) and continue along the bent direction.
            // The struck FACE reads here — the entry point, its outward normal — where the round
            // punched through. This is the one place "penetrated" is unambiguously true (the round
            // breached the plate into the interior), so the armor read earns its flame lick. Without
            // this trigger a clean perforation was visually silent on the struck face.
            commands.trigger(Impact {
                position: entry,
                normal: Vec3::from(normal),
                caliber: projectile.caliber,
                surface: ImpactSurface::Armor,
                penetrated: true,
                deflection: None,
            });
            // AUTHORITY → CLIENTS: the shot's TERMINAL (`ImpactConfirm` on the wire). A perforation
            // ends the COSMETIC shell at this entry-face read even though the authoritative shell
            // marches on into the interior — the documented choice on [`ShellTerminal`]: what an
            // external viewer sees at the struck plate IS this read, and the client cannot march the
            // interior. Emitted for the FIRST perforation/embed only; [`TerminalReport`] mutes both
            // same-tick interior crossings and later fixed ticks while `Shot` remains available for
            // damage attribution.
            if let Some(shot) = shot
                && !terminal_emitted
            {
                terminal_emitted = true;
                terminal_report.0 = true;
                commands.trigger(ShellTerminal {
                    shot: shot.0,
                    position: entry,
                    normal: Vec3::from(normal),
                    penetrated: true,
                    after_bounces: marks.ricochets.len() as u32,
                });
            }
            speed = speed_for(projectile.mass, cap - cost);
            // The body keeps the momentum the shell lost crossing it; the shell carries the rest on.
            if let Some(body) = body {
                commands.trigger(HitImpulse {
                    body,
                    impulse: projectile.mass * (v_in - Vec3::from(dir) * speed),
                    point: entry,
                });
            }
            let exit = entry + dir * span;
            marks.events.push(PenetrationEvent {
                entry,
                exit,
                overmatched,
            });
            path.points.push(exit);

            // Transit damage: the main penetrator drove through this volume — if it's a damageable
            // component, deposit the cost it paid crossing (design §6). Armor has no HP, so no-op.
            if deposit && let Ok(mut hp) = health.get_mut(node_entity) {
                let before = hp.current;
                hp.current = (before - cost * TRANSIT_K).max(0.0);
                damage_dealt += before - hp.current;
            }

            // Spall: the exit face throws a cone of fragments. The *count* comes from the material
            // chewed (cost) and the hole size (caliber) — the fragment supply; each fragment's
            // *energy* comes from the shot's residual (v_res²) and its position in the cone (on-axis
            // strongest). So a thin/soft body throws few fragments and a barely-through round throws
            // weak ones — both extremes low (design §5). Each fragment then penetrates per its energy.
            let count_f = SPALL_MAX_FRAGMENTS as f32
                * (cost / SPALL_COST_REF)
                * (projectile.caliber / SPALL_CALIBER_REF);
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
                        &spatial,
                        &volumes,
                        &parents,
                        &mut health,
                        &armor,
                        deposit,
                    );
                    damage_dealt += fragment_damage;
                    burst.fragments.push(fragment);
                }
                spall.bursts.push(burst);
            }

            pos = exit;
            remaining -= travelled + span;
        }

        // Reorient the shell to its travel direction so the mesh follows the (gravity-curved,
        // ricochet-bent) path instead of holding its launch heading.
        transform.translation = pos;
        transform.look_to(dir, Vec3::Y);
        path.points.push(pos);

        // A state snapshot cannot preserve how many shots caused its resulting HP. Raise one discrete,
        // shot-attributed fact; `net::shot_transport` routes it reliably to the fire-time owner.
        if deposit
            && damage_dealt > 0.0
            && !damage_report.0
            && let Some(shot) = shot
        {
            commands.trigger(ShellDamage {
                shot: shot.0,
                amount: damage_dealt,
            });
            damage_report.0 = true;
        }

        if holding {
            // Frozen at armor awaiting the bounce keyframe: keep the entity and its velocity untouched
            // (the `Held` handler at the top of the loop drives the wait next tick). The freeze point
            // was just recorded once, so the trail ends cleanly at the plate while the shell holds.
        } else if stopped {
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
        } else if pos.y < KILL_FLOOR {
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
            projectile.velocity = Vec3::from(dir) * speed;
            readout.speed = speed;
            readout.capability = capability(projectile.mass, speed);
        }
    }

    // Attribute this system's whole wall-time to the current fixed tick (`SPIKE_COST_TRACE`). Inert
    // (both `Option`s empty) unless the recorder is armed.
    if let (Some(cost), Some(t0)) = (cost.as_mut(), march_t0) {
        cost.record_march(t0.elapsed().as_secs_f64() * 1.0e6);
    }
}

/// Apply a crossing's momentum share to the struck body (immediate velocity change; the off-CoM
/// entry point also imparts the angular rock). A static or non-rigid owner simply won't match.
fn on_hit_impulse(
    hit: On<HitImpulse>,
    // Authority-only: on the net client (a replica) the struck body's motion is server-owned and
    // arrives by replication — applying a local impulse here would fight it (a divergent shove).
    replica: Option<Res<ClientReplica>>,
    mut bodies: Query<Forces>,
) {
    if replica.is_some() {
        return;
    }
    if let Ok(mut forces) = bodies.get_mut(hit.body) {
        forces.apply_linear_impulse_at_point(hit.impulse, hit.point);
    }
}

fn on_impact(impact: On<Impact>) {
    info!("shell impact at {:?}", impact.position);
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

    /// `advance_shell` IS the live march's open-air step: new velocity is the shared `freeflight_step`,
    /// and the position advances by that new velocity over `dt` (`p += v·dt`) — the exact `pos += dir *
    /// remaining` the ray-march does when a step hits nothing. Pinning it keeps the catch-up and the
    /// live march provably ONE implementation (ADR-0016) even if the march is later refactored.
    #[test]
    fn advance_shell_is_the_freeflight_step() {
        let pos = Vec3::new(2.0, 30.0, 5.0);
        let v = Vec3::new(500.0, -10.0, 40.0);
        let k = drag_k(0.088, 10.2);
        let dt = 1.0 / 64.0;
        let (p, nv) = advance_shell(pos, v, k, dt);
        let expected_v = freeflight_step(v, k, dt);
        assert_eq!(nv, expected_v, "velocity is the shared free-flight kernel");
        assert_eq!(
            p,
            pos + expected_v * dt,
            "position steps by the new velocity"
        );
    }

    /// The "one integrator" property (the test that matters): fast-forwarding a shell N ticks lands it
    /// in the SAME state as N single-tick advances. `fast_forward_shell` folds the shared
    /// `advance_shell` — the exact per-tick kernel the live march steps in open air — so a caught-up
    /// shell can't diverge from a natively integrated one. Guards against re-deriving the catch-up as a
    /// closed-form trajectory.
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

    #[test]
    fn sanctioned_outcome_buffer_holds_the_derived_thirty_player_automatic_horizon() {
        const COMBATANTS: u64 = 30;
        const WEAPONS: u8 = 2;
        // DERIVED ceiling: 750 rounds/minute × 3 seconds / 60 = 37.5 rounds per weapon.
        const SHOTS_PER_WEAPON: u32 = 38;
        let mut sanctioned = SanctionedShots::default();

        for combatant in 1..=COMBATANTS {
            for weapon in 0..WEAPONS {
                for fire_tick in 0..SHOTS_PER_WEAPON {
                    let shot = ShotId {
                        combatant: crate::CombatantId(combatant),
                        weapon,
                        fire_tick,
                    };
                    assert_eq!(
                        sanctioned.insert(
                            shot,
                            SanctionedBounce {
                                origin: Vec3::ZERO,
                                direction: Vec3::X,
                                speed: 500.0,
                                bounce_tick: fire_tick,
                                sequence: 0,
                            }
                        ),
                        SanctionedBounceInsert::Inserted
                    );
                }
            }
        }

        let expected = COMBATANTS as usize * WEAPONS as usize * SHOTS_PER_WEAPON as usize;
        assert_eq!(
            sanctioned.shots.len(),
            expected,
            "the configured outcome lifetime must not evict a valid 30-player automatic-fire horizon"
        );
    }

    /// Equal-age overflow eviction is deterministic even though the buffer is a hash map.
    #[test]
    fn sanctioned_outcome_capacity_evicts_the_stable_highest_shot_id_on_an_age_tie() {
        let mut sanctioned = SanctionedShots::default();
        for fire_tick in 0..SanctionedShots::MAX_SHOTS as u32 {
            let shot = ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick,
            };
            assert_eq!(
                sanctioned.insert(
                    shot,
                    SanctionedBounce {
                        origin: Vec3::ZERO,
                        direction: Vec3::X,
                        speed: 500.0,
                        bounce_tick: fire_tick,
                        sequence: 0,
                    }
                ),
                SanctionedBounceInsert::Inserted
            );
        }

        let incoming = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: SanctionedShots::MAX_SHOTS as u32,
        };
        assert_eq!(
            sanctioned.insert(
                incoming,
                SanctionedBounce {
                    origin: Vec3::ZERO,
                    direction: Vec3::X,
                    speed: 500.0,
                    bounce_tick: incoming.fire_tick,
                    sequence: 0,
                }
            ),
            SanctionedBounceInsert::Inserted
        );

        assert!(
            sanctioned.has_shot(ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: 0,
            }),
            "the lowest stable tie-break key remains"
        );
        assert!(sanctioned.has_shot(incoming), "the new fact is retained");
        assert!(
            !sanctioned.has_shot(ShotId {
                combatant: crate::CombatantId(1),
                weapon: 0,
                fire_tick: SanctionedShots::MAX_SHOTS as u32 - 1,
            }),
            "the previous highest stable tie-break key is evicted"
        );
    }

    /// One malformed shot cannot grow more buffered bounces than cosmetic reconstruction can
    /// consume. The bound is DERIVED from the shared segment horizon.
    #[test]
    fn sanctioned_outcome_rejects_distinct_bounces_beyond_the_per_shot_bound() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 100,
        };
        let mut sanctioned = SanctionedShots::default();
        for sequence in 0..MAX_COSMETIC_CATCH_UP_TICKS {
            assert_eq!(
                sanctioned.insert(
                    shot,
                    SanctionedBounce {
                        origin: Vec3::ZERO,
                        direction: Vec3::X,
                        speed: 500.0,
                        bounce_tick: 100 + sequence,
                        sequence,
                    }
                ),
                SanctionedBounceInsert::Inserted
            );
        }

        assert_eq!(
            sanctioned.insert(
                shot,
                SanctionedBounce {
                    origin: Vec3::ZERO,
                    direction: Vec3::X,
                    speed: 500.0,
                    bounce_tick: 100 + MAX_COSMETIC_CATCH_UP_TICKS,
                    sequence: MAX_COSMETIC_CATCH_UP_TICKS,
                }
            ),
            SanctionedBounceInsert::Capacity,
            "the first distinct bounce beyond the reconstruction bound is rejected"
        );
        assert_eq!(
            sanctioned.shots[&shot].bounces.len(),
            MAX_COSMETIC_CATCH_UP_TICKS as usize
        );
    }

    /// A catch-up with bounce 1 and a later terminal already buffered must partition free-flight at
    /// both authority outcomes. It may not integrate bounce 0's outgoing state all the way to the
    /// present and draw through facts it already knows.
    #[test]
    fn sanctioned_chain_stops_each_segment_before_the_next_known_outcome() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
        };
        let second = SanctionedBounce {
            origin: Vec3::new(3.5, 0.0, 0.0),
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: 104,
            sequence: 1,
        };
        let terminal = SanctionedTerminal {
            position: Vec3::new(3.5, 3.5, 0.0),
            normal: Vec3::NEG_Y,
            penetrated: true,
            impact_tick: 108,
            after_bounces: 2,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);
        sanctioned.insert_terminal(shot, terminal);

        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(110),
            0,
            &sanctioned,
            Vec3::X * first.speed,
            0.0,
            0.1,
        )
        .expect("the short authority chain is reconstructible");

        assert_eq!(
            caught_up.segments.len(),
            2,
            "both bounces partition the catch-up"
        );
        assert_eq!(caught_up.segments[0].bounce.sequence, 0);
        assert_eq!(caught_up.segments[1].bounce.sequence, 1);
        assert_eq!(
            caught_up.segments[0].points.len(),
            4,
            "DERIVED fixture: origin plus three complete ticks before the tick-104 bounce"
        );
        assert!(
            caught_up.segments[0]
                .points
                .iter()
                .all(|point| point.x < second.origin.x),
            "bounce 0 free-flight never crosses the already-known bounce 1 origin"
        );
        assert!(
            caught_up.segments[1]
                .points
                .iter()
                .all(|point| point.y < terminal.position.y),
            "bounce 1 free-flight never crosses the already-known terminal"
        );
        assert_eq!(
            caught_up.terminal.map(|terminal| terminal.position),
            Some(terminal.position)
        );
        assert_eq!(caught_up.position, terminal.position);
    }

    /// A bogus authority tick must not turn cosmetic recovery into an unbounded integration. The
    /// fallback is intentionally no trajectory: drawing a prefix would claim an authority path the
    /// client cannot safely reconstruct.
    #[test]
    fn sanctioned_chain_rejects_an_implausibly_late_first_bounce() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
        };
        let sanctioned = SanctionedShots::default();

        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS + 1),
            0,
            &sanctioned,
            Vec3::X * first.speed,
            0.0,
            0.1,
        );

        assert!(
            matches!(
                caught_up,
                Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon)
            ),
            "a catch-up beyond the configured cosmetic horizon must reject instead of drawing a partial trajectory"
        );
    }

    /// A later authority fact cannot create an unbounded intermediate segment either. The entire
    /// chain rejects, so the already-seen first bounce is not drawn as a misleading partial result.
    #[test]
    fn sanctioned_chain_rejects_an_implausible_inter_outcome_gap() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
        };
        let second = SanctionedBounce {
            origin: Vec3::X,
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS + 1,
            sequence: 1,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);

        assert!(
            matches!(
                catch_up_sanctioned_chain(
                    shot,
                    0,
                    first,
                    Some(second.bounce_tick),
                    0,
                    &sanctioned,
                    Vec3::X * first.speed,
                    0.0,
                    0.1,
                ),
                Err(SanctionedCatchUpReject::IntervalBeyondCosmeticHorizon)
            ),
            "the chain must not draw its first segment when the next sanctioned boundary is implausible"
        );
    }

    /// Individually plausible segments may not accumulate into unbounded cosmetic work. This chain
    /// would integrate 198 ticks: DERIVED as 99 pre-bounce steps plus 99 pre-terminal steps.
    #[test]
    fn sanctioned_chain_rejects_cumulative_multi_segment_work_beyond_its_horizon() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: 90,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: 100,
            sequence: 0,
        };
        let second = SanctionedBounce {
            origin: Vec3::X,
            direction: Vec3::Y,
            speed: 10.0,
            bounce_tick: first.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS,
            sequence: 1,
        };
        let terminal = SanctionedTerminal {
            position: Vec3::ONE,
            normal: Vec3::Y,
            penetrated: false,
            impact_tick: second.bounce_tick + MAX_COSMETIC_CATCH_UP_TICKS,
            after_bounces: 2,
        };
        let mut sanctioned = SanctionedShots::default();
        sanctioned.insert(shot, first);
        sanctioned.insert(shot, second);
        sanctioned.insert_terminal(shot, terminal);

        assert!(
            matches!(
                catch_up_sanctioned_chain(
                    shot,
                    0,
                    first,
                    Some(terminal.impact_tick),
                    0,
                    &sanctioned,
                    Vec3::X * first.speed,
                    0.0,
                    0.1,
                ),
                Err(SanctionedCatchUpReject::ChainBeyondCosmeticHorizon)
            ),
            "the complete chain must fail closed once its combined integration exceeds the horizon"
        );
    }

    /// A small true elapsed interval remains valid across the wrapping tick boundary.
    #[test]
    fn sanctioned_chain_accepts_a_small_wraparound_interval() {
        let shot = ShotId {
            combatant: crate::CombatantId(1),
            weapon: 0,
            fire_tick: u32::MAX - 3,
        };
        let first = SanctionedBounce {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 10.0,
            bounce_tick: u32::MAX - 2,
            sequence: 0,
        };
        let caught_up = catch_up_sanctioned_chain(
            shot,
            0,
            first,
            Some(3),
            0,
            &SanctionedShots::default(),
            Vec3::X * first.speed,
            0.0,
            0.1,
        )
        .expect("a six-tick wrapping interval is inside the cosmetic horizon");

        assert_eq!(
            caught_up.segments[0].points.len(),
            7,
            "DERIVED: origin plus six integrated ticks"
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

    use avian3d::prelude::{Collider, CollisionLayers, PhysicsPlugins, RigidBody};
    use bevy::prelude::*;
    use bevy::time::TimeUpdateStrategy;

    use super::*;

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
        let origin = Vec3::new(0.0, 2.0, 2.0);
        let dir = Vec3::NEG_Z;
        let speed = 800.0;
        app.world_mut()
            .spawn((
                Projectile {
                    velocity: dir * speed,
                    caliber: 0.088,
                    mass: 10.2,
                    drag_k: drag_k(0.088, 10.2),
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
            },
        );
        let stored = buf.terminal(shot, 0).expect("terminal stored");
        assert_eq!(stored.position, Vec3::X, "first insert wins");
        assert!(stored.penetrated, "first insert's verdict kept");
    }
}
