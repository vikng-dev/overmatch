//! Shared networking protocol registration (lightyear spike, `net` feature). Mounted by both
//! `spike_server` and `spike_client` — lightyear requires identical protocol registration on
//! both sides of the wire, added after `ServerPlugins`/`ClientPlugins` and before the
//! `Server`/`Client` connection entity is spawned (see the spike map §3 ordering note).

use avian3d::physics_transform::PhysicsTransformSystems;
use avian3d::prelude::{
    AngularVelocity, IslandPlugin, IslandSleepingPlugin, LinearVelocity,
    PhysicsInterpolationPlugin, PhysicsTransformPlugin, Position, RigidBody, Rotation,
};
use avian3d::schedule::PhysicsSystems;
use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
use lightyear::prediction::diagnostics::PredictionDiagnosticsPlugin;
// `Remote` (bevy_replicon's "this entity arrived by replication", re-exported): the honest
// authority-vs-replica discriminator — see `activate_bound_rig` on why `Predicted`/`Interpolated`
// are not (the server entity carries both markers itself).
use lightyear::prelude::client::Remote;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::tank::ServoCommand;
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
        // still falling when the script ended). `activate_bound_rig` flips it to Dynamic the
        // moment `Rig` lands — the spike-scale version of the game's spawn-before-bind race.
        avian3d::prelude::RigidBody::Static,
    )
}

