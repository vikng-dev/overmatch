//! The rollback watchdog — the backstop for lightyear 0.28's starved receive-time mismatch check.
//!
//! Client-only, predicted-tank-only. For an entity that receives confirmed updates every tick (our
//! tank — Position/Rotation/velocities mutate every server tick), lightyear's ONLY state-rollback
//! detector runs at RECEIVE time: `write_history::<C>` → `record_confirmed_and_maybe_check`
//! (lightyear_prediction-0.28.0 registry.rs:386-484). That check is gated STRICTLY on
//! `confirmed_tick < current_tick` (registry.rs:426-428): if the confirmed sample arrives stamped
//! at-or-ahead-of the client's current tick, the value is stored in `ConfirmedHistory<C>` but the
//! `should_rollback` comparison is skipped — and NEVER retried once the client's tick passes it
//! (registry.rs:448-458 logs the skip and moves on). The fallback unchanged-entity scan in
//! `check_rollback` can't save us either: it explicitly skips any entity whose Replicon
//! `ConfirmHistory` already contains the completed tick (rollback.rs:583-590) — i.e. exactly an
//! entity confirmed every tick.
//!
//! Our shipping `InputDelayConfig::balanced()` absorbs all RTT into input delay at LAN/loopback
//! latency, so the sync objective holds the client dead level with the server: EVERY confirmed
//! update arrives with `confirmed_tick >= current_tick`, every receive-time check is skipped, and
//! state rollback is permanently, silently dead. Measured consequence (beached-pose repro, 0 ms):
//! the client diverged 35-50 m from the server with fresh authority arriving every tick and zero
//! rollbacks firing.
//!
//! THE BACKSTOP. Every frame (PreUpdate, after replication receive has filled `ConfirmedHistory`,
//! before `check_rollback` consumes forced requests — see [`plugin`] on the ordering), re-run the
//! comparison lightyear skipped: for each predicted component, take the newest confirmed sample
//! `(T_c, value)` AT-OR-BEFORE the last completed tick (`current_tick - 1`) and compare it against
//! the client's own `PredictionHistory<C>` at `T_c` — the exact lookup (`history.get(T_c)`,
//! at-or-before semantics) and the exact metrics/thresholds (`protocol::*_error` /
//! `protocol::ROLLBACK_*`) the receive-time check would have used. At-or-before, NOT
//! `newest_present` alone: in a persistent server-ahead regime (the client running a hair behind),
//! the newest sample is perpetually stamped at-or-ahead of `current_tick` — by the time the tick
//! passes sample `T`, the newest is already `T+1` — and a newest-only watchdog would starve
//! exactly like the check it backstops. The at-or-before lookup makes every sample checkable one
//! tick after it lands, whichever side of `current_tick` newer samples sit on; the watchdog IS the
//! retry loop lightyear lacks. On a persistent breach it calls
//! `StateRollbackMetadata::request_forced_rollback` (manager.rs:252-258) — consumed by
//! `check_rollback` regardless of policy gates (rollback.rs:441-462) — restoring the confirmed
//! state and replaying to the present.
//!
//! Why a PERSISTENCE gate ([`BREACH_STREAK_TO_FIRE`]) and not fire-on-first-breach: at real
//! latency the margin is positive, the receive-time check is alive, and it records mismatches the
//! policy path consumes a frame or two later (once the completed mutate tick reaches them). The
//! watchdog runs before that consumption, so on those frames it would see the same breach the
//! policy path is already handling — firing there would shadow the healthy mechanism with forced
//! rollbacks on every genuine misprediction. Requiring the breach to persist across three DISTINCT
//! confirmed samples (~3 server ticks) gives the policy path its window: a handled mismatch is
//! repaired within a frame or two (`prepare_rollback` clears `PredictionHistory` and re-anchors it
//! at the rollback tick with the confirmed state — rollback.rs:938-945 — so the comparison comes
//! back clean, which is also the watchdog's own debounce), while the starved zero-margin case
//! breaches sample after sample forever and fires ~47 ms in. The streak is PER COMPONENT, and a
//! component's breach only counts when its OWN sample tick advanced — a single stale breaching
//! sample on a component whose confirmed feed stalled must not be re-counted just because other
//! components' feeds kept moving. This is a backstop: firings must be rare and visible, hence one
//! `info!` line per firing.
//!
//! SCAFFOLDING STATUS (ADR-0015): this module is Layer-2 netcode scaffolding — a workaround for a
//! named upstream defect, not architecture. The defect is lightyear's skipped-check-never-retried:
//! a confirmed sample stamped at-or-ahead of the current tick is stored but never re-checked once
//! the tick passes it. Removal condition: delete this module (or demote it to insurance behind a
//! rare-firing assert) when lightyear ships a deferred re-check of stored future samples, or
//! includes always-confirmed entities in the `check_rollback` unchanged-entity scan whenever the
//! receive-time check was skipped. The 3-sample persistence gate above is the part worth keeping
//! if it ever becomes insurance.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::*;

