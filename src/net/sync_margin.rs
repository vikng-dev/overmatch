//! The sync margins on both wire timelines, derived from the measured link rather than pinned.
//!
//! # Uplink (the input lead)
//!
//! Lightyear places the input (driving) timeline at `remote + rtt/2 + (jitter × jitter_multiple +
//! tick × jitter_margin) + 1 tick + tick × error_margin − input_delay` (`lightyear_sync`
//! `input.rs`), recomputed every frame from the live rtt/jitter EWMAs and converged by ±5 % clock
//! dilation. The rtt and jitter terms already track the link; `jitter_margin` and `error_margin`
//! are 1.0-tick constants. The wall-clock click→server-sims-the-authored-tick latency is that
//! objective plus `input_delay` — the delay cancels exactly, so this module NEVER touches the
//! delay config (lightyear's input-buffer encoding requires end ticks that advance exactly once
//! per local tick; `net::client` pins the constant shape).
//!
//! The derived law re-sizes only the two tick constants:
//!
//! - `jitter_margin` — the quantization floor. Input content advances once per fixed tick, so the
//!   authored tick's phase against the server's consumption boundary is uniform over one content
//!   interval; the floor carries that distribution's mean, half a tick. The tail beyond the mean
//!   is the jitter term's coverage, and the server's `INPUT-ARRIVAL` instrument
//!   (`net::diagnostics`) is what certifies it.
//! - `error_margin` — the sync controller's deadband, which the objective pays in full because the
//!   timeline may drift that far behind before the controller corrects. A deadband need only
//!   exceed the error signal's own noise, whose scale is the link's measured jitter — so it IS the
//!   measured jitter, expressed in ticks. At loopback it approaches zero; the controller's
//!   smallest correction is a bounded ±5 % dilation, never a step, so a tight deadband cannot
//!   produce a timeline jump.
//!
//! A late input degrades to hold-last steering for the late window plus a fail-closed trigger that
//! `net::fire_presentation` already covers — cheap only while the own hull rides the server
//! stream. The derived uplink law therefore arms on the observed role (own tank `Interpolated`,
//! the same latch `net::fire_presentation` arms on) and latches for the session.
//!
//! # Downlink (the interpolation timeline)
//!
//! The interpolation objective is `estimate − (delay + jitter × jitter_multiple + tick ×
//! jitter_margin)`. `net::interp_delay` owns `delay` (`min_delay`, rewritten every frame); the
//! sync margins on top ran lightyear's defaults (multiple 4, floor 1.0), never adjusted by us.
//! The law brings them to the uplink's shipped coverage multiple and the same half-tick floor —
//! keyframes replicate once per tick, so the cursor's phase against arrival is the same uniform
//! distribution. Static per session: the live factor (jitter) is read by lightyear each frame from
//! the same EWMA, so there is nothing for a per-frame writer to move.
//!
//! # Levers
//!
//! Mirroring `OVERMATCH_INTERP_DELAY_MS`: `OVERMATCH_INPUT_MARGIN_MS` and
//! `OVERMATCH_INTERP_MARGIN_MS` SET pin that timeline's whole fixed margin (jitter multiple zeroed,
//! deadband zeroed) — the certification instrument for deliberately-undersized runs; UNSET runs
//! the derived law. A pinned uplink margin applies regardless of drive mode.
//!
//! Derivation, the failure-mode inventory, and the certification protocol:
//! `.agents/scratch/input-lead-budget-2026-08-14.md`.

use core::time::Duration;

use bevy::prelude::*;
use lightyear::core::tick::TickDuration;
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::client::InputDelayConfig;
use lightyear::prelude::{InputTimelineConfig, Interpolated, PingManager, SyncConfig, SyncSystems};

use super::protocol::NetTank;
use crate::tank::Controlled;

