//! Offline and sandbox loading frames consume no time or fixed ticks. All their capture cues,
//! including the game's damage-model cues (game state only), share the placement origin.
//! Online clients and dedicated servers count from startup: loading consumes time there and
//! network arrivals are live input, never repeatable. A scripted client's frames count from its
//! tank's approach instead (`Anchor::Approach`), on the same live clock.
//!
//! [`install`] suspends the selected pacing; [`start_clock`] restores it before Bevy's time
//! update when the composition is ready. [`Cues`] gates every timeline consumer during loading.
//! The loading-writer and message-reader audit lives in `.agents/scratch/capture-clock/README.md`.

use bevy::prelude::*;
use bevy::time::{TimeSystems, TimeUpdateStrategy};
use std::time::{Duration, Instant};

/// Resolution: allow asynchronous asset decode and binding, but bound a broken readiness gate
/// independently of the suspended game clock, in wall seconds.
/// MEASURED: placement took 1.6–2.2 s on a local M4 debug build across ten runs.
const LOAD_DEADLINE: Duration = Duration::from_secs(120);

/// The composition event that opens a plan's timeline.
#[derive(Clone, Debug)]
pub enum Anchor {
    /// Offline scenarios (including every demo): Playing with every tank view bound.
    Scenario,
    /// Armour sandbox: the placed target's view is bound.
    TankView,
    /// Track sandbox: the rig has been built from its blueprint.
    TrackRig,
    /// Online clients and dedicated servers keep their startup clock, including loading time.
    Startup,
    /// A scripted network client keeps its startup clock; its frames count from the first frame
    /// its controlled tank comes within the plan's `frames.lead_m` of this construction instance.
    Approach(String),
}

/// Only compositions that suspend time have a readiness gate.
#[derive(Clone, Copy, Debug)]
enum Gate {
    Scenario,
    TankView,
    TrackRig,
}

#[derive(Resource)]
enum Clock {
    Waiting {
        gate: Gate,
        strategy: TimeUpdateStrategy,
        deadline: Instant,
    },
    Running,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Cues;

pub(super) fn install(app: &mut App, anchor: &Anchor) {
    app.configure_sets(PreUpdate, Cues.run_if(running))
        .configure_sets(Update, Cues.run_if(running))
        .configure_sets(RunFixedMainLoop, Cues.run_if(running))
        .configure_sets(PostUpdate, Cues.run_if(running));
    let gate = match anchor {
        // Startup precedes the first time update (Bevy 0.20.1 Main::run_main). The server runner
        // owns its primed real clock and tick admission; presentation must preserve both.
        Anchor::Startup | Anchor::Approach(_) => {
            app.insert_resource(Clock::Running);
            return;
        }
        Anchor::Scenario => Gate::Scenario,
        Anchor::TankView => Gate::TankView,
        Anchor::TrackRig => Gate::TrackRig,
    };
    let strategy = std::mem::replace(
        &mut *app.world_mut().resource_mut::<TimeUpdateStrategy>(),
        TimeUpdateStrategy::ManualDuration(Duration::ZERO),
    );
    app.insert_resource(Clock::Waiting {
        gate,
        strategy,
        deadline: Instant::now() + LOAD_DEADLINE,
    })
    .add_systems(First, start_clock.before(TimeSystems));
}

fn running(clock: Res<Clock>) -> bool {
    matches!(*clock, Clock::Running)
}

fn start_clock(
    mut clock: ResMut<Clock>,
    mut strategy: ResMut<TimeUpdateStrategy>,
    mut real: ResMut<Time<Real>>,
    state: Option<Res<State<crate::state::AppState>>>,
    tanks: Query<Has<crate::tank::TankViewReady>, With<crate::tank::Tank>>,
    rig: Option<Res<crate::track::rig_geom::RigGeom>>,
) {
    let Clock::Waiting { gate, deadline, .. } = &*clock else {
        return;
    };
    let views_ready = || !tanks.is_empty() && tanks.iter().all(|ready| ready);
    let playing = match gate {
        Gate::Scenario => {
            *state
                .as_ref()
                .expect("capture: Scenario anchor requires State<AppState>; missing AppState is a wiring bug")
                .get()
                == crate::state::AppState::Playing
        }
        Gate::TankView | Gate::TrackRig => false,
    };
    let ready = match gate {
        Gate::Scenario => playing && views_ready(),
        Gate::TankView => views_ready(),
        Gate::TrackRig => rig.is_some(),
    };
    if !ready {
        if Instant::now() >= *deadline {
            let mut missing = Vec::new();
            if matches!(gate, Gate::Scenario) && !playing {
                missing.push(format!(
                    "state {:?}; requires Playing",
                    state.as_ref().map(|s| s.get())
                ));
            }
            if matches!(gate, Gate::Scenario | Gate::TankView) && !views_ready() {
                let bound = tanks.iter().filter(|ready| *ready).count();
                missing.push(format!("{bound} of {} tank views bound; requires at least one tank and every view bound", tanks.iter().count()));
            }
            if matches!(gate, Gate::TrackRig) && rig.is_none() {
                missing.push("rig geometry absent".into());
            }
            panic!(
                "capture: {gate:?} anchor readiness deadline: {}",
                missing.join("; ")
            );
        }
        return;
    }
    let Clock::Waiting {
        strategy: saved, ..
    } = std::mem::replace(&mut *clock, Clock::Running)
    else {
        unreachable!("only a waiting capture clock can start")
    };
    *strategy = saved;
    // Automatic stepping must not inherit loading wall time. Every strategy starts with the
    // zero-delta update of a fresh Bevy real clock, then takes its selected frame steps.
    *real = Time::default();
    info!("capture: composition placed; timeline starts");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<crate::state::AppState>();
        app
    }

