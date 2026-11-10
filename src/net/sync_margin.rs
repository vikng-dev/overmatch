//! The sync margins on both wire timelines, derived from the STREAM-MEASURED arrival-delay
//! distribution rather than from the ping EWMA.
//!
//! # The estimator (NetEQ's relative-arrival-delay form)
//!
//! Every received transport packet is tick-stamped by its sender (`PacketReceived { remote_tick }`,
//! `lightyear_transport` plugin.rs). The estimator samples the FIRST packet to bring each new remote
//! tick — nominally 64 samples/s of the actual replication stream, where the ping path samples 10/s, assumes
//! a symmetric Gaussian, and CLAMPS outliers (`lightyear` ping estimator.rs) — the exact samples a
//! dejitter sizer must not discard. One sample per TICK, not per packet: every constant below
//! ([`quantile_p`], [`RING_CAP`], the warmup floor) is derived per tick, and the transport's packets
//! per tick is not ours to fix (MEASURED on the scripted 80/10 ms loopback run: ~1.24 under
//! lightyear 0.28, ~5 under 0.30 — per-packet sampling shrank the ring's 60 s retention to ~13 s).
//! A packet that brings no new tick carries content already superseded, so it is not a sample.
//! Per sample the estimator records the relative arrival delay
//! `d_i = arrival_instant − remote_tick × tick` (epoch-anchored; only differences of `d_i` carry
//! meaning) twice, once per bound of the frame that drained it (see [`record_packet_arrival`]):
//! drain-stamped `d_i`, and the lateness the cursor saw `l_i = d_i − frame` (the frame before the
//! drain was the last read that lacked the packet). It maintains:
//!
//! - `min{d_i}` over the [`SPIKE_WINDOW`] — the fastest recent packet, the anchor that absorbs
//!   path shifts and clock skew (NetEQ's ~2 s spike window precedent; tracks the measured 0.52 s
//!   burst period with margin), and `min{l_i}` beside it for the uplink's deadband;
//! - an eviction ring of [`RING_CAP`] `l_i` samples exposing the empirical quantile `Q_p` and the
//!   median.
//!
//! Relative-arrival delay, not inter-arrival: an EWMA of gaps reads a slowly accumulating delay as
//! "each gap normal" while the buffer starves (the NetEQ migration rationale; a test below pins
//! that failure case).
//!
//! # Sampling contract
//!
//! The ring retains one minute of per-tick samples (DERIVED: 3840 at the authority's 64 Hz).
//! Its empirical quantile covers `p = 1 - 1/N`, where `N = RING_CAP`: at capacity it excludes
//! one sample. Eviction counts samples, so skipped remote ticks lengthen wall-time retention.
//! The minimum anchors use a separate two-second spike window. These are sampling resolutions;
//! an excluded arrival sample is not a bound on the number of perceptible starvation events.
//!
//! # Downlink (the interpolation timeline)
//!
//! `net::interp_delay` writes the delay law, covering the full receipt spread, from this
//! estimator every frame. The Gaussian margin
//! chain on top is therefore ZEROED here
//! (`jitter_multiple = 0`) — the quantile subsumes it — keeping the structural half-tick floor
//! (per-tick keyframes put the cursor's phase against arrival uniform over one interval; the floor
//! carries that distribution's mean).
//!
//! # Uplink (the input timeline)
//!
//! The uplink cannot measure its own arrival at the client, so its margin is the downlink
//! distribution's spread — the SYMMETRY ASSUMPTION, stated: the measured burst structure (60–70 ms
//! every ~0.52 s, sessions of 2026-08-15) was observed in BOTH directions.
//! The objective's terms:
//!
//! - `jitter_margin` (coverage) = the half-tick content-phase floor +
//!   `max(Q_p − max(Q_50, min{l_i}), 0)/tick`;
//! - `error_margin` (deadband) = `max(Q_50 − min, 0)/tick` — a deadband need only exceed the error
//!   signal's own noise, whose scale is the bulk spread; the tail belongs to coverage;
//! - `jitter_multiple` = 0 — the Gaussian jitter term is off; RTT/2 remains in the objective.
//!
//! Before the acceptance clamp, the margins pay the floor plus the nonnegative spread
//! `Q_p − min{l_i}`, split at the median when it lies above that minimum. A sustained delay increase (including discarded
//! server wall time) becomes the baseline within [`SPIKE_WINDOW`] of resumed arrivals, rather
//! than inflating input lead until the ring median catches up. While the minimum exceeds the old
//! median, input coverage gives up the part of the burst tail below that new baseline; bursts
//! above it remain covered. Old burst history is not translated onto the new baseline, so input
//! coverage can briefly reach zero until a fresh burst is sampled.
//! The combined input margins are capped by the server's receive gate, reserving the fixed input
//! delay, the objective's pipeline tick, half RTT and [`INPUT_HEADROOM_TICKS`]. Coverage takes
//! priority over deadband. This bounds the adaptive objective, not remote-estimate error or the
//! clock controller's convergence; network delay alone can exhaust the available budget.
//! The announcement recovery deadline still reads [`ArrivalStats::coverage`]'s long-ring tail:
//! it bounds announcement/state skew, not the client's input lead.
//!
//! Research basis: `.agents/scratch/adaptive-cursor-frontier-2026-08-15.md` §1;
//! uplink failure-mode inventory: `.agents/scratch/input-lead-budget-2026-08-14.md`.

