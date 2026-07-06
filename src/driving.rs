//! Driving: the raycast-wheel locomotion seed (ADR-0005). Each roadwheel's suspension ray does
//! double duty — its spring holds the hull up (support, implemented here) and, later, its normal
//! load feeds the drive friction. The hull rides on its wheels; the hull box is only a collision
//! shape and a bottoming-out safety floor.

use avian3d::prelude::*;
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;
use serde::Deserialize;

use crate::Layer;
use crate::command::TankCommand;
use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, capability_available,
};
use crate::state::GameplaySet;
use crate::tank::{Roadwheel, Tank, TankSim, TrackSide, WheelIndex, rig_world_pose};

/// Coulomb coefficient: each wheel's total ground force is capped at MU × load (friction ellipse).
/// Per-environment (the track-vs-ground surface pair), not per-tank — destined for the terrain
/// mechanic, not the model (ADR-0007, bucket 3).
const MU: f32 = 0.9;
/// Lateral fraction of the friction ellipse: the sideways force budget is `LATERAL_GRIP_RATIO × MU ×
/// load`, modelling a track's turning-resistance coefficient μ_t against its longitudinal μ. Firm-
/// ground skid-steer theory (Wong/Merritt) puts μ_t ≈ 0.5 vs μ ≈ 0.9; this lower lateral grip is what
/// lets a heavy tank pivot at all — an isotropic circle nearly cancels the steer drive. Surface
/// property like [`MU`] (ADR-0007, bucket 3).
const LATERAL_GRIP_RATIO: f32 = 0.55;
/// Command ramp (per second): slews the tank's drive signal toward the commanded target, so a
/// binary key eases through the analog mid-range on the way to full. Vehicle response, not input
/// handling — it applies identically to a keyboard, a stick, or a network peer's command.
/// Universal feel (bucket 1).
const INPUT_RAMP: f32 = 4.0;
/// Centre of the static↔kinetic transition (m/s of contact planar speed): near here a wheel hands
/// off between gripping (brush anchor + static hold) and slipping (kinetic skid / coast-down). This
/// is a Karnopp-style zero-velocity band — what lets a stopped tank hold on a slope instead of
/// creeping away. Universal feel (bucket 1).
///
/// NO LONGER a hard gate: the regime is BLENDED across [`STICK_BAND`] around this speed (see
/// [`static_weight`]). A hard threshold made a binary force-regime flip that mm/s-scale cross-machine
/// velocity noise could straddle — the friction cousin of the sphere-cast's binary ray-contact
/// amplifier (playtest fork `friction-continuity.md`): under MP prediction the client and server land
/// on opposite sides of the gate and apply different force laws, diverging every tick. The smoothstep
/// makes the static fraction a *continuous* function of speed, so velocity noise only nudges the
/// blend weight instead of flipping the law.
const STICK_SPEED: f32 = 0.3;
/// Half-width of the static↔kinetic blend band, as a fraction of [`STICK_SPEED`]: the regime blends
/// smoothly across `[STICK_SPEED·(1−BAND), STICK_SPEED·(1+BAND)]` (here 0.18–0.42 m/s), fully static
/// below, fully kinetic above. Wide enough that the ~0.1–0.6 m/s velocity divergence a wedged tank
/// shows under prediction cannot cliff-edge the force regime; narrow enough that the hand-off is
/// still crisp on the feel (hill-hold releases into a glide over a ~0.24 m/s window, not a snap).
/// Feel dial (bucket 1); SIM-AFFECTING — both wire ends run the same blend, so no A/B fork is needed
/// for protocol identity.
const STICK_BAND: f32 = 0.4;
/// Per-tick low-pass rate for the sliding anchor's approach to its LuGre steady-state target (see
/// the anchor-target block in `apply_drive`): each tick the anchor closes this fraction of the
/// remaining gap, scaled by how much the wheel is sliding (`× (1 − w_static)`). At 1.0 the anchor
/// tracks the target directly — safe now that the target is CONTINUOUS in velocity (deflection
/// scales through zero instead of flipping across the ellipse with the slide direction's unit
/// vector; see the target's doc), so there is no teleporting input left to filter. Kept as a dial:
/// lowering it (~0.25) trades capture strength for extra smoothing of contact-velocity noise, the
/// preserved fallback if a future regime shows target churn again. Feel dial (bucket 1);
/// SIM-AFFECTING like its siblings.
const ANCHOR_RELAX_RATE: f32 = 1.0;
/// A per-track command below this magnitude counts as "no drive" — the wheel holds rather than
/// driving, so a feather-touch doesn't switch off the hill-hold. Universal feel (bucket 1).
const COMMAND_DEADBAND: f32 = 0.02;

/// Per-variant drivetrain characteristics — this tank's locomotion spec sheet, read by
/// `apply_drive`. Authored in the tank's `.tank.ron` spec sheet (ADR-0010); **required, with no
/// default** — a competitive sim must never run on guessed stats, so a failed spec load is fatal
/// (`report_failed_spec`) and a tank simply isn't driven until its `Drivetrain` has been applied.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Drivetrain {
    /// Max thrust per roadwheel at full throttle (N); ×16 wheels = total tractive force.
    pub max_thrust: f32,
    /// Longitudinal viscous term (N per m/s of forward speed): bounds top speed under thrust, and
    /// — throttle released, still rolling — IS the engine-brake / coast-down (heavy-glide dial).
    pub rolling_resistance: f32,
    /// Lateral grip (N per m/s of side-slip), kinetic regime — resists side-slip and yaw.
    pub lateral_grip: f32,
    /// Brush-anchor stiffness (N per m of slip): the static grip spring that holds the tank at rest.
    pub brush_stiffness: f32,
    /// Brush-anchor damping (N·s/m): settles the hold spring without buzzing at rest.
    pub brush_damping: f32,
}