/// Half of one content interval, in ticks — the mean of the uniform phase between per-tick content
/// (authored inputs up, replicated keyframes down) and the consuming clock's boundary. A term of
/// structure, not a tuned number: it moves only if content stops advancing once per tick.
const CONTENT_PHASE_MEAN_TICKS: f32 = 0.5;

/// Log-rate guard, not a term of the law: the deadband follows the jitter EWMA continuously, so an
/// unconditional log is per-frame spam.
const LOG_STEP: Duration = Duration::from_millis(2);

/// `OVERMATCH_INPUT_MARGIN_MS`: SET pins the input timeline's fixed margin for the session; UNSET
/// runs the derived law once the own tank's role resolves interpolated.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
enum InputMarginMode {
    Fixed(Duration),
    Derived,
}

/// What the per-frame rewrite rebuilds `InputTimelineConfig` from: the sync config `net::client`
/// installed (the coverage multiple lives there) and the SAME input-delay config it installed —
/// carried whole so the rebuild cannot change the delay's shape.
#[derive(Resource, Debug, Clone, Copy)]
struct InputSyncBase {
    sync: SyncConfig,
    input_delay: InputDelayConfig,
}

/// The uplink law. The floor replaces the 1.0-tick default; the deadband replaces the 1.0-tick
/// default with the measured jitter in ticks; every other field (the coverage multiple included)
/// carries over from the installed config.
fn derived_input_sync(installed: SyncConfig, jitter: Duration, tick: Duration) -> SyncConfig {
    SyncConfig {
        jitter_margin: CONTENT_PHASE_MEAN_TICKS,
        error_margin: in_ticks(jitter, tick),
        ..installed
    }
}

/// The certification pin: the whole fixed margin in one term, the jitter term and the deadband
/// zeroed, so the objective is exactly `rtt/2 + margin + 1 tick` regardless of measured jitter.
fn fixed_input_sync(installed: SyncConfig, margin: Duration, tick: Duration) -> SyncConfig {
    SyncConfig {
        jitter_multiple: 0,
        jitter_margin: in_ticks(margin, tick),
        error_margin: 0.0,
        ..installed
    }
}

/// The downlink law: the uplink's coverage multiple and the half-tick floor over otherwise-default
/// sync behavior. `error_margin` stays default — the interpolation objective never pays it (it
/// shapes only that timeline's speed controller), so shrinking it buys nothing.
fn derived_interp_sync(installed: SyncConfig) -> SyncConfig {
    SyncConfig {
        jitter_multiple: installed.jitter_multiple,
        jitter_margin: CONTENT_PHASE_MEAN_TICKS,
        ..SyncConfig::default()
    }
}

/// The downlink certification pin, same shape as [`fixed_input_sync`].
fn fixed_interp_sync(margin: Duration, tick: Duration) -> SyncConfig {
    SyncConfig {
        jitter_multiple: 0,
        jitter_margin: in_ticks(margin, tick),
        ..SyncConfig::default()
    }
}

fn in_ticks(duration: Duration, tick: Duration) -> f32 {
    (duration.as_secs_f64() / tick.as_secs_f64()) as f32
}

