//! The sync margins on both wire timelines, derived from the STREAM-MEASURED arrival-delay
//! distribution rather than from the ping EWMA.
//!
//! # The estimator (NetEQ's relative-arrival-delay form)
//!
//! Every received transport packet is tick-stamped by its sender (`PacketReceived { remote_tick }`,
//! `lightyear_transport` plugin.rs) — 64 samples/s of the actual replication stream, where the ping
//! path samples 10/s, assumes a symmetric Gaussian, and CLAMPS outliers (`lightyear` ping
//! estimator.rs) — the exact samples a dejitter sizer must not discard. Per packet the estimator
//! records the relative arrival delay `d_i = arrival_instant − remote_tick × tick` (epoch-anchored;
//! only differences of `d_i` carry meaning) and maintains:
//!
//! - `min{d_i}` over the [`SPIKE_WINDOW`] — the fastest recent packet, the anchor that absorbs
//!   path shifts and clock skew (NetEQ's ~2 s spike window precedent; tracks the measured 0.52 s
//!   burst period with margin);
//! - an eviction ring of [`ring_cap`] samples exposing the empirical quantile `Q_p` and the median.
//!
//! Relative-arrival delay, not inter-arrival: an EWMA of gaps reads a slowly accumulating delay as
//! "each gap normal" while the buffer starves (the NetEQ migration rationale; a test below pins
//! that failure case).
//!
//! # Contract constants (SLOs, not tunings)
//!
//! ```text
//! B         = 60 freezes/hour            certified freeze budget: one perceptible starvation
//!                                        event per minute, ceiling (initial product contract)
//! p         = 1 − B/(64 × 3600)          the covered fraction of per-tick arrivals = 1 − 1/3840
//! ring_cap  = 1/(1 − p) = 3840           the minimum sample count that resolves tail mass 1 − p;
//!                                        forgetting is by eviction (60 s at 64 Hz), so Q_p at
//!                                        capacity is the window maximum by construction
//! T         = 2 s                        min-anchor spike window (NetEQ precedent)
//! ```
//!
//! # Downlink (the interpolation timeline)
//!
//! `net::interp_delay` writes the delay law `min_delay = rtt/2 + (Q_p − min) + one send interval`
//! from this estimator every frame. The Gaussian margin chain on top is therefore ZEROED here
//! (`jitter_multiple = 0`) — the quantile subsumes it — keeping the structural half-tick floor
//! (per-tick keyframes put the cursor's phase against arrival uniform over one interval; the floor
//! carries that distribution's mean).
//!
//! # Uplink (the input timeline)
//!
//! The uplink cannot measure its own arrival at the client, so its margin is the downlink
//! distribution's spread — the SYMMETRY ASSUMPTION, stated: the measured burst structure (60–70 ms
//! every ~0.52 s, sessions of 2026-08-15) was observed in BOTH directions, and the server's
//! `INPUT-ARRIVAL` instrument (`net::diagnostics`) is the live certifier of this derivation.
//! The objective's terms:
//!
//! - `jitter_margin` (coverage) = the half-tick content-phase floor + `(Q_p − Q_50)/tick` — the
//!   burst tail above the median;
//! - `error_margin` (deadband) = `(Q_50 − min)/tick` — a deadband need only exceed the error
//!   signal's own noise, whose scale is the bulk spread; the tail belongs to coverage;
//! - `jitter_multiple` = 0 — the ping EWMA leaves the law entirely.
//!
//! Together the objective pays the floor plus exactly the measured spread `Q_p − min`, split at the
//! median between coverage and deadband.
//!
//! # Levers
//!
//! Mirroring `OVERMATCH_INTERP_DELAY_MS`: `OVERMATCH_INPUT_MARGIN_MS` and
//! `OVERMATCH_INTERP_MARGIN_MS` SET pin that timeline's whole fixed margin (jitter multiple zeroed,
//! deadband zeroed) — the certification instrument for deliberately-undersized runs; UNSET runs
//! the derived law. A pinned uplink margin applies regardless of drive mode.
//!
//! Research basis: `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §1;
//! uplink failure-mode inventory: `.agents/scratch/input-lead-budget-2026-08-14.md`.