use core::time::Duration;
use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::core::tick::{Tick, TickDuration};
use lightyear::interpolation::timeline::{InterpolationConfig, InterpolationTimeline};
use lightyear::prelude::client::InputDelayConfig;
use lightyear::prelude::{
    Client, Controlled as NetControlled, InputTimelineConfig, Interpolated, PingManager,
    SyncConfig, SyncSystems,
};
use lightyear_transport::plugin::PacketReceived;

use super::protocol::NetTank;
use crate::state::AppState;

/// Half of one content interval, in ticks — the mean of the uniform phase between per-tick content
/// (authored inputs up, replicated keyframes down) and the consuming clock's boundary. A term of
/// structure, not a tuned number: it moves only if content stops advancing once per tick.
const CONTENT_PHASE_MEAN_TICKS: f32 = 0.5;

/// Resolution: one minute of per-tick samples resolves a one-sample tail without unbounded
/// storage. DERIVED capacity: ceil(60 s / tick); skipped ticks lengthen wall-time retention.
const RING_CAP: usize = Duration::from_secs(60)
    .as_nanos()
    .div_ceil(super::server::TICK.as_nanos()) as usize;

/// Fact: lightyear_inputs 0.30.1 src/server.rs:59 sets its private future-input gate to 64;
/// receive_input_message applies it at :370 before any buffer write. Shared by the input law
/// and the server census; the census test drives the actual receive path on both sides.
pub(super) const MAX_INPUT_LOOKAHEAD_TICKS: i32 = 64;

/// Resolution: reserve one tick for fractional-tick rounding and receive-schedule phase.
const INPUT_HEADROOM_TICKS: f32 = 1.0;

/// DERIVED from the authority's timestep; sampling and warmup count remote ticks.
const TICK_RATE_HZ: f64 = 1.0 / super::server::TICK.as_secs_f64();

/// Empirical covered fraction: exclude one sample when the ring is full.
fn quantile_p() -> f64 {
    1.0 - 1.0 / RING_CAP as f64
}

/// Resolution: the min-anchor spike window T (NetEQ's ~2 s precedent). It retains the fastest
/// packet across the MEASURED 0.52 s burst period, while re-anchoring path shifts within seconds.
const SPIKE_WINDOW: Duration = Duration::from_secs(2);

/// Warmup ring floor: one full spike window of per-tick samples (2 s × 64 Hz = 128). The
/// distribution is valid only once the window is full ([`ArrivalStats::warmed`]) — below either
/// bound the quantile still reads the connect transient.
fn warmup_ring_floor() -> usize {
    (SPIKE_WINDOW.as_secs_f64() * TICK_RATE_HZ) as usize
}

/// The estimator's per-frame digest, refreshed by [`refresh_arrival_stats`] ahead of both margin
/// laws. Values are epoch-relative seconds — only their differences carry meaning.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct ArrivalStats {
    /// `min{d_i}`: the drain-stamped anchor, the downlink spread's reference.
    anchor_s: f64,
    /// `min{l_i}`: the uplink margins' reference, on the ring's own stamp.
    min_s: f64,
    q50_s: f64,
    qp_s: f64,
    pub(super) samples: u64,
    /// The distribution left warmup: the spike window holds a full [`SPIKE_WINDOW`] of samples
    /// AND the ring holds ≥ [`warmup_ring_floor`] of them.
    warmed: bool,
}

impl ArrivalStats {
    /// `max(Q_p{l} − min{d}, 0)`: the empirical downlink spread. Both stamps include sender pacing
    /// and the receiver's frame schedule; this is not an independent bound on link delay.
    pub(super) fn spread(&self) -> Duration {
        Duration::from_secs_f64((self.qp_s - self.anchor_s).max(0.0))
    }

    /// `Q_p − Q_50`: the long-ring burst tail above the median, bounding announcement/state
    /// skew for `net::fire_presentation`'s recovery wait.
    pub(super) fn coverage(&self) -> Duration {
        Duration::from_secs_f64((self.qp_s - self.q50_s).max(0.0))
    }

    /// Input lead excludes a sustained level increase once the short-window minimum has risen.
    fn input_coverage(&self) -> Duration {
        Duration::from_secs_f64((self.qp_s - self.q50_s.max(self.min_s)).max(0.0))
    }

    /// `Q_50 − min`: the bulk spread — the error signal's own noise scale, the uplink deadband.
    /// Clamped at zero when a worsening path shift lifts the short-window minimum above the
    /// ring median; neither input margin then carries the baseline shift.
    fn bulk(&self) -> Duration {
        Duration::from_secs_f64((self.q50_s - self.min_s).max(0.0))
    }

    /// Whether the distribution is valid for a derived law to consume (see the field).
    pub(super) fn warmed(&self) -> bool {
        self.warmed
    }

    /// A digest with a known `Q_p − min`, past warmup, for the law tests in sibling modules.
    #[cfg(test)]
    pub(super) fn test_spread(spread_s: f64) -> Self {
        Self {
            anchor_s: 0.0,
            min_s: 0.0,
            q50_s: 0.0,
            qp_s: spread_s,
            samples: 1,
            warmed: true,
        }
    }

    /// The same digest still inside warmup, for the arming tests in sibling modules.
    #[cfg(test)]
    pub(super) fn test_cold_spread(spread_s: f64) -> Self {
        Self {
            warmed: false,
            ..Self::test_spread(spread_s)
        }
    }
}

