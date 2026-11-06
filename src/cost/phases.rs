//! Complete fixed-tick intervals, including deferred commands and their synchronous hooks.
//! Existing leaf brackets are debited at each boundary; the remaining interval includes engine
//! systems, ECS application and executor dispatch. See `.agents/scratch/server-tick-attribution.md`.

use std::time::Instant;

use avian3d::prelude::PhysicsSystems;
use bevy::app::FixedMainScheduleOrder;
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::*;

use super::stages::{NamedTimes, Stages};
use crate::construction::breakage::{LawSet, SplitSet, SweepSet};
use crate::state::SimPhase;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, ScheduleLabel)]
enum Phase {
    FixedFirst,
    FixedPre,
    FixedUpdate,
    SweepEcs,
    PhysicsPrepare,
    PhysicsStep,
    PostPhysics,
    ConstructionEcs,
    PublicationEcs,
    FixedLast,
}

pub(crate) const NAMES: [&str; 10] = [
    "fixed_first_us",
    "fixed_pre_us",
    "fixed_update_us",
    "sweep_ecs_us",
    "physics_prepare_us",
    "physics_step_us",
    "post_physics_us",
    "construction_ecs_us",
    "publication_ecs_us",
    "fixed_last_us",
];
pub(crate) const COUNT: usize = NAMES.len();

#[derive(Resource, Default)]
pub(crate) struct Phases {
    active: Option<(Phase, Instant, u64)>,
    us: [f64; COUNT],
}

impl Phases {
    pub(super) fn open(&mut self, now: Instant, stages: &Stages) {
        assert!(self.active.is_none(), "the previous tick closes its phase");
        self.us.fill(0.0);
        self.active = Some((Phase::FixedFirst, now, stages.total_ns()));
    }

    pub(super) fn close(&mut self, now: Instant, stages: &Stages) {
        self.finish(now, stages.total_ns());
    }

    fn finish(&mut self, now: Instant, leaf_end: u64) {
        let (phase, start, leaf_start) =
            self.active.take().expect("the tick opens its first phase");
        let elapsed = u64::try_from(now.duration_since(start).as_nanos())
            .expect("a tick phase fits in the u64 nanosecond span");
        let leaves = leaf_end - leaf_start;
        let remaining = elapsed
            .checked_sub(leaves)
            .expect("leaf brackets finish inside their phase and do not overlap each other");
        self.us[phase as usize] += remaining as f64 / 1000.0;
    }

    fn advance(&mut self, next: Phase, stages: &Stages) {
        let now = Instant::now();
        let leaves = stages.total_ns();
        self.finish(now, leaves);
        self.active = Some((next, now, leaves));
    }

    pub(crate) fn times(&self) -> NamedTimes<COUNT> {
        NamedTimes::new(&NAMES, self.us)
    }
}

fn boundary(next: Phase) -> impl FnMut(ResMut<Phases>, Res<Stages>) {
    move |mut phases, stages| phases.advance(next, &stages)
}

fn before_schedule(app: &mut App, before: impl ScheduleLabel, phase: Phase) {
    app.add_systems(phase, boundary(phase));
    app.edit_schedule(phase, |s| {
        s.set_executor(SingleThreadedExecutor::new());
    });
    app.world_mut()
        .resource_mut::<FixedMainScheduleOrder>()
        .insert_before(before, phase);
}