use core::time::Duration;
use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::interpolation::timeline::InterpolationConfig;
use lightyear::prelude::client::InputDelayConfig;
use lightyear::prelude::{InputTimelineConfig, Interpolated, SyncConfig, SyncSystems};
use lightyear_transport::plugin::PacketReceived;

use super::protocol::NetTank;
use crate::tank::Controlled;

/// Half of one content interval, in ticks — the mean of the uniform phase between per-tick content
/// (authored inputs up, replicated keyframes down) and the consuming clock's boundary. A term of
/// structure, not a tuned number: it moves only if content stops advancing once per tick.
const CONTENT_PHASE_MEAN_TICKS: f32 = 0.5;

/// The certified freeze budget B: one perceptible starvation event per minute, ceiling. The
/// initial product contract behind the quantile; every other estimator constant derives from it.
const FREEZE_BUDGET_PER_HOUR: f64 = 60.0;

/// The fixed tick rate the budget is stated against (`ClientPlugins { tick_duration }`).
const TICK_RATE_HZ: f64 = 64.0;

/// `p = 1 − B/(64 × 3600)`: the fraction of per-tick arrivals the delay target must cover so that
/// at most B starvation events/hour escape it.
fn quantile_p() -> f64 {
    1.0 - FREEZE_BUDGET_PER_HOUR / (TICK_RATE_HZ * 3600.0)
}

/// Ring capacity `1/(1 − p)` = 3840: the minimum sample count that resolves tail mass `1 − p`.
/// Retention at 64 Hz is 60 s; forgetting is by eviction, so a one-off spike ages out in exactly
/// the budget's per-minute granularity.
fn ring_cap() -> usize {
    // `1/(1−p)` written from the budget directly: the subtraction form re-rounds an exact ratio.
    (TICK_RATE_HZ * 3600.0 / FREEZE_BUDGET_PER_HOUR).ceil() as usize
}

/// The min-anchor spike window T (NetEQ's ~2 s precedent): long enough to hold the fastest packet
/// across the measured 0.52 s burst period, short enough that a path shift or clock skew re-anchors
/// within seconds.
const SPIKE_WINDOW: Duration = Duration::from_secs(2);

/// Log-rate guard, not a term of the law: the margins follow the estimator continuously, so an
/// unconditional log is per-frame spam.
const LOG_STEP: Duration = Duration::from_millis(2);

/// The estimator's per-frame digest, refreshed by [`refresh_arrival_stats`] ahead of both margin
/// laws. Values are epoch-relative seconds — only their differences carry meaning.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct ArrivalStats {
    min_s: f64,
    q50_s: f64,
    qp_s: f64,
    pub(super) samples: u64,
}

impl ArrivalStats {
    /// `Q_p − min`: the whole measured spread — the downlink headroom term.
    pub(super) fn spread(&self) -> Duration {
        Duration::from_secs_f64((self.qp_s - self.min_s).max(0.0))
    }

    /// `Q_p − Q_50`: the burst tail above the median — the uplink coverage term, and the
    /// announcement-vs-state skew bound `net::fire_presentation`'s recovery wait reads.
    pub(super) fn coverage(&self) -> Duration {
        Duration::from_secs_f64((self.qp_s - self.q50_s).max(0.0))
    }

    /// `Q_50 − min`: the bulk spread — the error signal's own noise scale, the uplink deadband.
    /// Clamped at zero: during a worsening path shift the 2 s min outruns the 60 s median and the
    /// bulk term collapses while the coverage term carries the shift.
    fn bulk(&self) -> Duration {
        Duration::from_secs_f64((self.q50_s - self.min_s).max(0.0))
    }

    /// A digest with a known `Q_p − min`, for the law tests in sibling modules.
    #[cfg(test)]
    pub(super) fn test_spread(spread_s: f64) -> Self {
        Self {
            min_s: 0.0,
            q50_s: 0.0,
            qp_s: spread_s,
            samples: 1,
        }
    }
}

