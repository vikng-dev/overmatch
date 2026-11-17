//! Server-owned arrival record (U3/U4). Measures only; it neither consumes nor changes input.
//! Each connection owns its silence/window; the stats line is the only cross-owner reduction.

use std::collections::VecDeque;

use bevy::prelude::*;
use lightyear::input::input_message::InputMessage;
use lightyear::prelude::input::native::NativeStateSequence;
use lightyear::prelude::input::server::InputSystems;
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::{Connected, MessageReceiver, MessageSystems};
use serde_json::{Value, json};

use super::timing::{Timing, histogram};
use crate::command::TankCommand;

#[derive(Component, Default)]
pub(super) struct InputGap {
    silence_s: f64,
    gaps: VecDeque<(f64, f64)>,
    stale: bool,
    declarations: u64,
}

impl InputGap {
    fn threshold(&self, tick_s: f64, timing: &Timing) -> f64 {
        self.gaps
            .iter()
            .map(|&(_, gap)| gap)
            .max_by(f64::total_cmp)
            .map_or(timing.stale_floor_s, |gap| {
                (gap + tick_s).clamp(timing.stale_floor_s, timing.stale_ceil_s)
            })
    }

    fn receive(&mut self, arrived: bool, now_s: f64, frame_s: f64, tick_s: f64, timing: &Timing) {
        self.gaps
            .retain(|&(seen_s, _)| now_s - seen_s <= timing.uplink_window_s);
        if arrived {
            self.gaps.push_back((now_s, self.silence_s));
            self.silence_s = 0.0;
        } else {
            self.silence_s += frame_s.min(tick_s);
        }
        let stale = self.silence_s > self.threshold(tick_s, timing);
        if stale && !self.stale {
            self.declarations += 1;
        }
        self.stale = stale;
    }

    pub fn take_line(&mut self, tick_s: f64, timing: &Timing) -> Value {
        let mut gaps: Vec<_> = self.gaps.iter().map(|&(_, gap)| gap).collect();
        json!({"silence_ms": self.silence_s * 1e3,
            "gap_ms": histogram(&mut gaps, tick_s),
            "stale_threshold_ms": self.threshold(tick_s, timing) * 1e3,
            "stale_declarations": std::mem::take(&mut self.declarations)})
    }
}

/// The stats line reduces independent link records only when it emits a summary.
#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct Census<'w, 's> {
    links: Query<'w, 's, &'static mut InputGap>,
    timing: Res<'w, Timing>,
}

impl Census<'_, '_> {
    pub fn take_line(&mut self, link: Entity, tick_s: f64) -> Value {
        self.links
            .get_mut(link)
            .expect("every connected server link has its arrival record from input_gap::connect")
            .take_line(tick_s, &self.timing)
    }
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Timing>();
    app.add_observer(connect).add_systems(
        PreUpdate,
        receive
            .after(MessageSystems::Receive)
            .before(InputSystems::ValidateInputs),
    );
}

fn connect(event: On<Add<Connected>>, mut commands: Commands) {
    // Replacing the state resets silence and history on every connection epoch.
    commands.entity(event.entity).insert(InputGap::default());
}

fn receive(
    real: Res<Time<Real>>,
    fixed: Res<Time<Fixed>>,
    timing: Res<Timing>,
    mut links: Query<
        (
            &mut InputGap,
            Option<&MessageReceiver<InputMessage<NativeStateSequence<TankCommand>>>>,
        ),
        (With<ClientOf>, With<Connected>),
    >,
) {
    links.par_iter_mut().for_each(|(mut gap, receiver)| {
        gap.receive(
            receiver.is_some_and(MessageReceiver::has_messages),
            real.elapsed_secs_f64(),
            real.delta_secs_f64(),
            fixed.timestep().as_secs_f64(),
            &timing,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// U3 must not attribute the server's wall-time stall to the client. Even an empty receive
    /// charges at most one tick; queued input on the first frame back resets silence outright.
    #[test]
    fn a_server_stall_does_not_declare_a_live_client_stale() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        // Fixture: server frames much longer than the ceiling, with or without a queued message.
        for arrived in [false, true] {
            let mut gap = InputGap::default();
            gap.receive(
                arrived,
                timing.stale_ceil_s * 10.0,
                timing.stale_ceil_s * 10.0,
                tick,
                &timing,
            );
            assert!(gap.silence_s <= tick);
            assert_eq!(gap.declarations, 0);
        }
    }

    /// U4 bounds both cold and learned tolerance; the oldest gap expires by server real time.
    /// Tests read the owner tastes so a changed choice leaves these laws intact.
    #[test]
    fn stale_threshold_has_a_floor_ceiling_and_cold_start() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        let mut gap = InputGap::default();
        assert_eq!(gap.threshold(tick, &timing), timing.stale_floor_s);
        // Fixture: a long silence eventually becomes stale, exactly once before recovery.
        let frames = (timing.stale_ceil_s / tick).ceil() as usize + 1;
        for frame in 1..=frames {
            gap.receive(false, frame as f64 * tick, tick, tick, &timing);
        }
        assert_eq!(gap.declarations, 1);
        let now = frames as f64 * tick;
        gap.receive(true, now, tick, tick, &timing);
        assert_eq!(gap.silence_s, 0.0);
        assert!(gap.gaps.back().unwrap().1 >= timing.stale_ceil_s);
        assert_eq!(gap.threshold(tick, &timing), timing.stale_ceil_s);
        gap.receive(
            true,
            now + timing.uplink_window_s + tick,
            tick,
            tick,
            &timing,
        );
        assert_eq!(gap.threshold(tick, &timing), timing.stale_floor_s);
    }

    /// The arrival gap is accumulated capped empty receives, independent of sender timestamps.
    #[test]
    fn each_arrival_records_the_silence_then_resets_it() {
        let timing = Timing::shipped();
        let tick = super::super::server::TICK.as_secs_f64();
        let mut gap = InputGap::default();
        // Fixture: two empty receives, the second a server stall, then an arrival.
        gap.receive(false, tick, tick, tick, &timing);
        gap.receive(false, 1.0, 1.0, tick, &timing);
        gap.receive(true, 1.0 + tick, tick, tick, &timing);
        assert_eq!(gap.gaps.back().unwrap().1, 2.0 * tick);
        assert_eq!(gap.silence_s, 0.0);
    }
}