/// Only timer schedules use a single-thread executor: fixed children already run sequentially.
/// The simulation's executors, owner parallelism and existing visibility boundaries remain its own.
pub(crate) fn install(app: &mut App) {
    app.init_resource::<Stages>().init_resource::<Phases>();
    before_schedule(app, FixedPreUpdate, Phase::FixedPre);
    before_schedule(app, FixedUpdate, Phase::FixedUpdate);
    before_schedule(app, FixedPostUpdate, Phase::SweepEcs);
    before_schedule(app, FixedLast, Phase::FixedLast);
    // These are the simulation's existing ordered producer/consumer barriers. `after` includes
    // the producer's deferred buffers, so its hooks stay on the producer's side of the clock.
    app.add_systems(
        FixedPostUpdate,
        (
            boundary(Phase::PhysicsPrepare)
                .after(SweepSet)
                .before(PhysicsSystems::Prepare),
            boundary(Phase::PhysicsStep)
                .after(PhysicsSystems::Prepare)
                .before(PhysicsSystems::StepSimulation),
            boundary(Phase::PostPhysics)
                .after(PhysicsSystems::StepSimulation)
                .before(PhysicsSystems::Writeback),
            boundary(Phase::ConstructionEcs)
                .after(SimPhase::RecordedConsequences)
                .before(LawSet),
            boundary(Phase::PublicationEcs)
                .after(SplitSet)
                .before(SimPhase::Publish),
        )
            .chain_ignore_deferred(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{TickClock, arm_tick_clock, stages::Stage};
    use bevy::app::FixedMain;
    use bevy::ecs::{lifecycle::HookContext, world::DeferredWorld};

    #[derive(Resource, Default)]
    struct Witness(Vec<(Phase, Phase)>);

    #[derive(Component)]
    #[component(on_add = witness_hook)]
    struct Hooked(Phase);

    fn witness_hook(mut world: DeferredWorld, context: HookContext) {
        let expected = world.get::<Hooked>(context.entity).unwrap().0;
        let actual = world.resource::<Phases>().active.unwrap().0;
        world.resource_mut::<Witness>().0.push((expected, actual));
    }

    #[derive(Event)]
    struct Probe(Phase);

    fn fixture_app(instrumented: bool) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        for label in [
            FixedFirst.intern(),
            FixedPreUpdate.intern(),
            FixedUpdate.intern(),
            FixedPostUpdate.intern(),
            FixedLast.intern(),
        ] {
            app.init_schedule(label);
        }
        // Fixture: no game systems; only the ordered phase interfaces the timer consumes.
        app.configure_sets(
            FixedPostUpdate,
            (
                SweepSet,
                PhysicsSystems::Prepare,
                PhysicsSystems::StepSimulation,
                PhysicsSystems::Writeback,
                SimPhase::RecordedConsequences,
                LawSet,
                SplitSet,
                SimPhase::Publish,
            )
                .chain(),
        );
        arm_tick_clock(&mut app);
        if instrumented {
            install(&mut app);
        }
        app
    }

    fn commands_in(phase: Phase) -> impl FnMut(Commands) {
        move |mut commands| {
            commands.queue(move |world: &mut World| {
                let actual = world.resource::<Phases>().active.unwrap().0;
                world.resource_mut::<Witness>().0.push((phase, actual));
            });
            commands.spawn(Hooked(phase));
            commands.trigger(Probe(phase));
        }
    }

    /// Stats readers need final flushes and their hooks/observers on the producer's side of
    /// each boundary, including FixedLast; leaf time must be debited exactly once on every tick.
    #[test]
    fn complete_intervals_include_deferred_hooks_and_observers_without_double_counting() {
        let mut app = fixture_app(true);
        app.init_resource::<Witness>()
            .add_observer(
                |probe: On<Probe>,
                 phases: Res<Phases>,
                 stages: Res<Stages>,
                 mut w: ResMut<Witness>| {
                    let _leaf = Stages::start(Some(&stages), Stage::Intake);
                    w.0.push((probe.0, phases.active.unwrap().0));
                },
            )
            .add_systems(FixedFirst, commands_in(Phase::FixedFirst))
            .add_systems(
                FixedPostUpdate,
                commands_in(Phase::SweepEcs).in_set(SweepSet),
            )
            .add_systems(
                FixedPostUpdate,
                commands_in(Phase::ConstructionEcs).in_set(SplitSet),
            )
            .add_systems(
                FixedPostUpdate,
                commands_in(Phase::PublicationEcs).in_set(SimPhase::Publish),
            )
            .add_systems(FixedLast, commands_in(Phase::FixedLast));
        // Fixture: consecutive ticks expose stale accumulation as well as the first tick's wiring.
        for _ in 0..2 {
            app.world_mut().run_schedule(FixedMain);
            let phases = app.world().resource::<Phases>();
            assert!(phases.active.is_none());
            let phase_us: f64 = phases.times().values.iter().sum();
            let leaves: f64 = app.world().resource::<Stages>().take_us().iter().sum();
            let total = app.world().resource::<TickClock>().closed().unwrap().us;
            // DERIVED allowance: floating conversions of integer nanoseconds to microseconds.
            assert!((phase_us + leaves - total).abs() < total * f64::EPSILON * 32.0);
            let witness = &mut app.world_mut().resource_mut::<Witness>().0;
            assert_eq!(
                witness.len(),
                15,
                "each producer applies a command, hook and observer"
            );
            assert!(witness.iter().all(|(expected, actual)| expected == actual));
            witness.clear();
        }
    }

    /// Local instrument observation, not a game-performance threshold. This isolates the added
    /// timers and their schedule dispatch from simulation variance; the collectors run separately.
    #[test]
    fn observe_empty_tick_instrument_cost() {
        // Resolution: warm schedule construction, then amortize sampling noise over many ticks.
        let warmup = 128;
        let samples = 10_000;
        let mut means = Vec::new();
        for instrumented in [false, true] {
            let mut app = fixture_app(instrumented);
            let mut total = 0.0;
            for tick in 0..warmup + samples {
                let start = Instant::now();
                app.world_mut().run_schedule(FixedMain);
                let elapsed = start.elapsed().as_secs_f64() * 1.0e6;
                if tick >= warmup {
                    total += elapsed;
                }
            }
            means.push(total / f64::from(samples));
        }
        eprintln!(
            "MEASURED empty FixedMain wall mean, phase instrument off/on: {:.3}/{:.3} us; difference {:.3} us ({samples} samples, edit profile; includes timer finalization)",
            means[0],
            means[1],
            means[1] - means[0],
        );
    }
}
