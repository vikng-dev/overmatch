//! Retry rollback comparisons that Lightyear 0.28 skips for future-stamped confirmations.
//!
//! Owner: this module. It compares each component's newest confirmed sample at or before the last
//! completed tick with its prediction history and requests a forced rollback only after distinct
//! samples persistently breach the protocol thresholds. Remove it when Lightyear retries those
//! stored samples itself; see ADR-0015.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::*;

use super::protocol::{
    NetTank, ROLLBACK_POSITION_M, ROLLBACK_ROTATION_RAD, ROLLBACK_VELOCITY, angular_velocity_error,
    linear_velocity_error, position_error, rotation_error,
};

/// Distinct samples required before this fallback requests a rollback.
const BREACH_STREAK_TO_FIRE: u8 = 3;

/// Per-component state; a sample contributes to the streak at most once.
#[derive(Default, Clone, Copy)]
struct ComponentState {
    last_tick: Option<Tick>,
    streak: u8,
}

/// Watchdog memory in `rollback_watchdog` check order.
#[derive(Default)]
struct WatchdogState {
    components: [ComponentState; 4],
}

/// Run after receive populates history and before rollback consumes forced requests.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PreUpdate,
        rollback_watchdog
            .after(ReplicationSystems::Receive)
            .before(RollbackSystems::Check)
            .run_if(not(is_in_rollback)),
    );
}

/// One component's verdict for the current pass.
struct Check {
    name: &'static str,
    threshold: f32,
    /// Newest comparable confirmed tick, if its prediction is retained.
    checkable: Option<Tick>,
    /// Breach magnitude when it meets the component threshold.
    breach: Option<f32>,
}

/// Return the newest confirmed sample at or before `tick` without consuming history.
fn newest_present_at_or_before<C>(history: &ConfirmedHistory<C>, tick: Tick) -> Option<(Tick, &C)> {
    history.into_iter().take_while(|(t, _)| *t <= tick).last()
}

/// Compare the newest completed confirmed sample with prediction history at the same tick.
fn check<C: Component>(
    name: &'static str,
    threshold: f32,
    magnitude: impl Fn(&C, &C) -> f32,
    confirmed: Option<&ConfirmedHistory<C>>,
    predicted: Option<&PredictionHistory<C>>,
    latest_checkable: Tick,
) -> Check {
    let mut result = Check {
        name,
        threshold,
        checkable: None,
        breach: None,
    };
    let Some((confirmed_tick, confirmed_value)) =
        confirmed.and_then(|h| newest_present_at_or_before(h, latest_checkable))
    else {
        return result;
    };
    let Some(predicted_value) = predicted.and_then(|h| h.get(confirmed_tick)) else {
        debug!(
            "watchdog: {name} has no retained predicted value at confirmed tick {confirmed_tick:?} — skipping"
        );
        return result;
    };
    let magnitude = magnitude(confirmed_value, predicted_value);
    result.checkable = Some(confirmed_tick);
    result.breach = (magnitude >= threshold).then_some(magnitude);
    result
}

/// Request a bounded forced rollback only for persistent, checkable component breaches.
fn rollback_watchdog(
    timeline: Res<LocalTimeline>,
    mut metadata: ResMut<StateRollbackMetadata>,
    manager: Query<&PredictionManager, With<IsSynced<InputTimeline>>>,
    #[allow(clippy::type_complexity)] tank: Query<
        (
            Option<&ConfirmedHistory<Position>>,
            Option<&PredictionHistory<Position>>,
            Option<&ConfirmedHistory<Rotation>>,
            Option<&PredictionHistory<Rotation>>,
            Option<&ConfirmedHistory<LinearVelocity>>,
            Option<&PredictionHistory<LinearVelocity>>,
            Option<&ConfirmedHistory<AngularVelocity>>,
            Option<&PredictionHistory<AngularVelocity>>,
        ),
        (With<Predicted>, With<NetTank>, Without<DisableRollback>),
    >,
    mut state: Local<WatchdogState>,
) {
    if metadata.forced_rollback_tick().is_some() {
        return;
    }
    let Ok(manager) = manager.single() else {
        return;
    };
    let Ok((conf_p, pred_p, conf_q, pred_q, conf_v, pred_v, conf_w, pred_w)) = tank.single() else {
        return;
    };
    let current_tick = timeline.tick();
    // Never compare authority against a prediction that has not completed its tick.
    let latest_checkable = current_tick - 1u32;

    // Keep this order aligned with `WatchdogState::components`.
    let checks = [
        check(
            "Position",
            ROLLBACK_POSITION_M,
            position_error,
            conf_p,
            pred_p,
            latest_checkable,
        ),
        check(
            "Rotation",
            ROLLBACK_ROTATION_RAD,
            rotation_error,
            conf_q,
            pred_q,
            latest_checkable,
        ),
        check(
            "LinearVelocity",
            ROLLBACK_VELOCITY,
            linear_velocity_error,
            conf_v,
            pred_v,
            latest_checkable,
        ),
        check(
            "AngularVelocity",
            ROLLBACK_VELOCITY,
            angular_velocity_error,
            conf_w,
            pred_w,
            latest_checkable,
        ),
    ];

    // A timeline regression starts a new session; stale sample ticks must not suppress it.
    if checks.iter().zip(&state.components).any(|(c, s)| {
        c.checkable
            .zip(s.last_tick)
            .is_some_and(|(t, last)| t < last)
    }) {
        debug!("watchdog: confirmed timeline regressed — resetting for the new session");
        *state = WatchdogState::default();
    }

    // Count each component's confirmed sample once, independent of frame rate and other feeds.
    let mut fire = false;
    for (check, component) in checks.iter().zip(state.components.iter_mut()) {
        let Some(tick) = check.checkable else {
            continue;
        };
        if component.last_tick.is_some_and(|last| tick <= last) {
            continue;
        }
        component.last_tick = Some(tick);
        if check.breach.is_some() {
            component.streak = component.streak.saturating_add(1);
            fire |= component.streak >= BREACH_STREAK_TO_FIRE;
        } else {
            component.streak = 0;
        }
    }
    if !fire {
        return;
    }

    // The restore tick must fit the prediction manager's rollback-depth cap.
    let max_depth = i32::from(manager.rollback_policy.max_rollback_ticks);
    let Some(rollback_tick) = checks
        .iter()
        .filter_map(|c| c.breach.and(c.checkable))
        .filter(|tick| current_tick - *tick <= max_depth)
        .min()
    else {
        debug!(
            "watchdog: persistent breach, but every breaching sample is deeper than the \
             {max_depth}-tick rollback cap (current {current_tick:?}) — skipping"
        );
        return;
    };
    let Some((name, magnitude, threshold)) = checks
        .iter()
        .filter_map(|c| c.breach.map(|magnitude| (c.name, magnitude, c.threshold)))
        .max_by(|a, b| (a.1 / a.2).total_cmp(&(b.1 / b.2)))
    else {
        return;
    };
    for component in &mut state.components {
        component.streak = 0;
    }
    metadata.request_forced_rollback(rollback_tick);
    info!(
        "watchdog: receive-time rollback check starved — forcing rollback at {rollback_tick:?}: \
         {name} off by {magnitude:.3} (bar {threshold}), depth {} ticks (current {current_tick:?})",
        current_tick - rollback_tick,
    );
}