/// Per-variant suspension characteristics, authored in the `.tank.ron` spec sheet (ADR-0010) and
/// applied to the hull. Required, no default (ADR-0011): the tank has no suspension until applied.
#[derive(Component, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuspensionParams {
    /// How far a roadwheel's suspension ray reaches from the hub (m). Must exceed the effective
    /// radius (~0.5166) so it finds the ground at rest, with margin for droop. Read per-step by
    /// `apply_suspension`'s in-system cast.
    pub ray_length: f32,
    /// Spring free length from the hub (m). Longer than the effective radius so at rest the spring
    /// is compressed enough to carry the tank's weight at the authored ride height.
    pub rest_length: f32,
    /// Spring stiffness per wheel (N/m): ~16 wheels × this × static compression ≈ the tank's weight.
    pub stiffness: f32,
    /// Suspension damping per wheel (N·s/m), ~0.6 of critical, so it settles without bouncing.
    pub damping: f32,
    /// Effective roadwheel radius (m) — hub to the wheel's ground-contact surface. Only the
    /// `sphere` probe reads it (`apply_suspension`): its ball of THIS radius rounds every terrain
    /// edge by the wheel's radius, making contact distance a continuous function of pose. Must be
    /// less than `ray_length + SPHERE_PROBE_RETRACT` so the cast can still reach droop.
    ///
    /// **Not authored today** and `#[serde(default)]` so the existing spec sheets stay valid — the
    /// geometry extractor carries only each wheel's node + side, not its radius, so there is no
    /// spec-driven path yet; this defaults to the Tiger's effective radius (~0.5166, the value the
    /// ray-model doc comments already name). Flat-ground ride height is INDEPENDENT of it — the
    /// probe's offset algebra cancels the radius on flat ground (see `apply_suspension`), so a
    /// mis-set value only re-rounds edges, never the equilibrium. Override in the `.tank.ron` once
    /// a per-variant number is warranted (no schema invented here — a defaulted field, not a
    /// required one).
    #[serde(default = "default_wheel_radius")]
    pub wheel_radius: f32,
}

/// The Tiger's effective roadwheel radius (m) — [`SuspensionParams::wheel_radius`]'s default when a
/// spec sheet omits it. Matches the `~0.5166` the ray-model doc comments already reference.
fn default_wheel_radius() -> f32 {
    0.5166
}

/// Retract margin (m) for the `sphere` probe's shape cast: the ball starts backed off this far UP
/// the cast axis, so a wheel already touching or slightly penetrating terrain still reports a hit
/// (avian's shape cast returns distance 0 for a shape that begins already intersecting, which would
/// lose the penetration depth). It bounds the maximum penetration the probe can resolve, and it
/// CANCELS out of the flat-ground compression (see the offset algebra in `apply_suspension`), so
/// its exact value never shifts the ride height — only how deep a bump the ball can measure.
const SPHERE_PROBE_RETRACT: f32 = 0.3;

/// Bump-stop stiffness as a multiple of the linear spring [`SuspensionParams::stiffness`]: the
/// progressive catch spring that engages in the last stretch of suspension travel is this much
/// stiffer than the main spring. ~10–20× is the vehicle-sim norm — firm enough to arrest a hard
/// landing inside the remaining travel, soft enough that the catch is felt as a firm compression
/// rather than a wall. Feel-tuning dial (bucket 1); it scales off the per-wheel `stiffness`, so it
/// tracks any per-variant spring. SIM-AFFECTING — both wire ends run this same law, so no A/B
/// switch is needed for protocol identity.
const BUMP_STOP_STIFFNESS_RATIO: f32 = 15.0;
/// Where the bump-stop engages, as a multiple of the even-share STATIC compression
/// `c_static = M·g / (n·k)` — the compression at which one wheel's linear spring carries exactly
/// 1/n of the tank's weight, computed per body in `apply_suspension` from its `ComputedMass`,
/// wheel count, and gravity. Engagement MUST sit strictly above the at-rest band or the stop
/// itself corrupts the flat-ground equilibrium: measured on the Tiger, per-wheel static
/// compression spreads to ~1.13× the even share (the COM is not centered between the axles), and
/// a first cut that engaged at a fixed 85% of max travel (0.0709 m) sat INSIDE that band
/// (0.055–0.072 m) — 5/16 wheels rode the stop at rest, ride height rose 0.055→0.087 and ground
/// contact flickered. 1.25× clears the measured 1.13× spread with margin while still engaging
/// well before the travel clamp (Tiger: engage ≈ 0.0792 m vs max travel 0.0834 m). The engagement
/// is PROGRESSIVE (force ramps from zero at the threshold and climbs with further compression) ON
/// PURPOSE, and this is REQUIRED, not cosmetic: a bare travel clamp would re-introduce a binary
/// force step at the limit (the force derivative jumps), exactly the discontinuous-contact
/// divergence-amplifier class this investigation removed. The soft, growing catch keeps the
/// contact force continuous. SIM-AFFECTING like everything here; both ends derive it from
/// replicated-identical inputs (authored mass, spec spring, wheel count).
const BUMP_STOP_ENGAGE_LOAD_RATIO: f32 = 1.25;
/// Upper cap on the engage point, as a fraction of max travel: however heavy a variant sags, the
/// last (1 − this) of travel always ramps the stop, so the catch never degenerates into the bare
/// clamp (zero ramp width — the binary force step again). A variant whose static sag pushes the
/// load-ratio engage past this cap will feel the stop at rest — a spec-authoring signal (its
/// travel geometry can't carry its weight), not a law failure; the force stays continuous.
const BUMP_STOP_LATEST_ENGAGE: f32 = 0.95;
/// The bump-stop is a DAMPED stiff spring, not a bare one. Its damping is the main [`SuspensionParams::damping`]
/// scaled by `sqrt(BUMP_STOP_STIFFNESS_RATIO)`, which holds the same ~0.6-of-critical damping
/// FRACTION the main spring is tuned to, now at the ~15× bump rate (critical damping scales with the
/// square root of stiffness). Measured why this is mandatory: an UNDAMPED bump-stop is a trampoline,
/// not a catch — a 4 m drop-test rebounded to ~2.8 m and never settled (wheels flickering in and out
/// of contact at "rest"), the exact "finicky, snaps around" failure this suspension work exists to
/// remove. The damper dissipates the landing energy so the catch settles. Ramped in with the spring
/// (from zero at engagement) so it adds no force step of its own. Derived, not authored — it tracks
/// whatever `stiffness`/`damping` a variant carries.
///
/// The stop's whole force is impulse-CAPPED at application (see `apply_suspension`) — a spring
/// this stiff cannot be integrated raw at 64 Hz. Measured: one tick of penetration at landing
/// speed puts `k·v·dt²` at ~0.6 of the wheel-share momentum `m·v`, so a raw 15× spring returns
/// MORE impulse than the fall brought in (restitution ≥ 1) and the tank re-launches through the
/// engage threshold forever — a self-sustaining hammer cycle (500–600 kN wheel spikes, ground
/// contact flickering at "rest") that this damping alone measurably did not fix.
fn bump_stop_damping(params: &SuspensionParams) -> f32 {
    params.damping * BUMP_STOP_STIFFNESS_RATIO.sqrt()
}

