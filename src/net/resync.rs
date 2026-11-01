//! Presentation continuity at the interpolation history boundary.
//!
//! Already-presented replica components hold their last stream value when the cursor rewinds
//! behind retained history. `queue_history_presence` in lightyear_interpolation removes a live
//! component for both unknown history and an explicit removal; only the latter is a disappearance
//! in game state. The removal observer restores unknown-history samples in the same command flush,
//! before any scheduled consumer can see a missing pose. No extrapolation or history is invented.
//! New components still wait for their first visible sample, and explicit removals and despawns
//! still retire them. Register this policy beside each continuous interpolation registration.
//! The cause, decision and capture evidence live in
//! `.agents/docs/design/client-resync-continuity.md`.

use core::marker::PhantomData;

use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::core::history_buffer::HistoryState;
use lightyear::interpolation::plugin::InterpolationSystems;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::client::Remote;
use lightyear::prelude::{Interpolated, NetworkTimeline};

/// A component is tracked only after a stream sample has actually presented. Holding is a named
/// presentation state, not an authoritative value at the rewound cursor.
#[derive(Component)]
enum Presented<C: Component> {
    Sampled(PhantomData<fn() -> C>),
    Held,
}

pub(super) fn retain_stream<C: Component + Clone>(app: &mut App) {
    app.add_observer(hold_unknown_history::<C>).add_systems(
        Update,
        note_presented::<C>
            .in_set(InterpolationSystems::Interpolate)
            .run_if(resource_exists::<InterpolationTimeline>),
    );
}

fn hold_unknown_history<C: Component + Clone>(
    remove: On<Remove, C>,
    replicas: Query<(&C, &ConfirmedHistory<C>, &Presented<C>), (With<Remote>, With<Interpolated>)>,
    timeline: Option<Res<InterpolationTimeline>>,
    mut commands: Commands,
) {
    // Authority components and replicas that have never presented have no held stream sample.
    let (Some(timeline), Ok((value, history, _))) = (timeline, replicas.get(remove.entity)) else {
        return;
    };
    // Remove also runs during despawn. Both queued operations deliberately expire if the
    // entity has departed by the time commands flush; a departed replica must stay gone.
    if history.get_state_at_or_before(timeline.tick()).is_none() {
        commands
            .entity(remove.entity)
            .try_insert((value.clone(), Presented::<C>::Held));
    } else {
        commands.entity(remove.entity).try_remove::<Presented<C>>();
    }
}

