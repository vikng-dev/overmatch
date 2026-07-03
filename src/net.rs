//! Shared networking protocol registration (lightyear spike, `net` feature). Mounted by both
//! `spike_server` and `spike_client` — lightyear requires identical protocol registration on
//! both sides of the wire, added after `ServerPlugins`/`ClientPlugins` and before the
//! `Server`/`Client` connection entity is spawned (see the spike map §3 ordering note).

use avian3d::prelude::{
    AngularVelocity, IslandPlugin, IslandSleepingPlugin, LinearVelocity,
    PhysicsInterpolationPlugin, PhysicsTransformPlugin, Position, Rotation,
};
use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
use lightyear::prediction::diagnostics::PredictionDiagnosticsPlugin;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{
    DriveState, GameplaySet, Reload, Rig, Roadwheel, ServoState, Suspension, Tank, TankCommand,
    TankSpecHandle,
};

/// A trivial replicated marker — step 3 of the spike: proves `.replicate()` registration and
/// ordering before any real game state rides the wire.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpikeBeacon;

/// Marks the spike's predicted tank root (the increment-5 primitive originally; the real Tiger rig
/// since increment 6) — what the spike's own logging/diagnostics key off, as opposed to the game's
/// `Tank` marker the sim itself uses.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpikeTank;

/// The increment-5 primitive's dimensions/mass — retired from the spawn path in increment 6 (the
/// real rig's `on_tank_ready` supplies Mass/AngularInertia from the spec instead), but `TANK_MASS`
/// stays: the server's perturbation impulse sizing comment (`spike_server.rs`) still references the
/// real Tiger's mass, and the two happen to match (`tiger_1.tank.ron`'s `mass: 57000.0`).
pub const TANK_HALF_EXTENTS: Vec3 = Vec3::new(1.8, 1.4, 3.5);
pub const TANK_MASS: f32 = 57_000.0;

/// The Tiger's spec+scene load dependency, kicked off once at startup on both sides (sandbox.rs's
/// `load_target`/`PendingTarget` pattern) — `on_tank_ready` requires the spec already loaded
/// (asserts on it), so nothing may spawn a tank root until this resolves.
#[derive(Resource)]
pub struct PendingTankSpec(pub Handle<crate::TankSpec>);

pub fn load_tank_spec(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.insert_resource(PendingTankSpec(
        asset_server.load("tiger_1/tiger_1.tank.ron"),
    ));
}

/// The root bundle every spike tank spawn needs regardless of side: the real Tiger scene + spec
/// handle (drives `on_tank_ready`) plus the `RigidBody` the binder itself does not insert (it only
/// adds Mass/AngularInertia/colliders on children — `tank.rs::spawn_tank` inserts `RigidBody`
/// alongside the scene for the same reason, mirrored here). `SpikeTank` rides alongside the real
/// `Tank` marker: the spike's diagnostics key off `SpikeTank`, while `Tank`/`on_tank_ready` (and,
/// from step 7, the whole `SimPlugin` — driving/aim/shooting) are the real rig's own contract.
pub fn spike_tank_rig(asset_server: &AssetServer, spec: &Handle<crate::TankSpec>) -> impl Bundle {
    (
        WorldAssetRoot(
            asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
        ),
        TankSpecHandle(spec.clone()),
        Tank,
        SpikeTank,
        // Explicit, because on the CLIENT this bundle lands on a replicon-spawned root that has
        // only the replicated components (Position/Rotation) — without a Transform the scene
        // hierarchy under it never gets GlobalTransforms (Bevy B0004), the binder captures wrong
        // collider offsets, and the client settles at a different rest height than the server
        // (measured: +1.25 vs −0.28 → rollback on every packet). lightyear's avian sync owns
        // writing this from Position afterwards.
        Transform::default(),
        // Static until the rig binds: the glb loads async (~seconds), and a Dynamic body with no
        // collider yet free-falls through the ground for the whole window (measured: y = −425 and
        // still falling when the script ended). `activate_bound_rigs` flips it to Dynamic the
        // moment `Rig` lands — the spike-scale version of the game's spawn-before-bind race.
        avian3d::prelude::RigidBody::Static,
    )
}