/// Which ground-probe geometry each wheel's suspension uses (`apply_suspension`) — the A/B switch
/// for the continuous-contact slice. Read ONCE from the `SUSPENSION_PROBE` env var at startup
/// ([`SuspensionProbe::from_env`]), never per-tick, and held in this resource.
///
/// **SIM-AFFECTING**: the probe geometry sets the per-wheel contact distance and thus every spring
/// force — client and server MUST run the same value or they diverge every tick and rollback
/// endlessly. It is logged loudly at startup ([`log_suspension_probe`]) and must be set identically
/// on both processes. `Sphere` is the default (the continuous-contact fix); `Ray` is the preserved
/// line-ray alternative for the playtest fork (`.agents/scratch/playtest-forks/`).
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum SuspensionProbe {
    /// The original per-wheel line ray: contact is a binary hit/miss of a single downward ray, so a
    /// terrain edge teleports the contact — the MP-jitter amplifier this slice replaces.
    Ray,
    /// A wheel-radius sphere cast: geometrically rounds every terrain edge by the wheel radius, so
    /// contact distance (hence spring force) is a continuous function of pose. The default.
    Sphere,
}

impl SuspensionProbe {
    /// Parse `SUSPENSION_PROBE` once. `ray`/`sphere` select the model; anything else (including
    /// unset) defaults to `Sphere` — the continuous-contact fix — with a warning for a typo'd value.
    fn from_env() -> Self {
        match std::env::var("SUSPENSION_PROBE").as_deref() {
            Ok("ray") => Self::Ray,
            Ok("sphere") => Self::Sphere,
            Err(_) => Self::Sphere,
            Ok(other) => {
                warn!("SUSPENSION_PROBE=`{other}` unrecognised (want ray|sphere) — using sphere");
                Self::Sphere
            }
        }
    }
}

/// Announce the active probe at startup, loudly — it is sim-affecting, so a mismatched client and
/// server (one `ray`, one `sphere`) would diverge silently otherwise. Runs on both ends.
fn log_suspension_probe(probe: Res<SuspensionProbe>) {
    info!(
        "SUSPENSION_PROBE={:?} — SIM-AFFECTING: client and server MUST match this value",
        *probe
    );
}

pub fn plugin(app: &mut App) {
    // The body's centre of mass needs no system here: `tank::spawn_tank_sim` inserts
    // `CenterOfMass` from the authored `Center_Of_Mass` empty's extracted position at spawn
    // (the model owns the COM; `NoAutoCenterOfMass` keeps the collision proxies' centroid from
    // diluting it — ADR-0011).
    app.insert_resource(SuspensionProbe::from_env())
        .add_systems(Startup, log_suspension_probe)
        .add_observer(attach_suspension)
        .add_observer(attach_drive_state)
        // Order matters within the fixed step: ramp the command into the drive signal, settle
        // springs (sets per-wheel load), then drive (reads that load for the friction circle).
        // All gated by the gameplay set.
        .add_systems(
            FixedUpdate,
            (ramp_drive, apply_suspension, apply_drive)
                .chain()
                .in_set(GameplaySet),
        );
}

/// Per-roadwheel DERIVED suspension state, recomputed from this tick's ray cast before anything
/// reads it — never carried across ticks, so it needs no rollback history. The one piece of
/// carried per-wheel state, the brush ANCHOR, lives root-resident in `TankSim::anchors` (see
/// `TankSim` on why carried state must sit on the root under prediction). `contact: None` =
/// wheel airborne.
#[derive(Component, Default, Clone, PartialEq, Debug)]
pub struct Suspension {
    /// Ground contact this tick (world) — where drive force is applied. `None` = airborne.
    pub contact: Option<Vec3>,
    /// Magnitude of the spring force currently applied (N) — the wheel's normal load.
    pub load: f32,
    /// Horizontal ground force applied this tick (thrust + friction), kept for the debug viz.
    pub drive_force: Vec3,
}

/// Attach `Suspension` the moment the rig binds a `Roadwheel` (observer, ungated).
fn attach_suspension(add: On<Add, Roadwheel>, mut commands: Commands) {
    commands.entity(add.entity).insert(Suspension::default());
}

