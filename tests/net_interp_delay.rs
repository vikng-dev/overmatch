//! UPSTREAM TRIPWIRE for the interpolation-delay degenerate that `src/net/client.rs`'s
//! `InterpolationConfig { min_delay: 100ms }` insert compensates for (the 2026-07-11 remote-tank
//! "teleports along the driving path" fix).
//!
//! The mechanism being pinned, in lightyear 0.28 (`lightyear_interpolation` timeline.rs):
//! interpolated remotes render at `I = server_estimate − (delay + jitter_margin)` with
//! `delay = max(remote_send_interval · send_interval_ratio, min_delay)`. Our server replicates
//! every tick and therefore advertises `send_interval = 0` (`ReplicationMetadata` default), which
//! KILLS the ratio term — an acknowledged upstream hole (`// TODO: deal with
//! server_send_interval = 0 (set to frame rate)` in `to_duration`; issues cBournhonesque/lightyear
//! #890 and #423). The delay collapses to the 5 ms `min_delay` default, while the server estimate
//! sits RTT/2 AHEAD of the newest received keyframe — so on any real link the interpolation clock
//! overruns the keyframe buffer and lightyear clamps (freeze, then step). Our fix pins
//! `min_delay = 100 ms` client-side, sized to droplet-range RTT.
//!
//! WHAT FIRES WHEN: these tests FAIL when a lightyear upgrade changes the degenerate — the TODO
//! implemented (ratio falling back to tick/frame rate), `InterpolationConfig` defaults changed
//! (#890), or the objective re-anchored (e.g. to newest-received instead of server-estimate,
//! which would make the delay RTT-independent). A failure here is NOT a regression — it is the
//! signal to re-derive the `min_delay` sizing in `src/net/client.rs` (possibly shrinking or
//! deleting the insert) and to revisit the parked upstream filing
//! (`.agents/scratch/wave-a-adoption-memo.md`, "lightyear interpolation delay" section), which
//! this degenerate is the evidence for.
//!
//! Direct `lightyear_sync`/`lightyear_core` dev-dependencies (same locked 0.28.0 the facade
//! uses): the `SyncedTimeline`/`SyncTargetTimeline` traits and the fixed-point time types are not
//! re-exported by the `lightyear` facade, and `sync_objective` is the honest observable — it is
//! the exact function the sync systems call each frame to place the interpolation clock.

use core::time::Duration;

use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::PingManager;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncTargetTimeline, SyncedTimeline};

/// The game's fixed tick (64 Hz), matching `ClientPlugins { tick_duration }` in `net::client`.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

/// The upstream defaults our fix overrides. If these move (issue #890's likely fix shape),
/// the sizing rationale in `src/net/client.rs` must be re-derived against the new baseline.
#[test]
fn upstream_interpolation_config_defaults_unchanged() {
    let config = InterpolationConfig::default();
    assert_eq!(
        config.min_delay,
        Duration::from_millis(5),
        "lightyear changed InterpolationConfig::default().min_delay — re-derive the min_delay \
         sizing in src/net/client.rs and revisit the parked upstream filing (see module doc)"
    );
    assert!(
        (config.send_interval_ratio - 1.7).abs() < 1e-6,
        "lightyear changed InterpolationConfig::default().send_interval_ratio (was 1.7, now {}) \
         — re-derive the min_delay sizing in src/net/client.rs (see module doc)",
        config.send_interval_ratio
    );
}

/// The degenerate itself: with `remote_send_interval = 0` (what our per-tick server advertises),
/// a default-config interpolation clock is placed only `min_delay + 1 tick` = ~20.6 ms behind the
/// SERVER ESTIMATE — which is RTT/2 ahead of the newest real keyframe, i.e. negative headroom on
/// any link with RTT ≳ 41 ms. Pins the exact objective lightyear computes today; fires if the
/// `send_interval = 0` TODO is ever implemented or the anchoring changes.
#[test]
fn send_interval_zero_degenerate_still_collapses_to_min_delay() {
    // Defaults throughout = our production shape minus the fix: a fresh timeline carries
    // `remote_send_interval = 0` (no SenderMetadata received; identical to a server advertising
    // interval 0), and a fresh PingManager reports zero jitter, so the objective reduces to
    // `estimate − (min_delay + tick_duration · jitter_margin)` with jitter_margin = 1.0.
    let timeline = InterpolationTimeline::default();
    let remote = RemoteTimeline::default();
    let pings = PingManager::default();
    let config = InterpolationConfig::default();

    let objective = timeline.sync_objective(&remote, &config, &pings, TICK);
    let lag = (remote.current_estimate() - objective).to_duration(TICK);

    // 5 ms (min_delay, the collapsed delay term) + 15.625 ms (the 1-tick jitter_margin floor).
    // Fixed-point overstep quantization makes this approximate; ±1 ms is far tighter than any
    // meaningful upstream change (the TODO's own fallback would land at ≥ tick·1.7 ≈ 26.6 ms).
    let expected = Duration::from_micros(5_000 + 15_625);
    let error = lag.abs_diff(expected);
    assert!(
        error < Duration::from_millis(1),
        "lightyear's send_interval=0 interpolation objective moved: clock now sits {lag:?} \
         behind the server estimate (this pin expects {expected:?}). The upstream degenerate \
         (timeline.rs `TODO: deal with server_send_interval = 0`) has likely been fixed — \
         re-derive the min_delay=100ms sizing in src/net/client.rs and revisit the parked \
         upstream filing (see module doc)"
    );
}
