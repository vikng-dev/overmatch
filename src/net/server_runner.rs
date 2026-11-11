//! Tick-paced dedicated-server runner; the client keeps its render-driven runner.

use core::time::Duration;

use bevy::app::{FixedMain, PluginsState, RunFixedMainLoop, RunFixedMainLoopSystems};
use bevy::ecs::schedule::ScheduleCleanupPolicy;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::time::{TimeUpdateStrategy, run_fixed_main_schedule};
use lightyear::prelude::server::ServerPlugins;

pub(super) fn plugin(app: &mut App) {
    app.add_plugins(TickRunner(WallClock));
}

/// The clock seam controls waiting and Bevy's time sample together.
trait Clock: Clone + Send + Sync + 'static {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

#[derive(Clone)]
struct WallClock;

impl Clock for WallClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Wall time elapsed since `Time<Fixed>` last admitted any. Its overstep is the admitted debt.
#[derive(Resource, Default)]
struct UnadmittedWall(Duration);

/// Wall time past the admission cap that a batch dropped, summed since startup, and the batches
/// that dropped any. The stats line reads it; nothing in the runner does.
#[derive(Resource, Default)]
struct DiscardedWall {
    total: Duration,
    batches: u64,
}

/// The runner's wall-time accounts, for `net::server_stats`. INVARIANT: the `Time<Real>` deltas
/// since the runner primed sum to `Time<Fixed>::elapsed` plus [`Self::drift`], because each delta
/// enters [`UnadmittedWall`] and leaves it only as admitted overstep or discard.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct WallLedger<'w> {
    discarded: Res<'w, DiscardedWall>,
    unadmitted: Res<'w, UnadmittedWall>,
    fixed: Res<'w, Time<Fixed>>,
}

impl WallLedger<'_> {
    /// Wall time dropped since startup, and the batches that dropped it.
    pub(super) fn discarded(&self) -> (Duration, u64) {
        (self.discarded.total, self.discarded.batches)
    }

    /// How far simulated time trails wall time: dropped for good, plus the debt still owed.
    pub(super) fn drift(&self) -> Duration {
        self.discarded.total + self.fixed.overstep() + self.unadmitted.0
    }
}

/// Bevy's `run_fixed_main_schedule`, spending one admitted tick per frame so each is sent. A new
/// batch is admitted only once the last is spent, under the same cap Bevy's virtual clock applies
/// to one frame. `Time<Real>` stays wall time, because networking counts its deltas.
fn run_one_admitted_tick(world: &mut World) {
    let elapsed = world.resource::<Time<Real>>().delta();
    let cap = world.resource::<Time<Virtual>>().max_delta();
    let mut unadmitted = world.resource_mut::<UnadmittedWall>();
    unadmitted.0 += elapsed;
    let pending = unadmitted.0;
    let mut fixed = world.resource_mut::<Time<Fixed>>();
    let timestep = fixed.timestep();
    let admits = fixed.overstep() < timestep;
    if admits {
        fixed.accumulate_overstep(pending.min(cap));
    }
    assert!(
        fixed.overstep() >= timestep,
        "the runner starts a frame only when a tick is due"
    );
    fixed.discard_overstep(timestep);
    fixed.advance_by(timestep);
    if admits {
        world.resource_mut::<UnadmittedWall>().0 = Duration::ZERO;
        let dropped = pending.saturating_sub(cap);
        if !dropped.is_zero() {
            let mut discarded = world.resource_mut::<DiscardedWall>();
            discarded.total += dropped;
            discarded.batches += 1;
        }
    }

    *world.resource_mut::<Time>() = world.resource::<Time<Fixed>>().as_generic();
    world.run_schedule(FixedMain);
    *world.resource_mut::<Time>() = world.resource::<Time<Virtual>>().as_generic();
}

struct TickRunner<C>(C);