/// The stream-measured arrival-delay estimator. Fed by [`record_packet_arrival`] per received
/// packet; digested once per frame into [`ArrivalStats`].
#[derive(Resource, Debug, Default)]
pub(super) struct ArrivalDelay {
    /// First-sample anchor `(arrival_secs, remote_tick)`; every `d_i` is relative to it, so tick
    /// wrap and the meaningless absolute offset both cancel.
    epoch: Option<(f64, Tick)>,
    /// `(arrival_secs, d_i)` pruned to [`SPIKE_WINDOW`] — the min anchor.
    window: VecDeque<(f64, f64)>,
    /// `d_i` eviction ring of [`ring_cap`] samples — the quantile distribution.
    ring: VecDeque<f64>,
    /// Newest remote tick seen on any packet (wrapping max) — the "server has simulated through
    /// here" bound `net::fire_presentation`'s heal deadlines read.
    newest_remote: Option<Tick>,
    samples: u64,
    /// The per-frame digest, refreshed by [`refresh_arrival_stats`].
    pub(super) stats: ArrivalStats,
}

impl ArrivalDelay {
    fn record(&mut self, arrival_secs: f64, remote_tick: Tick, tick_secs: f64) {
        let (epoch_secs, epoch_tick) = *self.epoch.get_or_insert((arrival_secs, remote_tick));
        let d = (arrival_secs - epoch_secs) - f64::from(remote_tick - epoch_tick) * tick_secs;
        self.window.push_back((arrival_secs, d));
        let horizon = arrival_secs - SPIKE_WINDOW.as_secs_f64();
        while self.window.front().is_some_and(|(at, _)| *at < horizon) {
            self.window.pop_front();
        }
        self.ring.push_back(d);
        if self.ring.len() > ring_cap() {
            self.ring.pop_front();
        }
        if self
            .newest_remote
            .is_none_or(|newest| remote_tick - newest > 0)
        {
            self.newest_remote = Some(remote_tick);
        }
        self.samples += 1;
    }

    /// Recompute the digest. `Q_p` is the smallest sample with cumulative mass ≥ p (at ring
    /// capacity `1/(1−p)` that is the window maximum by construction); `min` anchors on the spike
    /// window.
    fn refresh(&mut self, scratch: &mut Vec<f64>) {
        if self.ring.is_empty() {
            self.stats = ArrivalStats::default();
            return;
        }
        let min_s = self
            .window
            .iter()
            .map(|(_, d)| *d)
            .fold(f64::INFINITY, f64::min);
        scratch.clear();
        scratch.extend(self.ring.iter().copied());
        let n = scratch.len();
        let index = |p: f64| ((p * n as f64).ceil() as usize).clamp(1, n) - 1;
        let i50 = index(0.5);
        let (_, q50, _) = scratch.select_nth_unstable_by(i50, f64::total_cmp);
        let q50_s = *q50;
        let ip = index(quantile_p());
        let (_, qp, _) = scratch.select_nth_unstable_by(ip, f64::total_cmp);
        let qp_s = *qp;
        self.stats = ArrivalStats {
            min_s,
            q50_s,
            qp_s,
            samples: self.samples,
        };
    }

    /// Newest remote tick seen on any packet — the "the server has simulated through here" bound
    /// `net::fire_presentation`'s reveal stamps and heal deadlines read.
    pub(super) fn newest_remote(&self) -> Option<Tick> {
        self.newest_remote
    }

    /// A digest with a known newest remote tick, for sibling-module deadline tests.
    #[cfg(test)]
    pub(super) fn test_with_newest_remote(tick: Tick) -> Self {
        Self {
            newest_remote: Some(tick),
            ..Self::default()
        }
    }

    /// One line for the FRONTIER summary: the anchored spreads in ms and the sample count.
    pub(super) fn describe(&self) -> String {
        let stats = &self.stats;
        format!(
            "arrival(q50-min={:.1}ms qp-min={:.1}ms n={})",
            stats.bulk().as_secs_f64() * 1000.0,
            stats.spread().as_secs_f64() * 1000.0,
            stats.samples,
        )
    }
}