/// Resolve both margin modes, mount the uplink deriving system, and hand back the interpolation
/// config with its sync margins applied (they are static per session — see the module doc). Single
/// call site (`net::client`), so each env var is read exactly once.
pub(super) fn install(
    app: &mut App,
    tick: Duration,
    installed: SyncConfig,
    input_delay: InputDelayConfig,
    mut interpolation: InterpolationConfig,
) -> InterpolationConfig {
    let input_mode = match super::harness::env_parse::<u64>("OVERMATCH_INPUT_MARGIN_MS") {
        Some(ms) => InputMarginMode::Fixed(Duration::from_millis(ms)),
        None => InputMarginMode::Derived,
    };
    match input_mode {
        InputMarginMode::Fixed(margin) => info!(
            "net: input sync margin FIXED {} ms, jitter term off [OVERMATCH_INPUT_MARGIN_MS] — \
             derived law off",
            margin.as_millis()
        ),
        InputMarginMode::Derived => info!(
            "net: input sync margin DERIVED = {CONTENT_PHASE_MEAN_TICKS} tick floor + measured \
             jitter deadband, arming when the own hull rides the server stream \
             [OVERMATCH_INPUT_MARGIN_MS unset]"
        ),
    }
    interpolation.sync = match super::harness::env_parse::<u64>("OVERMATCH_INTERP_MARGIN_MS") {
        Some(ms) => {
            let margin = Duration::from_millis(ms);
            info!(
                "net: interpolation sync margin FIXED {ms} ms, jitter term off \
                 [OVERMATCH_INTERP_MARGIN_MS] — derived margins off"
            );
            fixed_interp_sync(margin, tick)
        }
        None => {
            let sync = derived_interp_sync(installed);
            info!(
                "net: interpolation sync margin DERIVED = {} x jitter + {CONTENT_PHASE_MEAN_TICKS} \
                 x tick [OVERMATCH_INTERP_MARGIN_MS unset]",
                sync.jitter_multiple
            );
            sync
        }
    };
    app.insert_resource(input_mode);
    app.insert_resource(InputSyncBase {
        sync: installed,
        input_delay,
    });
    app.add_systems(PostUpdate, derive_input_margins.before(SyncSystems::Sync));
    interpolation
}

