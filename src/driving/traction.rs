use avian3d::prelude::*;
use bevy::prelude::*;
use serde::Deserialize;

use crate::command::TankCommand;
use crate::damage::{
    Capability, TankCapabilities, TankVolumes, VolumeFacets, capability_available,
};
use crate::tank::{Roadwheel, Tank, TankSim, TrackSide, WheelIndex};
use crate::trace::num;

use super::susp_trace;
use super::suspension::Suspension;

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

    /// Test-only constructor: the divergence-hash unit tests (`trace.rs`) need a non-default
    /// `DriveState` to prove a drive-field flip localizes to the `hdrv` sub-hash. The fields stay
    /// private in production code — `ramp_drive` remains their only writer.
    #[cfg(test)]
    pub(crate) fn test_new(throttle: f32, steer: f32) -> Self {
        Self { throttle, steer }
    }
}

/// Slew each tank's drive signal toward its commanded targets. Tank-agnostic: a zeroed command
/// (swapped away, idle) bleeds back to rest through the same ramp it drove up on.
pub(super) fn ramp_drive(time: Res<Time>, mut tanks: Query<(&TankCommand, &mut DriveState)>) {
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
pub(super) fn apply_drive(
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
    // Suspension-force recorder (`susp_trace`): hoisted once per run like `apply_suspension`'s
    // check, so the per-wheel hot loop below pays nothing when tracing is off. Reads (never
    // bumps) the tick counter — `apply_suspension` runs first in the step, so this is the same
    // tick's join key.
    let trace_tick = if susp_trace::enabled() {
        susp_trace::tick()
    } else {
        0
    };
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
            let d_sat_fwd = grip / drivetrain.brush_stiffness * (v_fwd / v_ref).clamp(-1.0, 1.0);
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

            // Suspension-force recorder: the exact drive/anchor force entering
            // `apply_force_at_point` plus the contact velocity it was computed from — `pow` is
            // the instantaneous power this force feeds the body (positive = energy in). f32
            // fields go through `crate::trace::num` (NaN/inf → null, never invalid JSON).
            if trace_tick != 0 {
                susp_trace::write(&serde_json::json!({
                    "k": "d",
                    "n": trace_tick,
                    "w": wheel_slot.0,
                    "vf": num(v_fwd),
                    "vl": num(v_lat),
                    "ws": num(w_static),
                    "df": num(d_fwd),
                    "dl": num(d_lat),
                    "f": [num(force.x), num(force.y), num(force.z)],
                    "pow": num(force.dot(velocity)),
                }));
            }

            forces.apply_force_at_point(force, contact);
            suspension.drive_force = force;
        }
    }
}
