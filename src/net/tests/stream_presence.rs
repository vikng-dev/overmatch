//! Surviving stream-presence and assembled-replica contracts; game damage model is game state only.

#[cfg(test)]
mod tests {
    use avian3d::prelude::{Position, Rotation};
    use bevy::prelude::*;
    use lightyear::core::confirmed_history::ConfirmedHistory;
    use lightyear::core::time::TickInstant;
    use lightyear::interpolation::timeline::InterpolationTimeline;
    use lightyear::prelude::client::{ClientPlugins, Remote};
    use lightyear::prelude::{Interpolated, NetworkTimeline, Tick};

    use crate::net::test_harness::{TICK, finish, net_physics_app};

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
        crate::net::protocol::plugin(&mut app);
        finish(&mut app);
        app
    }

    /// Every stream consumer relies on the last sample holding until an explicit removal;
    /// despawn must win, and a future init snapshot stays hidden until its first crossing.
    #[test]
    fn continuous_streams_hold_until_explicit_removal_or_despawn() {
        use crate::net::debris_stress::DebrisPose;
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
        cursor(&mut app, 101);
        app.update();
        assert_eq!(app.world().get::<TrackDrive>(entity), Some(&drive));
        assert_eq!(app.world().get::<DriveSignal>(entity), Some(&signal));
        assert_eq!(app.world().get::<DebrisPose>(entity), Some(&debris));
        app.world_mut()
            .get_mut::<ConfirmedHistory<DriveSignal>>(entity)
            .unwrap()
            .insert_removed(Tick(102));
        cursor(&mut app, 102);
        app.update();
        assert!(
            app.world().get::<DriveSignal>(entity).is_none(),
            "an explicit server removal must not be held"
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

    #[derive(Resource, Default)]
    struct FixedFrames(u64);

    // Fixture: completed authority ticks on a clean link; the production receiver and driver
    // still choose the cursor. The isolated interpolation tests above own their sampler input.
    fn complete_tick(app: &mut App, tick: u32) {
        use bevy_replicon::shared::replication::storage::ReplicationStorage;
        use bevy_replicon::{
            client::server_mutate_ticks::MutateTickReceived, prelude::RepliconTick,
        };
        use lightyear::prelude::ReplicationCheckpointMap;
        let checkpoint = RepliconTick::new(tick);
        app.world_mut()
            .resource_mut::<ReplicationCheckpointMap>()
            .record(checkpoint, Tick(tick));
        app.world_mut()
            .resource_mut::<ReplicationStorage>()
            .global
            .get_or_default::<crate::net::tick_stamp::ReceivedStamps>()
            .queue
            .push_back((
                checkpoint,
                crate::net::tick_stamp::TickStamp::new(f64::from(tick) * TICK.as_secs_f64(), 0.0)
                    .unwrap(),
            ));
        app.world_mut()
            .write_message(MutateTickReceived { tick: checkpoint });
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
                crate::net::physics::physics_plugins(),
                crate::net::plugin,
                crate::SimPlugin,
            ));

        crate::net::view_clock::install(&mut app);
        crate::net::client::install_replica_consumers(&mut app);
        app.init_resource::<FixedFrames>()
            .add_systems(FixedUpdate, |mut frames: ResMut<FixedFrames>| frames.0 += 1);
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
    /// hold and recovery cannot break spawn-complete pose queries or their child colliders.
    /// The shipped vehicle is fixture data; the invariant applies to every assembled tank.
    #[test]
    fn assembled_replicas_keep_complete_poses_while_the_cursor_holds() {
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
                        crate::net::protocol::NetTank,
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
        complete_tick(&mut app, 89);
        complete_tick(&mut app, 90);
        app.update();
        for &root in &roots {
            assert!(
                app.world().get::<Tank>(root).is_none(),
                "a replica must not assemble before its first pose becomes visible"
            );
        }
        for tick in 91..=100 {
            complete_tick(&mut app, tick);
            app.update();
        }
        for _ in 0..3 {
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
        let fixed_before = world.resource::<FixedFrames>().0;
        for tick in [100, 100, 100, 101, 102] {
            if tick > 100 {
                complete_tick(&mut app, tick);
            }
            app.update();
            let world = app.world_mut();
            if tick == 100 {
                assert_eq!(
                    world.resource::<crate::WeaponClock>().0.raw(),
                    clock,
                    "view-owned countdowns cannot mature during a presentation hold"
                );
            }
            for &root in &roots {
                assert!(world.get::<Position>(root).is_some());
                assert!(world.get::<Rotation>(root).is_some());
            }
            for owner in world.query::<&ColliderOf>().iter(world) {
                assert!(
                    world.get::<Position>(owner.body).is_some(),
                    "every child collider's body retains a pose through a hold"
                );
            }
        }
        assert!(
            app.world().resource::<FixedFrames>().0 > fixed_before,
            "the independent fixed-gameplay probe must run even while view time holds"
        );
    }
}