/// Damped-spring suspension: each grounded wheel pushes the hull up at its contact point, so
/// ride height, pitch, roll, and weight transfer all emerge from the per-wheel springs.
///
/// The ground probe is one of two geometries ([`SuspensionProbe`], the `SUSPENSION_PROBE` A/B
/// switch): a line **ray** (the original — contact is a binary hit/miss, so a terrain edge
/// teleports the contact between a curb top and the road below, the MP-jitter amplifier), or a
/// wheel-radius **sphere** cast (the default — geometrically rounds every edge by the wheel radius,
/// so contact distance, and thus spring force, is a continuous function of pose). Only the
/// contact-distance SOURCE and contact POINT differ between them; the spring/damper math, the force
/// direction, and everything downstream are identical. See the offset-algebra note at the cast.
///
/// Probes are cast HERE, fresh each tick via `SpatialQuery`, from the wheel's tick-truth pose
/// (`rig_world_pose`) — never from `RayCaster`/`RayHits` components. Those are refreshed by avian
/// in `FixedPostUpdate`, *after* the step, so a reader in `FixedUpdate` gets last tick's hits; on
/// the FIRST replayed tick of a rollback "last tick" is the abandoned timeline's final tick (up to
/// the full rollback depth divergent), and 16 wheels applying spring forces from those stale
/// distances re-diverged every replay — the step-8 rollback storm's sim-side pump. Casting
/// in-system reads the restored `Position`/`Rotation` directly, replay ticks included.
///
/// Constraint this relies on: suspension rays only hit `Layer::Terrain`, which is all STATIC
/// geometry — the spatial-query BVH is refreshed inside the physics step, so a mid-tick cast is
/// only trustworthy against colliders that never move. If terrain ever grows moving platforms,
/// this needs revisiting.
fn apply_suspension(
    // Runs for *every* tank — support is tank-agnostic (each body rides on its own wheels),
    // unlike thrust, which each tank takes from its own command. The `&SuspensionParams` gates a
    // body in: no suspension until the spec is applied to the hull (ADR-0011 — no default spring).
    spatial: SpatialQuery,
    // The active probe geometry, read once at startup (see [`SuspensionProbe`]). Sim-affecting: the
    // same value must run on both client and server.
    probe: Res<SuspensionProbe>,
    // Gravity + the body's `ComputedMass` feed the bump-stop's engage point (the even-share static
    // compression `M·g / (n·k)` — see [`BUMP_STOP_ENGAGE_LOAD_RATIO`]). Both are deterministic
    // spawn-time data (authored mass, avian's default gravity), identical on both wire ends.
    gravity: Res<Gravity>,
    // The fixed timestep (this runs in `FixedUpdate`, so `Time` IS `Time<Fixed>`), for the
    // bump-stop's impulse cap: force × dt against the contact's effective momentum.
    time: Res<Time>,
    mut bodies: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Forces,
            &ComputedMass,
            &ComputedAngularInertia,
            &ComputedCenterOfMass,
            &SuspensionParams,
            &mut TankSim,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut wheels: Query<(&WheelIndex, &mut Suspension), With<Roadwheel>>,
) {
    let filter = SpatialQueryFilter::from_mask(Layer::Terrain);
    for (body, position, rotation, mut forces, mass, inertia, com, params, mut sim) in &mut bodies {
        // Travel-limit geometry, shared by every wheel of this body (see the clamp at the spring):
        // at full compression the hub sits exactly `wheel_radius` above the contact, so
        // `max_travel = rest_length - wheel_radius`.
        let max_travel = params.rest_length - params.wheel_radius;
        // The bump-stop's engage point: `BUMP_STOP_ENGAGE_LOAD_RATIO ×` the even-share static
        // compression `c_static = M·g / (n·k)` (n = this tank's wheel count — `TankSim::anchors`
        // has one slot per roadwheel), capped at [`BUMP_STOP_LATEST_ENGAGE`] of max travel so a
        // ramp zone always remains before the clamp. Derived per body, not authored: it tracks
        // whatever mass/spring/wheel-count a variant carries, and by construction sits a
        // documented margin ABOVE flat-ground rest — the stop must never load the equilibrium
        // (measured failure mode in [`BUMP_STOP_ENGAGE_LOAD_RATIO`]'s doc).
        let wheel_count = sim.anchors.len().max(1) as f32;
        let static_compression =
            mass.value() * gravity.0.length() / (wheel_count * params.stiffness);
        let engage = (BUMP_STOP_ENGAGE_LOAD_RATIO * static_compression)
            .min(BUMP_STOP_LATEST_ENGAGE * max_travel);
        // The bump-stop's impulse budget (see the cap at the stop): the world-space inverse
        // inertia and centre of mass feed a per-contact effective mass, contact-solver style —
        // but with the LINEAR compliance scaled by the wheel count:
        // `cap_i = closing / (dt · (n/M + (r×n)·I⁻¹·(r×n)))`. Both simpler bounds measurably
        // FAILED, one per term:
        // - even-share `(M/n)·closing/dt` (no rotational term) over-estimates pitch/roll modes —
        //   the true effective mass at a far contact is much smaller than M/n, so the "cap"
        //   over-cancelled the contact velocity, reversed it, and pumped rotation geometrically
        //   until a single-tick ~100 MN kick launched the tank ~66 m off flat ground (washboard
        //   run, hard-throttle rear-wheel engagement);
        // - the exact single-contact `1/(1/M + rot)` over-budgets SIMULTANEITY — near the COM it
        //   approaches the full M per wheel, 16 wheels superpose in one tick, and the settled
        //   tank breathed/flickered again (rest run: gnd 11–16, py std ×3).
        // Splitting the linear term n ways bounds the SUM of the wheels' linear impulses by
        // M·closing however many engage (flat multi-wheel landings get the full budget, exactly
        // the even share that made the drop-test catch clean), while the per-contact rotational
        // term keeps a lone corner hit at or below its true effective mass (under-capped when
        // fewer wheels engage — conservative: an absorber may under-absorb, never energize).
        // All spawn-derived deterministic data (authored mass/inertia), identical both ends.
        //
        // Known accepted softness: with the cap dissipative-only, a slow QUASI-STATIC deep press
        // (the post-curb-jump landing) recovers on the clamped linear spring alone (1.32× weight
        // total) — measured ~0.5 s at py ≈ −0.14, the same magnitude as the baseline's own worst
        // settle dip. Two attempts to firm that regime with a static weight-share floor on the
        // cap (whole engage zone, then depth-gated past the clamp) BOTH re-armed a sustained
        // settle limit cycle (instantaneous ~100 kN wheel spikes at "rest", ride biased +12 mm)
        // and were reverted — any velocity-independent force the settle breathing can reach
        // keeps re-exciting it. Firming this catch further means substepping or an implicit
        // stop, not a bigger force.
        let inv_inertia_world = inertia.rotated(rotation.0).inverse();
        let com_world = position.0 + rotation.0 * com.0;
        let dt = time.delta_secs();
        // Only this body's own roadwheels (its rig descendants) push on it — otherwise a second
        // tank's wheel hits would load this hull. An unsupported wheel also releases its brush
        // anchor (the carried state in `TankSim`) — airborne tracks grip nothing.
        for wheel in children.iter_descendants(body) {
            let Ok((wheel_slot, mut suspension)) = wheels.get_mut(wheel) else {
                continue;
            };
            // Unsupported (airborne / corrupt frame / no compression): no derived state this
            // tick, AND the brush anchor releases — airborne tracks grip nothing.
            let unsupported = |suspension: &mut Suspension, sim: &mut TankSim| {
                *suspension = Suspension::default();
                if let Some(anchor) = sim.anchors.get_mut(wheel_slot.0) {
                    *anchor = None;
                }
            };
            // Wheel nodes are authored with identity rotation, so wheel-local −Y is hull-down.
            let Some((origin, wheel_rotation)) =
                rig_world_pose(wheel, body, position.0, rotation.0, &parents, &locals)
            else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };
            // Same NaN discipline as the aim path: a corrupt pose frame must not flow through
            // the cast into `apply_force_at_point` and poison the body. `Dir3::new` already
            // rejects a non-finite direction; the origin needs its own guard. (First measured in
            // the old async-bind era's rollback bursts; kept as general discipline — any future
            // corruption source hits the same funnel.)
            if !origin.is_finite() {
                unsupported(&mut suspension, &mut sim);
                continue;
            }
            let Ok(down) = Dir3::new(wheel_rotation * Vec3::NEG_Y) else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };
            let dir = Vec3::from(down);

            // Probe the ground for `(ground_distance, contact)`: `ground_distance` is the hub-to-
            // ground distance ALONG THE CAST AXIS — the exact quantity the spring compresses against
            // — and `contact` is the world point where drive/friction act.
            //
            // Offset algebra (why flat-ground equilibrium is byte-identical between the models):
            // the ray reports `ground_distance = hit.distance` directly (hub to the ground point
            // straight below). The sphere starts its centre backed off UP the axis by
            // `SPHERE_PROBE_RETRACT` (the retract trick) and travels `hit.distance` until its
            // surface touches; on flat ground the centre stops one radius `r` above the ground, so
            // the hub sits `hit.distance + r - SPHERE_PROBE_RETRACT` above it. Reconstructing
            // `ground_distance` that way makes `compression = rest_length - ground_distance`
            // identical for both probes on flat ground for ANY radius and ANY retract (both cancel)
            // — same ride height, same 16 loads, same preload. The radius only bites where the
            // ground is NOT flat: there the sphere touches the nearest terrain point in any
            // direction (an edge, a lateral rise), rounding it off by `r` instead of the ray's
            // teleporting point. `hit.point1` is the closest point on the HIT shape (the terrain) in
            // world space (avian `ShapeHitData`), i.e. the true ground contact — on flat ground it
            // coincides with the ray's `origin + dir * hit.distance`, so drive/friction match too.
            let probed = match *probe {
                SuspensionProbe::Ray => spatial
                    .cast_ray(origin, down, params.ray_length, true, &filter)
                    .map(|hit| (hit.distance, origin + dir * hit.distance)),
                SuspensionProbe::Sphere => spatial
                    .cast_shape(
                        &Collider::sphere(params.wheel_radius),
                        origin - dir * SPHERE_PROBE_RETRACT,
                        Quat::IDENTITY,
                        down,
                        &ShapeCastConfig {
                            // Mirror the ray's reach: the sphere can find ground until the hub is
                            // `ray_length` above it (`hit.distance = ray_length + retract - r`).
                            max_distance: params.ray_length + SPHERE_PROBE_RETRACT
                                - params.wheel_radius,
                            ..default()
                        },
                        &filter,
                    )
                    .map(|hit| {
                        (
                            hit.distance + params.wheel_radius - SPHERE_PROBE_RETRACT,
                            hit.point1,
                        )
                    }),
            };
            let Some((ground_distance, contact)) = probed else {
                unsupported(&mut suspension, &mut sim);
                continue;
            };

            let raw_compression = params.rest_length - ground_distance;
            if raw_compression <= 0.0 {
                unsupported(&mut suspension, &mut sim);
                continue;
            }

            // Travel clamp. Cap the compression the LINEAR spring sees at the geometric limit where
            // the wheel hub sits exactly `wheel_radius` above the contact. The probe reports
            // `ground_distance` hub-to-ground along the cast axis, so the wheel's own bottom touches
            // the surface when `ground_distance == wheel_radius`, i.e. when
            // `compression == rest_length - wheel_radius` (`max_travel`, hoisted above the wheel
            // loop). Past that the wheel — and the hull that rides on it (the spring's whole job is
            // to hold the hull up) — would sink below ground, the measured "phasing" (root reached
            // y = −0.12 on a hard landing). Clamping the linear term flattens it beyond the limit,
            // but a constant force cannot arrest a fast descent on its own — the bump-stop below
            // supplies the growing restoring force. Flat-ground rest never reaches the clamp
            // (Tiger: static compression 0.055–0.072 m vs max travel 0.0834 m), so ride height is
            // untouched.
            let compression = raw_compression.min(max_travel);

            let up = -dir;

            // Damped spring along the suspension axis. velocity_at_point gives the hull's speed at
            // the contact; its component along `up` is the compression rate (negative while
            // settling).
            let spring_speed = forces.velocity_at_point(contact).dot(up);
            let mut load = params.stiffness * compression - params.damping * spring_speed;

            // Bump-stop: a much stiffer DAMPED spring (`BUMP_STOP_STIFFNESS_RATIO × stiffness`)
            // engaging above `engage` (a load-ratio margin over static rest — hoisted per body
            // above) and climbing PAST the clamp — its spring term is driven by the UNCLAMPED
            // compression, so the deeper the over-travel the harder it pushes back: a firm catch,
            // not a bottom-out. The progressive ramp (force grows from zero at `engage`) is
            // REQUIRED — see [`BUMP_STOP_ENGAGE_LOAD_RATIO`]: a hard clamp alone would be a
            // discontinuous contact-force step, the divergence amplifier this work removed. The
            // matched damper (see [`bump_stop_damping`]) fades in from zero across the
            // engage→clamp zone (`ramp`), so neither term introduces a step of its own.
            //
            // The whole stop is impulse-capped to be NON-RESTITUTIVE: this tick it may push no
            // harder than the force that zeroes this contact's share of the closing momentum in
            // one step (the simultaneity-safe effective mass — algebra and the two measured
            // failure modes it fixes at the `inv_inertia_world` hoist). A stiff spring
            // integrated explicitly at 64 Hz otherwise returns MORE impulse than the impact
            // brought in (restitution ≥ 1, measured: a 4 m drop re-launched itself through the
            // engage threshold indefinitely, hammering 500–600 kN wheel spikes at "rest").
            // Under the cap the stop can only absorb the fall, never re-launch it; rebound is
            // handed to the (stable) linear spring. The cap also structurally protects rest:
            // closing speed ≈ 0 at equilibrium, so a wheel drifting past `engage` on solver
            // noise gets ~zero stop force — the flat-ground equilibrium stays the linear
            // spring's alone. Continuity holds through the capped regime too: the cap scales
            // linearly with closing speed, so no force step appears at the v = 0 crossover.
            if raw_compression > engage {
                let over = raw_compression - engage;
                let ramp = (over / (max_travel - engage)).min(1.0);
                let stop = BUMP_STOP_STIFFNESS_RATIO * params.stiffness * over
                    - bump_stop_damping(params) * spring_speed * ramp;
                // `spring_speed` < 0 while compressing (see its comment), so the closing speed is
                // its negation, floored at 0 — a rebounding wheel gets no stop force at all.
                let closing = (-spring_speed).max(0.0);
                let lever = (contact - com_world).cross(up);
                let inv_effective_mass =
                    wheel_count * mass.inverse() + lever.dot(inv_inertia_world * lever);
                load += stop.clamp(0.0, closing / (dt * inv_effective_mass));
            }

            let load = load.max(0.0);

            forces.apply_force_at_point(up * load, contact);
            suspension.contact = Some(contact);
            suspension.load = load;
        }
    }
}