impl<C: Clock> Plugin for TickRunner<C> {
    fn build(&self, app: &mut App) {
        app.add_plugins(ServerPlugins {
            tick_duration: super::server::TICK,
        })
        .init_resource::<UnadmittedWall>()
        .init_resource::<DiscardedWall>();
        let clock = self.0.clone();
        app.set_runner(move |mut app| {
            if app.plugins_state() != PluginsState::Cleaned {
                while app.plugins_state() == PluginsState::Adding {
                    bevy::tasks::tick_global_task_pools_on_main_thread();
                }
                app.finish();
                app.cleanup();
            }

            app.remove_systems_in_set(
                RunFixedMainLoop,
                run_fixed_main_schedule,
                ScheduleCleanupPolicy::RemoveSystemsOnly,
            )
            .expect("TimePlugin registers Bevy's fixed loop");
            app.add_systems(
                RunFixedMainLoop,
                run_one_admitted_tick.in_set(RunFixedMainLoopSystems::FixedMainLoop),
            );

            // Real's first sample has zero delta. Prime it without running a tickless frame.
            app.world_mut()
                .resource_mut::<Time<Real>>()
                .update_with_instant(clock.now());
            loop {
                let world = app.world();
                let virtual_time = world.resource::<Time<Virtual>>();
                assert!(
                    !virtual_time.is_paused() && virtual_time.relative_speed_f64() == 1.0,
                    "the dedicated server clock is unpaused and runs at wall speed"
                );
                let fixed = world.resource::<Time<Fixed>>();
                // Invariant: the deadline spends exactly the time still missing from a tick,
                // counting admitted debt and wall time awaiting admission.
                let missing = fixed
                    .timestep()
                    .saturating_sub(fixed.overstep() + world.resource::<UnadmittedWall>().0);
                let due = world
                    .resource::<Time<Real>>()
                    .last_update()
                    .expect("the runner primes real time before its first frame")
                    + missing;
                let mut now = clock.now();
                while now < due {
                    clock.sleep(due - now);
                    now = clock.now();
                }
                app.insert_resource(TimeUpdateStrategy::ManualInstant(now));
                app.update();
                if let Some(exit) = app.should_exit() {
                    return exit;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone)]
    struct TestClock {
        now: Arc<Mutex<Instant>>,
        sleeps: Arc<Mutex<Vec<Duration>>>,
        oversleep: Duration,
    }

    impl Clock for TestClock {
        fn now(&self) -> Instant {
            *self.now.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) {
            self.sleeps.lock().unwrap().push(duration);
            *self.now.lock().unwrap() += duration + self.oversleep;
        }
    }

    /// Real waits, so a real socket peer keeps its pace, with injectable stalls of wall time.
    #[derive(Clone)]
    struct StallClock {
        offset: Arc<Mutex<Duration>>,
    }

    impl Clock for StallClock {
        fn now(&self) -> Instant {
            Instant::now() + *self.offset.lock().unwrap()
        }

        fn sleep(&self, duration: Duration) {
            std::thread::sleep(duration);
        }
    }

    #[derive(Default, Debug)]
    struct Frame {
        ticks: usize,
        elapsed: Duration,
        fixed_elapsed: Duration,
        overstep: Duration,
        drift: Duration,
        discarded: (Duration, u64),
        stages: Vec<&'static str>,
    }

    #[derive(Resource, Clone, Default)]
    struct Frames(Arc<Mutex<Vec<Frame>>>);

    fn probe(clock: TestClock, work: Vec<Duration>) -> (App, Frames) {
        let mut app = App::new();
        let frames = Frames::default();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(TickRunner(clock.clone()))
            .insert_resource(frames.clone())
            .add_systems(PreUpdate, |frames: Res<Frames>, real: Res<Time<Real>>| {
                frames.0.lock().unwrap().push(Frame {
                    elapsed: real.elapsed(),
                    stages: vec!["receive"],
                    ..default()
                });
            })
            .add_systems(FixedFirst, |frames: Res<Frames>| {
                let mut frames = frames.0.lock().unwrap();
                let frame = frames.last_mut().unwrap();
                frame.ticks += 1;
                frame.stages.push("tick");
            })
            .add_systems(PostUpdate, |frames: Res<Frames>| {
                frames
                    .0
                    .lock()
                    .unwrap()
                    .last_mut()
                    .unwrap()
                    .stages
                    .push("send");
            })
            .add_systems(
                Last,
                move |frames: Res<Frames>,
                      fixed: Res<Time<Fixed>>,
                      ledger: WallLedger,
                      mut exit: MessageWriter<bevy::app::AppExit>| {
                    let mut frames = frames.0.lock().unwrap();
                    let count = frames.len();
                    let frame = frames.last_mut().unwrap();
                    frame.stages.push("last");
                    frame.fixed_elapsed = fixed.elapsed();
                    frame.overstep = fixed.overstep();
                    frame.drift = ledger.drift();
                    frame.discarded = ledger.discarded();
                    *clock.now.lock().unwrap() += work[count - 1];
                    if count == work.len() {
                        exit.write(bevy::app::AppExit::Success);
                    }
                },
            );
        (app, frames)
    }

    /// Replication needs one checkpoint per normal frame. An independent frame period admits
    /// tickless frames; sleeping a whole tick after work drifts into double-tick frames.
    #[test]
    fn each_normal_frame_receives_ticks_once_and_sends_only_when_due() {
        let tick = super::super::server::TICK;
        // Fixture: varying sub-budget work and a small scheduler oversleep across many ticks.
        let clock = TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: default(),
            oversleep: tick / 16,
        };
        let (mut app, frames) = probe(clock, (0..128).map(|i| tick * (i % 3) / 8).collect());
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        let frames = frames.0.lock().unwrap();
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.ticks, 1, "frame {index}: {frame:?}");
            assert_eq!(frame.stages, ["receive", "tick", "send", "last"]);
            assert_eq!(frame.fixed_elapsed, tick * (index as u32 + 1));
            assert_eq!(frame.elapsed, frame.fixed_elapsed + frame.overstep);
            assert!(frame.elapsed >= tick * (index as u32 + 1));
            assert!(frame.elapsed < tick * (index as u32 + 2));
        }
    }

