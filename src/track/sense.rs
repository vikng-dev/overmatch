//! What the Driver feels of the hull (the game's tank driving model): its speed along its heading
//! and the resistance it meets, the resistance averaged over one shift cut of driven ticks. Speeds
//! are m/s and forces N, along the hull's heading with forward positive.
//!
//! Resistance closes the hull's force ledger from its own side: the belts' traction on the hull
//! along its heading, less hull mass times the hull's gain in velocity along it. Whatever else acts
//! along the heading (grade, rolling, a contact's support, an obstacle's reaction, a tow, recoil)
//! is in it, so no load model is needed; a gear change does not move it, because the push the hull
//! receives is what the ground returned, whatever gear sent it. The window is one shift cut because
//! the landing forecast needs exactly the speed the world takes from the hull in one cut, and a
//! single-tick impulse must not decide alone.
//!
//! Each tick's sample is taken along the heading that tick opened with, where its sampled contacts
//! were evaluated: speeds along two headings differ by `V·Δh` besides the gain (DERIVED:
//! `d(V·h)/dt = a·h + V·dh/dt`), which a pitching or yawing hull would read as resistance.
//!
//! A paid cut's ticks are not felt: with the drive interrupted there is nothing for it to meet, and
//! an obstacle's reaction, which exists only under push, vanishes with it. Sampling the cut would
//! read a stalled hull's obstacle as gone at the cut's end, where the free choice at rest reads it.

use std::collections::VecDeque;

use bevy::prelude::*;

use super::transmission::{PARK_ENGAGE_SPEED, TransmissionReport};

/// One driven tick of the ledger: the belts' traction on the hull along its heading through it,
/// `[left, right]` (N), its patches' grip budget (N; `None` where a solver contact pushed), and
/// the hull's gain in speed along its heading over it (m/s).
#[derive(Clone, Copy, Debug)]
struct Sample {
    push: [f32; 2],
    grip: Option<f32>,
    gain: f32,
}

/// This tick's opened sample: the hull's start velocity (m/s, world) and heading, the
/// sampled-contact push along that heading and those contacts' grip budget (N), and the drive
/// direction this tick's step pulled along (±1 along the heading; `None` when it asked no pull).
#[derive(Clone, Copy, Debug)]
struct Open {
    velocity: Vec3,
    heading: Vec3,
    push: [f32; 2],
    grip: f32,
    pull: Option<f32>,
}

/// What a stalled hull's force balance bounds: at rest under a push that did not move it, the
/// world returned at least that push, and how much more it would return is unknown.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stall {
    /// The drive direction pulled along, ±1 along the heading.
    pub dir: f32,
    /// The largest push along `dir` that has failed to move the hull (N).
    pub push: f32,
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
    /// The sampled patches' mean grip budget over the window (N): the most traction any gear can
    /// put into the ground along the belts, which they carry when every patch slides along them.
    /// `None` when a solver contact pushed within the window: the solver's friction is Coulomb
    /// too, but its coefficient is not read back, so its grip limit is unmeasured.
    pub grip: Option<f32>,
    /// The bound a stall gives: `None` once the hull moves or no pull is asked.
    pub stall: Option<Stall>,
}

/// The belts' contacts on the hull at one driving tick (N, world frame).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Contacts {
    /// The traction this tick's sampled contacts apply to the hull, `[left, right]`.
    pub(crate) soft: [Vec3; 2],
    /// Those contacts' grip budget, both sides ([`super::forces::SideReport::grip_budget`]).
    pub(crate) grip: f32,
    /// The solver contacts' friction on the hull over the physics step that just ran, `None`
    /// where a side had no solver contact.
    pub(crate) hard: [Option<Vec3>; 2],
}