/// A tank's smoothed drive signal in [-1, 1]: its `TankCommand` targets, slewed through the
/// input ramp. Per-tank sim state (not part of the command), so every tank — local, swapped-away,
/// or a future network peer — responds to its command with the same vehicle feel.
///
/// `pub` + `Clone`/`PartialEq`/`Debug` are for `local_rollback::<DriveState>()` (step 7, `net`
/// feature): it lives on the tank root itself, so it's a drop-in `local_rollback` target — see
/// `lightyear-step7-map.md` §3.
#[derive(Component, Default, Clone, PartialEq, Debug)]
pub struct DriveState {
    throttle: f32,
    steer: f32,
}

impl DriveState {
    /// The current (ramped) throttle signal — read-only, for the jitter-trace recorder (`trace.rs`),
    /// which logs sim-truth drive intent alongside the tick pose. The fields stay private so
    /// `ramp_drive` remains their only writer.
    pub(crate) fn throttle(&self) -> f32 {
        self.throttle
    }

    /// The current (ramped) steer signal — read-only companion to [`DriveState::throttle`].
    pub(crate) fn steer(&self) -> f32 {
        self.steer
    }
}

/// Attach `DriveState` the moment a `Tank` exists (observer, ungated) — the sim-side partner of
/// the `TankCommand` that `command` attaches on the same trigger.
fn attach_drive_state(add: On<Add, Tank>, mut commands: Commands) {
    commands.entity(add.entity).insert(DriveState::default());
}

