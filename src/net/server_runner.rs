//! Tick-paced dedicated-server runner; the client keeps its render-driven runner.

use core::time::Duration;

use bevy::app::PluginsState;
use bevy::platform::time::Instant;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
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

struct TickRunner<C>(C);

impl<C: Clock> Plugin for TickRunner<C> {
    fn build(&self, app: &mut App) {
        app.add_plugins(ServerPlugins {
            tick_duration: super::server::TICK,
        });
        let clock = self.0.clone();
        app.set_runner(move |mut app| {
            if app.plugins_state() != PluginsState::Cleaned {
                while app.plugins_state() == PluginsState::Adding {
                    bevy::tasks::tick_global_task_pools_on_main_thread();
                }
                app.finish();
                app.cleanup();
            }

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
                let backlog_bound = virtual_time.max_delta();
                let fixed = world.resource::<Time<Fixed>>();
                let timestep = fixed.timestep();
                let last = world
                    .resource::<Time<Real>>()
                    .last_update()
                    .expect("the runner primes real time before its first frame");
                // Invariant: the deadline spends exactly the accumulator's missing time.
                let mut due = last
                    + timestep
                        .checked_sub(fixed.overstep())
                        .expect("Bevy's fixed loop exhausts all complete timesteps");
                let mut now = clock.now();
                while now < due {
                    clock.sleep(due - now);
                    now = clock.now();
                }
                // Backlog past Bevy's virtual cap is never simulated. One-tick samples cannot
                // reach that cap, so the skip happens here against the same bound.
                let skipped = (now - last).saturating_sub(backlog_bound);
                if !skipped.is_zero() {
                    app.world_mut()
                        .resource_mut::<Time<Real>>()
                        .update_with_instant(last + skipped);
                    due += skipped;
                }
                // Each frame runs one tick between receive and send, overdue or not: a sample
                // less than a timestep past due admits exactly one, and a later wake samples the
                // deadline itself, leaving the backlog to the next frames.
                let sample = if now - due < timestep { now } else { due };
                app.insert_resource(TimeUpdateStrategy::ManualInstant(sample));
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

    #[derive(Default, Debug)]
    struct Frame {
        ticks: usize,
        elapsed: Duration,
        fixed_elapsed: Duration,
        overstep: Duration,
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
                      mut exit: MessageWriter<bevy::app::AppExit>| {
                    let mut frames = frames.0.lock().unwrap();
                    let count = frames.len();
                    let frame = frames.last_mut().unwrap();
                    frame.stages.push("last");
                    frame.fixed_elapsed = fixed.elapsed();
                    frame.overstep = fixed.overstep();
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
    }
}