fn note_presented<C: Component>(
    replicas: Query<
        (
            Entity,
            Option<&C>,
            &ConfirmedHistory<C>,
            Option<&Presented<C>>,
        ),
        (With<Remote>, With<Interpolated>),
    >,
    timeline: Res<InterpolationTimeline>,
    mut commands: Commands,
) {
    for (entity, value, history, presented) in &replicas {
        match history.get_state_at_or_before(timeline.tick()) {
            Some(HistoryState::Updated(_)) => {
                assert!(
                    value.is_some(),
                    "interpolation preparation inserts a component whose stream sample is visible"
                );
                if !matches!(presented, Some(Presented::Sampled(_))) {
                    commands
                        .entity(entity)
                        .insert(Presented::<C>::Sampled(PhantomData));
                }
            }
            Some(HistoryState::Removed) if presented.is_some() => {
                commands.entity(entity).remove::<Presented<C>>();
            }
            // No history is either a not-yet-visible component or a held resync sample.
            // An explicit removal with no presentation marker is already retired.
            None | Some(HistoryState::Removed) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use avian3d::prelude::{Position, RigidBody, Rotation};
    use bevy::prelude::*;
    use lightyear::core::confirmed_history::ConfirmedHistory;
    use lightyear::core::time::TickInstant;
    use lightyear::interpolation::timeline::InterpolationTimeline;
    use lightyear::prelude::client::{ClientPlugins, Remote};
    use lightyear::prelude::{Interpolated, NetworkTimeline, Tick};

    use super::super::test_harness::{TICK, finish, net_physics_app};

    fn cursor(app: &mut App, tick: u32) {
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_now(TickInstant::from(Tick(tick)));
    }

    fn history<C>(value: C) -> ConfirmedHistory<C> {
        let mut history = ConfirmedHistory::default();
        // ASSUMED fixture ticks, kept away from the monotonic clock's zero boundary.
        history.insert_present_explicit(Tick(100), value);
        history
    }

    fn interpolation_app() -> App {
        let mut app = net_physics_app();
        app.set_error_handler(bevy::ecs::error::panic);
        app.add_plugins(ClientPlugins {
            tick_duration: TICK,
        });
        super::super::protocol::plugin(&mut app);
        finish(&mut app);
        app
    }

    /// The game's damage model and netcode: a cursor rewind behind retained history must hold the
    /// last presented pose for every consumer, then resume sampling. Shooting and collider history
    /// rely on required-pose queries; a per-consumer skip cannot satisfy this invariant.
    #[test]
    fn resync_keeps_a_replica_pose_through_every_schedule() {
        let mut app = interpolation_app();
        let at = Position(Vec3::new(10.0, 2.0, 30.0));
        let rotation = Rotation(Quat::from_rotation_y(0.4));
        let mut positions = history(at);
        positions.insert_present(Tick(102), Position(at.0 + Vec3::X));
        let mut rotations = history(rotation);
        rotations.insert_present(Tick(102), rotation);
        let root = app
            .world_mut()
            .spawn((
                Remote,
                Interpolated,
                RigidBody::Static,
                at,
                rotation,
                positions,
                rotations,
            ))
            .id();
        cursor(&mut app, 100);
        app.update();
        let last = *app.world().get::<Position>(root).unwrap();
        for tick in 90..94 {
            cursor(&mut app, tick);
            app.update();
            assert_eq!(
                app.world().get::<Position>(root),
                Some(&last),
                "a resync must hold the last presented pose, without extrapolating"
            );
            assert_eq!(app.world().get::<Rotation>(root), Some(&rotation));
            assert!(matches!(
                app.world().get::<super::Presented<Position>>(root),
                Some(super::Presented::Held)
            ));
        }
        cursor(&mut app, 102);
        app.update();
        assert_eq!(app.world().get::<Position>(root).unwrap().0, at.0 + Vec3::X);
        assert!(matches!(
            app.world().get::<super::Presented<Position>>(root),
            Some(super::Presented::Sampled(_))
        ));
    }

    /// Retention belongs to every continuous stream registration, including a carrier that is
    /// not a tank. A pose-only repair would leave drivetrain consumers exposed to the same gap.
    #[test]
    fn every_continuous_stream_holds_but_explicit_removal_and_despawn_win() {
        use super::super::debris_stress::DebrisPose;
        use crate::track::sim::{DriveSignal, TrackDrive};

        let mut app = interpolation_app();
        let drive = TrackDrive::default();
        let signal = DriveSignal {
            throttle: 0.5,
            steer: -0.25,
        };
        let debris = DebrisPose {
            position: Vec3::Y,
            rotation: Quat::IDENTITY,
        };
        let entity = app
            .world_mut()
            .spawn((
                Remote,
                Interpolated,
                drive,
                history(drive),
                signal,
                history(signal),
                debris,
                history(debris),
            ))
            .id();
        cursor(&mut app, 100);
        app.update();
        cursor(&mut app, 90);
        app.update();
        assert_eq!(app.world().get::<TrackDrive>(entity), Some(&drive));
        assert_eq!(app.world().get::<DriveSignal>(entity), Some(&signal));
        assert_eq!(app.world().get::<DebrisPose>(entity), Some(&debris));
        app.world_mut()
            .get_mut::<ConfirmedHistory<DriveSignal>>(entity)
            .unwrap()
            .insert_removed(Tick(90));
        cursor(&mut app, 90);
        app.update();
        assert!(
            app.world().get::<DriveSignal>(entity).is_none(),
            "an explicit server removal must not be held"
        );
        assert!(
            app.world()
                .get::<super::Presented<DriveSignal>>(entity)
                .is_none()
        );
        app.world_mut().entity_mut(entity).despawn();
        app.world_mut().flush();
        assert!(
            app.world().get_entity(entity).is_err(),
            "a held replica must not resurrect"
        );
        let sampled = app
            .world_mut()
            .spawn((Remote, Interpolated, signal, history(signal)))
            .id();
        cursor(&mut app, 100);
        app.update();
        app.world_mut().entity_mut(sampled).despawn();
        app.world_mut().flush();
        assert!(
            app.world().get_entity(sampled).is_err(),
            "retiring a sampled replica must not queue a command error after despawn"
        );
    }

    /// Before a component's first visible sample it has no previous presentation to hold. Keeping
    /// its init-snapshot value would reveal a new object ahead of the shared cursor.
    #[test]
    fn a_new_stream_waits_for_its_first_visible_sample() {
        use crate::track::sim::DriveSignal;

        let mut app = interpolation_app();
        let value = DriveSignal {
            throttle: 0.5,
            steer: 0.25,
        };
        let entity = app
            .world_mut()
            .spawn((Remote, Interpolated, value, history(value)))
            .id();
        cursor(&mut app, 90);
        app.update();
        assert!(app.world().get::<DriveSignal>(entity).is_none());
        cursor(&mut app, 100);
        app.update();
        assert_eq!(app.world().get::<DriveSignal>(entity), Some(&value));
    }

    fn client_gameplay_app() -> App {
        let mut app = crate::headless_test::headless_shell();
        app.insert_resource(crate::terrain_grid::ForceFlatWorld)
            .insert_resource(crate::ClientReplica)
            .insert_resource(crate::tank::PendingTankAssets {
                spec: Handle::default(),
                scene: None,
                effects: Vec::new(),
            })
            .add_plugins((
                ClientPlugins {
                    tick_duration: TICK,
                },
                super::super::physics::physics_plugins(),
                super::super::plugin,
                crate::SimPlugin,
                super::super::fire_presentation::plugin,
            ));
        super::super::frontier::install(&mut app);
        super::super::client::install_replica_consumers(&mut app);
        super::super::client::install_input_buffer_guard(&mut app);
        super::super::client::install_input_send_cadence(&mut app);
        finish(&mut app);
        crate::headless_test::start_fixed_clock(&mut app);
        app.world_mut()
            .resource_mut::<NextState<crate::state::AppState>>()
            .set(crate::state::AppState::Playing);
        app.update();
        app
    }

    /// The game's netcode and damage model: replica tanks run the entire client gameplay frame,
    /// including shooting, collider preparation and the public-path presentation schedule. A
    /// rewind and recovery cannot break spawn-complete pose queries or their child colliders.
    /// The shipped vehicle is fixture data; the invariant applies to every assembled tank.
    #[test]
    fn assembled_replicas_survive_a_resync_across_the_client_gameplay_schedule() {
        use crate::tank::{Muzzle, Tank};
        use avian3d::prelude::{Collider, ColliderOf};

        let _lease = crate::headless_test::BOOT_LEASE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = client_gameplay_app();
        let roots: Vec<_> = [Vec3::new(0.0, 2.0, 0.0), Vec3::new(20.0, 2.0, 0.0)]
            .into_iter()
            .map(|at| {
                app.world_mut()
                    .spawn((
                        Remote,
                        Interpolated,
                        super::super::protocol::NetTank,
                        crate::track::sim::EngineRpm(0),
                        Position(at),
                        Rotation::default(),
                        history(Position(at)),
                        history(Rotation::default()),
                        crate::track::sim::TrackDrive::default(),
                        history(crate::track::sim::TrackDrive::default()),
                    ))
                    .id()
            })
            .collect();
        cursor(&mut app, 90);
        app.update();
        for &root in &roots {
            assert!(
                app.world().get::<Tank>(root).is_none(),
                "a replica must not assemble before its first pose becomes visible"
            );
        }
        for _ in 0..3 {
            cursor(&mut app, 100);
            app.update();
        }
        for &root in &roots {
            assert!(
                app.world().get::<Tank>(root).is_some(),
                "production rig attachment ran"
            );
        }
        let world = app.world_mut();
        assert!(
            world
                .query_filtered::<Entity, With<Muzzle>>()
                .iter(world)
                .count()
                >= roots.len()
        );
        assert!(
            world
                .query_filtered::<Entity, With<Collider>>()
                .iter(world)
                .count()
                > roots.len()
        );
        let clock = world.resource::<crate::WeaponClock>().0.raw();
        for tick in [90, 91, 92, 100, 101] {
            cursor(&mut app, tick);
            app.update();
            let world = app.world_mut();
            for &root in &roots {
                assert!(world.get::<Position>(root).is_some());
                assert!(world.get::<Rotation>(root).is_some());
            }
            for owner in world.query::<&ColliderOf>().iter(world) {
                assert!(
                    world.get::<Position>(owner.body).is_some(),
                    "every child collider's body retains a pose through resync"
                );
            }
        }
        assert!(
            app.world().resource::<crate::WeaponClock>().0.raw() > clock,
            "the fixed gameplay loop, including shooting, must actually run"
        );
    }
}
