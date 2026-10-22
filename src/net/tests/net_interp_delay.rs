//! UPSTREAM TRIPWIRE for the interpolation-delay degenerate that `src/net/interp_delay.rs`'s
//! derived `min_delay` compensates for (the 2026-07-11 remote-tank "teleports along the driving
//! path" fix; the derived law replaced the 100 ms pin on 2026-08-14).
//!
//! The mechanism being pinned, in lightyear 0.30 (`lightyear_interpolation` timeline.rs):
//! interpolated remotes render at `I = server_estimate − (delay + jitter_margin)` with
//! `delay = max(remote_send_interval · send_interval_ratio, min_delay)`. Our server replicates
//! every tick and therefore advertises `send_interval = 0` (`ReplicationMetadata` default), which
//! KILLS the ratio term — an acknowledged upstream hole (`// TODO: deal with
//! server_send_interval = 0 (set to frame rate)` in `to_duration`; issues cBournhonesque/lightyear
//! #890 — closed 2026-08-03 without a fix — and #423). The delay collapses to the 5 ms `min_delay`
//! default, while the server estimate sits RTT/2 AHEAD of the newest received keyframe — so on any
//! real link the interpolation clock overruns the keyframe buffer and lightyear clamps (freeze,
//! then step). Our fix writes `min_delay = rtt/2 + (arrival spread beyond the extrapolation
//! horizon) + one tick` client-side every frame; `net::interp_delay`'s unit tests pin the
//! arithmetic. The law's gap term is a whole tick of its own and does not read
//! `send_interval_ratio` — at `send_interval = 0` the ratio multiplies zero.
//!
//! WHAT FIRES WHEN: the test below FAILS when a lightyear upgrade changes the degenerate — the TODO
//! implemented (ratio falling back to tick/frame rate), the `InterpolationConfig::default()`
//! `min_delay` changed (#890), or the objective re-anchored (e.g. to newest-received instead of
//! server-estimate, which would make the delay RTT-independent and retire the `rtt/2` term). A
//! failure here is NOT a regression — it is the signal to re-derive the LAW in
//! `src/net/interp_delay.rs` (derivation:
//! `.agents/scratch/interp-delay-derivation-2026-08-14.md`) and to revisit the parked upstream
//! filing (`git show 90ba478^:.agents/scratch/wave-a-adoption-memo.md`, "lightyear interpolation
//! delay" section),
//! which this degenerate is the evidence for.
//!
//! Direct `lightyear_sync`/`lightyear_core` dev-dependencies (same locked version the facade
//! uses): `SyncTargetTimeline` and `RemoteTimeline` are not re-exported by the `lightyear` facade,
//! and `TimelineSync::sync_objective` is the honest observable — it is the exact function the sync
//! systems call each frame to place the interpolation clock.

use core::time::Duration;

use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::PingManager;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncTargetTimeline, TimelineSync};

/// The game's fixed tick (64 Hz), matching `ClientPlugins { tick_duration }` in `net::client`.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

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
         re-derive the min_delay law in src/net/interp_delay.rs and revisit the parked \
         upstream filing (see module doc)"
    );
}
