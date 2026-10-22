//! Beacon round-trip estimates: a few timed TCP connects per beacon, summarised as median and
//! spread.
//!
//! **The probe is a TCP handshake.** Edgegap documents beacons as answering "ICMP, UDP, or TCP
//! ping" and lists a `tcp_port` "used to perform TCP ping", but publishes no UDP payload format;
//! its own Unity SDK pings by ICMP, which needs raw-socket privileges a game client does not have.
//! A TCP connect needs no payload: the SYN → SYN-ACK exchange is one round trip, and
//! `TcpStream::connect_timeout` returns when it completes. It is also how the hosting risk test
//! measured the beacons (`.agents/scratch/hosting-2026-10/risk-test.md`).
//!
//! **The summary reads like `net::sync_margin::ArrivalStats`:** the median is the nearest-rank
//! Q50 (index `ceil(0.5·n) − 1`, the estimator's own index law), and the spread is Qp − min,
//! which over a handful of samples is max − min (Qp with p = 1 − 1/3840 is the largest sample at
//! any n < 3840).

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// Connects timed per beacon.
pub(super) const SAMPLES: usize = 5;
/// The pause between two connects to one beacon, so the samples see more than one instant of the
/// path.
pub(super) const SAMPLE_GAP: Duration = Duration::from_millis(150);
/// A connect slower than this is a lost sample, not a measurement.
pub(super) const SAMPLE_TIMEOUT: Duration = Duration::from_millis(1500);

/// A beacon's round-trip summary over its successful samples, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PingSummary {
    pub(super) median_ms: f64,
    pub(super) spread_ms: f64,
    pub(super) samples: usize,
}

impl PingSummary {
    /// The value sent to the Worker: the median in whole milliseconds.
    pub(super) fn median_whole_ms(&self) -> u32 {
        self.median_ms.round().clamp(0.0, f64::from(u32::MAX)) as u32
    }
}

/// `None` when no sample succeeded.
pub(super) fn summarize(samples_ms: &[f64]) -> Option<PingSummary> {
    if samples_ms.is_empty() {
        return None;
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len();
    let median_index = ((0.5 * n as f64).ceil() as usize).clamp(1, n) - 1;
    Some(PingSummary {
        median_ms: sorted[median_index],
        spread_ms: sorted[n - 1] - sorted[0],
        samples: n,
    })
}

/// The beacon's TCP endpoint: `host` is an IP in Edgegap's list; a hostname is resolved, IPv4
/// first. Blocking (DNS), so it runs on the probe's own thread.
pub(super) fn beacon_addr(host: &str, port: u16) -> Option<SocketAddr> {
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, port));
    }
    let resolved: Vec<SocketAddr> = (host, port).to_socket_addrs().ok()?.collect();
    resolved
        .iter()
        .find(|addr| addr.is_ipv4())
        .or_else(|| resolved.first())
        .copied()
}

/// Time `samples` TCP connects to `addr`, `gap` apart; each failed or timed-out connect is
/// dropped. Blocking for up to `samples × (timeout + gap)`: call it off the frame loop.
pub(super) fn probe_tcp(
    addr: SocketAddr,
    samples: usize,
    gap: Duration,
    timeout: Duration,
) -> Vec<f64> {
    let mut rtts = Vec::with_capacity(samples);
    for sample in 0..samples {
        if sample > 0 {
            std::thread::sleep(gap);
        }
        let start = Instant::now();
        if let Ok(stream) = TcpStream::connect_timeout(&addr, timeout) {
            rtts.push(start.elapsed().as_secs_f64() * 1000.0);
            drop(stream);
        }
    }
    rtts
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, TcpListener};

    use super::*;

    #[test]
    fn summary_is_nearest_rank_median_and_full_spread() {
        let summary = summarize(&[48.0, 40.0, 45.0, 120.0, 44.0]).unwrap();
        assert_eq!(summary.median_ms, 45.0);
        assert_eq!(summary.spread_ms, 80.0);
        assert_eq!(summary.samples, 5);
        assert_eq!(summary.median_whole_ms(), 45);

        // An even count takes the lower middle sample, the estimator's `ceil(0.5·n) − 1` index.
        let summary = summarize(&[30.0, 10.0, 40.0, 20.0]).unwrap();
        assert_eq!(summary.median_ms, 20.0);
        assert_eq!(summary.spread_ms, 30.0);

        let summary = summarize(&[61.4]).unwrap();
        assert_eq!(summary.median_ms, 61.4);
        assert_eq!(summary.spread_ms, 0.0);
        assert_eq!(summary.median_whole_ms(), 61);
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn probe_times_a_listening_port_and_drops_a_refused_one() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let open = listener.local_addr().unwrap();
        let rtts = probe_tcp(
            open,
            3,
            Duration::from_millis(1),
            Duration::from_millis(500),
        );
        assert_eq!(rtts.len(), 3);
        assert!(rtts.iter().all(|ms| ms.is_finite() && *ms >= 0.0));

        drop(listener);
        let refused = probe_tcp(
            open,
            2,
            Duration::from_millis(1),
            Duration::from_millis(500),
        );
        assert!(refused.is_empty(), "a closed port yields no samples");
    }
}