/// The Driver's ledger: server-side driving state, built empty at spawn from the tank's cut
/// length. Nothing is sensed until one driven tick has closed.
#[derive(Component, Clone, Debug)]
pub struct DriverSense {
    window: usize,
    samples: VecDeque<Sample>,
    /// The solver's push for this tick is read on the next one, so the sample closes there.
    open: Option<Open>,
    stall: Option<Stall>,
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
            stall: None,
            reading: None,
        }
    }

    /// One driving tick. `velocity` is the hull's velocity now (m/s, world), `heading` its unit
    /// forward axis and `mass` its mass (kg). A contact's traction counts as push only along the
    /// sample's heading: a contact's support, and its traction's other components, are what the
    /// hull meets. The step that just ran carried last tick's sampled push and this tick's `hard`
    /// push from last tick's velocity to `velocity`, so that sample closes here.
    pub(crate) fn tick(
        &mut self,
        velocity: Vec3,
        heading: Vec3,
        mass: f32,
        contacts: Contacts,
        dt: f32,
    ) -> Option<Sensed> {
        let Contacts { soft, grip, hard } = contacts;
        if let Some(open) = self.open.take() {
            let mut push = open.push;
            for (push, hard) in push.iter_mut().zip(hard) {
                // No solver contact pushed a side without one.
                *push += hard.map_or(0.0, |hard| hard.dot(open.heading));
            }
            let grip = hard.iter().all(Option::is_none).then_some(open.grip);
            let gain = (velocity - open.velocity).dot(open.heading);
            let at_rest = |velocity: Vec3| velocity.dot(open.heading).abs() < PARK_ENGAGE_SPEED;
            if !at_rest(velocity) {
                self.stall = None; // The hull moves: no push has failed to move it.
            } else if let Some(dir) = open.pull
                && at_rest(open.velocity)
                && dir * gain <= 0.0
            {
                let failed = dir * (push[0] + push[1]);
                self.stall = Some(Stall {
                    dir,
                    push: match self.stall {
                        Some(stall) if stall.dir == dir => stall.push.max(failed),
                        _ => failed,
                    },
                });
            }
            if self.samples.len() == self.window {
                self.samples.pop_front();
            }
            self.samples.push_back(Sample { push, grip, gain });
        }
        self.open = Some(Open {
            velocity,
            heading,
            push: soft.map(|soft| soft.dot(heading)),
            grip,
            pull: None,
        });
        self.reading = (!self.samples.is_empty()).then(|| {
            assert!(
                mass.is_finite() && mass > 0.0 && dt > 0.0,
                "a driven hull has finite mass and its tick a positive step"
            );
            let count = self.samples.len() as f32;
            let [left, right, gain] = self.samples.iter().fold([0.0; 3], |[l, r, g], s| {
                [l + s.push[0], r + s.push[1], g + s.gain]
            });
            let grip = self.samples.iter().map(|s| s.grip).sum::<Option<f32>>();
            Sensed {
                speed: velocity.dot(heading),
                resistance: (left + right) / count - mass * gain / (count * dt),
                push_difference: (left - right) / count,
                grip: grip.map(|grip| grip / count),
                stall: self.stall,
            }
        });
        self.reading
    }

    /// This tick's step pays a shift cut: the drive does not reach the belts, so the tick is not
    /// felt and its sample is dropped.
    pub(crate) fn cut(&mut self) {
        self.open = None;
    }

    /// This tick's step as delivery ran it: a paid cut is not felt, and a step that asked no pull
    /// withdraws the push a stall's bound was read from.
    pub(crate) fn drove(&mut self, report: &TransmissionReport) {
        let dir = if report.reverse { -1.0 } else { 1.0 };
        let pull = report.pulling.then_some(dir);
        if self.stall.is_some_and(|stall| pull != Some(stall.dir)) {
            self.stall = None;
        }
        if report.shifting {
            self.cut();
        } else if let Some(open) = &mut self.open {
            open.pull = pull;
        }
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
        let forward = Vec3::NEG_Z;
        (0..200)
            .map(|tick| {
                let push = push(tick);
                let soft = [push * 0.25, push * 0.5].map(|push| forward * push);
                let contacts = Contacts {
                    soft,
                    grip: 0.0,
                    hard: reported,
                };
                let reading = sense.tick(forward * speed, forward, MASS, contacts, DT);
                if push == 0.0 {
                    sense.cut();
                }
                speed += (push - resist(push)) / MASS * DT;
                reported = [Some(forward * push * 0.25), None];
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

    /// A stalled hull bounds what it meets by the largest push that failed to move it, only while
    /// it stays stalled under that pull: the Driver's choice at a stall relies on the bound, and on
    /// its dropping the moment the hull moves or the pull is withdrawn. Fixture: a hull held at rest
    /// under a push of 30, 60 then 15 kN, then moving at 1 m/s, stalled again, and released.
    #[test]
    fn a_stall_bounds_resistance_by_its_largest_failed_push() {
        let forward = Vec3::NEG_Z;
        let mut sense = DriverSense::for_cut(CUT);
        let pulling = TransmissionReport {
            pulling: true,
            ..Default::default()
        };
        let released = TransmissionReport::default();
        let mut feel = |speed: f32, push: f32, report: &TransmissionReport| {
            let contacts = Contacts {
                soft: [forward * push * 0.5; 2],
                grip: 0.0,
                hard: [None; 2],
            };
            let reading = sense.tick(forward * speed, forward, MASS, contacts, DT);
            sense.drove(report);
            reading
                .and_then(|reading| reading.stall)
                .map(|stall| stall.push)
        };
        let script = [
            (0.0, 30_000.0, &pulling, None),
            (0.0, 60_000.0, &pulling, Some(30_000.0)),
            (0.0, 15_000.0, &pulling, Some(60_000.0)),
            (0.0, 15_000.0, &pulling, Some(60_000.0)),
            (1.0, 15_000.0, &pulling, None),
            (0.0, 15_000.0, &pulling, None),
            (0.0, 15_000.0, &released, Some(15_000.0)),
            (0.0, 15_000.0, &pulling, None),
        ];
        for (tick, (speed, push, report, want)) in script.into_iter().enumerate() {
            assert_eq!(feel(speed, push, report), want, "tick {tick}");
        }
    }

    /// Turning the heading is not acceleration: a hull coasting at constant velocity while it
    /// pitches and yaws meets nothing and reads no resistance, where speeds along each tick's own
    /// heading read `m·V·dh/dt`. Fixture: 3 m/s forward and 0.5 m/s up, the heading turning at
    /// 0.5 rad/s about an axis between pitch and yaw, no force.
    #[test]
    fn a_turning_heading_reads_no_resistance() {
        let mut sense = DriverSense::for_cut(CUT);
        let velocity = Vec3::new(0.0, 0.5, -3.0);
        let axis = Vec3::new(1.0, 1.0, 0.0).normalize();
        for tick in 0..200 {
            let heading = Quat::from_axis_angle(axis, 0.5 * DT * tick as f32) * Vec3::NEG_Z;
            let contacts = Contacts {
                soft: [Vec3::ZERO; 2],
                grip: 0.0,
                hard: [None; 2],
            };
            let reading = sense.tick(velocity, heading, MASS, contacts, DT);
            if let Some(reading) = reading {
                // DERIVED: no push and no change of velocity close every sample at zero.
                assert_eq!(reading.resistance, 0.0, "tick {tick}");
            }
        }
    }
}
