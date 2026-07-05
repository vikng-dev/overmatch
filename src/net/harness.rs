//! Reproducible workload levers for the rollback/feel measurements — env-gated test drivers that
//! actively steer the sim: the client's scripted input, the server's forced-rollback impulse, and
//! the input-delay A/B knob. All off or inert unless their `SPIKE_*` env var asks for them.

use core::time::Duration;

use avian3d::prelude::{Forces, WriteRigidBodyForces};
use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, InputMarker};

use crate::command::TankCommand;

/// `--simulate-input` state: a fixed-tick counter driving a scripted throttle window, then a
/// clean exit once enough time has passed to observe the forced rollback + convergence.
/// `fire_tick` defaults to 300 (mid-drive, well clear of the perturbation); `SPIKE_FIRE_TICK`
/// overrides it for the forced-rollback-with-fire pass (~110 lands beside the ~2 s perturbation).
/// `SPIKE_SIM_LONG=1` (rollback-storm diagnostic): drive straight at full throttle for ~15 s —
/// from spawn that crosses the speed bump (z≈−70) and the washboard (z≈−82…−90), the terrain the
/// user's rollback-stream report singled out; the default 4 s arc never leaves the flat pad.
/// `SPIKE_SIM_IDLE=1` (beached-rest diagnostic): hold zero throttle/steer, never fire, aim
/// constant, for the whole default ~600-tick run — a pure idle observation window, so a tank
/// spawned onto a resting contact (`SPIKE_SPAWN_POSE`) is watched settling, with no drive input
/// perturbing the contact state the client must re-form each rollback.
#[derive(Resource)]
pub(crate) struct SimulateInput {
    pub(crate) ticks: u32,
    fire_tick: u32,
    /// Last tick of the throttle window (steer is zeroed when extended, so the course features
    /// dead ahead are actually reached).
    drive_until: u32,
    /// Script length — exit after this many ticks.
    pub(crate) total: u32,
    /// `SPIKE_SIM_IDLE`: suppress ALL drive input (throttle/steer stay 0, no fire) for the whole
    /// run — the beached-rest observation window.
    idle: bool,
}

