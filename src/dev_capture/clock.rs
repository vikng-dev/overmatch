//! A capture's loading frames consume no time or fixed ticks. All cue readers, including the
//! game's damage-model scenarios using `Time<Real>`, share this origin.

use bevy::prelude::*;
use bevy::time::{TimeSystems, TimeUpdateStrategy};

/// The composition event that opens a plan's timeline. Network menus need no possessed tank;
/// network captures observe live server state and do not promise repeatable network arrivals.
#[derive(Clone, Copy)]
pub enum Anchor {
    /// Offline scenarios (including every demo): Playing with every tank view bound.
    Scenario,
    /// Armour sandbox: the placed target's view is bound.
    TankView,
    /// Track sandbox: the rig has been built from its blueprint.
    TrackRig,
    /// Online menus and dedicated-server wall cues: the startup world has been placed.
    Startup,
}

#[derive(Resource)]
enum Clock {
    Waiting {
        anchor: Anchor,
        strategy: TimeUpdateStrategy,
    },
    Running,
}

#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Cues;

pub(super) fn install(app: &mut App, anchor: Anchor) {
    app.configure_sets(PreUpdate, Cues.run_if(running))
        .configure_sets(Update, Cues.run_if(running));
    if matches!(anchor, Anchor::Startup) {
        // Startup precedes Bevy's first time update. The server runner owns its primed real
        // clock and first admitted tick; neither may be reset by a presentation plan.
        app.insert_resource(Clock::Running);
    } else {
        let strategy = std::mem::replace(
            &mut *app.world_mut().resource_mut::<TimeUpdateStrategy>(),
            TimeUpdateStrategy::ManualDuration(std::time::Duration::ZERO),
        );
        app.insert_resource(Clock::Waiting { anchor, strategy })
            .add_systems(First, start_clock.before(TimeSystems));
    }
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
    // Bevy 0.20.1 Main::run_main completes Startup before the first First schedule.
    let Clock::Waiting { anchor, .. } = &*clock else {
        return;
    };
    let views_ready = || !tanks.is_empty() && tanks.iter().all(|ready| ready);
    let ready = match anchor {
        Anchor::Scenario => {
            *state.expect("the scenario root mounts AppState").get()
                == crate::state::AppState::Playing
                && views_ready()
        }
        Anchor::TankView => views_ready(),
        Anchor::TrackRig => rig.is_some(),
        Anchor::Startup => true,
    };
    if ready {
        let Clock::Waiting {
            strategy: saved, ..
        } = std::mem::replace(&mut *clock, Clock::Running)
        else {
            unreachable!("only a waiting capture clock can start")
        };
        *strategy = saved;
        // Automatic stepping must not inherit the wall time spent loading. Manual stepping gets
        // the same first zero-delta update as a fresh Bevy clock, then its authored frame steps.
        *real = Time::default();
        info!("capture: composition placed; timeline starts");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Before/after captures rely on loading consuming neither cue time nor fixed simulation
    /// ticks. Moving the clock start ahead of placement breaks this even with a fixed frame step.
    #[test]
    fn loading_does_not_advance_the_plan_or_simulation() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::state::app::StatesPlugin))
            .init_state::<crate::state::AppState>();
        let step = Duration::from_millis(20); // Fixture frame step, independent of sim frequency.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        install(&mut app, Anchor::Scenario);
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
            // Fixture asynchronous loading frames: a bound view alone cannot open gameplay.
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
        assert!(
            !matches!(app.world().resource::<Clock>(), Clock::Running),
            "a placed simulation still awaits its view"
        );
        app.world_mut()
            .entity_mut(tank)
            .insert(crate::tank::TankViewReady);
        app.update();
        assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
        assert_eq!(app.world().resource::<DueCues>().0, 1);
        let before = app.world().resource::<Time>().elapsed();
        app.update();
        assert_eq!(app.world().resource::<Time>().elapsed() - before, step);
        assert!(app.world().resource::<Time<Fixed>>().elapsed() > Duration::ZERO);
    }

    /// Online menus need no possession; the dedicated server's wall cues must preserve its
    /// primed real clock and tick-admission strategy. Resetting either breaks its first tick.
    #[test]
    fn startup_anchor_preserves_the_compositions_clock_ownership() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let step = Duration::from_millis(20); // Fixture runner pacing.
        app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .update_with_duration(step);
        install(&mut app, Anchor::Startup);
        app.update();
        assert!(matches!(app.world().resource::<Clock>(), Clock::Running));
        assert_eq!(app.world().resource::<Time<Real>>().delta(), step);
    }
}
