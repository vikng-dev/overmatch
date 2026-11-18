//! Local simulation attachment for replicated tank roots, and the aim they present.
//!
//! ADR-0014 exception: a replicated root receives simulation only after it has a wire pose;
//! children derive from root simulation. Every replica — the own hull included — rides the
//! interpolated server stream on a `Static` body; every attached aim rig also draws its
//! turret and gun from that stream, at the same view cursor. In the game's damage-model
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
    PendingTankAssets, RemoteServos, Rig, ServoIndex, ServoState, ServoView, TankTemplate,
    attach_replicated_tank_body,
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
        Entity,
        (
            With<Remote>,
            With<NetTank>,
            // Wait until Lightyear declares the replica's role.
            With<Interpolated>,
            With<Position>,
            With<Rotation>,
            // The aim stream rides the same init snapshot; an attached rig always has one to
            // present (`present_streamed_aim`).
            With<ServoAngles>,
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
    for entity in &tanks {
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
    mut tanks: Query<(&ServoAngles, &Rig, &mut RemoteServos), With<Remote>>,
    slots: Query<&ServoIndex>,
    launched: Query<(), With<LaunchedTurret>>,
) {
    for (angles, rig, mut servos) in &mut tanks {
        // The game's damage model: a launched assembly follows `LaunchedTurretPose`, not aim.
        if launched.contains(rig.turret) {
            continue;
        }
        for (servo, angle) in [
            (rig.gunner_yaw, angles.gunner_yaw),
            (rig.gun, angles.gunner_pitch),
        ] {
            let slot = slots
                .get(servo)
                .expect("complete spawn gives every attached aim servo its index");
            *servos
                .0
                .get_mut(slot.0)
                .expect("complete spawn gives every aim servo a client state slot") =
                ServoState::presented(angle);
        }
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
        // Fixture sampled angles and prior motion.
        let angles = ServoAngles {
            gunner_yaw: 0.75,
            gunner_pitch: -0.2,
        };
        let prior = ServoState::test_new(0.1, 0.05, 0.3);
        let mut world = World::new();
        let mut spawn = |owned: bool, launched: bool| {
            let turret = world.spawn_empty().id();
            if launched {
                world.entity_mut(turret).insert(LaunchedTurret);
            }
            let rig = Rig {
                hull: world.spawn_empty().id(),
                turret,
                gunner_yaw: world.spawn(ServoIndex(1)).id(),
                gun: world.spawn(ServoIndex(0)).id(),
                muzzle: world.spawn_empty().id(),
            };
            let mut root = world.spawn((Remote, angles, rig, RemoteServos(vec![prior, prior])));
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
            assert_eq!(
                world.get::<RemoteServos>(root).unwrap().0,
                vec![ServoState::presented(-0.2), ServoState::presented(0.75)],
            );
        }
        assert_eq!(
            world.get::<RemoteServos>(launched).unwrap().0,
            vec![prior, prior]
        );
    }
}