impl Default for SimulateInput {
    fn default() -> Self {
        let long = std::env::var("SPIKE_SIM_LONG").is_ok();
        Self {
            ticks: 0,
            fire_tick: std::env::var("SPIKE_FIRE_TICK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(300),
            drive_until: if long { 1088 } else { 384 },
            total: if long { 1280 } else { 600 },
            idle: std::env::var("SPIKE_SIM_IDLE").is_ok(),
        }
    }
}

/// Headless simulate mode: write the scripted `TankCommand` into the lightyear `ActionState` slot
/// each tick. Whole-state snapshot per tick, no devices.
pub(crate) fn buffer_input(
    mut sim: ResMut<SimulateInput>,
    mut slots: Query<&mut ActionState<TankCommand>, With<InputMarker<TankCommand>>>,
) {
    let Ok(mut state) = slots.single_mut() else {
        return;
    };
    sim.ticks += 1;
    let t = sim.ticks;
    // Step-7 script, exercising the real sim under prediction: 2 s idle (rig binds, suspension
    // settles) → 4 s throttle 1.0 + steer 0.3 (ramp_drive + suspension + skid-steer, spanning
    // the ~2 s server perturbation) → coast to rest. The aim intention + range are held from
    // tick 0 so the turret/gun servos slew (drive_aim_servos → drive_servos) while driving;
    // one fire click at tick 300 (Reload starts ready) exercises fire + recoil + reload.
    // Idle window (`SPIKE_SIM_IDLE`): never drive — the tank just sits on whatever contact it
    // spawned onto, so the trace isolates the beached-rest rollback storm from any drive churn.
    let driving = !sim.idle && (128..sim.drive_until).contains(&t);
    state.0.throttle = if driving { 1.0 } else { 0.0 };
    // The long course run drives dead straight (the bump/washboard are on the spawn axis); the
    // default short script arcs to exercise skid-steer.
    state.0.steer = if driving && sim.drive_until == 384 {
        0.3
    } else {
        0.0
    };
    // Hull-local, far off-axis so the yaw servo visibly slews; range 800 m dials in real
    // superelevation from the weapon's range table.
    // SPIKE_SIM_AIM_SWEEP (rollback-storm diagnostic): instead of the constant point, sweep the
    // aim around the tank at ~1.3 rad/s — a player scanning with the mouse. A human recommits the
    // aim EVERY frame from the camera ray; the constant-aim script never exercised that churn.
    state.0.aim = if std::env::var("SPIKE_SIM_AIM_SWEEP").is_ok() {
        let theta = 0.02 * t as f32;
        Some(Vec3::new(800.0 * theta.sin(), 0.0, -800.0 * theta.cos()))
    } else {
        Some(Vec3::new(200.0, 0.0, -800.0))
    };
    state.0.range = 800.0;
    // No fire in the idle window — a recoil impulse would disturb the resting contact under study.
    state.0.fire_primary = !sim.idle && t == sim.fire_tick;
}

/// Simulate mode: exit cleanly once the script has played out (long enough to cover the ~2s
/// server perturbation and settle afterward), or bail on a wall-clock timeout if the connection
/// never came up.
pub(crate) fn simulate_watchdog(
    simulate: Res<SimulateInput>,
    time: Res<Time<Real>>,
    mut exit: MessageWriter<AppExit>,
) {
    if simulate.ticks >= simulate.total {
        info!("client: simulation script complete, exiting");
        exit.write(AppExit::Success);
    } else if time.elapsed_secs() > 40.0 {
        error!("client: watchdog timeout — never got an input slot");
        exit.write(AppExit::error());
    }
}

/// Server levers, read once at boot. `SPIKE_PERTURB=0` drops the forced-rollback impulse — pure
/// noise during a feel test; on by default so the step-7 evidence runs stay reproducible.
#[derive(Resource)]
pub(crate) struct PerturbConfig {
    pub(crate) perturb: bool,
}

pub(crate) fn env_flag(name: &str, default: bool) -> bool {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
        .map(|v| v != 0)
        .unwrap_or(default)
}

/// Per-client one-shot: fires ~2 s after connect, applying a large lateral impulse the client
/// cannot have predicted (server-only side effect) — guarantees a misprediction and thus a
/// rollback (increment 5 success criterion).
#[derive(Component)]
pub(crate) struct PendingPerturbation {
    pub(crate) at: Duration,
}

/// Applies the forced-rollback perturbation once, ~2 s after spawn — a lateral impulse only the
/// server applies, so the client's prediction (which never saw it coming) mispredicts and must
/// roll back when the replicated `Position` disagrees.
pub(crate) fn perturb_after_delay(
    mut tanks: Query<(Entity, &PendingPerturbation, Forces)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for (entity, pending, mut forces) in &mut tanks {
        if time.elapsed() < pending.at {
            continue;
        }
        // Sized for ~3 m/s of lateral delta-v on the 57 t tank (`tiger_1.tank.ron`'s
        // `mass: 57000.0`) — comfortably above the 0.01 m/s-equivalent rollback threshold (forces
        // exactly one misprediction) but small next to the ~4-15 m/s cruise speed, so the resulting
        // one-tick displacement stays under the ROLLBACK-SNAP detector's 0.5 m bar. The previous
        // 4,000,000 N*s value injected ~70 m/s instantly — legitimate per-tick motion at that speed
        // (~1.1 m/tick) was tripping the snap detector on its own, misread as rollback oscillation
        // (see spike log).
        const IMPULSE: f32 = 171_000.0;
        forces.apply_linear_impulse(Vec3::X * IMPULSE);
        info!("server: {entity} perturbation impulse applied (forced rollback trigger)");
        commands.entity(entity).remove::<PendingPerturbation>();
    }
}

/// `SPIKE_SPAWN_POSE="x,y,z,qx,qy,qz,qw"` (server): override the spawned tank's initial
/// `Position`/`Rotation` — parsed once at boot, applied in `spawn_pending_tanks`. Seven
/// comma-separated f32s (translation metres, then an xyzw quaternion, normalized on read); any
/// malformed value logs and falls back to the default spawn. Used to place the tank onto a known
/// resting contact (the field-captured beached pose on the §2 side-slope slab edge) so the
/// rollback storm reproduces deterministically. Inert when unset.
pub(crate) fn spawn_pose() -> Option<(Vec3, Quat)> {
    let raw = std::env::var("SPIKE_SPAWN_POSE").ok()?;
    let nums: Vec<f32> = raw.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    if nums.len() != 7 {
        error!("server: SPIKE_SPAWN_POSE=\"{raw}\" is not seven f32s (x,y,z,qx,qy,qz,qw) — ignored");
        return None;
    }
    let pos = Vec3::new(nums[0], nums[1], nums[2]);
    // xyzw, matching the trace/analysis quaternion layout; normalize so a hand-entered field
    // capture (not bit-exact unit) lands as a valid `Rotation`.
    let rot = Quat::from_xyzw(nums[3], nums[4], nums[5], nums[6]).normalize();
    info!("server: SPIKE_SPAWN_POSE pos={pos:?} rot={rot:?}");
    Some((pos, rot))
}

/// `SPIKE_INPUT_DELAY_TICKS` (default 0, i.e. `InputDelayConfig::no_input_delay()`'s behavior via
/// `fixed_input_delay(0)`): an A/B lever per map §5 — a small fixed input delay shrinks how often
/// prediction needs to run ahead at all, a second and complementary knob to the threshold
/// coarsening in `protocol`. Off by default so the baseline A/B isn't itself perturbed by this lever.
pub(crate) fn input_delay_ticks() -> u16 {
    std::env::var("SPIKE_INPUT_DELAY_TICKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