/// Feed the estimator from every received transport packet — the tick-stamped arrival stream the
/// ping estimator never reads.
fn record_packet_arrival(
    event: On<PacketReceived>,
    time: Res<Time<Real>>,
    tick: Res<TickDuration>,
    mut estimator: ResMut<ArrivalDelay>,
) {
    estimator.record(
        time.elapsed_secs_f64(),
        event.remote_tick,
        tick.0.as_secs_f64(),
    );
}

/// Digest the estimator once per frame, ahead of both margin laws and the delay law.
pub(super) fn refresh_arrival_stats(
    mut estimator: ResMut<ArrivalDelay>,
    mut scratch: Local<Vec<f64>>,
) {
    estimator.refresh(&mut scratch);
}

/// `OVERMATCH_INPUT_MARGIN_MS`: SET pins the input timeline's fixed margin for the session; UNSET
/// runs the derived law once the own tank's role resolves interpolated.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
enum InputMarginMode {
    Fixed(Duration),
    Derived,
}

/// What the per-frame rewrite rebuilds `InputTimelineConfig` from: the sync config `net::client`
/// installed (controller bounds live there) and the SAME input-delay config it installed —
/// carried whole so the rebuild cannot change the delay's shape.
#[derive(Resource, Debug, Clone, Copy)]
struct InputSyncBase {
    sync: SyncConfig,
    input_delay: InputDelayConfig,
}

