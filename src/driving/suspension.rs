use avian3d::prelude::*;
use bevy::prelude::*;
use serde::Deserialize;

use crate::Layer;
use crate::tank::{Roadwheel, Tank, TankSim, WheelIndex, rig_world_pose};
use crate::trace::num;

use super::contact::sphere_cast_ground_contact;
use super::susp_trace;

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
    pub(super) fn from_env() -> Self {
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
pub(super) fn log_suspension_probe(probe: Res<SuspensionProbe>) {
    info!(
        "SUSPENSION_PROBE={:?} — SIM-AFFECTING: client and server MUST match this value",
        *probe
    );
}

#[derive(Component, Default, Clone, PartialEq, Debug)]
pub struct Suspension {
    /// Ground contact this tick (world) — where drive force is applied. `None` = airborne.
    pub contact: Option<Vec3>,
    /// Magnitude of the spring force currently applied (N) — the wheel's normal load.
    pub load: f32,
    /// Horizontal ground force applied this tick (thrust + friction), kept for the debug viz.
    pub drive_force: Vec3,
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
pub(super) fn apply_suspension(
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
    // Suspension-force recorder (`susp_trace`): the per-run counter, this tick's row join key.
    let trace_tick = if susp_trace::enabled() {
        susp_trace::next_tick()
    } else {
        0
    };
    for (body, position, rotation, mut forces, mass, inertia, com, params, mut sim) in &mut bodies {
        // Mass properties are computed by avian AFTER the spawn-tick collider flush, so on a body's
        // very first tick `ComputedMass`/`ComputedAngularInertia` are still zero. Every quantity
        // below scales off them — static compression, the bump-stop engage point, and the impulse
        // cap's effective mass all collapse to zero. A pose already compressed on that tick (a
        // terrain-intersecting `SPIKE_SPAWN_POSE`) then enters the stop with `inv_effective_mass = 0`
        // and, at rest, `closing = 0`, so `cap = 0/0 = NaN` panics the clamp. No valid suspension
        // force exists without mass, so skip the body until avian fills it in — but release its
        // wheels to unsupported FIRST (clear contact/load, drop brush anchors, exactly the airborne
        // path below), so if mass ever drops out mid-run no stale contact/anchor state leaks into
        // `apply_drive`. (Same NaN-discipline funnel as the per-wheel origin/direction guards below.)
        if !(mass.value() > 0.0 && mass.value().is_finite()) {
            for wheel in children.iter_descendants(body) {
                if let Ok((wheel_slot, mut suspension)) = wheels.get_mut(wheel) {
                    *suspension = Suspension::default();
                    if let Some(anchor) = sim.anchors.get_mut(wheel_slot.0) {
                        *anchor = None;
                    }
                }
            }
            continue;
        }
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
            // `SPHERE_PROBE_RETRACT` (the retract trick) and travels until its surface touches; on
            // flat ground the centre stops one radius `r` above the ground, so the hub sits
            // `travel + r - SPHERE_PROBE_RETRACT` above it. Reconstructing `ground_distance` that
            // way makes `compression = rest_length - ground_distance` identical for both probes on
            // flat ground for ANY radius and ANY retract (both cancel) — same ride height, same 16
            // loads, same preload. The radius only bites where the ground is NOT flat: there the
            // sphere touches the nearest terrain point in any direction (an edge, a lateral rise),
            // rounding it off by `r` instead of the ray's teleporting point. `hit.point1` is the
            // closest point on the HIT shape (the terrain) in world space (avian `ShapeHitData`),
            // i.e. the true ground contact — on flat ground it coincides with the ray's
            // `origin + dir * hit.distance`, so drive/friction match too.
            //
            // The travel is NOT `hit.distance` alone: parry's shape-cast TOI is only relative-
            // tolerance accurate (millimetres-to-centimetres short against the large ground slab,
            // pose-discontinuous — the at-rest limit-cycle pump) while the witness pair of the
            // same hit is exact, so [`sphere_cast_ground_contact`] reconstructs the travel from
            // `hit.point1`/`hit.normal1`, clamped to the TOI-based band, with conservative
            // fallbacks for penetrating starts and corrupt witnesses — full derivation, guard
            // semantics, error measurements, and the ADR-0015 continuity argument at that
            // function. This arm is a thin adapter over it: everything except the raw cast lives
            // in the tested helper.
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
                            // `ray_length` above it (`travel = ray_length + retract - r`).
                            max_distance: params.ray_length + SPHERE_PROBE_RETRACT
                                - params.wheel_radius,
                            ..default()
                        },
                        &filter,
                    )
                    .map(|hit| {
                        sphere_cast_ground_contact(
                            origin,
                            dir,
                            params.wheel_radius,
                            SPHERE_PROBE_RETRACT,
                            hit.distance,
                            hit.point1,
                            hit.normal1,
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
            // The two linear terms named once — the load sum below AND the recorder rows read
            // them, so tracing adds no recomputation to the hot loop.
            let spring_force = params.stiffness * compression;
            let damper_force = -params.damping * spring_speed;
            let mut load = spring_force + damper_force;
            // Recorder taps (`susp_trace`): the bump-stop's applied force + cap state (plain
            // copies of values the force math computes anyway — free when tracing is off).
            let mut trace_stop = 0.0_f32;
            let mut trace_capped = false;

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
                let cap = closing / (dt * inv_effective_mass);
                // Defence in depth at the line that once panicked: the body-level mass guard covers
                // the "no mass yet" NaN, but `inv_effective_mass` also carries the rotational term
                // `lever·(I⁻¹·lever)` — a degenerate `ComputedAngularInertia` (a zero principal
                // moment → infinite inverse) would make it non-finite even with mass valid, and
                // `clamp(0.0, NaN)` panics. Treat any non-finite cap as "no stop force this tick".
                let applied = if cap.is_finite() {
                    stop.clamp(0.0, cap)
                } else {
                    0.0
                };
                // Recorder taps (`susp_trace`): copies, no recomputation.
                trace_stop = applied;
                trace_capped = stop > cap;
                load += applied;
            }

            let load = load.max(0.0);

            // Suspension-force recorder: the exact values entering `apply_force_at_point`
            // (`clip` = the force the `max(0)` floor removed) — schema at `susp_trace`. f32
            // fields go through `crate::trace::num` (NaN/inf → null, never invalid JSON).
            if trace_tick != 0 {
                let hull_v = forces.linear_velocity();
                let hull_w = forces.angular_velocity();
                susp_trace::write(&serde_json::json!({
                    "k": "s",
                    "n": trace_tick,
                    "w": wheel_slot.0,
                    "c": num(raw_compression),
                    "cc": num(compression),
                    "ss": num(spring_speed),
                    "fs": num(spring_force),
                    "fd": num(damper_force),
                    "fb": num(trace_stop),
                    "cap": trace_capped,
                    "clip": num((spring_force + damper_force + trace_stop).min(0.0)),
                    "ld": num(load),
                    "cy": num(contact.y),
                    "py": num(position.0.y),
                    "vy": num(hull_v.y),
                    "wx": num(hull_w.x),
                    "wz": num(hull_w.z),
                    "oy": num(origin.y),
                    "gd": num(ground_distance),
                    "we": wheel.to_bits(),
                }));
            }

            forces.apply_force_at_point(up * load, contact);
            suspension.contact = Some(contact);
            suspension.load = load;
        }
    }
}