/// The stream-measured arrival-delay estimator. Fed by [`record_packet_arrival`] once per new
/// remote tick (see the module doc); digested once per frame into [`ArrivalStats`].
#[derive(Resource, Debug, Default)]
pub(super) struct ArrivalDelay {
    /// First-sample anchor `(arrival_secs, remote_tick)`; every `d_i` is relative to it, so the
    /// meaningless absolute offset between the two clocks cancels.
    epoch: Option<(f64, Tick)>,
    /// `(arrival_secs, d_i, l_i)` pruned to [`SPIKE_WINDOW`] — the min anchors.
    window: VecDeque<(f64, f64, f64)>,
    /// `l_i` eviction ring of [`RING_CAP`] samples — the quantile distribution.
    ring: VecDeque<f64>,
    /// Newest remote tick seen on any packet (numeric max) — the "server has simulated through
    /// here" bound `net::fire_presentation`'s heal deadlines read.
    newest_remote: Option<Tick>,
    samples: u64,
    /// Links this estimator has been reset for ([`Self::reset_for_new_link`]); an arming latch
    /// that reads the estimator holds for one link and re-arms on the next.
    link: u64,
    /// The per-frame digest, refreshed by [`refresh_arrival_stats`].
    pub(super) stats: ArrivalStats,
    /// `samples` when `stats` was last digested; `None` until the first digest of this link. The
    /// digest is a pure function of the samples, so an equal count means it is current.
    digested: Option<u64>,
}

impl ArrivalDelay {
    /// One drained packet: `arrival_secs` is its drain frame's stamp and `frame_secs` that
    /// frame's length.
    fn record(&mut self, arrival_secs: f64, frame_secs: f64, remote_tick: Tick, tick_secs: f64) {
        // One sample per new remote tick: a packet that advances nothing is not a sample.
        if self
            .newest_remote
            .is_some_and(|newest| remote_tick - newest <= 0)
        {
            return;
        }
        let (epoch_secs, epoch_tick) = *self.epoch.get_or_insert((arrival_secs, remote_tick));
        let d = (arrival_secs - epoch_secs) - f64::from(remote_tick - epoch_tick) * tick_secs;
        let late = d - frame_secs;
        self.window.push_back((arrival_secs, d, late));
        let horizon = arrival_secs - SPIKE_WINDOW.as_secs_f64();
        while self.window.front().is_some_and(|(at, ..)| *at < horizon) {
            self.window.pop_front();
        }
        self.ring.push_back(late);
        if self.ring.len() > RING_CAP {
            self.ring.pop_front();
        }
        self.newest_remote = Some(remote_tick);
        self.samples += 1;
    }