/// Wake a spike tank's physics once its rig has bound (colliders + mass exist now) — the second
/// half of the Static-until-bound spawn above. Shared: each side flips at its own bind moment, so
/// neither ever simulates a collider-less falling body.
fn activate_bound_rigs(
    tanks: Query<Entity, (Added<crate::Rig>, With<SpikeTank>)>,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} rig bound — body goes Dynamic");
        commands
            .entity(entity)
            .insert(avian3d::prelude::RigidBody::Dynamic);
    }
}

/// Decorate a bound rig's non-root parts as rollback-participant the moment both `Rig` (the
/// binder's terminal insert) and `Predicted` are present on the root (order either can land in —
/// `Predicted` arrives at spawn, `Rig` seconds later on glb load, so this is really gated on `Rig`
/// alone in practice, but querying both is the honest precondition per the step-7 map §7 design).
///
/// `DeterministicPredicted` marks each part as predicted-but-uncompared: it gets a
/// `PredictionHistory` and rolls back with the root, but never itself trips a rollback from state
/// mismatch (it isn't replicated, so it has nothing to mismatch against). Without this,
/// `local_rollback::<ServoState>()` etc. below would silently no-op on these entities (map §3: the
/// history-attach observer gates on the trigger entity carrying `Predicted`/`PreSpawned`/
/// `DeterministicPredicted`/`CatchUpGated` directly — no hierarchy traversal exists).
///
/// `skip_despawn: true`, REVERSING the map §7 amendment 1 on live evidence: with the default
/// (`skip_despawn: false`) every rollback whose target tick predates the decoration tick despawns
/// the children (`deterministic_despawn` drain, rollback.rs) — and rollbacks fire *continuously*
/// through the post-bind suspension-settle burst, so the "vanishingly narrow" collision window is
/// actually the common case: measured, all 19 children despawned ~16 ms after decoration, rig
/// permanently broken client-side, 201 rollbacks/15 s. The skip_despawn variant instead stamps
/// `DisableRollback` during the grace window (`enable_rollback_after`, default 20 ticks) and then
/// lifts it — the children survive the burst and become full rollback participants ~300 ms later.
fn decorate_rig_children(
    ready: Query<(Entity, &Rig), (Added<Rig>, With<Predicted>)>,
    all_children: Query<&Children>,
    roadwheels: Query<(), With<Roadwheel>>,
    mut commands: Commands,
) {
    let decoration = DeterministicPredicted {
        skip_despawn: true,
        ..default()
    };
    for (root, rig) in &ready {
        let mut decorated = 3;
        for part in [rig.turret, rig.gun, rig.muzzle] {
            commands.entity(part).insert(decoration);
        }
        // Roadwheels aren't in `Rig` (no fixed count/field) — walk the root's whole subtree for
        // the `Roadwheel` marker, the same descendant-walk shape `on_tank_ready` itself uses
        // (from the ROOT: wheels are siblings of `Hull` in the model, not under it).
        for wheel in all_children.iter_descendants(root) {
            if roadwheels.contains(wheel) {
                commands.entity(wheel).insert(decoration);
                decorated += 1;
            }
        }
        info!("net: {root} rig children decorated DeterministicPredicted (count={decorated})");
    }
}