/// Wake a spike tank's physics the instant its rig binds — an OBSERVER on the binder's terminal
/// `Rig` insert, so `RigidBody::Dynamic` applies in the same command-flush cascade as the binder's
/// collider constructors, BEFORE any avian system runs that frame. Ordering is load-bearing,
/// established empirically (step-8 NaN-crash bisection at 100 ms):
///   - Dynamic landing after avian attached the constructed child colliders → every child
///     collider's `Position` goes NaN within a frame (avian finite assert, 8/8 with an
///     attachment-gated flip, ~60% with the old `Added<Rig>` Update system whose one-frame gap
///     sometimes let attachment win the race);
///   - colliders attaching to an already-Dynamic body → clean, and it is the only ordering the
///     rest of the game exercises (SP spawns tanks Dynamic from birth).
///
/// Only where the local side simulates the body: the authority (`Without<Remote>` — the server
/// spawned it) or the client's own predicted tank. A remote (interpolated) tank — other players'
/// tanks, from step 9 — stays `Static`: its `Position` is written by replication+interpolation
/// (the same sync that already carries the pre-bind Static body), and a Dynamic body would
/// free-run local physics against it.
///
/// NOT keyed on `Interpolated`: `PredictionTarget`/`InterpolationTarget` are
/// `ReplicationTarget<Predicted>`/`<Interpolated>` with the marker as a *required component*, so
/// the server entity carries BOTH markers itself (send.rs registers the pairs; the markers are
/// then target-filtered replicated components) — `Without<Interpolated>` excludes the authority.
/// Measured: server rig bound but never went Dynamic, wheels 0/16 both ends.
fn activate_bound_rig(
    add: On<Add, crate::Rig>,
    eligible: Query<Entity, (With<SpikeTank>, Or<(With<Predicted>, Without<Remote>)>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!("net: {} rig bound — body goes Dynamic", add.entity);
    commands.entity(add.entity).insert(RigidBody::Dynamic);
}

/// Diagnostic (bind-window NaN): at the top of each physics tick, name every entity whose
/// physics state or `ColliderTransform` is non-finite — with values — then latch. Runs before
/// `PhysicsSystems::Prepare`, i.e. before the step that would hit avian's panicking asserts.
fn fixed_nan_probe(
    bodies: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
        ),
        With<Tank>,
    >,
    parts: Query<
        (
            Entity,
            Option<&Name>,
            Option<&Position>,
            Option<&Rotation>,
            Option<&avian3d::prelude::ColliderTransform>,
        ),
        Or<(
            With<avian3d::prelude::ColliderOf>,
            With<avian3d::prelude::RayCaster>,
        )>,
    >,
    mut latched: Local<bool>,
) {
    if *latched {
        return;
    }
    let mut corrupt = false;
    for (entity, position, rotation, linear, angular) in &bodies {
        let bad_vel =
            linear.is_some_and(|v| !v.0.is_finite()) || angular.is_some_and(|v| !v.0.is_finite());
        if !position.0.is_finite() || !rotation.0.is_finite() || bad_vel {
            error!(
                "net: FIXED-NAN root {entity}: pos={:?} rot={:?} linvel={:?} angvel={:?}",
                position.0,
                rotation.0,
                linear.map(|v| v.0),
                angular.map(|v| v.0)
            );
            corrupt = true;
        }
    }
    for (entity, name, position, rotation, collider_transform) in &parts {
        let bad = position.is_some_and(|p| !p.0.is_finite())
            || rotation.is_some_and(|r| !r.0.is_finite())
            || collider_transform
                .is_some_and(|t| !(t.translation.is_finite() && t.rotation.0.is_finite()));
        if bad {
            error!(
                "net: FIXED-NAN part {entity} ({:?}): pos={:?} rot={:?} collider_transform={:?}",
                name.map(|n| n.as_str()),
                position.map(|p| p.0),
                rotation.map(|r| r.0),
                collider_transform
            );
            corrupt = true;
        }
    }
    if corrupt {
        *latched = true;
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

/// Authoritative turret/gun angles (radians, parent-local — `ServoState::current`'s own frame),
/// published on the tank root by the authority and replicated. Remote (interpolated) tanks —
/// other players' tanks, from step 9 — have no local servo sim; this is how their rigs lay.
///
/// Applied as `ServoCommand` *targets*, not written into `ServoState`: the local servo mechanism
/// (`drive_servos`) chases the authoritative angle under its real speed/accel profile, which
/// smooths replication-rate steps for free — no interpolation registration, no transform fights
/// with `interpolate_servos`. The hull MG's servos are deliberately not covered yet (per-weapon
/// laying is its own slice); a remote hull MG rests until then.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct ServoAngles {
    pub turret: f32,
    pub gun: f32,
}

/// Authority side: mirror the live `ServoState` angles onto the replicated root component.
/// `FixedPostUpdate`, so it reads what `drive_servos` (FixedUpdate, after `GameplaySet`) just
/// stepped. `Without<Remote>` makes it authority-only in shared code: every client-side tank
/// arrived by replication and carries `Remote` (see `activate_bound_rig` on why the
/// `Predicted`/`Interpolated` markers can NOT discriminate here — the server carries both).
fn publish_servo_angles(
    mut tanks: Query<(&Rig, &mut ServoAngles), Without<Remote>>,
    servos: Query<&ServoState>,
) {
    for (rig, mut angles) in &mut tanks {
        let (Ok(turret), Ok(gun)) = (servos.get(rig.turret), servos.get(rig.gun)) else {
            continue;
        };
        // `set_if_neq`: no change-detection churn (and no replication resends) while at rest.
        angles.set_if_neq(ServoAngles {
            turret: turret.current(),
            gun: gun.current(),
        });
    }
}

/// Client side, remote (interpolated) tanks: feed the replicated angles to the local servos as
/// targets — the mechanism does the rest (see [`ServoAngles`]). In `GameplaySet` so it shares the
/// Playing gate with the rest of the sim; `drive_servos` orders itself after the whole set, so the
/// targets land before the mechanism steps. No write conflict with `drive_aim_servos` (also in the
/// set): a remote tank's `TankCommand` stays default (no input slot, and the bridge below skips
/// non-simulated tanks), so `aim` is `None` and that system never touches these tanks' servos.
fn apply_servo_angles(
    tanks: Query<(&ServoAngles, &Rig), (With<Remote>, Without<Predicted>)>,
    mut servos: Query<&mut ServoCommand>,
) {
    for (angles, rig) in &tanks {
        if let Ok(mut turret) = servos.get_mut(rig.turret) {
            turret.target = angles.turret;
        }
        if let Ok(mut gun) = servos.get_mut(rig.gun) {
            gun.target = angles.gun;
        }
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
///
/// Velocity is coarser still: rough terrain (the course's bump/washboard) puts sustained vertical-
/// velocity transients through the suspension, and client/server solver noise on those transients
/// tripped 0.05 at 20–60 rollbacks/s at ZERO latency — every recorded cause was `LinearVelocity`
/// (step-8 washboard investigation). 0.20 cut that stream ~64% with convergence unchanged
/// (positions agree to centimetres mid-washboard); velocity errors self-damp through the
/// suspension, and the position/rotation bars still catch real drift.
const ROLLBACK_POSITION_M: f32 = 0.05;
const ROLLBACK_ROTATION_RAD: f32 = 0.05;
const ROLLBACK_VELOCITY: f32 = 0.20;

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub fn plugin(app: &mut App) {
    app.component::<SpikeBeacon>().replicate();
    app.component::<SpikeTank>().replicate();
    // Plain replication, no `.predict()`/interpolation: predicted tanks simulate their own servos,
    // and non-predicted consumers chase the raw angle through the servo mechanism (see the type).
    app.component::<ServoAngles>().replicate();
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

    // THE BIND-WINDOW NaN FIX. Disabling `PhysicsTransformPlugin` (required by
    // `LightyearAvianPlugin`, see `physics_plugins()`) also removes the ONLY `configure_sets`
    // that anchors `PhysicsTransformSystems::Propagate` inside `PhysicsSystems::Prepare`
    // (avian `physics_transform/mod.rs:86`) — but `ColliderTransformPlugin` (mounted by the
    // collider backend, NOT disabled) still adds `propagate_collider_transforms` to that set in
    // `FixedPostUpdate`. Unanchored, it ran at an arbitrary point relative to the physics step:
    // when a freshly-bound rig's colliders caught the wrong interleaving, their
    // `ColliderTransform`s went NaN and took every child collider `Position` with them (~70% of
    // 100 ms runs, within a frame of Dynamic activation; activation-order fixes empirically
    // falsified — see the spike log). Re-anchoring the set restores avian's own ordering.
    app.configure_sets(
        FixedPostUpdate,
        PhysicsTransformSystems::Propagate.in_set(PhysicsSystems::Prepare),
    );

    app.add_observer(activate_bound_rig);
    app.add_systems(Update, decorate_rig_children);
    // Probe ahead of the physics pass, so the first corrupt value is named BEFORE avian's own
    // finite-asserts panic mid-step (the Update-schedule tripwire never sees it — corruption and
    // panic land inside one FixedMain run).
    app.add_systems(
        FixedPostUpdate,
        fixed_nan_probe.before(PhysicsSystems::Prepare),
    );
    app.add_systems(FixedPostUpdate, publish_servo_angles);
    app.add_systems(FixedUpdate, apply_servo_angles.in_set(GameplaySet));
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
/// networked input and every sim system. Only entities carrying both, which are exactly the
/// locally-simulated tanks: the server's tanks get `ActionState` at spawn, the client's own
/// predicted tank gets it when `InputMarker<TankCommand>` claims the slot (`claim_input_slot`,
/// client bin); remote (interpolated) tanks never carry one. `TankCommand` itself comes from
/// `command::core_plugin`'s `attach_command` observer (`On<Add, Tank>`).
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
