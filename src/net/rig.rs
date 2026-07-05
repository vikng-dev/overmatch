//! The networked tank-rig lifecycle: spawning a replicated tank's local sim body and arming the
//! predicted root's render smoothing. Since phase 1 of the sim/view split the body is built
//! SYNCHRONOUSLY from extracted data (`tank::spawn_tank_sim`) the moment the replicated root is
//! usable — colliders, servo frames, `Rig`, `TankSim`, all in one command flush; the glb scene
//! attaches later as pure view. Invariants this module maintains:
//!   1. the sim body attaches only to a valid replicated pose — never to avian's require-inserted
//!      placeholder (`attach_replicated_rig`'s pose gate, the cold-start crash fix);
//!   2. the body goes `Dynamic` in the same command-flush as the collider inserts
//!      (`activate_bound_rig`, an `Add<Rig>` observer in that flush's cascade) — never before
//!      (free-fall) and never after (the NaN ordering class);
//!   3. no rollback replays the entity before the end of its first full physics tick
//!      (`DisableRollback`, lifted by `enable_rollback_after_first_tick`) — the children's
//!      `ColliderTransform`s need one `PhysicsSystems::Prepare` propagation first.
//!
//! The rig's CHILDREN are not rollback participants at all: every carried child state lives
//! root-resident in `tank::TankSim` (one `local_rollback` on the predicted root), and child
//! transforms are derived from it each tick — which is what retired the whole
//! `DeterministicPredicted` decoration / pose-history-stripping / despawn-grace machinery this
//! module used to maintain (steps 7–8's hazard cluster).

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
// `Remote` (bevy_replicon's "this entity arrived by replication", re-exported): the honest
// authority-vs-replica discriminator — see `activate_bound_rig` on why `Predicted`/`Interpolated`
// are not (the server entity carries both markers itself).
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{Rig, TankSimSource, bind_tank_view, spawn_tank_sim};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(activate_bound_rig);
    // FixedLast = the earliest point provably AFTER the fresh rig's first full physics tick
    // (collider-transform propagation included) — see `enable_rollback_after_first_tick`.
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// The networked composition of the shared spawn core (`tank::tank_rig` — scene-as-view + spec +
/// `Tank`): adds the wire identity marker and the `RigidBody` (`tank.rs::spawn_tank` inserts it
/// alongside the core for the same reason — the sim spawner only adds mass properties and child
/// colliders). Used by both networked spawn paths: the server's per-client spawn
/// (`net::server::spawn_pending_tanks`) and the client's replicated-tank attach
/// (`attach_replicated_rig`); both call `spawn_tank_sim` in the same command batch.
pub(crate) fn net_tank_rig(assets: &crate::tank::PendingTankAssets) -> impl Bundle {
    (
        crate::tank::tank_rig(assets),
        NetTank,
        // Explicit, because on the CLIENT this bundle lands on a replicon-spawned root that has
        // only the replicated components (Position/Rotation) — without a Transform the hierarchy
        // under it never gets GlobalTransforms (Bevy B0004), collider offsets go wrong, and the
        // client settles at a different rest height than the server (measured: +1.25 vs −0.28 →
        // rollback on every packet). lightyear's avian sync owns writing this from Position
        // afterwards.
        Transform::default(),
        // Static in the bundle; `activate_bound_rig` (an `Add<Rig>` observer) flips the locally
        // simulated compositions to Dynamic within the same flush cascade, once the skeleton's
        // colliders are in the queue — remote (interpolated) tanks stay Static for good.
        avian3d::prelude::RigidBody::Static,
    )
}