/// Coarsened rollback thresholds for the tank root (map §1): the reference examples' 1 cm / 0.01
/// rad bar is tuned for a single-collider capsule character, not a 16-contact 57 t rig — solver
/// noise on a body this complex trips that bar far more often than genuine misprediction (measured:
/// ~430 rollbacks/15s at 100ms latency vs 13 for the increment-5 primitive, all invisible/converging
/// per the increment-6 log). Correction smoothing (`add_linear_correction_fn`, already wired) hides
/// a ≤5 cm snap; coarsening to 0.05 trades some correctness-under-genuine-desync for a large CPU
/// win on the honest-noise case. Position in metres, Rotation in radians, velocities in m/s or
/// rad/s-equivalent — same shape as the map §1(b) reference thresholds, five times coarser.
const ROLLBACK_POSITION_M: f32 = 0.05;
const ROLLBACK_ROTATION_RAD: f32 = 0.05;
const ROLLBACK_VELOCITY: f32 = 0.05;

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub fn plugin(app: &mut App) {
    app.component::<SpikeBeacon>().replicate();
    app.component::<SpikeTank>().replicate();
    app.add_plugins(input::native::InputPlugin::<TankCommand>::default());

    // Avian replication (map §5): mount lightyear_avian3d's ordering fixes, then register the
    // root's Position/Rotation/velocities as predicted+rollback-eligible. Verbatim rollback
    // conditions/correction/interpolation fns from `avian_3d_character`'s `protocol.rs` — the only
    // real 3D reference in the lightyear repo for this registration shape, except the thresholds
    // (see `ROLLBACK_POSITION_M` etc. above — coarsened for step 7).
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position,
        ..default()
    });
    app.component::<Position>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Position, b: &Position| {
            (a.0 - b.0).length() >= ROLLBACK_POSITION_M
        })
        .add_linear_correction_fn()
        .add_linear_interpolation();
    app.component::<Rotation>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Rotation, b: &Rotation| {
            a.angle_between(*b) >= ROLLBACK_ROTATION_RAD
        })
        .add_linear_correction_fn()
        .add_linear_interpolation();
    // Without an explicit condition these default to `PartialEq::ne` (exact bit equality), which
    // f32 solver output essentially never satisfies between client and server — see the Position
    // comment above for the coarsening rationale (same thresholds, applied uniformly).
    app.component::<LinearVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &LinearVelocity, b: &LinearVelocity| {
            (a.0 - b.0).length() >= ROLLBACK_VELOCITY
        });
    app.component::<AngularVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &AngularVelocity, b: &AngularVelocity| {
            (a.0 - b.0).length() >= ROLLBACK_VELOCITY
        });

    // Rig-child rollback participation (map §3/§7): `DriveState` lives on the root itself, a
    // drop-in `local_rollback` target. `ServoState`/`Reload`/`Suspension` live on turret/gun/
    // muzzle/roadwheel children — `decorate_rig_children` above marks those `DeterministicPredicted`
    // so history actually attaches; without it these calls compile but silently do nothing for
    // those entities (no error, no panic — confirmed map §3.3).
    app.local_rollback::<DriveState>();
    app.local_rollback::<ServoState>();
    app.local_rollback::<Reload>();
    app.local_rollback::<Suspension>();

    app.add_systems(Update, (activate_bound_rigs, decorate_rig_children));
    // Bridge lightyear's input buffer into the sim's own `TankCommand` (command.rs's contract):
    // sim systems (`ramp_drive`, `fire`, `drive_aim_servos`) read `TankCommand`, never
    // `ActionState` directly, so this is the one seam translating net input into sim input.
    // `.before(GameplaySet)`, NOT merely `.before(ConsumeCommandEdges)`: every consumer — the
    // readers (`fire`, `ramp_drive`, `drive_aim_servos`) AND the edge-clearer (`consume_edges`)
    // — lives in `GameplaySet`, and ordering only against `ConsumeCommandEdges` leaves the bridge
    // unordered vs `fire`. Measured failure with the weaker constraint: `fire` ran first, read
    // the pre-bridge command, then `consume_edges` cleared the edge the bridge had just written —
    // the click vanished without any tick consuming it (reload never left 0.0).
    // Not gated `.run_if(not(is_in_rollback))`: replay must re-feed the same historical
    // `ActionState` lightyear itself restores per tick (map §3.4's "no gate needed" class — this
    // is a pure copy from already-correctly-restored state, not an externality).
    app.add_systems(
        FixedUpdate,
        bridge_action_state_to_tank_command.before(GameplaySet),
    );
}