    /// Recompute the digest if a sample arrived since the last one. `Q_p` is the smallest sample
    /// with cumulative mass ≥ p. DERIVED: at capacity `N = 1/(1−p)`, its rank is `N−1`, so one
    /// sample is outside coverage; `min` anchors on the spike window.
    fn refresh(&mut self, scratch: &mut Vec<f64>) {
        if self.digested == Some(self.samples) {
            return;
        }
        self.digested = Some(self.samples);
        if self.ring.is_empty() {
            self.stats = ArrivalStats::default();
            return;
        }
        let (anchor_s, min_s) = self.window.iter().fold(
            (f64::INFINITY, f64::INFINITY),
            |(anchor, min), (_, d, late)| (anchor.min(*d), min.min(*late)),
        );
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
        // Warmed = the newest sample lies a full spike window past the first-ever sample (the
        // window has evicted once, so it holds its whole 2 s) and the ring holds one window's
        // worth of per-tick samples.
        let warmed = self.ring.len() >= warmup_ring_floor()
            && self.epoch.zip(self.window.back()).is_some_and(
                |((epoch_secs, _), (newest_at, ..))| {
                    newest_at - epoch_secs >= SPIKE_WINDOW.as_secs_f64()
                },
            );
        self.stats = ArrivalStats {
            anchor_s,
            min_s,
            q50_s,
            qp_s,
            samples: self.samples,
            warmed,
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

    /// Empty the estimator for the next link: the new server's ticks start over.
    pub(super) fn reset_for_new_link(&mut self) {
        *self = Self {
            link: self.link + 1,
            ..Self::default()
        };
    }

    /// Which link the samples are from, counted by [`Self::reset_for_new_link`].
    pub(super) fn link(&self) -> u64 {
        self.link
    }
}

/// Feed the estimator from every received transport packet — the tick-stamped arrival stream the
/// ping estimator never reads. Samples enter only while `AppState::Playing` is active and the
/// `InterpolationTimeline` reports synced: before either there is no established stream.
///
/// THE DRAIN FRAME: a packet is stamped on `Time<Real>` when its frame drains the socket, so its
/// socket arrival lies inside that frame — after the previous frame's read, by this one's. The
/// cursor reads once per frame, after the drain, so the packet was missing only from the reads
/// before this frame, the last of them one frame earlier: the lateness presentation can starve on
/// is `d_i − frame`, which the quantile ring takes. The drain stamp `d_i` anchors the min (a packet
/// drained the moment it lands carries no error). In the synthetic unchanged-path model, a client
/// stall adds no upper-tail lateness, while a link burst drained by ordinary frames keeps its
/// depth less one frame. Sender pacing, frame phase and timeline resyncs remain observable; the
/// prior measurements and their limits are in `.agents/docs/design/net-motion-contract.md`.
/// A stall's own samples pull the window's `min{l_i}` down for its 2 s, widening only the uplink's
/// deadband (`bulk`) meanwhile (MEASURED hidden: 1128.8 ms for ~2 s).
fn record_packet_arrival(
    event: On<PacketReceived>,
    time: Res<Time<Real>>,
    tick: Res<TickDuration>,
    state: Res<State<AppState>>,
    timeline: Option<Res<InterpolationTimeline>>,
    mut estimator: ResMut<ArrivalDelay>,
) {
    let synced = timeline.is_some_and(|timeline| timeline.is_synced());
    if *state.get() != AppState::Playing || !synced {
        return;
    }
    estimator.record(
        time.elapsed_secs_f64(),
        time.delta_secs_f64(),
        event.remote_tick,
        tick.0.as_secs_f64(),
    );
}

/// Digest the estimator on each frame that brought a sample (or followed a reset), ahead of both
/// margin laws and the delay law.
pub(super) fn refresh_arrival_stats(
    mut estimator: ResMut<ArrivalDelay>,
    mut scratch: Local<Vec<f64>>,
) {
    estimator.refresh(&mut scratch);
}

/// What the per-frame rewrite rebuilds `InputTimelineConfig` from: the sync config `net::client`
/// installed (controller bounds live there) and the SAME input-delay config it installed —
/// carried whole so the rebuild cannot change the delay's shape.
#[derive(Resource, Debug, Clone, Copy)]
struct InputSyncBase {
    sync: SyncConfig,
    input_delay: InputDelayConfig,
}

#[derive(Default)]
struct InputMarginState {
    armed: bool,
    written: Option<(u8, f32, f32)>,
}

/// The uplink law prioritizes burst coverage over deadband within the acceptance budget.
/// Controller bounds carry over from the installed config.
fn derived_input_sync(
    installed: SyncConfig,
    stats: &ArrivalStats,
    tick: Duration,
    rtt: Duration,
) -> SyncConfig {
    // Lightyear 0.30.1 timeline/input.rs:599-630 adds RTT/2, a pipeline tick and both margins,
    // subtracting input delay; the sender re-adds that delay to the message's end tick.
    // Conservatively reserve the fixed delay too. Cover beyond the gate buys nothing:
    // receive_input_message drops those inputs by construction, before any buffer write.
    let budget = (MAX_INPUT_LOOKAHEAD_TICKS as f32
        - f32::from(super::client::SHIPPING_INPUT_DELAY_TICKS)
        - 1.0 // The objective's pipeline tick.
        - INPUT_HEADROOM_TICKS
        - in_ticks(rtt / 2, tick))
    .max(0.0);
    let jitter_margin =
        (CONTENT_PHASE_MEAN_TICKS + in_ticks(stats.input_coverage(), tick)).min(budget);
    SyncConfig {
        jitter_multiple: 0,
        jitter_margin,
        error_margin: in_ticks(stats.bulk(), tick).min((budget - jitter_margin).max(0.0)),
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

fn in_ticks(duration: Duration, tick: Duration) -> f32 {
    (duration.as_secs_f64() / tick.as_secs_f64()) as f32
}

/// Mount the estimator and the uplink deriving system, and hand back the interpolation config
/// with its sync margins applied (static per session — the live factor, the quantile spread,
/// reaches the downlink through `net::interp_delay`'s per-frame `min_delay` write instead).
pub(super) fn install(
    app: &mut App,
    installed: SyncConfig,
    input_delay: InputDelayConfig,
    mut interpolation: InterpolationConfig,
) -> InterpolationConfig {
    interpolation.sync = derived_interp_sync();
    app.init_resource::<ArrivalDelay>();
    app.add_observer(record_packet_arrival);
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

/// Rewrite the input timeline's sync margins ahead of the frame's timeline sync. The law waits
/// for the own tank to resolve `Interpolated` and then latches for the session (the role
/// cannot change mid-session; a respawn gap must not revert the margins). The rebuild reuses the
/// installed `InputDelayConfig` by value and assigns in place — no insert, so lightyear's
/// input-delay recompute observer never runs and the delay stays byte-identical.
fn derive_input_margins(
    base: Res<InputSyncBase>,
    tick: Res<TickDuration>,
    estimator: Res<ArrivalDelay>,
    own: Query<
        (),
        (
            With<NetControlled>,
            With<NetTank>,
            With<Interpolated>,
            Without<ChildOf>,
        ),
    >,
    config: Option<ResMut<InputTimelineConfig>>,
    pings: Query<&PingManager, With<Client>>,
    mut state: Local<InputMarginState>,
) {
    if !state.armed {
        if own.is_empty() {
            return;
        }
        state.armed = true;
        info!("net: input sync margins ARMED derived — own hull rides the server stream");
    }
    if let Some(mut config) = config {
        // The armed role can outlive its link during disconnect cleanup.
        if pings.is_empty() {
            return;
        }
        let pings = pings
            .single()
            .expect("the client has exactly one server link");
        let sync = derived_input_sync(base.sync, &estimator.stats, tick.0, pings.rtt());
        let key = (sync.jitter_multiple, sync.jitter_margin, sync.error_margin);
        if state.written == Some(key) {
            return;
        }
        *config = InputTimelineConfig::new(sync, base.input_delay);
        state.written = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::prelude::{App, Real, State, Time};
    use lightyear::core::tick::{Tick, TickDuration};
    use lightyear::interpolation::timeline::InterpolationTimeline;
    use lightyear::prelude::{SyncConfig, TimelineSync};
    use lightyear_transport::plugin::PacketReceived;

    use super::{
        ArrivalDelay, ArrivalStats, derived_input_sync, derived_interp_sync, record_packet_arrival,
    };
    use crate::state::AppState;

    /// Read the authority's tick so a permitted rate change preserves the timing invariants.
    const TICK: Duration = crate::net::server::TICK;
    const TICK_SECS: f64 = TICK.as_secs_f64();

    /// The shipped uplink config shape: controller bounds off defaults, so carry-over is visible.
    fn installed() -> SyncConfig {
        SyncConfig {
            jitter_multiple: 2,
            max_error_margin: 12.5,
            ..SyncConfig::default()
        }
    }

    /// Input synchronization follows the wire role before presentation can claim a complete body.
    #[test]
    fn input_margins_arm_before_body_attachment() {
        use bevy::prelude::{DetectChanges, Update};
        use lightyear::prelude::{Controlled, InputTimelineConfig, Interpolated};
        let mut app = App::new();
        let input_delay = crate::net::client::shipping_input_delay();
        app.insert_resource(super::InputSyncBase {
            sync: installed(),
            input_delay,
        })
        .insert_resource(TickDuration(TICK))
        .init_resource::<ArrivalDelay>()
        .insert_resource(InputTimelineConfig::new(installed(), input_delay))
        .add_systems(Update, super::derive_input_margins);
        app.world_mut()
            .spawn((super::Client, super::PingManager::default()));
        app.update();
        let before = app
            .world()
            .get_resource_ref::<InputTimelineConfig>()
            .unwrap()
            .last_changed();
        app.world_mut()
            .spawn((Controlled, super::NetTank, Interpolated));
        app.update();
        assert_ne!(
            app.world()
                .get_resource_ref::<InputTimelineConfig>()
                .unwrap()
                .last_changed(),
            before,
            "the input clock must adopt its stream margins without waiting for a local body"
        );
    }

    fn assert_close(actual: f32, expected: f32, what: &str) {
        assert!(
            (actual - expected).abs() < 1e-4,
            "{what}: got {actual}, expected {expected}"
        );
    }

    fn stats(anchor_ms: f64, min_ms: f64, q50_ms: f64, qp_ms: f64) -> ArrivalStats {
        ArrivalStats {
            anchor_s: anchor_ms / 1000.0,
            min_s: min_ms / 1000.0,
            q50_s: q50_ms / 1000.0,
            qp_s: qp_ms / 1000.0,
            samples: 1,
            warmed: true,
        }
    }

    /// Drive a synthetic arrival series through the estimator: ticks at exact cadence, one sample
    /// per tick with a KNOWN delay pattern, each drained the moment it lands (a zero-length
    /// frame: both stamps agree), then refresh and read the digest.
    fn fed(delays_ms: impl IntoIterator<Item = f64>) -> ArrivalDelay {
        let mut estimator = ArrivalDelay::default();
        for (i, delay_ms) in delays_ms.into_iter().enumerate() {
            let tick = Tick(1_000 + i as u32);
            let arrival = i as f64 * TICK_SECS + delay_ms / 1000.0;
            estimator.record(arrival, 0.0, tick, TICK_SECS);
        }
        estimator.refresh(&mut Vec::new());
        estimator
    }

    /// Drive per-tick packets, each `path_ms` after its send, through a client whose frames read
    /// every `frame_ms` — except one frame stalled `stall_ms` long starting at `stall_at_s` — and
    /// read the digest. Every packet is stamped at the frame that drains it, with that frame's
    /// length, as `record_packet_arrival` stamps it.
    fn drained(path_ms: f64, frame_ms: f64, stall_at_s: f64, stall_ms: f64) -> ArrivalDelay {
        let mut frames = Vec::new();
        let mut at = 0.0;
        while at < 4.0 {
            let step = if at <= stall_at_s && at + frame_ms / 1000.0 > stall_at_s {
                stall_ms / 1000.0
            } else {
                frame_ms / 1000.0
            };
            at += step;
            frames.push((at, step));
        }
        let mut estimator = ArrivalDelay::default();
        for i in 0..240_u32 {
            let lands = f64::from(i) * TICK_SECS + path_ms / 1000.0;
            let &(drain, frame) = frames
                .iter()
                .find(|(end, _)| *end >= lands)
                .expect("the frames outlast the packets");
            estimator.record(drain, frame, Tick(1_000 + i), TICK_SECS);
        }
        estimator.refresh(&mut Vec::new());
        estimator
    }

    /// A CLIENT STALL IS NOT LINK DELAY: the ticks a stalled frame drains at once were not due at
    /// the read before it, so a 1.1 s stall (FIXTURE DATA at the scale of the hidden loopback run's
    /// own-tank stall) on a clean 60 fps stream leaves the spread inside one ordinary frame. Mutant:
    /// the quantile on the drain stamp alone reads the stall whole (~1.1 s) and reds; the anchor on
    /// the ring's stamp reads the stall as an early packet and reds the same way.
    #[test]
    fn a_client_stall_adds_nothing_to_the_spread() {
        let clean = drained(5.0, 1000.0 / 60.0, 1.5, 1000.0 / 60.0);
        let stalled = drained(5.0, 1000.0 / 60.0, 1.5, 1100.0);
        let spread_ms = |estimator: &ArrivalDelay| estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            spread_ms(&clean) <= 1000.0 / 60.0,
            "a clean stream pays at most one frame (got {} ms)",
            spread_ms(&clean)
        );
        assert!(
            spread_ms(&stalled) <= 1000.0 / 60.0,
            "the stall adds nothing (got {} ms)",
            spread_ms(&stalled)
        );
    }

    /// A LINK BURST DRAINED BY ORDINARY FRAMES KEEPS ITS DEPTH: packets held 60 ms in the path
    /// starve the reads between, which keep running, so the spread covers the burst less at most
    /// one frame. Mutant: subtracting more than the drain frame from a sample (stamping it at an
    /// earlier read than the last one that lacked it) hides the burst and reds.
    #[test]
    fn a_link_burst_under_ordinary_frames_keeps_its_depth() {
        let frame = 1000.0 / 60.0;
        let mut frames = Vec::new();
        let mut at = 0.0;
        while at < 4.0 {
            at += frame / 1000.0;
            frames.push(at);
        }
        let mut estimator = ArrivalDelay::default();
        for i in 0..240_u32 {
            let burst = if i % 33 == 0 { 60.0 } else { 0.0 };
            let lands = f64::from(i) * TICK_SECS + (5.0 + burst) / 1000.0;
            let drain = *frames
                .iter()
                .find(|end| **end >= lands)
                .expect("the frames outlast the packets");
            estimator.record(drain, frame / 1000.0, Tick(1_000 + i), TICK_SECS);
        }
        estimator.refresh(&mut Vec::new());
        let spread_ms = estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            spread_ms >= 60.0 - frame,
            "the burst's depth less one frame is covered (got {spread_ms} ms)"
        );
    }

    /// DERIVED: at ring capacity the quantile excludes one sample, not zero. Input and delay
    /// controllers rely on that empirical rank; calling it a maximum overstates coverage.
    #[test]
    fn the_full_ring_quantile_excludes_exactly_one_sample() {
        let n = super::RING_CAP;
        let one = fed(std::iter::repeat_n(0.0, n - 1).chain([80.0]));
        assert_eq!(one.stats.spread(), Duration::ZERO);
        let two = fed(std::iter::repeat_n(0.0, n - 2).chain([80.0, 80.0]));
        assert!((two.stats.spread().as_secs_f64() - 0.080).abs() < 1e-6);
    }

    /// THE MEASURED BURST SIGNATURE (60 ms every 33 ticks) IS COVERED WITHIN ONE SEND INTERVAL:
    /// the spike mass (1/33 ≈ 3 %) dwarfs 1 − p, so Q_p must clear the burst depth while Q_50
    /// sits in the bulk. A Gaussian-multiple margin over a clamped jitter EWMA (which reads this
    /// stream as ~3 ms jitter) is exactly what reds here.
    #[test]
    fn the_measured_burst_train_is_covered_by_the_target() {
        let delays = (0..640).map(|i| if i % 33 == 0 { 60.0 } else { 0.0 });
        let estimator = fed(delays);
        let spread = estimator.stats.spread().as_secs_f64() * 1000.0;
        assert!(
            spread >= 60.0 - 1e-6 && spread <= 60.0 + TICK.as_secs_f64() * 1000.0,
            "the target must cover the 60 ms burst within one send interval (got {spread} ms)"
        );
        let bulk_ms = estimator.stats.bulk().as_secs_f64() * 1000.0;
        assert!(bulk_ms < 1.0, "the median sits in the bulk (got {bulk_ms})");
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

    /// THE WARMUP GATE NEEDS BOTH BOUNDS: a full spike window of elapsed samples AND a full
    /// window's worth in the ring (128 = 2 s × 64 Hz). Per-tick feeding crosses both at the same
    /// boundary (129 samples span exactly 2 s); the sparse series passes the span but not the
    /// ring, the dense series the ring but not the span — dropping either bound reds its case,
    /// and dropping the gate entirely (always-warmed) reds all three cold cases.
    #[test]
    fn the_distribution_warms_only_on_a_full_window_and_ring() {
        assert!(
            !fed((0..128).map(|_| 0.0)).stats.warmed(),
            "128 per-tick samples span 1.98 s — the window is not yet full"
        );
        assert!(
            fed((0..129).map(|_| 0.0)).stats.warmed(),
            "129 per-tick samples span exactly the 2 s window with a full ring"
        );
        // Sparse: two-tick spacing spans 3.1 s but holds only 100 samples.
        let mut sparse = ArrivalDelay::default();
        for i in 0..100_u32 {
            sparse.record(
                f64::from(i) * 2.0 * TICK_SECS,
                0.0,
                Tick(1_000 + 2 * i),
                TICK_SECS,
            );
        }
        sparse.refresh(&mut Vec::new());
        assert!(
            !sparse.stats.warmed(),
            "a 3 s span with a short ring is still warmup"
        );
        // Dense: 5 ms arrivals fill the ring inside 0.7 s.
        let mut dense = ArrivalDelay::default();
        for i in 0..140_u32 {
            dense.record(f64::from(i) * 0.005, 0.0, Tick(1_000 + i), TICK_SECS);
        }
        dense.refresh(&mut Vec::new());
        assert!(
            !dense.stats.warmed(),
            "a full ring inside a short span is still warmup"
        );
    }

    /// A NEW LINK DIGESTS EMPTY ON ITS FIRST FRAME: the digest is skipped while the sample count
    /// stands, and a reset starts the count over, so a reset must still re-digest or the new link
    /// reads the old link's spread (the delay law and the debug readout both arm on it). Mutant:
    /// a plain count defaulting to 0 in place of the `None` marker — the reset's zero count reads
    /// as digested, the stale digest survives and this reds.
    #[test]
    fn a_reset_redigests_though_no_sample_arrived() {
        let mut estimator = fed((0..129).map(|_| 0.0));
        assert!(estimator.stats.warmed(), "the old link's digest is live");
        estimator.reset_for_new_link();
        estimator.refresh(&mut Vec::new());
        assert_eq!(estimator.stats, ArrivalStats::default());
        estimator.record(0.0, 0.0, Tick(5), TICK_SECS);
        estimator.refresh(&mut Vec::new());
        assert_eq!(
            estimator.stats.samples, 1,
            "a sample after the reset is digested"
        );
    }

    /// ONE SAMPLE PER TICK: the ring and quantile are derived per tick, so a tick the transport
    /// splits across several packets samples once (its first arrival), and a late packet whose
    /// tick is already covered samples not at all. Mutant: drop the early return in `record` —
    /// the repeat and stale packets record and the count reds.
    #[test]
    fn a_tick_samples_once_however_many_packets_carry_it() {
        let mut estimator = ArrivalDelay::default();
        // Tick 1000 in three packets, tick 1001 in two, then a reordered straggler for 1000.
        for (at, tick) in [
            (0.000, 1_000),
            (0.001, 1_000),
            (0.002, 1_000),
            (0.016, 1_001),
            (0.017, 1_001),
            (0.030, 1_000),
        ] {
            estimator.record(at, 0.0, Tick(tick), TICK_SECS);
        }
        assert_eq!(estimator.samples, 2, "two ticks, two samples");
        assert_eq!(estimator.ring.len(), 2);
        assert_eq!(estimator.newest_remote, Some(Tick(1_001)));
    }

    /// THE RECORDING GATE IS Playing ∧ a synced InterpolationTimeline: an arrival while either
    /// conjunct is down never enters the estimator. Mutant: delete the guard's early return in
    /// `record_packet_arrival` (or either conjunct) — the matching trigger records and its count
    /// assertion reds; the in-play triggers pin that the gated path still records, a stalled
    /// drain included (its lateness is the ring's to correct, not the gate's to drop).
    #[test]
    fn pre_gate_arrivals_never_enter_the_estimator() {
        let mut app = App::new();
        app.insert_resource(TickDuration(TICK));
        app.insert_resource(State::new(AppState::Loading));
        app.init_resource::<Time<Real>>();
        app.init_resource::<ArrivalDelay>();
        app.add_observer(record_packet_arrival);
        let transport = app.world_mut().spawn_empty().id();
        let arrive = |app: &mut App, tick: u32| {
            app.world_mut().trigger(PacketReceived {
                entity: transport,
                remote_tick: Tick(tick),
            });
            app.world_mut().flush();
            app.world().resource::<ArrivalDelay>().samples
        };
        assert_eq!(
            arrive(&mut app, 1_000),
            0,
            "loading ∧ unsynced never records"
        );
        app.insert_resource(State::new(AppState::Playing));
        assert_eq!(
            arrive(&mut app, 1_001),
            0,
            "playing without a synced interpolation timeline never records"
        );
        app.insert_resource(State::new(AppState::Loading));
        let mut timeline = InterpolationTimeline::default();
        timeline.set_synced(true);
        app.insert_resource(timeline);
        assert_eq!(
            arrive(&mut app, 1_002),
            0,
            "a synced timeline outside Playing never records"
        );
        app.insert_resource(State::new(AppState::Playing));
        assert_eq!(
            arrive(&mut app, 1_003),
            1,
            "playing ∧ synced records — the observer stays live"
        );
        let now = app.world().resource::<Time<Real>>().startup();
        let mut real = app.world_mut().resource_mut::<Time<Real>>();
        real.update_with_instant(now);
        real.update_with_instant(now + Duration::from_millis(500));
        assert_eq!(arrive(&mut app, 1_004), 2, "a stalled drain still records");
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

    /// The uplink law splits the spread at max(median, minimum): coverage above it plus the
    /// structural floor, nonnegative bulk below it, ping off, controller bounds carried over. Fails if either
    /// term reads the wrong spread, the floor is dropped or absorbed, the ping multiple survives,
    /// or the law rebuilds from `SyncConfig::default()`.
    #[test]
    fn the_uplink_law_splits_the_spread_at_the_higher_of_median_and_minimum() {
        // Fixture: median half a tick above min, tail one tick above median.
        let tick_ms = TICK.as_secs_f64() * 1000.0;
        let sync = derived_input_sync(
            installed(),
            &stats(-tick_ms * 4.0, 0.0, tick_ms / 2.0, tick_ms * 1.5),
            TICK,
            Duration::ZERO,
        );
        assert_eq!(
            sync.jitter_multiple, 0,
            "the Gaussian jitter term leaves the margins"
        );
        assert_close(sync.jitter_margin, 1.5, "floor 0.5 + coverage 1.0");
        assert_close(sync.error_margin, 0.5, "deadband = bulk spread");
        assert_close(sync.max_error_margin, 12.5, "resync bound must carry over");
        // Fixture: a risen minimum above the old median, with one tick of tail still above it.
        let shifted = derived_input_sync(
            installed(),
            &stats(0.0, tick_ms * 2.0, tick_ms / 2.0, tick_ms * 3.0),
            TICK,
            Duration::ZERO,
        );
        assert_close(
            shifted.jitter_margin,
            1.5,
            "tail above the new baseline stays covered",
        );
        assert_close(
            shifted.error_margin,
            0.0,
            "the old bulk is below the new baseline",
        );
        // An empty estimator degenerates to the structural floor alone.
        let cold = derived_input_sync(installed(), &ArrivalStats::default(), TICK, Duration::ZERO);
        assert_close(cold.jitter_margin, 0.5, "cold-start floor");
        assert_close(cold.error_margin, 0.0, "cold-start deadband");
    }

    /// The sender needs the full objective (pipeline tick and deadband included) inside the
    /// receive gate. Zero input delay is conservative: shipping subtracts it from this objective
    /// and re-adds it to the message tick. This exercises Lightyear's law, not clock convergence.
    fn input_objective_lead(stats: &ArrivalStats, rtt: Duration) -> f32 {
        use lightyear::prelude::{InputTimelineConfig, PingManager};
        use lightyear_sync::prelude::client::RemoteTimeline;
        use lightyear_sync::timeline::input::{InputDelayConfig, LocalTimelineSync};
        use lightyear_sync::timeline::sync::{SyncTargetTimeline, TimelineSync};
        let remote = RemoteTimeline::default();
        let config = InputTimelineConfig::new(
            derived_input_sync(installed(), stats, TICK, rtt),
            InputDelayConfig::no_input_delay(),
        );
        let mut pings = PingManager::default();
        pings.rtt_estimator_ewma.final_stats.rtt = rtt;
        let objective = LocalTimelineSync::default().sync_objective(&remote, &config, &pings, TICK);
        (objective - remote.current_estimate()).to_f32()
    }

    /// The sender's baseline recovers within the declared two-second contract, independently of
    /// the implementation's window constant. A clamp alone must not leave a step at the cap.
    #[test]
    fn a_server_stall_releases_input_coverage_within_two_seconds() {
        // Fixture: five discarded seconds after a full steady ring; ordinary drain frames.
        let mut estimator = fed(std::iter::repeat_n(0.0, super::RING_CAP));
        let mut scratch = Vec::new();
        let recovery_ticks = (Duration::from_secs(2).as_nanos() / TICK.as_nanos()) as u32 + 1;
        let start = super::RING_CAP as u32;
        for after in 0..=recovery_ticks {
            let i = start + after;
            estimator.record(
                f64::from(i) * TICK_SECS + 5.0,
                TICK_SECS,
                Tick(1_000 + i),
                TICK_SECS,
            );
            estimator.refresh(&mut scratch);
        }
        assert_close(
            input_objective_lead(&estimator.stats, Duration::ZERO),
            1.5,
            "pipeline + phase floor",
        );
    }

    /// Drive every sample through the real estimator and objective; the input sender must never
    /// buy coverage that the receive gate drops. Mutant: remove the combined-margin clamp.
    fn assert_trace_within_gate(arrivals: impl IntoIterator<Item = f64>) {
        let mut estimator = fed(std::iter::repeat_n(0.0, super::RING_CAP));
        let mut scratch = Vec::new();
        let start = super::RING_CAP as u32;
        for (after, arrival) in arrivals.into_iter().enumerate() {
            estimator.record(
                f64::from(start) * TICK_SECS + arrival,
                0.0,
                Tick(1_000 + start + after as u32),
                TICK_SECS,
            );
            estimator.refresh(&mut scratch);
            for rtt in [Duration::ZERO, Duration::from_millis(250)] {
                assert_objective_within_gate(&estimator.stats, rtt, after);
            }
        }
    }

    fn assert_objective_within_gate(stats: &ArrivalStats, rtt: Duration, sample: usize) {
        let lead = input_objective_lead(stats, rtt);
        assert!(
            lead <= super::MAX_INPUT_LOOKAHEAD_TICKS as f32
                - f32::from(crate::net::client::SHIPPING_INPUT_DELAY_TICKS)
                - 1.0,
            "sample {sample}, RTT {rtt:?}: objective {lead} must reserve fixed delay and one tick of headroom",
        );
    }

    /// The sender's budget includes deadband and RTT as well as coverage. Clamping only the tail
    /// fails the bulk-heavy case; omitting half RTT fails the nonzero-RTT case.
    #[test]
    fn input_acceptance_budget_counts_deadband_and_half_rtt() {
        // Fixture: a bulk-heavy distribution and a tail-heavy one, both beyond the gate.
        let tick_ms = TICK_SECS * 1000.0;
        for stats in [
            stats(0.0, 0.0, tick_ms * 100.0, tick_ms * 101.0),
            stats(0.0, 0.0, 0.0, tick_ms * 200.0),
        ] {
            for rtt in [Duration::ZERO, Duration::from_millis(250)] {
                assert_objective_within_gate(&stats, rtt, 0);
            }
        }
    }

    #[test]
    fn input_lead_stays_inside_acceptance_during_a_server_stall_ramp() {
        // Fixture: 20 Hz for two seconds, then four seconds at the authority's normal cadence.
        assert_trace_within_gate((0..40 + 256).map(|i| {
            if i < 40 {
                f64::from(i) / 20.0
            } else {
                2.0 + f64::from(i - 40) * TICK_SECS
            }
        }));
    }

    #[test]
    fn input_lead_stays_inside_acceptance_while_a_transient_lingers_in_the_ring() {
        // Fixture: two packets delayed 1.25 s; following packets drain in order behind them,
        // then resume normal cadence. Replay beyond a full ring so tail eviction is covered too.
        let mut last_arrival: f64 = 0.0;
        let arrivals = (0..super::RING_CAP + 129).map(|i| {
            let delay = if i < 2 { 1.25 } else { 0.0 };
            last_arrival = last_arrival.max(i as f64 * TICK_SECS + delay);
            last_arrival
        });
        assert_trace_within_gate(arrivals);
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
    }
}