    /// Before/after captures require no loading cue time or fixed simulation ticks. A bound view
    /// alone cannot open gameplay; Playing alone cannot open an unbound view.
    #[test]
    fn loading_does_not_advance_the_plan_or_simulation() {
        let mut app = app();
        let step = app.world().resource::<Time<Fixed>>().timestep() / 2;
        app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        install(&mut app, &Anchor::Scenario);
        #[derive(Resource, Default)]
        struct DueCues(usize);
        app.init_resource::<DueCues>().add_systems(
            Update,
            (|mut cues: ResMut<DueCues>| cues.0 += 1).in_set(Cues),
        );
        let tank = app
            .world_mut()
            .spawn((crate::tank::Tank, crate::tank::TankViewReady))
            .id();
        for _ in 0..7 {
            // Fixture asynchronous loading frames.
            app.update();
            assert_eq!(app.world().resource::<DueCues>().0, 0);
            assert_eq!(app.world().resource::<Time>().elapsed(), Duration::ZERO);
            assert_eq!(
                app.world().resource::<Time<Real>>().elapsed(),
                Duration::ZERO
            );
            assert_eq!(
                app.world().resource::<Time<Fixed>>().elapsed(),
                Duration::ZERO
            );
        }
        app.world_mut()
            .entity_mut(tank)
            .remove::<crate::tank::TankViewReady>();
        app.world_mut()
            .resource_mut::<NextState<crate::state::AppState>>()
            .set(crate::state::AppState::Playing);
        app.update();
        app.update();
        assert!(!matches!(app.world().resource::<Clock>(), Clock::Running));
        app.world_mut()
            .entity_mut(tank)
            .insert(crate::tank::TankViewReady);
        app.update();
        assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
        assert_eq!(app.world().resource::<DueCues>().0, 1);
        let before = app.world().resource::<Time>().elapsed();
        let fixed_step = app.world().resource::<Time<Fixed>>().timestep();
        let updates = fixed_step.as_nanos().div_ceil(step.as_nanos()) as usize;
        for _ in 0..updates {
            app.update();
        }
        assert_eq!(
            app.world().resource::<Time>().elapsed() - before,
            step * updates as u32
        );
        assert!(app.world().resource::<Time<Fixed>>().elapsed() > Duration::ZERO);
    }

    /// Automatic captures must not repay loading as a burst of simulation steps at release.
    #[test]
    fn release_discards_loading_wall_time_before_the_next_fixed_step() {
        #[derive(Resource, Default)]
        struct Ticks(usize);
        let mut app = app();
        app.init_resource::<Ticks>()
            .add_systems(FixedUpdate, |mut ticks: ResMut<Ticks>| ticks.0 += 1);
        let start = app.world().resource::<Time<Real>>().startup();
        let release = start + Duration::from_secs(10); // Fixture loading wall time.
        app.insert_resource(TimeUpdateStrategy::ManualInstant(release));
        install(&mut app, &Anchor::TankView);
        app.update();
        app.update();
        app.world_mut()
            .spawn((crate::tank::Tank, crate::tank::TankViewReady));
        app.update();
        assert_eq!(app.world().resource::<Time<Real>>().delta(), Duration::ZERO);
        assert_eq!(app.world().resource::<Ticks>().0, 0);
        let step = app.world().resource::<Time<Fixed>>().timestep();
        app.insert_resource(TimeUpdateStrategy::ManualInstant(release + step));
        app.update();
        assert!(app.world().resource::<Ticks>().0 <= 1);
        assert_eq!(app.world().resource::<Time<Real>>().delta(), step);
    }

