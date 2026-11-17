//! Timing tastes and log distributions shared by the two measured records.

use bevy::prelude::*;
use serde_json::{Value, json};

#[derive(Resource, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Timing {
    pub view_window_s: f64,
    pub uplink_window_s: f64,
    pub stale_floor_s: f64,
    pub stale_ceil_s: f64,
}

impl FromWorld for Timing {
    fn from_world(_: &mut World) -> Self {
        Self::shipped()
    }
}

impl Timing {
    pub fn shipped() -> Self {
        let timing: Self = ron::from_str(include_str!("../../assets/net_timing.ron"))
            .expect("assets/net_timing.ron: fix the required timing fields and RON syntax");
        for (field, value) in [
            ("view_window_s", timing.view_window_s),
            ("uplink_window_s", timing.uplink_window_s),
            ("stale_floor_s", timing.stale_floor_s),
            ("stale_ceil_s", timing.stale_ceil_s),
        ] {
            assert!(
                value.is_finite() && value > 0.0,
                "assets/net_timing.ron: {field} must be finite and positive; fix the timing data"
            );
        }
        assert!(
            timing.stale_floor_s <= timing.stale_ceil_s,
            "assets/net_timing.ron: stale_floor_s must not exceed stale_ceil_s; fix the timing data"
        );
        timing
    }
}

/// Nearest-rank measured samples, plus counts in tick-sized bins. Bin i covers [iT, (i+1)T).
/// Resolution: the overflow bin begins at 16T, keeping log rows bounded through an outage.
pub(super) fn histogram(seconds: &mut [f64], tick_s: f64) -> Value {
    if seconds.is_empty() {
        return Value::Null;
    }
    seconds.sort_by(f64::total_cmp);
    let rank = |p: f64| {
        seconds[((p * seconds.len() as f64).ceil() as usize).clamp(1, seconds.len()) - 1] * 1e3
    };
    const BINS: usize = 16;
    let mut bins = [0u64; BINS + 1];
    for &value in seconds.iter() {
        bins[((value / tick_s) as usize).min(BINS)] += 1;
    }
    json!({"n": seconds.len(), "min": rank(0.0), "p50": rank(0.5),
        "p95": rank(0.95), "max": rank(1.0), "bin_ms": tick_s * 1e3, "bins": bins})
}
