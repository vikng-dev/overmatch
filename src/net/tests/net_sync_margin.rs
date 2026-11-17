//! UPSTREAM TRIPWIRES for the sync-margin structure that `src/net/sync_margin.rs`'s derived laws
//! re-size (companion to `src/net/tests/net_interp_delay.rs`, which pins the `min_delay` degenerate
//! the other half of the downlink law compensates for).
//!
//! The mechanisms being pinned, in lightyear 0.31.0:
//!
//! - the INPUT timeline objective (`lightyear_sync` input.rs): `remote + rtt/2 + (jitter ·
//!   jitter_multiple + tick · jitter_margin) + 1 tick + tick · error_margin − input_delay`. The
//!   derived uplink law writes `jitter_margin` (the quantization floor) and `error_margin` (the
//!   deadband, sized to the measured jitter) and relies on exactly this term placement: the floor
//!   is paid once, the deadband is paid once AND bounds the speed controller's dead zone.
//! - `SyncConfig::jitter_margin` (`lightyear_sync` sync.rs): `jitter · jitter_multiple + tick ·
//!   jitter_margin` — the composition both timelines share.
//! - the INTERPOLATION timeline objective (`lightyear_interpolation` timeline.rs): `estimate −
//!   (delay + jitter_margin(..))` — the downlink law's floor lands through the same composition.
//! - the `SyncConfig` defaults, which are the baseline every saving in
//!   `.agents/scratch/input-lead-budget-2026-08-14.md` is stated against, and which the
//!   interpolation timeline ran until `net::sync_margin` landed.
//!
//! WHAT FIRES WHEN: these tests FAIL when a lightyear upgrade changes the margin structure — a
//! term added or dropped from either objective, the deadband decoupled from the objective, or the
//! defaults moved. A failure here is NOT a regression — it is the signal to re-derive the laws in
//! `src/net/sync_margin.rs` against the new baseline (derivation:
//! `.agents/scratch/input-lead-budget-2026-08-14.md`).
//!
//! Direct `lightyear_sync`/`lightyear_core` dev-dependencies for the same reason as
//! `src/net/tests/net_interp_delay.rs`: `sync_objective` is the honest observable — the exact
//! function the sync systems call each frame.

use core::time::Duration;

use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::{PingManager, SyncConfig};
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncTargetTimeline, TimelineSync};

/// The game's fixed tick (64 Hz), matching `ClientPlugins { tick_duration }` in `net::client`.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

/// The upstream defaults the derived laws replace at runtime — and the baseline their savings are
/// stated against. If these move, the budget arithmetic in the derivation is stale and the laws
/// must be re-derived against the new baseline.
#[test]
fn upstream_sync_config_defaults_unchanged() {
    let config = SyncConfig::default();
    assert_eq!(
        config.jitter_multiple, 4,
        "lightyear changed SyncConfig::default().jitter_multiple — re-derive src/net/sync_margin.rs \
         (see module doc)"
    );
    assert!(
        (config.jitter_margin - 1.0).abs() < 1e-6,
        "lightyear changed SyncConfig::default().jitter_margin (was 1.0, now {}) — the floor the \
         derived laws replace moved; re-derive src/net/sync_margin.rs",
        config.jitter_margin
    );
    assert!(
        (config.error_margin - 1.0).abs() < 1e-6,
        "lightyear changed SyncConfig::default().error_margin (was 1.0, now {}) — the deadband the \
         uplink law replaces moved; re-derive src/net/sync_margin.rs",
        config.error_margin
    );
    assert!(
        (config.max_error_margin - 10.0).abs() < 1e-6,
        "lightyear changed SyncConfig::default().max_error_margin (was 10.0, now {}) — the resync \
         snap bound the laws lean on moved; re-derive src/net/sync_margin.rs",
        config.max_error_margin
    );
}

/// The shared margin composition: measured jitter scaled by the multiple, plus the fractional-tick
/// floor. Both laws write their floor through this function, so a change to its form retires them.
#[test]
fn jitter_margin_composes_multiple_and_floor() {
    let config = SyncConfig {
        jitter_multiple: 3,
        jitter_margin: 0.5,
        ..SyncConfig::default()
    };
    let margin = config.jitter_margin(Duration::from_millis(4), TICK);
    // 3 x 4 ms + 0.5 x 15.625 ms.
    let expected = Duration::from_micros(12_000 + 7_812);
    assert!(
        margin.abs_diff(expected) < Duration::from_micros(10),
        "lightyear's SyncConfig::jitter_margin composition moved: {margin:?}, expected \
         {expected:?} — re-derive src/net/sync_margin.rs"
    );
}

/// The interpolation objective honors the sync floor on top of the delay term: with
/// `remote_send_interval = 0` (our per-tick server) and zero jitter, the cursor sits exactly
/// `min_delay + floor · tick` behind the estimate. Fires if the downlink margin stops flowing
/// through the objective — which would strand the law's saving claim.
#[test]
fn interp_objective_honors_the_sync_floor() {
    let timeline = InterpolationTimeline::default();
    let remote = RemoteTimeline::default();
    let pings = PingManager::default();
    let config = InterpolationConfig {
        min_delay: Duration::from_millis(30),
        sync: SyncConfig {
            jitter_multiple: 2,
            jitter_margin: 0.5,
            ..SyncConfig::default()
        },
        ..InterpolationConfig::default()
    };

    let objective = timeline.sync_objective(&remote, &config, &pings, TICK);
    let lag = (remote.current_estimate() - objective).to_duration(TICK);
    // 30 ms (min_delay) + 0.5 x 15.625 ms (the floor).
    let expected = Duration::from_micros(30_000 + 7_812);
    assert!(
        lag.abs_diff(expected) < Duration::from_millis(1),
        "lightyear's interpolation objective moved: cursor sits {lag:?} behind the estimate, \
         expected {expected:?} — re-derive the downlink margins in src/net/sync_margin.rs"
    );
}