/// Slew each tank's drive signal toward its commanded targets. Tank-agnostic: a zeroed command
/// (swapped away, idle) bleeds back to rest through the same ramp it drove up on.
fn ramp_drive(time: Res<Time>, mut tanks: Query<(&TankCommand, &mut DriveState)>) {
    let step = INPUT_RAMP * time.delta_secs();
    for (command, mut state) in &mut tanks {
        state.throttle = approach(state.throttle, command.throttle, step);
        state.steer = approach(state.steer, command.steer, step);
    }
}

/// Move `current` toward `target` by at most `step`.
fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

/// Continuous static fraction for a contact sliding at planar `speed` (m/s): `1.0` fully gripping
/// (static brush hold), `0.0` fully sliding (kinetic), smoothstepped across the [`STICK_BAND`] band
/// around [`STICK_SPEED`]. Replaces the old hard `speed < STICK_SPEED` gate so mm/s-scale velocity
/// noise near the threshold can only nudge the blend weight, never flip the force law (the MP
/// divergence amplifier — see [`STICK_SPEED`]). At the endpoints it reproduces the gate exactly
/// (`1.0` below the band, `0.0` above), so rest hill-hold and full-speed skid are unchanged; only the
/// hand-off between them is now continuous.
fn static_weight(speed: f32) -> f32 {
    let lo = STICK_SPEED * (1.0 - STICK_BAND);
    let hi = STICK_SPEED * (1.0 + STICK_BAND);
    let t = ((speed - lo) / (hi - lo)).clamp(0.0, 1.0);
    // 1 − smoothstep(t): high (static) at low speed, decaying to 0 (kinetic) across the band.
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Differential-thrust drive with skid-steer friction. Each grounded wheel applies, at its
/// contact: longitudinal thrust (its track's command) minus rolling resistance, plus lateral
/// grip resisting side-slip — the whole vector capped on the friction ellipse (μ·load fore-aft, a
/// lower lateral budget sideways). Yaw, turning resistance, and weight transfer all emerge from
/// per-contact forces; nothing scripts the turn.
fn apply_drive(
    mut bodies: Query<
        (
            Entity,
            &Rotation,
            Forces,
            &Drivetrain,
            &DriveState,
            &mut TankSim,
            Option<&TankVolumes>,
            Option<&TankCapabilities>,
        ),
        With<Tank>,
    >,
    children: Query<&Children>,
    volumes: Query<VolumeFacets>,
    mut wheels: Query<(&Roadwheel, &WheelIndex, &mut Suspension)>,
) {
    // Per tank. `Drivetrain` is required per-variant data with no fallback (ADR-0010): we never
    // guess stats. It's absent only in the startup frames before the spec applies (a failed load is
    // fatal — see `report_failed_spec`), so a tank with no `Drivetrain` is simply not driven yet.
    for (body, tank_rotation, mut forces, drivetrain, state, mut sim, tank_volumes, tank_caps) in
        &mut bodies
    {
        // Drive gates *thrust*, not grip: only a tank with a live `Drive` capability applies its
        // drive signal. One with a dead driver/engine/transmission gets zero command but still runs
        // the full friction model below, so the tracks hold the tank in place via the brush anchor
        // instead of sliding frictionlessly. Which tank has a non-zero signal at all is the command
        // layer's business (`gather_commands` writes only the controlled tank's command).
        let drive_ok = capability_available(tank_volumes, tank_caps, Capability::Drive, &volumes);
        let (throttle, steer) = if drive_ok {
            (state.throttle, state.steer)
        } else {
            (0.0, 0.0)
        };

        // Ground-plane drive basis from the hull orientation: forward flattened onto the ground,
        // and right as forward rotated −90° about Y (avoids depending on a separate `right()`).
        // Physics `Rotation`, not `GlobalTransform`: force directions are sim math and must come
        // from tick-truth state — the render transform lags physics by up to a frame (differently
        // on client vs server) and freezes through rollback replays, which measurably streamed
        // rollbacks under high yaw/pitch rates (step-8 washboard finding).
        let forward = tank_rotation.0 * Vec3::NEG_Z;
        let forward = Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero();
        let right = Vec3::new(-forward.z, 0.0, forward.x);

        // Only this tank's own roadwheels (its rig descendants) — otherwise the other tank's wheels
        // would take this tank's drive.
        for wheel_entity in children.iter_descendants(body) {
            let Ok((wheel, wheel_slot, mut suspension)) = wheels.get_mut(wheel_entity) else {
                continue;
            };
            // The wheel's carried brush anchor, root-resident (see `TankSim`).
            let Some(anchor) = sim.anchors.get_mut(wheel_slot.0) else {
                continue;
            };
            let (Some(contact), load) = (suspension.contact, suspension.load) else {
                continue;
            };
            if load <= 0.0 {
                suspension.drive_force = Vec3::ZERO;
                *anchor = None;
                continue;
            }

            // Additive differential: D adds to the left track and subtracts from the right, so steer
            // yaws the nose the same way regardless of throttle, and a pure steer pivots in place.
            let command = match wheel.side {
                TrackSide::Left => throttle + steer,
                TrackSide::Right => throttle - steer,
            }
            .clamp(-1.0, 1.0);
            let driving = command.abs() > COMMAND_DEADBAND;

            let velocity = forces.velocity_at_point(contact);
            let v_fwd = velocity.dot(forward);
            let v_lat = velocity.dot(right);

            // Static↔kinetic blend (was a hard `speed < STICK_SPEED` gate — the binary force-regime
            // flip mm/s velocity noise could straddle, the MP divergence amplifier). `w_static` is a
            // continuous static fraction: 1 fully gripping (brush hold), 0 fully sliding (kinetic),
            // smoothstepped across the band around the stick speed. Every force below is a
            // `w_static`-blend of its static and kinetic laws, so a cross-machine velocity difference
            // moves the weight a hair rather than switching the law.
            let speed = v_fwd.hypot(v_lat);
            let w_static = static_weight(speed);

            // Friction ellipse: tracks grip hard fore-aft (full μ·load) but skid sideways at the
            // lower turning-resistance coefficient μ_t = ratio·μ (Wong/Merritt firm-ground
            // skid-steer). The lateral semi-axis is what lets a heavy tank pivot — an isotropic
            // circle nearly cancels the steer drive.
            let grip = MU * load;
            let grip_lat = grip * LATERAL_GRIP_RATIO;

            // Anchor plant/release no longer flips at the stick-speed gate — that Some/None flicker
            // (measured ~14–29 /s in the powered wedge; each flip reset the slip integral) is exactly
            // the binary transition this slice removes. Instead the anchor stays planted while the
            // wheel bears load and RELAXES toward the LuGre kinetic steady state in proportion to how
            // much the wheel is sliding (`1 − w_static`): the bristle trailing on the friction
            // ellipse, deflected along the slide so its spring force opposes it (`z_ss =
            // sign(v)·g(v)/σ0` in LuGre terms). The trail deflection is a CONTINUOUS function of the
            // contact velocity — each axis scales linearly with `v/v_ref` and clamps at its ellipse
            // semi-axis (`v_ref` = the band top, where the regime is fully kinetic) — NOT the unit
            // slide direction: `v̂` is discontinuous through v = 0, and in the near-band regime
            // (planar speed ~0.2 m/s, comparable to contact-velocity noise from angular jitter) a
            // `v̂`-scaled target teleports across the ellipse (±2·grip/k ≈ 0.18 m) every direction
            // flip — measured re-arming the wedge storm this slice kills (lat0 wedge 1 → 48
            // rollbacks). With the linear-through-zero form, velocity noise moves the target
            // proportionally (mm for mm/s), while a sustained slide (> `v_ref`) still saturates it.
            //
            // Fully gripping (`w_static = 1`) the anchor is frozen — the static hill-hold spring,
            // bit-identical to the old planted anchor. Sliding, it re-grips from ~Coulomb capture
            // strength (`w·μ·load` opposing the slide), which is what arrests a slide on a slope the
            // way the old hard gate's instant re-plant did — but continuously. Two other rejected
            // variants, measured: relax toward the CONTACT (zero deflection) leaked the hold
            // deflection under contact-velocity jitter and a tank nudged loose on the 20° ramp
            // never re-gripped (slid 7 m off; baseline parks in 0.6 m); NO relax (keep the stale
            // planted point) re-gripped from a stale-direction saturated spring — a ~31 kN force
            // step that re-amplified washboard coast-down rollbacks 1→32 at lat0. Anchor releases
            // (`None`) only when the wheel goes airborne/unloaded (the `load <= 0` path here and
            // `apply_suspension`'s unsupported paths).
            let v_ref = STICK_SPEED * (1.0 + STICK_BAND);
            let d_sat_fwd =
                grip / drivetrain.brush_stiffness * (v_fwd / v_ref).clamp(-1.0, 1.0);
            let d_sat_lat =
                grip_lat / drivetrain.brush_stiffness * (v_lat / v_ref).clamp(-1.0, 1.0);
            let anchor_target = contact - forward * d_sat_fwd - right * d_sat_lat;
            // `planted` carries the anchor point through this wheel's force math — every write
            // below keeps `*anchor` in sync with it, so the `Option` never needs re-testing (the
            // wheel is loaded, and a loaded wheel's anchor is always `Some` from here on).
            let mut planted = match *anchor {
                None => anchor_target,
                Some(a) => a + (anchor_target - a) * ((1.0 - w_static) * ANCHOR_RELAX_RATE),
            };
            *anchor = Some(planted);

            // Slip from the planted anchor, split into the ground-plane axes.
            let (mut d_fwd, mut d_lat) = (
                (contact - planted).dot(forward),
                (contact - planted).dot(right),
            );

            // Bristle saturation (LuGre steady-state deflection) on the ellipse: a brush bristle
            // stretches only to its slip point — d_fwd to grip/k, d_lat to grip_lat/k. Past the
            // ellipse the bristle *trails* the contact at that fixed deflection (a smooth Coulomb
            // slide) instead of snapping back to zero, which removes the low-speed stick-slip cycle.
            {
                let a_fwd = grip / drivetrain.brush_stiffness;
                let a_lat = grip_lat / drivetrain.brush_stiffness;
                let e = (d_fwd / a_fwd).powi(2) + (d_lat / a_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    d_fwd *= s;
                    d_lat *= s;
                    planted = contact - forward * d_fwd - right * d_lat;
                    *anchor = Some(planted);
                }
            }

            // Longitudinal: thrust when commanded (bleeding the anchor's forward slip so the static
            // spring doesn't fight the drive — the wheel "rolls"); else the static hold and the
            // kinetic engine-brake / coast-down BLENDED by `w_static`, so the release from hill-hold
            // into a glide is continuous across the stick band instead of a gate flip.
            let f_fwd = if driving {
                *anchor = Some(planted + forward * d_fwd);
                command * drivetrain.max_thrust - drivetrain.rolling_resistance * v_fwd
            } else {
                let hold = -drivetrain.brush_stiffness * d_fwd - drivetrain.brush_damping * v_fwd;
                let coast = -drivetrain.rolling_resistance * v_fwd;
                w_static * hold + (1.0 - w_static) * coast
            };

            // Lateral: static spring holds the tracks fixed at rest (kills sideways creep) BLENDED
            // with the kinetic stiff grip that resists side-slip and yaw while moving (skid steer) —
            // `w_static` hands off continuously, so a canted wedge riding near the stick speed can't
            // flip a huge lateral force step (static spring ≫ kinetic damper) tick-to-tick.
            let f_lat = {
                let hold = -drivetrain.brush_stiffness * d_lat - drivetrain.brush_damping * v_lat;
                let slip = -drivetrain.lateral_grip * v_lat;
                w_static * hold + (1.0 - w_static) * slip
            };

            let mut force = forward * f_fwd + right * f_lat;

            // Cap the tangential force on the friction ellipse (μ·load fore-aft, grip_lat sideways)
            // by scaling the vector onto its boundary. The bounded bristle rarely overshoots, so
            // this only trims the thrust+grip vector sum — and never resets the anchor (that snap is
            // the stick-slip source).
            let e = (f_fwd / grip).powi(2) + (f_lat / grip_lat).powi(2);
            if e > 1.0 {
                force *= e.sqrt().recip();
            }

            forces.apply_force_at_point(force, contact);
            suspension.drive_force = force;
        }
    }
}