    /// A batch's own cost is new wall time, admitted once the batch is spent; re-capping it while
    /// debt remains would drop ticks Bevy's loop had already admitted.
    #[test]
    fn a_catch_up_frames_cost_is_admitted_after_the_batch_not_lost() {
        let tick = super::super::server::TICK;
        let clock = TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: default(),
            oversleep: Duration::ZERO,
        };
        let admitted = Time::<Virtual>::default().max_delta();
        let batch = (admitted.as_nanos() / tick.as_nanos()) as u32;
        // Fixture: a stall of exactly the cap, then one catch-up frame costing two ticks.
        let later = 2;
        let mut work = vec![admitted, tick * later];
        let caught_up = 1 + (batch + later) as usize;
        work.resize(caught_up + 2, Duration::ZERO);
        let (mut app, frames) = probe(clock.clone(), work);
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        let frames = frames.0.lock().unwrap();
        assert!(frames.iter().all(|frame| frame.ticks == 1), "{frames:?}");
        // The first frame waits, the batch and the frame's own cost run unslept, the last two wait.
        assert_eq!(clock.sleeps.lock().unwrap().len(), 3);
    }

    /// Debt from wall time stays bounded under sustained overload: admitting while debt remains
    /// would grow it without limit and leave the world ever further behind.
    #[test]
    fn sustained_overload_keeps_the_debt_within_the_cap() {
        let tick = super::super::server::TICK;
        let clock = TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: default(),
            oversleep: Duration::ZERO,
        };
        let admitted = Time::<Virtual>::default().max_delta();
        let batch = (admitted.as_nanos() / tick.as_nanos()) as usize;
        // Fixture: every frame costs two ticks, for several batches.
        let (mut app, frames) = probe(clock, vec![tick * 2; batch * 4]);
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        let frames = frames.0.lock().unwrap();
        assert!(frames.iter().all(|frame| frame.ticks == 1), "{frames:?}");
        let deepest = frames.iter().map(|frame| frame.overstep).max().unwrap();
        assert!(deepest + tick <= admitted, "{deepest:?}");
        assert_wall_accounted(&frames);
    }

    /// The stats line's drift reading: every wall delta is simulated, owed or dropped, exactly
    /// once. Dropping the cap instead of the excess, or not counting the drop, breaks it.
    fn assert_wall_accounted(frames: &[Frame]) {
        for frame in frames {
            assert_eq!(
                frame.elapsed,
                frame.fixed_elapsed + frame.drift,
                "{frame:?}"
            );
        }
    }

    /// Netcode counts `Time<Real>` deltas: a stall must reach its timeout whole, not one tick's
    /// worth per catch-up frame, or a silent client stays connected seconds past its limit.
    #[test]
    fn a_silent_client_times_out_on_wall_time_across_a_capped_stall() {
        use std::net::{Ipv4Addr, SocketAddr};
        use std::sync::atomic::{AtomicBool, Ordering};

        use lightyear::connection::client_of::ClientOf;
        use lightyear::prelude::client::{ClientPlugins, Connect, NetcodeClient, NetcodeConfig};
        use lightyear::prelude::server::{
            NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerUdpIo, Start,
        };
        use lightyear::prelude::*;

        // Fixture: any protocol both ends share, a timeout shorter than the stall.
        const PROTOCOL: u64 = 0x5e4d;
        const TIMEOUT_SECS: i32 = 3;
        let stall = Duration::from_secs(4);
        let tick = super::super::server::TICK;
        let port = super::super::test_harness::free_port();
        let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
        let silent = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));

        let client = {
            let (silent, stopped) = (silent.clone(), stopped.clone());
            std::thread::spawn(move || {
                let mut app = App::new();
                app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
                    .add_plugins(ClientPlugins {
                        tick_duration: tick,
                    });
                app.finish();
                app.cleanup();
                let entity = app
                    .world_mut()
                    .spawn((
                        Client,
                        Link::default(),
                        LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
                        PeerAddr(server_addr),
                        NetcodeClient::new(
                            Authentication::Manual {
                                server_addr,
                                client_id: 1,
                                private_key: [0; 32],
                                protocol_id: PROTOCOL,
                            },
                            NetcodeConfig {
                                client_timeout_secs: TIMEOUT_SECS,
                                ..default()
                            },
                        )
                        .expect("a manual token builds"),
                        UdpIo::default(),
                    ))
                    .id();
                app.world_mut().commands().trigger(Connect { entity });
                while !silent.load(Ordering::SeqCst) {
                    app.update();
                    std::thread::sleep(Duration::from_millis(1));
                }
                stopped.store(true, Ordering::SeqCst);
            })
        };

        let clock = StallClock { offset: default() };
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .add_plugins(TickRunner(clock.clone()));
        let server = app
            .world_mut()
            .spawn((
                NetcodeServer::new(ServerNetcodeConfig {
                    protocol_id: PROTOCOL,
                    private_key: [0; 32],
                    ..default()
                }),
                LocalAddr(server_addr),
                ServerUdpIo::default(),
            ))
            .id();
        app.world_mut().commands().trigger(Start { entity: server });

        let connected_after = Arc::new(Mutex::new(None));
        let outcome = connected_after.clone();
        let started = std::time::Instant::now();
        let mut frames_after = None;
        app.add_systems(
            Last,
            move |links: Query<(), (With<ClientOf>, With<Connected>)>,
                  mut exit: MessageWriter<bevy::app::AppExit>| {
                match frames_after {
                    None if links.is_empty() => assert!(
                        started.elapsed() < Duration::from_secs(30),
                        "the loopback client never connected"
                    ),
                    None => {
                        silent.store(true, Ordering::SeqCst);
                        while !stopped.load(Ordering::SeqCst) {
                            std::thread::yield_now();
                        }
                        *clock.offset.lock().unwrap() += stall;
                        frames_after = Some(0);
                    }
                    // One frame reads the stall and flushes the disconnect; one more settles it.
                    Some(1) => {
                        *outcome.lock().unwrap() = Some(!links.is_empty());
                        exit.write(bevy::app::AppExit::Success);
                    }
                    Some(count) => frames_after = Some(count + 1),
                }
            },
        );
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        client.join().unwrap();
        assert_eq!(*connected_after.lock().unwrap(), Some(false));
    }

    /// Clients are sent one state per frame, so a catch-up frame holding several ticks starves
    /// their view. Every missed tick still runs, back to back, each between receive and send.
    #[test]
    fn a_missed_tick_runs_in_its_own_frame_between_receive_and_send() {
        let tick = super::super::server::TICK;
        let clock = TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: default(),
            oversleep: Duration::ZERO,
        };
        // Fixture: one frame overruns by five ticks, then frames cost nothing.
        let mut work = vec![tick * 5];
        work.resize(8, Duration::ZERO);
        let (mut app, frames) = probe(clock.clone(), work);
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        let frames = frames.0.lock().unwrap();
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.ticks, 1, "frame {index}: {frame:?}");
            assert_eq!(frame.stages, ["receive", "tick", "send", "last"]);
            assert_eq!(frame.fixed_elapsed, tick * (index as u32 + 1));
        }
        // The first frame waits for its tick; five ticks then run unslept; the last two wait.
        assert_eq!(clock.sleeps.lock().unwrap().len(), 3);
    }

    /// One-tick frames must not lengthen catch-up: a stall admits the same simulated time as
    /// Bevy's virtual cap, and everything past that bound is skipped, not owed.
    #[test]
    fn a_stall_past_the_cap_admits_exactly_the_cap_and_skips_the_rest() {
        let tick = super::super::server::TICK;
        let clock = TestClock {
            now: Arc::new(Mutex::new(Instant::now())),
            sleeps: default(),
            oversleep: Duration::ZERO,
        };
        let admitted = Time::<Virtual>::default().max_delta();
        let cap_ticks = (admitted.as_nanos() / tick.as_nanos()) as usize;
        // Fixture: a wall stall four times the admission bound, then costless frames.
        let mut work = vec![admitted * 4];
        work.resize(cap_ticks + 3, Duration::ZERO);
        let (mut app, frames) = probe(clock.clone(), work);
        assert_eq!(
            app.world().resource::<Time<Virtual>>().max_delta(),
            admitted
        );
        assert_eq!(app.run(), bevy::app::AppExit::Success);
        let frames = frames.0.lock().unwrap();
        assert!(frames.iter().all(|frame| frame.ticks == 1), "{frames:?}");
        // The stalled frame's tick, then the cap's worth unslept; the next tick waits its turn.
        assert_eq!(
            frames[cap_ticks].fixed_elapsed,
            tick * (cap_ticks as u32 + 1)
        );
        assert_eq!(clock.sleeps.lock().unwrap().len(), 3);
        // Wall time past the bound never reached the simulation.
        let settled = &frames[cap_ticks];
        assert_eq!(settled.elapsed - settled.fixed_elapsed, admitted * 3);
        let last = frames.last().unwrap();
        assert_eq!(last.discarded, (admitted * 3, 1), "one batch dropped it");
        assert_wall_accounted(&frames);
    }
}