use super::protocol::{
    NetTank, ROLLBACK_POSITION_M, ROLLBACK_ROTATION_RAD, ROLLBACK_VELOCITY, angular_velocity_error,
    linear_velocity_error, position_error, rotation_error,
};

/// How many consecutive DISTINCT confirmed samples of ONE component must breach before the
/// watchdog fires. Each sample is one server tick of fresh authority (~15.6 ms at 64 Hz), so 3 ≈
/// 47 ms of sustained, unhandled desync — long enough for the receive-time path to have consumed
/// any mismatch it recorded (it acts within a frame or two), short enough that a genuine runaway
/// is caught before it is felt (the un-backstopped failure ran to 35-50 m).
const BREACH_STREAK_TO_FIRE: u8 = 3;

/// One component's watchdog memory across frames: the newest checkable confirmed sample already
/// evaluated (frames run ~8× faster than ticks headless — without this, one breaching sample would
/// inflate the streak within milliseconds and the persistence gate would gate nothing), and how
/// many of this component's OWN consecutive distinct samples have breached.
#[derive(Default, Clone, Copy)]
struct ComponentState {
    last_tick: Option<Tick>,
    streak: u8,
}

/// Watchdog memory, one slot per checked component (same order as the `checks` array in
/// [`rollback_watchdog`]).
#[derive(Default)]
struct WatchdogState {
    components: [ComponentState; 4],
}

/// PreUpdate, `after(ReplicationSystems::Receive)` — `ConfirmedHistory` is current, every sample
/// this frame's messages carried is visible — and `before(RollbackSystems::Check)` — a forced
/// request is consumed (and the rollback fully executed: Check → Prepare → Rollback → EndRollback
/// all chain inside this same PreUpdate, rollback.rs:121-132) in the SAME frame, no extra frame of
/// desync. Both anchors are the ones `check_rollback` itself orders against (rollback.rs:234-239),
/// so this ordering is exactly as stable as lightyear's own.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PreUpdate,
        rollback_watchdog
            .after(ReplicationSystems::Receive)
            .before(RollbackSystems::Check)
            .run_if(not(is_in_rollback)),
    );
}

/// One component's verdict this pass — built by [`check`], which states the component's name and
/// threshold exactly once.
struct Check {
    name: &'static str,
    threshold: f32,
    /// Newest confirmed tick that was actually comparable this pass (at-or-before
    /// `current_tick - 1`, with a retained predicted value at that tick). `None` = nothing to say
    /// about this component.
    checkable: Option<Tick>,
    /// `Some(magnitude)` iff the comparison at `checkable` breached the threshold
    /// (`magnitude >= threshold`, the same verdict `trace::note_if_tripped` computes for the
    /// registered rollback conditions).
    breach: Option<f32>,
}

/// The newest confirmed present sample at-or-before `tick`, WITH its tick. Same resolution as
/// `ConfirmedHistory::get_state_at_or_before` (confirmed_history.rs:124 — `SameAsPrecedent`
/// entries resolve to the nearest preceding explicit value, `Removed` yields nothing), which the
/// watchdog can't use directly because the public getter drops the sample's tick — needed both as
/// the `PredictionHistory` lookup key and as the streak's distinct-sample key. `&self` iteration
/// over the sorted buffer: strictly non-destructive, unlike the `pop_present` access the
/// interpolation consumers use — the receive path and `prepare_rollback` own this buffer's
/// lifecycle and must find it untouched.
fn newest_present_at_or_before<C>(history: &ConfirmedHistory<C>, tick: Tick) -> Option<(Tick, &C)> {
    history.into_iter().take_while(|(t, _)| *t <= tick).last()
}

