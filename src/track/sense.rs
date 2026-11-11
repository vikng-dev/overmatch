//! What the Driver feels of the hull (the game's tank driving model): its speed along its heading
//! and the resistance it meets, both averaged over one shift cut. Speeds are m/s and forces N,
//! along the hull's heading with forward positive.
//!
//! Resistance closes the hull's force ledger from its own side: the belts' push on the hull, less
//! hull mass times the hull's gain in speed. Whatever else acts along the heading (grade, rolling,
//! an obstacle's reaction, a tow, recoil) is in it, so no load model is needed; a gear change does
//! not move it, because the push the hull receives is what the ground returned, whatever gear sent
//! it. The window is one shift cut because the landing forecast needs exactly the speed the world
//! takes from the hull in one cut, and a single-tick impulse must not decide alone.

use std::collections::VecDeque;

use bevy::prelude::*;

/// One tick of the ledger: the hull's speed along its heading at the tick's start (m/s) and the
/// belts' push on the hull through that tick, `[left, right]` (N).
#[derive(Clone, Copy, Debug)]
struct Sample {
    speed: f32,
    push: [f32; 2],
}

/// The Driver's reading for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sensed {
    /// The hull's speed along its heading at this tick's start (m/s).
    pub speed: f32,
    /// What resisted forward travel over the window (N): the belts' mean push less hull mass
    /// times the hull's mean acceleration along its heading.
    pub resistance: f32,
    /// The left belt's mean push less the right's over the window (N).
    pub push_difference: f32,
}

/// The Driver's ledger: server-side driving state, built empty at spawn from the tank's cut
/// length. Nothing is sensed until one tick has closed.
#[derive(Component, Clone, Debug)]
pub struct DriverSense {
    window: usize,
    samples: VecDeque<Sample>,
    /// This tick's start speed and sampled-contact push; the solver's push for the same tick is
    /// read on the next one, so the sample closes there.
    open: Option<Sample>,
    /// The reading this tick's choice used: telemetry and probes read it.
    pub(crate) reading: Option<Sensed>,
}

impl DriverSense {
    /// An empty ledger averaging one cut of `shift_ticks`; a cut of no ticks averages one tick.
    pub fn for_cut(shift_ticks: u8) -> Self {
        let window = usize::from(shift_ticks).max(1);
        Self {
            window,
            samples: VecDeque::with_capacity(window),
            open: None,
            reading: None,
        }
    }

    /// One driving tick. `speed` is the hull's speed along its heading now, `mass` its mass (kg),
    /// `soft` the push this tick's sampled contacts apply and `hard` the solver contact push of the
    /// physics step that just ran (N, `None` where a side had no solver contact). The step that
    /// just ran carried last tick's sampled push and this `hard` push from last tick's speed to
    /// `speed`, so that sample closes here.
    pub(crate) fn tick(
        &mut self,
        speed: f32,
        mass: f32,
        soft: [f32; 2],
        hard: [Option<f32>; 2],
        dt: f32,
    ) -> Option<Sensed> {
        if let Some(mut closed) = self.open.take() {
            for (push, hard) in closed.push.iter_mut().zip(hard) {
                *push += hard.unwrap_or(0.0); // No solver contact pushed that side.
            }
            if self.samples.len() == self.window {
                self.samples.pop_front();
            }
            self.samples.push_back(closed);
        }
        self.open = Some(Sample { speed, push: soft });
        self.reading = self.samples.front().map(|first| {
            assert!(
                mass.is_finite() && mass > 0.0 && dt > 0.0,
                "a driven hull has finite mass and its tick a positive step"
            );
            let count = self.samples.len() as f32;
            let [left, right] = self
                .samples
                .iter()
                .fold([0.0; 2], |[l, r], s| [l + s.push[0], r + s.push[1]]);
            Sensed {
                speed,
                resistance: (left + right) / count - mass * (speed - first.speed) / (count * dt),
                push_difference: (left - right) / count,
            }
        });
        self.reading
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ledger reads the force that resists the hull, whatever the belts push: a cut that drops
    /// the push to zero leaves the reading on the true resistance. The landing forecast relies on
    /// this; a push-only or acceleration-only reading moves through the cut. Fixture: a point hull
    /// under a constant resisting force, a push stepping through a cut, explicit integration.
    #[test]
    fn resistance_is_unchanged_through_a_paid_cut() {
        let (mass, dt, resist) = (40_000.0_f32, 1.0 / 64.0, 30_000.0_f32);
        let cut = 20;
        let mut sense = DriverSense::for_cut(cut);
        let mut speed = 3.0_f32;
        let mut readings = Vec::new();
        let mut reported = [None; 2];
        for tick in 0..200 {
            // Fixture: driving push, a cut with no push, then a stronger push.
            let push = match tick {
                0..60 => 45_000.0,
                60..80 => 0.0,
                _ => 60_000.0,
            };
            // A quarter of the push is the left belt's solver contact, reported a tick late.
            let (soft, hard) = ([push * 0.25, push * 0.5], [Some(push * 0.25), None]);
            readings.push(sense.tick(speed, mass, soft, reported, dt));
            speed += (push - resist) / mass * dt;
            reported = hard;
        }
        assert!(
            readings[0].is_none(),
            "nothing is sensed before a tick closes"
        );
        for (tick, reading) in readings.iter().enumerate().skip(1) {
            let reading = reading.expect("every later tick reads the ledger");
            assert!(
                (reading.resistance - resist).abs() < resist * 1e-3,
                "tick {tick}: read {} against {resist}",
                reading.resistance
            );
        }
    }
}