/// The uplink law: coverage = floor + the measured burst tail, deadband = the measured bulk
/// spread, ping term off. Controller bounds carry over from the installed config.
fn derived_input_sync(installed: SyncConfig, stats: &ArrivalStats, tick: Duration) -> SyncConfig {
    SyncConfig {
        jitter_multiple: 0,
        jitter_margin: CONTENT_PHASE_MEAN_TICKS + in_ticks(stats.coverage(), tick),
        error_margin: in_ticks(stats.bulk(), tick),
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

/// The downlink sync margins: the Gaussian chain zeroed (the quantile law in `net::interp_delay`
/// subsumes it) over the half-tick floor. `error_margin` stays default — the interpolation
/// objective never pays it (it shapes only that timeline's speed controller), so shrinking it buys
/// nothing.
fn derived_interp_sync() -> SyncConfig {
    SyncConfig {
        jitter_multiple: 0,
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

/// Resolve both margin modes, mount the estimator and the uplink deriving system, and hand back
/// the interpolation config with its sync margins applied (static per session — the live factor,
/// the quantile spread, reaches the downlink through `net::interp_delay`'s per-frame `min_delay`
/// write instead). Single call site (`net::client`), so each env var is read exactly once.
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
            "net: input sync margin DERIVED = {CONTENT_PHASE_MEAN_TICKS} tick floor + \
             stream-measured (Qp−Q50) coverage + (Q50−min) deadband, arming when the own hull \
             rides the server stream [OVERMATCH_INPUT_MARGIN_MS unset]"
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
            info!(
                "net: interpolation sync margin DERIVED = {CONTENT_PHASE_MEAN_TICKS} x tick \
                 floor, jitter chain 0 (the arrival-delay quantile in min_delay subsumes it) \
                 [OVERMATCH_INTERP_MARGIN_MS unset]"
            );
            derived_interp_sync()
        }
    };
    app.init_resource::<ArrivalDelay>();
    app.add_observer(record_packet_arrival);
    app.insert_resource(input_mode);
    app.insert_resource(InputSyncBase {
        sync: installed,
        input_delay,
    });
    app.add_systems(
        PostUpdate,
        (refresh_arrival_stats, derive_input_margins)
            .chain()
            .before(SyncSystems::Sync)
            .before(super::interp_delay::derive_interpolation_delay),
    );
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
    estimator: Res<ArrivalDelay>,
    own: Query<
        (),
        (
            With<Controlled>,
            With<NetTank>,
            With<Interpolated>,
            Without<ChildOf>,
        ),
    >,
    mut clients: Query<&mut InputTimelineConfig>,
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
    for mut config in &mut clients {
        let sync = match fixed {
            Some(margin) => fixed_input_sync(base.sync, margin, tick.0),
            None => derived_input_sync(base.sync, &estimator.stats, tick.0),
        };
        let key = (sync.jitter_multiple, sync.jitter_margin, sync.error_margin);
        if *written == Some(key) {
            continue;
        }
        let coverage = estimator.stats.coverage();
        if fixed.is_none() && logged.is_none_or(|last| last.abs_diff(coverage) >= LOG_STEP) {
            info!(
                "net: input sync margins floor+coverage {:.2} t, deadband {:.2} t ({})",
                sync.jitter_margin,
                sync.error_margin,
                estimator.describe(),
            );
            *logged = Some(coverage);
        }
        *config = InputTimelineConfig::new(sync, base.input_delay);
        *written = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use lightyear::core::tick::Tick;
    use lightyear::prelude::SyncConfig;

    use super::{
        ArrivalDelay, ArrivalStats, derived_input_sync, derived_interp_sync, fixed_input_sync,
        fixed_interp_sync, quantile_p, ring_cap,
    };

    /// The game's fixed tick (64 Hz), spelled out: these tests pin the arithmetic independently of
    /// any runtime binding.
    const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);
    const TICK_SECS: f64 = 1.0 / 64.0;

    /// The shipped uplink config shape: controller bounds off defaults, so carry-over is visible.
    fn installed() -> SyncConfig {
        SyncConfig {
            jitter_multiple: 2,
            max_error_margin: 12.5,
            ..SyncConfig::default()
        }
    }

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "{what}: got {actual}, expected {expected}"
        );
    }

    fn stats(min_ms: f64, q50_ms: f64, qp_ms: f64) -> ArrivalStats {
        ArrivalStats {
            min_s: min_ms / 1000.0,
            q50_s: q50_ms / 1000.0,
            qp_s: qp_ms / 1000.0,
            samples: 1,
        }
    }

    /// THE CONTRACT CONSTANTS ARE THE BUDGET INVERTED, recomputed from B = 60 and 64 Hz
    /// independently of the production expressions. Moving the budget, the rate, or the
    /// ring-capacity derivation reds one of these.
    #[test]
    fn the_quantile_contract_derives_from_the_freeze_budget() {
        let p = 1.0 - 60.0 / (64.0 * 3600.0);
        assert!((quantile_p() - p).abs() < 1e-12, "p = 1 − B/(64·3600)");
        assert_eq!(ring_cap(), 3840, "ring capacity = 1/(1−p)");
        assert!(
            (ring_cap() as f64 - 1.0 / (1.0 - quantile_p())).abs() < 1.0,
            "capacity is the tail-resolution bound, not an independent number"
        );
    }

    /// Drive a synthetic arrival series through the estimator: ticks at exact cadence, one sample
    /// per tick with a KNOWN delay pattern, then refresh and read the digest.
    fn fed(delays_ms: impl IntoIterator<Item = f64>) -> ArrivalDelay {
        let mut estimator = ArrivalDelay::default();
        for (i, delay_ms) in delays_ms.into_iter().enumerate() {
            let tick = Tick(1_000 + i as u32);
            let arrival = i as f64 * TICK_SECS + delay_ms / 1000.0;
            estimator.record(arrival, tick, TICK_SECS);
        }
        estimator.refresh(&mut Vec::new());
        estimator
    }

    /// A KNOWN QUANTILE: 100 samples of 0 ms and 3 of 50 ms — the 50 ms tail is 3/103 ≈ 2.9 % of
    /// mass, far above 1 − p, so Q_p must sit in the tail while Q_50 and min sit in the bulk.
    /// An estimator that averages, clamps outliers, or reads inter-arrival gaps reds this.
    #[test]
    fn the_quantile_reads_the_tail_of_a_known_series() {
        let delays = (0..103).map(|i| if i % 34 == 33 { 50.0 } else { 0.0 });
        let estimator = fed(delays);
        let spread_ms = estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            (spread_ms - 50.0).abs() < 1.0,
            "Q_p − min must be the 50 ms tail (got {spread_ms})"
        );
        let bulk_ms = estimator.stats.bulk().as_secs_f64() * 1000.0;
        assert!(bulk_ms < 1.0, "the median sits in the bulk (got {bulk_ms})");
    }

    /// THE MEASURED BURST SIGNATURE (60 ms every 33 ticks) IS COVERED WITHIN ONE SEND INTERVAL:
    /// the spike mass (1/33 ≈ 3 %) dwarfs 1 − p, so the target must clear the burst depth. A
    /// Gaussian-multiple margin over a clamped jitter EWMA (which reads this stream as ~3 ms
    /// jitter) is exactly what reds here.
    #[test]
    fn the_measured_burst_train_is_covered_by_the_target() {
        let delays = (0..640).map(|i| if i % 33 == 0 { 60.0 } else { 0.0 });
        let estimator = fed(delays);
        let spread = estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            spread >= 60.0 - 1e-6 && spread <= 60.0 + TICK.as_secs_f64() * 1000.0,
            "the target must cover the 60 ms burst within one send interval (got {spread} ms)"
        );
    }

    /// THE NETEQ SLOW-RAMP CASE: delay accumulating 1 ms per packet. Every inter-arrival gap reads
    /// as a constant ~1 ms step (an inter-arrival EWMA sees nothing), but the relative-arrival
    /// form must report a spread of at least the ramp across the min window. The estimator class
    /// this law replaced reds here.
    #[test]
    fn a_slow_ramp_is_visible_to_the_relative_arrival_form() {
        let estimator = fed((0..640).map(|i| i as f64));
        let spread = estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            spread >= 100.0,
            "1 ms/packet accumulation over the 2 s min window must show ≥ ~128 ms of spread \
             (got {spread} ms)"
        );
    }

    /// THE MIN ANCHOR FORGETS ON THE SPIKE WINDOW: after a path improvement, the min re-anchors
    /// within T while the quantile ring holds the old tail (the safe direction). Anchoring min on
    /// the full ring reds this.
    #[test]
    fn the_min_anchor_rides_the_spike_window() {
        // 5 s of 80 ms baseline, then 4 s at 0 ms: the last 2 s window holds only 0 ms samples.
        let delays = (0..576).map(|i| if i < 320 { 80.0 } else { 0.0 });
        let estimator = fed(delays);
        let bulk = estimator.stats.bulk().as_secs_f64() * 1000.0;
        assert!(
            bulk >= 79.0,
            "the ring keeps the old bulk while the min re-anchors — Q50−min ≈ 80 ms (got {bulk})"
        );
    }

    /// THE UPLINK LAW SPLITS THE MEASURED SPREAD AT THE MEDIAN: coverage = floor + (Qp−Q50)/tick,
    /// deadband = (Q50−min)/tick, ping term off, controller bounds carried over. Fails if either
    /// term reads the wrong spread, the floor is dropped or absorbed, the ping multiple survives,
    /// or the law rebuilds from `SyncConfig::default()`.
    #[test]
    fn the_uplink_law_splits_the_spread_at_the_median() {
        // min 0, Q50 = 7.8125 ms (0.5 t), Qp = 23.4375 ms (1.5 t): coverage 1 t, deadband 0.5 t.
        let sync = derived_input_sync(installed(), &stats(0.0, 7.8125, 23.4375), TICK);
        assert_eq!(sync.jitter_multiple, 0, "the ping EWMA leaves the law");
        assert_close(sync.jitter_margin, 1.5, "floor 0.5 + coverage 1.0");
        assert_close(sync.error_margin, 0.5, "deadband = bulk spread");
        assert_close(sync.max_error_margin, 12.5, "resync bound must carry over");
        // An empty estimator degenerates to the structural floor alone.
        let cold = derived_input_sync(installed(), &ArrivalStats::default(), TICK);
        assert_close(cold.jitter_margin, 0.5, "cold-start floor");
        assert_close(cold.error_margin, 0.0, "cold-start deadband");
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

    /// The downlink margins: the Gaussian chain zeroed (the quantile subsumes it), the half-tick
    /// floor kept, the deadband left default (the interpolation objective never pays it). Fails if
    /// the multiple survives, the floor moves, or the deadband is dragged along.
    #[test]
    fn interp_margins_zero_the_gaussian_chain_and_keep_the_floor() {
        let sync = derived_interp_sync();
        assert_eq!(sync.jitter_multiple, 0, "the quantile subsumes the chain");
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
