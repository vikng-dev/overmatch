//! Server-owned arrival record. Each link owns its silence/window; summaries alone reduce links.

use std::collections::{HashMap, VecDeque};

use bevy::prelude::*;
use bevy::utils::Parallel;
use lightyear::prelude::Connected;
use lightyear::prelude::server::ClientOf;
use serde_json::{Value, json};

use super::timing::{Timing, WindowMax, histogram};

#[derive(Default)]
enum State {
    #[default]
    AwaitingFirstArrival,
    Active {
        silence_s: f64,
        stale: bool,
    },
}

#[derive(Component, Default)]
pub(super) struct InputGap {
    state: State,
    gaps: VecDeque<(f64, f64)>,
    maximum: WindowMax,
    declarations: u64,
    stale_arrivals: u64,
}

impl InputGap {
    pub(super) fn silence_s(&self) -> f64 {
        match self.state {
            State::AwaitingFirstArrival => 0.0,
            State::Active { silence_s, .. } => silence_s,
        }
    }

    pub(super) fn stale(&self) -> bool {
        matches!(self.state, State::Active { stale: true, .. })
    }

    /// Per-link counters cover the interval since the preceding stats emission.
    fn take_line(&mut self, tick_s: f64, timing: &Timing) -> Value {
        let mut gaps: Vec<_> = self.gaps.iter().map(|&(_, gap)| gap).collect();
        json!({"state": match self.state {
                State::AwaitingFirstArrival => "awaiting_first_arrival",
                State::Active { stale: true, .. } => "stale",
                State::Active { stale: false, .. } => "live",
            },
            "silence_ms": self.silence_s() * 1e3,
            "gap_ms": histogram(&mut gaps, tick_s),
            "stale_threshold_ms": self.threshold(tick_s, timing) * 1e3,
            "stale_declarations": std::mem::take(&mut self.declarations),
            "stale_arrivals": std::mem::take(&mut self.stale_arrivals)})
    }

    pub(super) fn threshold(&self, tick_s: f64, timing: &Timing) -> f64 {
        self.maximum.get().map_or(timing.stale_floor_s, |gap| {
            (gap + tick_s).clamp(timing.stale_floor_s, timing.stale_ceil_s)
        })
    }

    /// Retire observations before validation without changing the link's arrival state.
    pub(super) fn expire(&mut self, now_s: f64, timing: &Timing) {
        let oldest = now_s - timing.uplink_window_s;
        while self
            .gaps
            .front()
            .is_some_and(|&(seen_s, _)| seen_s < oldest)
        {
            self.gaps.pop_front();
        }
        self.maximum.expire(oldest);
    }

    /// Classify against the old threshold before admitting the arrival's gap; a connection's
    /// first arrival is neither a gap nor a recovery.
    pub(super) fn receive(
        &mut self,
        arrived: bool,
        now_s: f64,
        frame_s: f64,
        tick_s: f64,
        timing: &Timing,
    ) {
        self.expire(now_s, timing);
        let threshold = self.threshold(tick_s, timing);
        match &mut self.state {
            State::AwaitingFirstArrival => {
                if arrived {
                    self.state = State::Active {
                        silence_s: 0.0,
                        stale: false,
                    };
                }
            }
            State::Active { silence_s, stale } => {
                if arrived {
                    if *silence_s > threshold {
                        self.stale_arrivals += 1;
                    }
                    self.gaps.push_back((now_s, *silence_s));
                    self.maximum.push(now_s, silence_s.min(timing.stale_ceil_s));
                    *silence_s = 0.0;
                    *stale = false;
                } else {
                    *silence_s += frame_s.min(tick_s);
                    let next_stale = *silence_s > threshold;
                    if next_stale && !*stale {
                        self.declarations += 1;
                    }
                    *stale = next_stale;
                }
            }
        }
    }
}

/// Per-link sorts and counter resets run on independent workers only when emission is due.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Census<'w, 's> {
    links: Query<'w, 's, (Entity, &'static mut InputGap), (With<ClientOf>, With<Connected>)>,
    timing: Res<'w, Timing>,
    rows: Local<'s, Parallel<Vec<(Entity, Value)>>>,
}