/// Give the replicated tank its LOCAL sim body (map §6's `handle_new_character` pattern), built
/// synchronously from the extracted geometry the moment the replicated root is usable: avian
/// components are not replicated, and a predicted entity without a body cannot be re-simulated
/// during rollback replay — the symptom is continuous rollback from spawn, every confirmed packet
/// disagreeing with a frozen prediction. A plain system (not an observer on `Predicted`) because
/// `NetTank` arrives by replication and may land after the marker; waits on the asset gate (the
/// spec feeds the spawner, the preloaded glb keeps the view pop-in short).
///
/// `With<Remote>` = every replicated tank, whichever markers rode along: the own (predicted)
/// tank today, other players' (interpolated) tanks at step 9. A remote tank gets the same full
/// sim skeleton — node mapping, servos, and view anchors are what the camera/HUD and
/// `apply_servo_angles` lay the model with — but its body stays `Static` (`activate_bound_rig`
/// skips it): replication owns its pose, nothing local simulates it.
pub(crate) fn attach_replicated_rig(
    // `With<Position>, With<Rotation>`: THE COLD-START PLACEHOLDER GUARD. The bundle's
    // `RigidBody` require-inserts `Position::PLACEHOLDER`/`Rotation::PLACEHOLDER` (f32::MAX)
    // if the entity doesn't have them yet — and the replicated pose can land a few frames after
    // the `NetTank` marker (`.predict()` components ride the prediction sync, plain markers the
    // replication apply). Lose that race and the body's first Dynamic tick integrates from
    // f32::MAX and NaNs the root (measured: 9/9 cold-cache runs, root pos/rot = 3.4e38 at the
    // first post-bind probe; warm runs won the race by luck). Gating the body on the pose closes
    // the hole for every timing.
    tanks: Query<
        Entity,
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            Without<RigidBody>,
        ),
    >,
    assets: Option<Res<crate::tank::PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    for entity in &tanks {
        info!("client: {entity} replicated tank gets local sim body (assets loaded)");
        commands
            .entity(entity)
            .insert((
                net_tank_rig(&assets),
                // Defense-in-depth (NOT the placeholder crash — that's the pose gate above,
                // verified separately): no rollback may replay this entity until its body has
                // taken one full physics tick, because a replay before that steps physics over
                // child colliders whose `ColliderTransform`s haven't had their first
                // `PhysicsSystems::Prepare` propagation — the rollback check (PreUpdate) runs
                // before the first post-spawn FixedMain tick can clean them. `check_rollback`
                // skips `DisableRollback` entities entirely (and stamps them `Disabled` during
                // other entities' replays); `enable_rollback_after_first_tick` lifts it.
                DisableRollback,
            ))
            .observe(bind_tank_view);
        spawn_tank_sim(&mut commands, entity, geometry, spec);
    }
}

/// Lift [`DisableRollback`] once the fresh body has completed one full physics tick — at
/// `FixedLast` of that tick, `PhysicsSystems::Prepare` (FixedPostUpdate) has already replaced the
/// children's placeholder poses with propagated ones, so replays are safe from here on. Gated on
/// `Rig` (present from the spawn flush, so this fires at the first post-spawn FixedLast) and
/// not-in-rollback (during a replay triggered by another entity, this one is `Disabled` and must
/// stay protected).
fn enable_rollback_after_first_tick(
    fresh: Query<Entity, (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for entity in &fresh {
        info!("net: {entity} first post-bind tick complete — rollback enabled");
        commands.entity(entity).remove::<DisableRollback>();
    }
}

/// Wake a networked tank's physics the instant its sim body exists — an OBSERVER on the spawner's
/// `Rig` insert, so `RigidBody::Dynamic` applies in the same command-flush cascade as the
/// skeleton's collider spawns, BEFORE any avian system runs that frame. Ordering is load-bearing,
/// established empirically against the old async binder (step-8 NaN-crash bisection at 100 ms):
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
    add: On<Add, Rig>,
    eligible: Query<Entity, (With<NetTank>, Or<(With<Predicted>, Without<Remote>)>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!("net: {} rig bound — body goes Dynamic", add.entity);
    commands.entity(add.entity).insert(RigidBody::Dynamic);
}

/// Client-side render smoothing for the predicted tank — the half of lightyear's prediction stack
/// `LightyearAvianPlugin` does NOT mount (it only *orders* these systems' sets; the plugins and the
/// per-entity `FrameInterpolate` markers are the app's job, per `lightyear_frame_interpolation`'s
/// docs and the `avian_3d_character` example). Two effects:
///   - between fixed ticks the root's Position/Rotation render as an overstep blend instead of raw
///     64 Hz steps;
///   - rollback *correction* arms: `update_frame_interpolation_post_rollback` requires
///     `FrameInterpolate<C>` on the entity, so without it the registered correction fn is inert and
///     every rollback SNAPS the tank (measured 10–26 rollbacks/s while driving at 80 ms — the
///     rubber-banding) instead of decaying the error over `CorrectionPolicy` (~200 ms half-life).
pub fn client_smoothing_plugin(app: &mut App) {
    app.add_plugins((
        FrameInterpolationPlugin::<Position>::default(),
        FrameInterpolationPlugin::<Rotation>::default(),
    ));
    app.add_systems(Update, arm_predicted_smoothing);
}

/// Decorate the predicted tank ROOT with `FrameInterpolate` once `Predicted` and `Position` are
/// both present. A polling system, not an `Add` observer: the prediction sync copies components
/// from the confirmed entity in no guaranteed order (same shape as `strip_child_pose_history`).
/// Root only — the children's poses are DERIVED state (root pose ∘ collider/servo transforms);
/// frame-interpolating them would fight the systems that derive them.
fn arm_predicted_smoothing(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<NetTank>,
            With<Position>,
            Without<FrameInterpolate<Position>>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed for frame interpolation + correction");
        commands.entity(entity).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
    }
}