/// Rewrite the input timeline's sync margins ahead of the frame's timeline sync. Derived mode
/// waits for the own tank to resolve `Interpolated` and then latches for the session (the role
/// cannot change mid-session; a respawn gap must not revert the margins). The rebuild reuses the
/// installed `InputDelayConfig` by value and assigns in place — no insert, so lightyear's
/// input-delay recompute observer never runs and the delay stays byte-identical.
fn derive_input_margins(
    mode: Res<InputMarginMode>,
    base: Res<InputSyncBase>,
    tick: Res<TickDuration>,
    own: Query<
        (),
        (
            With<Controlled>,
            With<NetTank>,
            With<Interpolated>,
            Without<ChildOf>,
        ),
    >,
    mut clients: Query<(&mut InputTimelineConfig, &PingManager)>,
    mut armed: Local<bool>,
    mut written: Local<Option<(u8, f32, f32)>>,
    mut logged: Local<Option<Duration>>,
) {
    let fixed = match *mode {
        InputMarginMode::Fixed(margin) => Some(margin),
        InputMarginMode::Derived => None,
    };
    if fixed.is_none() && !*armed {
        if own.is_empty() {
            return;
        }
        *armed = true;
        info!("net: input sync margins ARMED derived — own hull rides the server stream");
    }
    for (mut config, pings) in &mut clients {
        let sync = match fixed {
            Some(margin) => fixed_input_sync(base.sync, margin, tick.0),
            None => derived_input_sync(base.sync, pings.jitter(), tick.0),
        };
        let key = (sync.jitter_multiple, sync.jitter_margin, sync.error_margin);
        if *written == Some(key) {
            continue;
        }
        if fixed.is_none() && logged.is_none_or(|last| last.abs_diff(pings.jitter()) >= LOG_STEP) {
            info!(
                "net: input sync margins floor {:.2} t, deadband {:.2} t (jitter {:.1} ms)",
                sync.jitter_margin,
                sync.error_margin,
                pings.jitter().as_secs_f64() * 1000.0
            );
            *logged = Some(pings.jitter());
        }
        *config = InputTimelineConfig::new(sync, base.input_delay);
        *written = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lightyear::prelude::SyncConfig;

    use super::{derived_input_sync, derived_interp_sync, fixed_input_sync, fixed_interp_sync};

    /// The game's fixed tick (64 Hz), spelled out: these tests pin the arithmetic independently of
    /// any runtime binding.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

    /// The shipped uplink config shape: coverage multiple 2 over otherwise-default fields.
    fn installed() -> SyncConfig {
        SyncConfig {
            jitter_multiple: 2,
            ..SyncConfig::default()
        }
    }

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "{what}: got {actual}, expected {expected}"
        );
    }

    /// The deadband is exactly the measured jitter in ticks. Fails if the deadband is a constant
    /// (any constant), scales by the wrong factor, or reads a term other than jitter.
    #[test]
    fn derived_deadband_is_the_measured_jitter_in_ticks() {
        let zero = derived_input_sync(installed(), Duration::ZERO, TICK);
        assert_close(zero.error_margin, 0.0, "loopback deadband");
        let five = derived_input_sync(installed(), Duration::from_millis(5), TICK);
        assert_close(five.error_margin, 0.32, "5 ms deadband");
        let tickwide = derived_input_sync(installed(), TICK, TICK);
        assert_close(tickwide.error_margin, 1.0, "one-tick-jitter deadband");
    }

    /// The floor is half a tick and does not move with jitter — the mean-phase term is structural,
    /// the tail belongs to the jitter term. Fails if the floor keeps the 1.0 default, absorbs
    /// jitter, or swaps places with the deadband (5 ms is not 7.8125 ms).
    #[test]
    fn derived_floor_is_half_a_tick_independent_of_jitter() {
        for jitter_ms in [0_u64, 5, 40] {
            let sync = derived_input_sync(installed(), Duration::from_millis(jitter_ms), TICK);
            assert_close(sync.jitter_margin, 0.5, "floor");
        }
    }

    /// The coverage multiple and controller bounds carry over from the installed config — the law
    /// re-sizes the two tick constants and nothing else. Fails if the law rebuilds from
    /// `SyncConfig::default()` (multiple 4) or zeroes the multiple.
    #[test]
    fn derived_law_preserves_the_installed_coverage_multiple() {
        let mut base = installed();
        base.jitter_multiple = 7;
        base.max_error_margin = 12.5;
        let sync = derived_input_sync(base, Duration::from_millis(3), TICK);
        assert_eq!(sync.jitter_multiple, 7, "coverage multiple must carry over");
        assert_close(sync.max_error_margin, 12.5, "resync bound must carry over");
    }

    /// The fixed lever pins the WHOLE margin: jitter term off, deadband off, floor = the pinned
    /// duration in ticks. Fails if any term stays live or the ms→tick conversion drifts.
    #[test]
    fn fixed_lever_pins_the_whole_margin() {
        let sync = fixed_input_sync(installed(), Duration::from_micros(7_812), TICK);
        assert_eq!(sync.jitter_multiple, 0, "jitter term must be off");
        assert_close(sync.error_margin, 0.0, "deadband must be off");
        assert_close(sync.jitter_margin, 0.49997, "margin in ticks");
        let two_ticks = fixed_input_sync(installed(), Duration::from_micros(31_250), TICK);
        assert_close(two_ticks.jitter_margin, 2.0, "31.25 ms is two ticks");
    }

    /// The downlink margins: the uplink's coverage multiple, the half-tick floor, and a default
    /// deadband (the interpolation objective never pays it). Fails if the multiple stays at
    /// lightyear's 4, the floor keeps 1.0, or the deadband is dragged along.
    #[test]
    fn interp_margins_take_the_uplink_multiple_and_the_half_tick_floor() {
        let sync = derived_interp_sync(installed());
        assert_eq!(sync.jitter_multiple, 2, "uplink coverage multiple");
        assert_close(sync.jitter_margin, 0.5, "floor");
        assert_close(
            sync.error_margin,
            SyncConfig::default().error_margin,
            "interp deadband stays default",
        );
        let fixed = fixed_interp_sync(Duration::from_micros(15_625), TICK);
        assert_eq!(fixed.jitter_multiple, 0, "pinned margin has no jitter term");
        assert_close(fixed.jitter_margin, 1.0, "15.625 ms is one tick");
    }
}