/// Copy this tick's `ActionState<TankCommand>` (lightyear's input-buffer-backed component) into the
/// entity's own `TankCommand` (the sim's actual read contract, `command.rs`) — the seam between
/// networked input and every sim system. Only entities carrying both: the tank root gets
/// `TankCommand` from `command::core_plugin`'s `attach_command` observer (`On<Add, Tank>` — the rig
/// bundle includes `Tank`, confirmed fires) and `ActionState<TankCommand>` from lightyear's own
/// input plugin once `InputMarker<TankCommand>` claims the slot (`claim_input_slot`, client bin).
fn bridge_action_state_to_tank_command(
    mut tanks: Query<(&ActionState<TankCommand>, &mut TankCommand)>,
) {
    for (action, mut command) in &mut tanks {
        // Whole-struct overwrite: matches `ActionState`'s own "absolute snapshot per tick"
        // contract (no per-field diffing needed).
        *command = action.0;
    }
}

/// The disables `LightyearAvianPlugin` requires, plus `IslandPlugin`/`IslandSleepingPlugin` (map
/// §8: sleeping bodies can corrupt rollback replay). Both bins build `PhysicsPlugins` with this,
/// instead of the game's `PhysicsInterpolationPlugin::interpolate_all()` — lightyear's own
/// `FrameInterpolationSystems` takes over that job (map §8's "REAL, already-identified conflict").
pub fn physics_plugins() -> bevy::app::PluginGroupBuilder {
    avian3d::prelude::PhysicsPlugins::default()
        .build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>()
        .disable::<IslandPlugin>()
        .disable::<IslandSleepingPlugin>()
}

/// Log `PredictionDiagnosticsPlugin`'s ROLLBACKS/ROLLBACK_DEPTH diagnostics periodically (every
/// ~5s) — `PredictionPlugin::build` already mounts the plugin unconditionally (spike log,
/// increment-5 setup notes), so this only reads `DiagnosticsStore`, it does not mount it again
/// (mounting a second `PredictionDiagnosticsPlugin` would panic on the duplicate-plugin check).
pub fn log_prediction_diagnostics(
    diagnostics: Res<DiagnosticsStore>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 5.0 {
        return;
    }
    *timer = 0.0;
    let rollbacks = diagnostics
        .get(&PredictionDiagnosticsPlugin::ROLLBACKS)
        .and_then(|d| d.value())
        .unwrap_or_default();
    let depth = diagnostics
        .get(&PredictionDiagnosticsPlugin::ROLLBACK_DEPTH)
        .and_then(|d| d.value())
        .unwrap_or_default();
    info!("net: PredictionDiagnostics rollbacks={rollbacks} rollback_depth={depth:.2}");
}

/// `SPIKE_INPUT_DELAY_TICKS` (default 0, i.e. `InputDelayConfig::no_input_delay()`'s behavior via
/// `fixed_input_delay(0)`): an A/B lever per map §5 — a small fixed input delay shrinks how often
/// prediction needs to run ahead at all, a second and complementary knob to the threshold
/// coarsening above. Off by default so the baseline A/B isn't itself perturbed by this lever.
pub fn input_delay_ticks() -> u16 {
    std::env::var("SPIKE_INPUT_DELAY_TICKS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// Step-7 verification readout (every ~2 s, both bins): proof the *real* sim is running, not the
/// retired stub — grounded-wheel count (suspension rays actually hitting the Terrain-layer game
/// ground), the main turret's servo angle (slewing toward the scripted aim), and every weapon's
/// reload timer (the Tiger has two — MainGun + Coax; the MainGun's goes non-zero after a fire
/// consumes the click). One tank per bin in this spike, so the single-turret read is unambiguous.
pub fn log_sim_evidence(
    turrets: Query<&ServoState, With<crate::Turret>>,
    reloads: Query<&Reload>,
    wheels: Query<&Suspension>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    let grounded = wheels.iter().filter(|s| s.contact.is_some()).count();
    let total = wheels.iter().count();
    let turret = turrets.iter().next().map(ServoState::current);
    let reloads: Vec<f32> = reloads.iter().map(|r| r.remaining).collect();
    info!(
        "net: SIM-EVIDENCE wheels_grounded={grounded}/{total} turret_angle={turret:?} reloads={reloads:?}"
    );
}
