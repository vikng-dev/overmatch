//! Local simulation for replicated tank roots.
//!
//! This is the explicit ADR-0014 exception: replication creates the root before local simulation
//! can. The adapter waits for a valid replicated pose, attaches the body and its `RigidBody` in one
//! command flush, then blocks rollback until one physics tick has prepared the body. A late
//! `Predicted` marker promotes a static body to dynamic and re-arms the guard. Rig children derive
//! their state from root-resident `TankSim` and are not rollback participants.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
// `Remote` is the authority boundary: server roots never carry it; replicated client roots do.
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{PendingTankAssets, Rig, TankSimSource, attach_replicated_tank_body};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(upgrade_predicted_to_dynamic);
    // FixedLast = the earliest point provably AFTER the fresh rig's first full physics tick
    // (collider-transform propagation included) — see `enable_rollback_after_first_tick`.
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// Attach the complete local body as soon as a replicated root has a valid pose. Presentation
/// handles may still be unresolved; simulation comes only from [`TankSimSource`]. A predicted root
/// is dynamic, while an interpolated root (or one whose `Predicted` marker is late) starts static.
pub(crate) fn attach_replicated_rig(
    // RigidBody require-inserts placeholder Position/Rotation. The wire pose must arrive first.
    tanks: Query<
        (Entity, Has<Predicted>),
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            Without<RigidBody>,
        ),
    >,
    assets: Option<Res<PendingTankAssets>>,
    source: TankSimSource,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    let Some(content) = source.get() else {
        return;
    };
    for (entity, predicted) in &tanks {
        let body = if predicted {
            RigidBody::Dynamic
        } else {
            RigidBody::Static
        };
        info!("client: {entity} replicated tank gets local sim body (predicted={predicted})");
        attach_replicated_tank_body(
            &mut commands,
            entity,
            content,
            assets.presentation(),
            (
                NetTank,
                // Required for the local hierarchy; Avian syncs it from Position/Rotation.
                Transform::default(),
                body,
                // Collider transforms need one PhysicsSystems::Prepare pass before replay.
                DisableRollback,
            ),
        );
    }
}

/// Enable rollback after one complete physics tick has prepared a newly attached or promoted body.
fn enable_rollback_after_first_tick(
    fresh: Query<Entity, (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for entity in &fresh {
        info!("net: {entity} first physics tick complete — rollback enabled");
        commands.entity(entity).remove::<DisableRollback>();
    }
}

/// Promote a replicated body when `Predicted` arrives after its pose. Marker and pose visibility are
/// independent, so the body may already exist as static. Promotion re-arms the first-tick guard.
fn upgrade_predicted_to_dynamic(
    add: On<Add, Predicted>,
    eligible: Query<(), (With<Remote>, With<NetTank>, With<Rig>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!(
        "net: {} predicted marker arrived after spawn — body goes Dynamic",
        add.entity
    );
    commands
        .entity(add.entity)
        .insert((RigidBody::Dynamic, DisableRollback));
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
///     rubber-banding). Since 597ec21 the correction policy is `instant_correction()` — the sim
///     snaps to the corrected present in one frame by design and the render-space error layer
///     (`net/render_error.rs`) absorbs the discontinuity on the view side; this arming remains
///     load-bearing for the between-ticks overstep blend and lightyear's correction plumbing.
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