/// The receive-time comparison, re-run: newest checkable confirmed sample vs `PredictionHistory`
/// at that tick. Mirrors `record_confirmed_and_maybe_check` (registry.rs:386-484) — the strict
/// `confirmed_tick < current_tick` gate (registry.rs:427; a sample at-or-ahead of the current tick
/// compares fresh authority against a prediction the client hasn't stepped yet — a moving tank
/// would false-breach by ~speed/64 per tick of gap) becomes the `latest_checkable` cap on the
/// sample LOOKUP rather than a skip, and the same at-or-before `history.get(T_c)` lookup
/// (registry.rs:430-432) fetches the prediction, whose `None` covers both a pruned history (oldest
/// retained tick past `T_c` — registry.rs:421-425's skip) and an explicit removal.
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
        // Pruned past the confirmed tick (or explicitly removed) — a pruned prediction cannot
        // prove a mismatch (registry.rs:416-425). Rare here: the tank's history is pruned by
        // rollbacks, which re-anchor it at the rollback tick, at-or-before any newer sample.
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

/// The watchdog pass. Reads all four predicted components off the tank root (both histories live
/// on the SAME entity — the replicated root IS the predicted entity in 0.28, and
/// `record_confirmed_and_maybe_check` reads `PredictionHistory` and writes `ConfirmedHistory`
/// through one `entity_mut`, registry.rs:405/476-482). Guards, in order: no double-request while a
/// forced rollback is pending (`forced_rollback_tick()` stays `Some` until `check_rollback`
/// consumes it — manager.rs:260-267 — which covers frames where that system doesn't run at all,
/// e.g. pre-sync); no synced client yet; no predicted tank yet (pre-spawn), or the tank still under
/// its spawn-protection `DisableRollback` (a body that must not replay yet must not be forced to).
///
/// A checkable sample tick REGRESSING below a component's remembered tick means the timeline
/// resynced backward (reconnect / server restart): every remembered tick belongs to the dead
/// session, and holding on to them would suppress observation until the new session's ticks catch
/// up — minutes of dead backstop. The whole state resets and observation restarts from the new
/// session's samples in the same pass.
///
/// The forced request is never allowed deeper than the policy cap
/// (`RollbackPolicy::max_rollback_ticks`): `check_rollback` aborts a deeper request with a warn
/// and clears it (rollback.rs:395-418), so requesting one would re-arm a warn-spam loop — if every
/// breaching sample is older than the cap, the watchdog logs at debug and waits for a fresher
/// breaching sample instead. Depth < 1 is impossible: checkable ticks are capped at
/// `current_tick - 1`.
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
    // The newest tick the client has actually simulated past — the receive-time check's strict
    // `confirmed_tick < current_tick` gate, as a lookup bound.
    let latest_checkable = current_tick - 1u32;

    // The same metrics and thresholds the registered rollback conditions use (`net::protocol` —
    // one definition of "desynced enough") — including velocity's deliberately desync-only 1.0
    // bar. Order must match `WatchdogState::components`.
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

    // Timeline regression = new session (see the system doc): forget everything, observe fresh.
    if checks.iter().zip(&state.components).any(|(c, s)| {
        c.checkable
            .zip(s.last_tick)
            .is_some_and(|(t, last)| t < last)
    }) {
        debug!("watchdog: confirmed timeline regressed — resetting for the new session");
        *state = WatchdogState::default();
    }

    // One "observation" per component = ITS newest checkable sample advanced. Anything else is a
    // frame re-reading a sample the streak already counted (frames outpace ticks ~8:1) — or a
    // stale sample on a stalled feed, which must not ride other components' advancing ticks.
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
            // Saturating: a depth-capped skip below leaves the streak armed indefinitely.
            component.streak = component.streak.saturating_add(1);
            fire |= component.streak >= BREACH_STREAK_TO_FIRE;
        } else {
            component.streak = 0;
        }
    }
    if !fire {
        return;
    }

    // Earliest breaching tick within the policy cap is the restore point — every breaching
    // component has confirmed data at-or-before its own tick, which is what `prepare_rollback`'s
    // forced-path lookup uses (rollback.rs:911-928). Breaches beyond the cap are unrequestable
    // (see the system doc), so they can't nominate the restore point.
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
    // Worst breach (by magnitude/threshold ratio) names the firing. `fire` implies a component
    // breached at an advanced tick this pass, so the max is over a non-empty iterator.
    let Some((name, magnitude, threshold)) = checks
        .iter()
        .filter_map(|c| c.breach.map(|magnitude| (c.name, magnitude, c.threshold)))
        .max_by(|a, b| (a.1 / a.2).total_cmp(&(b.1 / b.2)))
    else {
        return;
    };
    // Re-arm the persistence gate: the rollback clears and re-anchors every component's
    // `PredictionHistory` at `rollback_tick`, so all comparisons restart clean.
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
