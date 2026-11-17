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

    use crate::net::protocol::ServoAngles;
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
                        ServoAngles::default(),
                        history(ServoAngles::default()),
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

    /// The game's netcode: a visible replica's body waits for its public crank speed, so the engine
    /// the body brings starts at the authority's crank, never a reconstructed one. The
    /// snapshot-preserving attachment itself is `tank::spawn`'s
    /// `a_replica_attaches_around_its_join_in_progress_snapshot`.
    #[test]
    fn a_replica_body_waits_for_its_crank() {
        use crate::tank::Rig;
        use crate::track::sim::EngineRpm;

        let _lease = crate::headless_test::BOOT_LEASE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = client_gameplay_app();
        let at = Position(Vec3::new(0.0, 2.0, 0.0));
        let root = app
            .world_mut()
            .spawn((
                Remote,
                Interpolated,
                crate::net::protocol::NetTank,
                at,
                history(at),
                Rotation::default(),
                history(Rotation::default()),
                ServoAngles::default(),
                history(ServoAngles::default()),
                crate::track::sim::TrackDrive::default(),
                history(crate::track::sim::TrackDrive::default()),
            ))
            .id();
        for tick in 89..=100 {
            complete_tick(&mut app, tick);
            app.update();
        }
        for _ in 0..3 {
            app.update();
        }
        assert!(
            app.world().get::<Position>(root).is_some() && app.world().get::<Rig>(root).is_none(),
            "a visible replica without its crank speed stays unattached"
        );
        // Fixture crank.
        let crank = EngineRpm(1234);
        app.world_mut().entity_mut(root).insert(crank);
        app.update();
        assert!(
            app.world().get::<Rig>(root).is_some(),
            "the crank's arrival attaches the body"
        );
        assert_eq!(app.world().get::<EngineRpm>(root), Some(&crank));
    }

    /// The game's netcode: a replica's turret state is sampled on its hull's cursor. A remote
    /// tank turning while its turret holds a world bearing keeps that bearing in every frame's
    /// presented state; a turret applied on arrival, or on any clock but the hull's, leads or
    /// trails the hull by the presentation delay and swings the bearing. The drawn barrel is
    /// [`a_replica_barrel_is_drawn_where_the_stream_puts_it`]'s.
    #[test]
    fn a_replica_turret_is_sampled_on_its_hull_cursor() {
        use crate::tank::{RemoteServos, Rig, ServoIndex};

        // Fixture motion: the hull yaws RATE per tick, the turret counter-rotates as fast.
        const RATE: f32 = 0.01;
        const BEARING: f32 = 0.4;
        const FIRST: u32 = 100;
        const LAST: u32 = 130;
        let _lease = crate::headless_test::BOOT_LEASE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = client_gameplay_app();
        let turn = |tick: u32| RATE * (tick - FIRST) as f32;
        let mut hull = ConfirmedHistory::default();
        let mut aim = ConfirmedHistory::default();
        for tick in FIRST..=LAST {
            hull.insert_present_explicit(Tick(tick), Rotation(Quat::from_rotation_y(turn(tick))));
            aim.insert_present_explicit(
                Tick(tick),
                ServoAngles {
                    gunner_yaw: BEARING - turn(tick),
                    gunner_pitch: 0.0,
                },
            );
        }
        let at = Position(Vec3::new(0.0, 2.0, 0.0));
        let root = app
            .world_mut()
            .spawn((
                Remote,
                Interpolated,
                crate::net::protocol::NetTank,
                crate::track::sim::EngineRpm(0),
                at,
                history(at),
                Rotation::default(),
                hull,
                ServoAngles::default(),
                aim,
                crate::track::sim::TrackDrive::default(),
                history(crate::track::sim::TrackDrive::default()),
            ))
            .id();
        let mut turns = Vec::new();
        for tick in FIRST - 11..=LAST {
            complete_tick(&mut app, tick);
            app.update();
            let world = app.world_mut();
            let Some(rig) = world.get::<Rig>(root) else {
                continue; // Not attached until its first pose is visible.
            };
            let slot = world
                .get::<ServoIndex>(rig.gunner_yaw)
                .expect("an attached rig indexes its aim servo")
                .0;
            let yaw = world.get::<RemoteServos>(root).unwrap().0[slot].current();
            let (hull_yaw, ..) = world
                .get::<Rotation>(root)
                .unwrap()
                .0
                .to_euler(EulerRot::YXZ);
            assert!(
                (hull_yaw + yaw - BEARING).abs() < 1e-4,
                "the turret's world bearing left the stream's: hull {hull_yaw}, turret {yaw}"
            );
            turns.push(hull_yaw);
        }
        let swept = turns.last().unwrap() - turns.first().unwrap();
        assert!(
            swept > 10.0 * RATE,
            "the cursor must sweep the turn for the bearing to be tested: {swept} rad"
        );
    }

    /// Fixture stream law for [`a_replica_barrel_is_drawn_where_the_stream_puts_it`]: at rest
    /// until `first`, then hull yaw, turret yaw and gun pitch each turning at its own fixture rate
    /// per authority tick. Linear in ticks, so its value at a fractional cursor is the stream's.
    fn turning(first: u32, cursor: f64) -> (f32, f32, f32) {
        let ticks = (cursor - f64::from(first)).max(0.0) as f32;
        (0.01 * ticks, 0.02 * ticks, 0.0004 * ticks)
    }

    /// Push tick `tick` of [`turning`] into the replica's hull and aim streams and complete it.
    fn deliver(app: &mut App, root: Entity, first: u32, tick: u32) {
        let (hull, yaw, pitch) = turning(first, f64::from(tick));
        let mut entity = app.world_mut().entity_mut(root);
        entity
            .get_mut::<ConfirmedHistory<Rotation>>()
            .expect("the fixture replica carries its hull stream")
            .insert_present_explicit(Tick(tick), Rotation(Quat::from_rotation_y(hull)));
        entity
            .get_mut::<ConfirmedHistory<ServoAngles>>()
            .expect("the fixture replica carries its aim stream")
            .insert_present_explicit(
                Tick(tick),
                ServoAngles {
                    gunner_yaw: yaw,
                    gunner_pitch: pitch,
                },
            );
        complete_tick(app, tick);
    }

    /// The game's netcode: a remote barrel is drawn where the stream puts it at the frame's view
    /// cursor, through the shipped view of the tank (its glb bound by `bind_tank_view`, posed by
    /// the servo view, propagated). The expected bore composes the sim skeleton at the stream's
    /// angles for the cursor the frame drew, with the servo's own pose law, so it never reads
    /// presented state. Frames fall between ticks; the stream turns hull, turret and gun, and
    /// stalls past the view's buffer by half and then 1.5 of its continuity budget, read from the
    /// shipped timing, so the view holds and later cuts. A presenter ordered after the servo view
    /// draws the previous frame's aim; a view that never reads it draws a frozen turret.
    #[test]
    fn a_replica_barrel_is_drawn_where_the_stream_puts_it() {
        use crate::tank::{PendingTankAssets, Rig, ServoRest, ServoSpec, TankViewReady, ViewNode};
        use bevy::time::TimeUpdateStrategy;
        use std::time::{Duration, Instant};

        // Resolution: a render frame of 0.37 ticks puts frames at varied fractions of a tick.
        const FRAME: f64 = 0.37;
        // Resolution: f32 composition error is a few unit-vector ulps; one stale frame of the
        // fixture turret is 0.0074 rad (DERIVED: 0.02 rad per tick over 0.37 ticks).
        const TOLERANCE: f32 = 1e-5;
        let _lease = crate::headless_test::BOOT_LEASE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut app = client_gameplay_app();
        let scene = app
            .world()
            .resource::<AssetServer>()
            .load(GltfAssetLabel::Scene(0).from_asset(crate::tank::TANK_GLB_PATH));
        app.world_mut().resource_mut::<PendingTankAssets>().scene = Some(scene);
        app.insert_resource(TimeUpdateStrategy::ManualDuration(TICK.mul_f64(FRAME)));
        let at = Position(Vec3::new(0.0, 2.0, 0.0));
        let start = 100;
        let root = app
            .world_mut()
            .spawn((
                Remote,
                Interpolated,
                crate::net::protocol::NetTank,
                crate::track::sim::EngineRpm(0),
                at,
                history(at),
                Rotation::default(),
                ConfirmedHistory::<Rotation>::default(),
                ServoAngles::default(),
                ConfirmedHistory::<ServoAngles>::default(),
                crate::track::sim::TrackDrive::default(),
                history(crate::track::sim::TrackDrive::default()),
            ))
            .id();
        // The fixture server runs a tick ahead of the frames' clock: tick `start + n` completes
        // once `n - 1` ticks of frames have run. The stream is at rest until `first`.
        let mut first = u32::MAX;
        let mut next = start;
        let mut frames = 0u32;
        let mut frame = |app: &mut App, first: u32, streaming: bool, next: &mut u32| {
            while streaming && f64::from(*next - start) <= f64::from(frames) * FRAME + 1.0 {
                deliver(app, root, first, *next);
                *next += 1;
            }
            frames += 1;
            app.update();
        };
        // Real-time asset IO: the view glb loads and binds while the stream runs at rest.
        let loading = Instant::now();
        while app.world().get::<TankViewReady>(root).is_none() {
            assert!(
                loading.elapsed() < crate::headless_test::BOOT_DEADLINE,
                "the replica's view never bound"
            );
            frame(&mut app, first, true, &mut next);
            // Resolution: yield to the asset IO threads, as the headless boot does.
            std::thread::sleep(Duration::from_millis(1));
        }
        first = next;
        let (gunner_yaw, gun, muzzle) = app
            .world()
            .get::<Rig>(root)
            .map(|rig| (rig.gunner_yaw, rig.gun, rig.muzzle))
            .expect("a bound view belongs to an attached rig");
        for node in [gunner_yaw, gun, muzzle] {
            assert!(
                app.world().get::<ViewNode>(node).is_some(),
                "the shipped view binds every node of the bore's chain"
            );
        }
        let expected = |world: &World, cursor: f64| {
            let (hull, yaw, pitch) = turning(first, cursor);
            let mut chain = Vec::new();
            let mut node = muzzle;
            while node != root {
                chain.push(node);
                node = world.get::<ChildOf>(node).unwrap().parent();
            }
            let mut rotation = Quat::from_rotation_y(hull);
            for &link in chain.iter().rev() {
                rotation *= match [(gunner_yaw, yaw), (gun, pitch)]
                    .into_iter()
                    .find(|&(servo, _)| servo == link)
                {
                    Some((_, angle)) => crate::tank::servo_pose(
                        world.get::<ServoSpec>(link).unwrap().role(),
                        world.get::<ServoRest>(link).unwrap().0,
                        angle,
                    ),
                    None => world.get::<Transform>(link).unwrap().rotation,
                };
            }
            rotation * Vec3::NEG_Z
        };
        // A stall first drains the stream buffered ahead of the cursor, then holds while the backlog
        // grows. On resumption the view cuts a backlog longer than its continuity budget
        // (`view_cut_s`, floored by the admission cap), and otherwise catches up at most
        // `view_rate` faster than real time (`net::view_clock`).
        let (cut_ticks, catch_up) = {
            let timing = app.world().resource::<crate::net::timing::Timing>();
            (
                timing.view_cut_s.max(timing.admission_s) / TICK.as_secs_f64(),
                timing.view_rate,
            )
        };
        let cursor_of =
            |now: TickInstant| f64::from(now.tick().0) + f64::from(now.overstep().to_f32());
        let buffered = |world: &World| {
            let clock = world.resource::<crate::net::view_clock::ViewClock>();
            f64::from(clock.newest().expect("a streaming replica has stamps").0)
                - cursor_of(clock.now().expect("a streaming replica has a view cursor"))
        };
        let (mut fractions, mut holds, mut cuts) = (0, 0, 0);
        let mut last: Option<f64> = None;
        let mut check = |world: &World| {
            let clock = world.resource::<crate::net::view_clock::ViewClock>();
            let now = clock.now().expect("a streaming replica has a view cursor");
            let cursor = cursor_of(now);
            fractions += u32::from(now.overstep().to_f32() > 0.0);
            holds += u32::from(clock.held);
            cuts += u32::from(last.is_some_and(|last| cursor - last > cut_ticks));
            last = Some(cursor);
            let view = world.get::<ViewNode>(muzzle).unwrap().0;
            let drawn = world.get::<GlobalTransform>(view).unwrap().rotation() * Vec3::NEG_Z;
            let want = expected(world, cursor);
            // The chord between unit bores is their angle to first order; f32 `acos` near 1
            // resolves no better than 3e-4 rad.
            let off = (drawn - want).length();
            assert!(
                off < TOLERANCE,
                "at cursor {cursor:.3} the barrel is drawn {off:.6} rad off the stream's"
            );
        };
        let mut play = |app: &mut App, ticks: f64, streaming: bool| {
            for _ in 0..(ticks / FRAME).ceil() as u32 {
                frame(app, first, streaming, &mut next);
                check(app.world());
            }
        };
        play(&mut app, cut_ticks, true);
        // Fixture: half a budget past the buffer, a hold caught up without a cut.
        let hold = buffered(app.world()) + 0.5 * cut_ticks;
        play(&mut app, hold, false);
        // The view's delay grows by up to the stall it saw and slews at `view_rate`, as the
        // cursor's catch-up does; a budget's turning follows.
        play(&mut app, hold / catch_up + cut_ticks, true);
        // Fixture: 1.5 budgets past the buffer, a cut.
        let stall = buffered(app.world()) + 1.5 * cut_ticks;
        play(&mut app, stall, false);
        play(&mut app, cut_ticks, true);
        assert!(
            fractions > 0 && holds > 0 && cuts == 1,
            "the fixture must draw between ticks ({fractions}), through a hold ({holds}) and \
             exactly the long stall's cut ({cuts})"
        );
    }
}