    /// An empty tank query is not a placed scene, even when Playing has already arrived.
    #[test]
    fn tank_anchors_require_a_nonempty_fully_bound_composition() {
        for anchor in [Anchor::Scenario, Anchor::TankView] {
            let mut app = app();
            app.insert_resource(State::new(crate::state::AppState::Playing));
            install(&mut app, &anchor);
            app.update();
            assert!(!matches!(app.world().resource::<Clock>(), Clock::Running));
            let tank = app.world_mut().spawn(crate::tank::Tank).id();
            app.update();
            assert!(!matches!(app.world().resource::<Clock>(), Clock::Running));
            app.world_mut()
                .entity_mut(tank)
                .insert(crate::tank::TankViewReady);
            app.update();
            assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
        }
    }

    /// The track sandbox opens on geometry publication, independently of gameplay and tank views.
    #[test]
    fn track_anchor_waits_for_the_constructed_rig() {
        let mut app = app();
        install(&mut app, &Anchor::TrackRig);
        app.update();
        assert!(!matches!(app.world().resource::<Clock>(), Clock::Running));
        let mut construction = App::new();
        construction.add_plugins((crate::bake::plugin, crate::track::sim::sim_plugin));
        construction.world_mut().run_schedule(Startup);
        construction.world_mut().run_schedule(PreUpdate);
        app.insert_resource(
            construction
                .world()
                .resource::<crate::track::rig_geom::RigGeom>()
                .clone(),
        );
        app.update();
        assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
    }

    /// Broken readiness must terminate independently of frozen game time and identify its gate
    /// and missing facts, so unattended automation can distinguish loading failures.
    #[test]
    fn wall_deadline_names_the_anchor_and_missing_readiness() {
        for anchor in [Anchor::Scenario, Anchor::TankView, Anchor::TrackRig] {
            let mut app = app();
            install(&mut app, &anchor);
            app.world_mut()
                .spawn((crate::tank::Tank, crate::tank::TankViewReady));
            app.world_mut().spawn(crate::tank::Tank);
            app.update();
            let Clock::Waiting { deadline, .. } = &mut *app.world_mut().resource_mut::<Clock>()
            else {
                panic!("fixture is waiting")
            };
            *deadline = Instant::now();
            let error = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()))
                .unwrap_err();
            let message = error
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| error.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(message.contains(&format!("{anchor:?}")), "{message}");
            match anchor {
                Anchor::Scenario => {
                    assert!(
                        message.contains("state Some(Loading)")
                            && message.contains("1 of 2 tank views bound"),
                        "{message}"
                    );
                }
                Anchor::TankView => {
                    assert!(message.contains("1 of 2 tank views bound"), "{message}")
                }
                Anchor::TrackRig => assert!(message.contains("rig geometry absent"), "{message}"),
                Anchor::Startup | Anchor::Approach(_) => {
                    unreachable!("fixture uses only waiting gates")
                }
            }
            assert_eq!(
                app.world().resource::<Time<Real>>().elapsed(),
                Duration::ZERO
            );
        }
    }

    /// Scenario wiring must supply AppState on the first frame; automation must not wait for
    /// the asset-loading deadline to discover a missing resource.
    #[test]
    #[should_panic(
        expected = "Scenario anchor requires State<AppState>; missing AppState is a wiring bug"
    )]
    fn missing_scenario_state_panics_on_the_first_frame() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        install(&mut app, &Anchor::Scenario);
        app.update();
    }

    /// Startup clocks belong to online compositions, including their primed server tick strategy.
    #[test]
    fn startup_anchor_preserves_the_compositions_clock_ownership() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let step = app.world().resource::<Time<Fixed>>().timestep();
        app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .update_with_duration(step);
        install(&mut app, &Anchor::Startup);
        app.update();
        assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
        assert_eq!(app.world().resource::<Time<Real>>().delta(), step);
    }
}
