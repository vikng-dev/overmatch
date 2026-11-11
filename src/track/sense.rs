//! What the Driver feels of the hull (the game's tank driving model): its speed along its heading
//! and the resistance it meets, the resistance averaged over one shift cut of driven ticks. Speeds
//! are m/s and forces N, along the hull's heading with forward positive.
//!
//! Resistance closes the hull's force ledger from its own side: the belts' push on the hull, less
//! hull mass times the hull's gain in speed. Whatever else acts along the heading (grade, rolling,
//! an obstacle's reaction, a tow, recoil) is in it, so no load model is needed; a gear change does
//! not move it, because the push the hull receives is what the ground returned, whatever gear sent
//! it. The window is one shift cut because the landing forecast needs exactly the speed the world
//! takes from the hull in one cut, and a single-tick impulse must not decide alone.
//!
//! A paid cut's ticks are not felt: with the drive interrupted there is nothing for it to meet, and
//! an obstacle's reaction, which exists only under push, vanishes with it. Sampling the cut would
//! read a stalled hull's obstacle as gone at the cut's end, where the free choice at rest reads it.

use std::collections::VecDeque;

use bevy::prelude::*;

/// One driven tick of the ledger: the belts' push on the hull through it, `[left, right]` (N),
/// and the hull's gain in speed along its heading over it (m/s).
#[derive(Clone, Copy, Debug)]
struct Sample {
    push: [f32; 2],
    gain: f32,
}

/// This tick's opened sample: the hull's start speed (m/s) and the sampled-contact push (N).
#[derive(Clone, Copy, Debug)]
struct Open {
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
/// length. Nothing is sensed until one driven tick has closed.
#[derive(Component, Clone, Debug)]
pub struct DriverSense {
    window: usize,
    samples: VecDeque<Sample>,
    /// The solver's push for this tick is read on the next one, so the sample closes there.
    open: Option<Open>,
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
        if let Some(open) = self.open.take() {
            let mut push = open.push;
            for (push, hard) in push.iter_mut().zip(hard) {
                *push += hard.unwrap_or(0.0); // No solver contact pushed that side.
            }
            if self.samples.len() == self.window {
                self.samples.pop_front();
            }
            self.samples.push_back(Sample {
                push,
                gain: speed - open.speed,
            });
        }
        self.open = Some(Open { speed, push: soft });
        self.reading = (!self.samples.is_empty()).then(|| {
            assert!(
                mass.is_finite() && mass > 0.0 && dt > 0.0,
                "a driven hull has finite mass and its tick a positive step"
            );
            let count = self.samples.len() as f32;
            let [left, right, gain] = self.samples.iter().fold([0.0; 3], |[l, r, g], s| {
                [l + s.push[0], r + s.push[1], g + s.gain]
            });
            Sensed {
                speed,
                resistance: (left + right) / count - mass * gain / (count * dt),
                push_difference: (left - right) / count,
            }
        });
        self.reading
    }

    /// This tick's step pays a shift cut: the drive does not reach the belts, so the tick is not
    /// felt and its sample is dropped.
    pub(crate) fn cut(&mut self) {
        self.open = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture: a 40 t point hull at the shipped tick, a 20-tick cut from tick 60, and a push of
    /// 45 kN before it and 60 kN after.
    const MASS: f32 = 40_000.0;
    const DT: f32 = 1.0 / 64.0;
    const CUT: u8 = 20;
    const CUT_START: usize = 60;

    fn push(tick: usize) -> f32 {
        if tick < CUT_START {
            45_000.0
        } else if tick < CUT_START + usize::from(CUT) {
            0.0
        } else {
            60_000.0
        }
    }

    /// Each tick's reading under the fixture push, against a resisting force that is a function
    /// of the push. A quarter of the push is the solver contacts' share, reported a tick late.
    fn readings(resist: impl Fn(f32) -> f32) -> Vec<Option<Sensed>> {
        let mut sense = DriverSense::for_cut(CUT);
        let (mut speed, mut reported) = (3.0_f32, [None; 2]);
        (0..200)
            .map(|tick| {
                let push = push(tick);
                let reading = sense.tick(speed, MASS, [push * 0.25, push * 0.5], reported, DT);
                if push == 0.0 {
                    sense.cut();
                }
                speed += (push - resist(push)) / MASS * DT;
                reported = [Some(push * 0.25), None];
                reading
            })
            .collect()
    }

    /// A cut does not move the reading: the landing forecast and the choice at the cut's end rely
    /// on it. Under a constant resisting force the hull slows through the cut and the reading
    /// stays on the true resistance, where a push-only or acceleration-only reading moves. Against
    /// an obstacle that returns whatever pushes it, the reading at the cut's end is the push that
    /// failed to move it, where a ledger that sampled the cut reads the obstacle as gone.
    #[test]
    fn resistance_is_unchanged_through_a_paid_cut() {
        let near = |read: f32, want: f32| (read - want).abs() < want * 1e-3;
        let grade = readings(|_| 30_000.0);
        assert!(grade[0].is_none(), "nothing is sensed before a tick closes");
        for (tick, reading) in grade.iter().enumerate().skip(1) {
            let read = reading
                .expect("every later tick reads the ledger")
                .resistance;
            assert!(near(read, 30_000.0), "grade, tick {tick}: read {read}");
        }
        let obstacle = readings(|push| push);
        let end = CUT_START + usize::from(CUT);
        for (tick, want) in [(CUT_START, 45_000.0), (end, 45_000.0), (end + 20, 60_000.0)] {
            let read = obstacle[tick]
                .expect("the ledger has closed ticks")
                .resistance;
            assert!(
                near(read, want),
                "obstacle, tick {tick}: read {read}, want {want}"
            );
        }
    }
}
