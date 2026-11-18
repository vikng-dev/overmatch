//! Local simulation attachment for replicated tank roots, and the aim they present.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose;
//! children derive from root simulation. Every replica — the own hull included — rides the
//! interpolated server stream on a `Static` body; every attached aim rig draws every
//! servo from that stream, at the same view cursor. In the game's damage-model
//! presentation, a knocked-out tank's launched turret keeps its own replicated pose instead.

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::{NetTank, ServoAngles};
use crate::damage::LaunchedTurret;
use crate::tank::{
    PendingTankAssets, Rig, ServoView, StreamedServoPose, TankTemplate, attach_replicated_tank_body,
};
use crate::track::sim::EngineRpm;

/// Attach simulation only after interpolation has established this frame's visible component
/// set. An init snapshot may contain a pose whose first sample is still ahead of the cursor;
/// attaching before presence preparation would give that not-yet-visible root live consumers.
pub(crate) fn attach_replicated_rig() -> ScheduleConfigs<ScheduleSystem> {
    attach_visible_rigs
        .after(InterpolationSystems::All)
        .into_configs()
}

fn attach_visible_rigs(
    tanks: Query<
        (Entity, &ServoAngles),
        (
            With<Remote>,
            With<NetTank>,
            // Wait until Lightyear declares the replica's role.
            With<Interpolated>,
            With<Position>,
            With<Rotation>,
            // The replicated public crank speed must precede body attachment, so the engine sound
            // the body brings is born at the authority's crank, never a reconstructed JIP value.
            // It is the one transmission fact every client receives: `TankTransmission` reaches
            // only the owner, in the same init snapshot.
            With<EngineRpm>,
            Without<RigidBody>,
        ),
    >,
    assets: Res<PendingTankAssets>,
    template: Res<TankTemplate>,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    for (entity, angles) in &tanks {
        attach_replicated_tank_body(
            &mut commands,
            entity,
            &template,
            assets.presentation(),
            (
                NetTank,
                // The local hierarchy requires `Transform`; Avian writes it from the wire pose.
                Transform::default(),
                RigidBody::Static,
            ),
            StreamedServoPose(angles.0.clone()),
        );
    }
}

/// Pose every attached aim rig from its aim stream: this frame's
/// [`ServoAngles`], which Lightyear's interpolation sampled at the view cursor its hull pose was
/// sampled at, before the servo view draws them, regardless of ownership.
pub(crate) fn present_streamed_aim() -> ScheduleConfigs<ScheduleSystem> {
    present_aim
        .after(InterpolationSystems::All)
        // The frame a rig attaches already draws its stream's aim.
        .after(attach_visible_rigs)
        .before(ServoView)
        .into_configs()
}

fn present_aim(
    mut tanks: Query<(&ServoAngles, &Rig, &mut StreamedServoPose), With<Remote>>,
    launched: Query<(), With<LaunchedTurret>>,
) {
    for (angles, rig, mut servos) in &mut tanks {
        // The game's damage model: a launched assembly follows `LaunchedTurretPose`, not aim.
        if launched.contains(rig.turret) {
            continue;
        }
        assert_eq!(
            servos.0.len(),
            angles.0.len(),
            "wire admission checks the rig's slot count before presentation"
        );
        servos.0.copy_from_slice(&angles.0);
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;

    use super::*;

    /// Every player's barrel follows the sampled stream regardless of ownership. The game's
    /// damage model gives a launched assembly its own body pose instead of attached aim.
    #[test]
    fn the_stream_poses_own_and_remote_replicas_but_not_launched_assemblies() {
        // Fixture sampled pose includes slots outside the gunner chain.
        let angles = ServoAngles(vec![-0.2, 0.75, 0.3, -0.1]);
        let prior = vec![0.1; angles.0.len()];
        let mut world = World::new();
        let mut spawn = |owned: bool, launched: bool| {
            let turret = world.spawn_empty().id();
            if launched {
                world.entity_mut(turret).insert(LaunchedTurret);
            }
            let rig = Rig {
                hull: world.spawn_empty().id(),
                turret,
                gunner_yaw: world.spawn_empty().id(),
                gun: world.spawn_empty().id(),
                muzzle: world.spawn_empty().id(),
            };
            let mut root = world.spawn((
                Remote,
                angles.clone(),
                rig,
                StreamedServoPose(prior.clone()),
            ));
            if owned {
                root.insert(crate::tank::Controlled);
            }
            root.id()
        };
        let remote = spawn(false, false);
        let own = spawn(true, false);
        let launched = spawn(false, true);
        world.run_system_once(present_aim).unwrap();
        for root in [own, remote] {
            assert_eq!(world.get::<StreamedServoPose>(root).unwrap().0, angles.0);
        }
        assert_eq!(world.get::<StreamedServoPose>(launched).unwrap().0, prior);
    }
}
