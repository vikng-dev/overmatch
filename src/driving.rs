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
/// Below this contact planar speed (m/s) a wheel "grips": it plants a brush anchor and holds
/// statically instead of slipping. Above it, friction is kinetic (the skid / coast-down model).
/// This static↔kinetic gate is a Karnopp-style zero-velocity band — what lets a stopped tank
/// hold on a slope instead of creeping away. Universal feel (bucket 1).
const STICK_SPEED: f32 = 0.3;
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
    mut bodies: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Forces,
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
    for (body, position, rotation, mut forces, params, mut sim) in &mut bodies {
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

            let compression = params.rest_length - ground_distance;
            if compression <= 0.0 {
                unsupported(&mut suspension, &mut sim);
                continue;
            }

            let up = -dir;

            // Damped spring along the suspension axis. velocity_at_point gives the hull's speed at
            // the contact; its component along `up` is the compression rate (negative while
            // settling).
            let spring_speed = forces.velocity_at_point(contact).dot(up);
            let load = (params.stiffness * compression - params.damping * spring_speed).max(0.0);

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

            // Static↔kinetic gate: below the stick speed the contact grips (plant an anchor and
            // hold); above it, it slips and friction is the kinetic skid / coast-down model.
            let gripping = v_fwd.hypot(v_lat) < STICK_SPEED;
            if !gripping {
                *anchor = None;
            } else if anchor.is_none() {
                *anchor = Some(contact);
            }

            // Friction ellipse: tracks grip hard fore-aft (full μ·load) but skid sideways at the
            // lower turning-resistance coefficient μ_t = ratio·μ (Wong/Merritt firm-ground
            // skid-steer). The lateral semi-axis is what lets a heavy tank pivot — an isotropic
            // circle nearly cancels the steer drive.
            let grip = MU * load;
            let grip_lat = grip * LATERAL_GRIP_RATIO;

            // Slip from the planted anchor, split into the ground-plane axes.
            let (mut d_fwd, mut d_lat) = match *anchor {
                Some(anchor) => (
                    (contact - anchor).dot(forward),
                    (contact - anchor).dot(right),
                ),
                None => (0.0, 0.0),
            };

            // Bristle saturation (LuGre steady-state deflection) on the ellipse: a brush bristle
            // stretches only to its slip point — d_fwd to grip/k, d_lat to grip_lat/k. Past the
            // ellipse the bristle *trails* the contact at that fixed deflection (a smooth Coulomb
            // slide) instead of snapping back to zero, which removes the low-speed stick-slip cycle.
            if anchor.is_some() {
                let a_fwd = grip / drivetrain.brush_stiffness;
                let a_lat = grip_lat / drivetrain.brush_stiffness;
                let e = (d_fwd / a_fwd).powi(2) + (d_lat / a_lat).powi(2);
                if e > 1.0 {
                    let s = e.sqrt().recip();
                    d_fwd *= s;
                    d_lat *= s;
                    *anchor = Some(contact - forward * d_fwd - right * d_lat);
                }
            }

            // Longitudinal: thrust when commanded (bleeding the anchor's forward slip so the static
            // spring doesn't fight the drive — the wheel "rolls"); else hold (static spring) or,
            // while still rolling, the engine-brake / coast-down.
            let f_fwd = if driving {
                if let Some(planted) = *anchor {
                    *anchor = Some(planted + forward * d_fwd);
                }
                command * drivetrain.max_thrust - drivetrain.rolling_resistance * v_fwd
            } else if gripping {
                -drivetrain.brush_stiffness * d_fwd - drivetrain.brush_damping * v_fwd
            } else {
                -drivetrain.rolling_resistance * v_fwd
            };

            // Lateral: static spring holds the tracks fixed at rest (kills sideways creep); kinetic
            // stiff grip resists side-slip and yaw while moving (skid steer).
            let f_lat = if gripping {
                -drivetrain.brush_stiffness * d_lat - drivetrain.brush_damping * v_lat
            } else {
                -drivetrain.lateral_grip * v_lat
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