impl Census<'_, '_> {
    pub fn take_lines(&mut self, tick_s: f64) -> HashMap<Entity, Value> {
        let rows = &mut *self.rows;
        self.links.par_iter_mut().for_each(|(link, mut gap)| {
            let row = gap.take_line(tick_s, &self.timing);
            rows.scope(|rows| rows.push((link, row)));
        });
        rows.iter_mut().flat_map(|rows| rows.drain(..)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active(timing: &Timing, tick: f64) -> InputGap {
        let mut gap = InputGap::default();
        gap.receive(true, 0.0, tick, tick, timing);
        gap
    }

    /// No input exists before the first payload; neither silence nor startup is an outage.
    #[test]
    fn awaiting_first_arrival_records_no_silence_or_gap() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        let mut gap = InputGap::default();
        // Fixture: startup lasts far beyond the link ceiling.
        gap.receive(
            false,
            timing.stale_ceil_s * 10.0,
            timing.stale_ceil_s * 10.0,
            tick,
            &timing,
        );
        assert_eq!(gap.silence_s(), 0.0);
        assert_eq!(gap.declarations, 0);
        gap.receive(true, timing.stale_ceil_s * 11.0, tick, tick, &timing);
        assert_eq!(gap.take_line(tick, &timing)["state"], "live");
        assert!(gap.gaps.is_empty());
        assert_eq!(gap.threshold(tick, &timing), timing.stale_floor_s);
    }

    /// U3 must not attribute the server's wall-time stall to the client. Even an empty receive
    /// charges at most one tick; queued input on the first frame back resets silence outright.
    #[test]
    fn a_server_stall_charges_at_most_one_tick_of_silence() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        // Fixture: server frames much longer than the ceiling, with or without a queued message.
        for arrived in [false, true] {
            let mut gap = active(&timing, tick);
            gap.receive(
                arrived,
                timing.stale_ceil_s * 10.0,
                timing.stale_ceil_s * 10.0,
                tick,
                &timing,
            );
            assert_eq!(gap.silence_s(), if arrived { 0.0 } else { tick });
            assert_eq!(
                gap.declarations,
                u64::from(!arrived && tick > timing.stale_floor_s)
            );
        }
    }

    /// U4 bounds both cold and learned tolerance; the oldest gap expires by server real time.
    /// Tests read the owner tastes so a changed choice leaves these laws intact.
    #[test]
    fn stale_threshold_has_a_floor_ceiling_and_cold_start() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        let mut gap = active(&timing, tick);
        assert_eq!(gap.threshold(tick, &timing), timing.stale_floor_s);
        // Fixture: a long silence eventually becomes stale, exactly once before recovery.
        let frames = (timing.stale_ceil_s / tick).ceil() as usize + 1;
        for frame in 1..=frames {
            gap.receive(false, frame as f64 * tick, tick, tick, &timing);
        }
        assert_eq!(gap.declarations, 1);
        let now = frames as f64 * tick;
        gap.receive(true, now, tick, tick, &timing);
        assert_eq!(gap.stale_arrivals, 1);
        assert_eq!(gap.silence_s(), 0.0);
        assert!(gap.gaps.back().unwrap().1 >= timing.stale_ceil_s);
        assert_eq!(gap.threshold(tick, &timing), timing.stale_ceil_s);
        gap.receive(
            true,
            now + timing.uplink_window_s + tick,
            tick,
            tick,
            &timing,
        );
        assert_eq!(
            gap.threshold(tick, &timing),
            tick.clamp(timing.stale_floor_s, timing.stale_ceil_s)
        );
    }

    /// The arrival gap is accumulated capped empty receives, independent of sender timestamps.
    #[test]
    fn each_arrival_records_the_silence_then_resets_it() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        let mut gap = active(&timing, tick);
        // Fixture: two empty receives, the second a server stall, then an arrival.
        gap.receive(false, tick, tick, tick, &timing);
        gap.receive(false, 1.0, 1.0, tick, &timing);
        gap.receive(true, 1.0 + tick, tick, tick, &timing);
        assert_eq!(gap.gaps.back().unwrap().1, 2.0 * tick);
        assert_eq!(gap.silence_s(), 0.0);
    }

    /// The census counts stale arrivals before their own gaps can enlarge S and admit them.
    #[test]
    fn an_arrival_is_classified_before_its_gap_enters_the_window() {
        let tick = super::super::server::TICK.as_secs_f64();
        // Fixture: the gap lies between the old and enlarged thresholds.
        let timing = Timing {
            stale_floor_s: 2.0 * tick,
            stale_ceil_s: 8.0 * tick,
            ..Timing::shipped()
        };
        let mut gap = active(&timing, tick);
        for frame in 1..=3 {
            gap.receive(false, f64::from(frame) * tick, tick, tick, &timing);
        }
        gap.receive(true, 4.0 * tick, tick, tick, &timing);
        assert_eq!(gap.stale_arrivals, 1);
        assert_eq!(gap.threshold(tick, &timing), 4.0 * tick);
        // A subsequent live arrival cannot add another would-be refusal.
        gap.receive(true, 5.0 * tick, tick, tick, &timing);
        assert_eq!(gap.take_line(tick, &timing)["stale_arrivals"], 1);
        assert_eq!(gap.take_line(tick, &timing)["stale_arrivals"], 0);
    }
}
