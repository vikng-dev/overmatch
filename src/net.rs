//! Shared networking protocol registration (lightyear spike, `net` feature). Mounted by both
//! `spike_server` and `spike_client` — lightyear requires identical protocol registration on
//! both sides of the wire, added after `ServerPlugins`/`ClientPlugins` and before the
//! `Server`/`Client` connection entity is spawned (see the spike map §3 ordering note).

use avian3d::prelude::{
    AngularVelocity, Forces, IslandPlugin, IslandSleepingPlugin, LinearVelocity,
    PhysicsInterpolationPlugin, PhysicsTransformPlugin, Position, ReadRigidBodyForces, Rotation,
    WriteRigidBodyForces,
};
use bevy::prelude::*;
use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::{Tank, TankCommand, TankSpecHandle};

/// A trivial replicated marker — step 3 of the spike: proves `.replicate()` registration and
/// ordering before any real game state rides the wire.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SpikeBeacon;

/// Marks the predicted body the stub movement system drives — the increment-5 primitive, or (from
/// increment 6) the real tank root. One system covers both, since the driven bundle is always just
/// the root's rigid-body components, whichever visuals ride along.
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
/// `Tank` marker: `drive_stub_movement` and the increment-5 rollback plumbing key off `SpikeTank`,
/// while `Tank`/`on_tank_ready` are the real rig's own contract, unchanged.
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

/// The static ground both sides build for themselves (never moves, so it is not replicated) —
/// the client's rollback replays need terrain to collide with just as much as the server does.
pub fn spike_ground() -> impl Bundle {
    (
        Name::new("Ground"),
        avian3d::prelude::RigidBody::Static,
        // Big enough that the over-torqued stub can't drive off the edge mid-scenario.
        avian3d::prelude::Collider::cuboid(4000.0, 1.0, 4000.0),
        Position(Vec3::new(0.0, -0.5, 0.0)),
    )
}

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub fn plugin(app: &mut App) {
    app.component::<SpikeBeacon>().replicate();
    app.component::<SpikeTank>().replicate();
    app.add_plugins(input::native::InputPlugin::<TankCommand>::default());

    // Avian replication (map §5): mount lightyear_avian3d's ordering fixes, then register the
    // root's Position/Rotation/velocities as predicted+rollback-eligible. Verbatim rollback
    // conditions/correction/interpolation fns from `avian_3d_character`'s `protocol.rs` — the only
    // real 3D reference in the lightyear repo for this registration shape.
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position,
        ..default()
    });
    app.component::<Position>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Position, b: &Position| (a.0 - b.0).length() >= 0.01)
        .add_linear_correction_fn()
        .add_linear_interpolation();
    app.component::<Rotation>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Rotation, b: &Rotation| a.angle_between(*b) >= 0.01)
        .add_linear_correction_fn()
        .add_linear_interpolation();
    // Same 1 cm/s(-equivalent) threshold as Position/Rotation above — without an explicit
    // condition these default to `PartialEq::ne` (exact bit equality), which f32 solver output
    // essentially never satisfies between client and server. That was firing a rollback on almost
    // every packet (measured: 632/1.8s at zero latency) even in straight-line steady state, because
    // ANY one predicted component voting "rollback" forces the whole entity to roll back.
    // Verbatim thresholds from `avian_3d_character`'s `protocol.rs`.
    app.component::<LinearVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &LinearVelocity, b: &LinearVelocity| {
            (a.0 - b.0).length() >= 0.01
        });
    app.component::<AngularVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &AngularVelocity, b: &AngularVelocity| {
            (a.0 - b.0).length() >= 0.01
        });

    app.add_systems(Update, activate_bound_rigs);
    app.add_systems(FixedUpdate, drive_stub_movement);
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

/// Drives the predicted root from `TankCommand.throttle`/`.steer` — a placeholder for the real
/// `driving` module (step 7, out of scope). Runs identically on server and client: the client
/// needs it too, so prediction's rollback replay re-simulates the same forces (map §8's "rollback
/// re-runs the entire FixedMain schedule" — this system must be idempotent/deterministic, and it
/// is, being a pure function of this tick's command + current velocity).
///
/// Sized against friction: ~57 t at avian's default μ=0.5 costs ~280 kN to keep sliding — 500 kN
/// nets ~3.9 m/s², reaching ~15 m/s over the 4 s script: actual tank speeds, so the rollback-rate
/// measurement reflects the real game, not a 60 m/s missile (where the 1 cm rollback threshold is
/// 1% of one tick's motion and trips on solver noise every packet — measured, 633 rollbacks at
/// zero latency before this was tamed).
const DRIVE_FORCE: f32 = 500_000.0;
const STEER_TORQUE: f32 = 200_000.0;

fn drive_stub_movement(mut tanks: Query<(&ActionState<TankCommand>, Forces), With<SpikeTank>>) {
    for (action, mut forces) in &mut tanks {
        let cmd = &action.0;
        let forward = forces.rotation().0 * Vec3::NEG_Z;
        forces.apply_force(forward * cmd.throttle * DRIVE_FORCE);
        forces.apply_torque(Vec3::Y * -cmd.steer * STEER_TORQUE);
    }
}
