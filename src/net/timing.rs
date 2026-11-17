//! Timing tastes and log distributions shared by the two measured records.

use bevy::prelude::*;
use serde_json::{Value, json};
use std::collections::VecDeque;

use crate::provenance::{Sourced, from_shipped_str};

#[derive(Resource, Clone, Debug)]
pub(super) struct Timing {
    pub view_window_s: f64,
    pub uplink_window_s: f64,
    pub stale_floor_s: f64,
    pub stale_ceil_s: f64,
    pub link_warning_s: f64,
    pub refused_cue_s: f64,
    pub refused_pitch: f32,
}

impl FromWorld for Timing {
    fn from_world(_: &mut World) -> Self {
        Self::shipped()
    }
}

impl Timing {
    pub fn shipped() -> Self {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Authored {
            view_window_s: Sourced<f64>,
            uplink_window_s: Sourced<f64>,
            stale_floor_s: Sourced<f64>,
            stale_ceil_s: Sourced<f64>,
            link_warning_s: Sourced<f64>,
            refused_cue_s: Sourced<f64>,
            refused_pitch: Sourced<f32>,
        }
        let authored: Authored = from_shipped_str(
            "assets/net_timing.ron",
            include_str!("../../assets/net_timing.ron"),
        )
        .unwrap_or_else(|err| panic!("{err}; fix the required timing fields and provenance"));
        let timing = Self {
            view_window_s: authored.view_window_s.0,
            uplink_window_s: authored.uplink_window_s.0,
            stale_floor_s: authored.stale_floor_s.0,
            stale_ceil_s: authored.stale_ceil_s.0,
            link_warning_s: authored.link_warning_s.0,
            refused_cue_s: authored.refused_cue_s.0,
            refused_pitch: authored.refused_pitch.0,
        };
        for (field, value) in [
            ("view_window_s", timing.view_window_s),
            ("uplink_window_s", timing.uplink_window_s),
            ("stale_floor_s", timing.stale_floor_s),
            ("stale_ceil_s", timing.stale_ceil_s),
            ("link_warning_s", timing.link_warning_s),
            ("refused_cue_s", timing.refused_cue_s),
            ("refused_pitch", f64::from(timing.refused_pitch)),
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

/// Monotonic maxima avoid scanning retained observations every frame. Times are insertion times,
/// in nondecreasing local Real order; each entry is pushed and popped at most once.
#[derive(Default)]
pub(super) struct WindowMax(VecDeque<(f64, f64)>);

impl WindowMax {
    pub fn push(&mut self, now_s: f64, value: f64) {
        while self.0.back().is_some_and(|&(_, older)| older <= value) {
            self.0.pop_back();
        }
        self.0.push_back((now_s, value));
    }

    pub fn expire(&mut self, oldest_s: f64) {
        while self.0.front().is_some_and(|&(seen_s, _)| seen_s < oldest_s) {
            self.0.pop_front();
        }
    }

    pub fn get(&self) -> Option<f64> {
        self.0.front().map(|&(_, value)| value)
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
