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
use std::num::NonZeroU8;

use bevy::prelude::*;

use super::stop_contacts::HardReaction;
use super::transmission::{PARK_ENGAGE_SPEED, TransmissionReport};

/// One driven tick of the ledger: the belts' traction on the hull along its heading through it,
/// `[left, right]` (N), its patches' grip budget (N; `None` where a solver contact pushed), the
/// hull's gain in speed along its heading over it and the belts' mean gain in speed (m/s), and
/// both sprockets' force as the tick's step applied it (N).
#[derive(Clone, Copy, Debug)]
struct Sample {
    push: [f32; 2],
    grip: Option<f32>,
    gain: f32,
    belt_gain: f32,
    drive: f32,
}

/// This tick's opened sample: the hull's start velocity (m/s, world) and heading, the belts' mean
/// start speed (m/s), the sampled-contact push along that heading and those contacts' grip budget
/// (N), and what this tick's step reported: the drive direction it pulled along (±1 along the
/// heading; `None` when it asked no pull), the ladder and gear asked, whether its clutch sent
/// drive and whether it held, and both sprockets' force (N; `None` until it reports).
#[derive(Clone, Copy, Debug)]
struct Open {
    velocity: Vec3,
    heading: Vec3,
    belts: f32,
    push: [f32; 2],
    grip: f32,
    pull: Option<f32>,
    pulled_by: Option<Engaged>,
    driving: bool,
    locked: bool,
    drive: Option<f32>,
}

/// A ladder and gear: `(reverse, gear)`.
type Engaged = (bool, u8);

/// What a stalled hull's force balance bounds: at rest under a push that did not move it, the
/// world returned at least that push, and how much more it would return is unknown. A stall is
/// read only under belts the push did not move either, at rest or not gaining: belts gaining under
/// a stalled hull slide at the ground's limit, which no gear raises (MEASURED, ram into the standing
/// heap: third, second and first each stalled with the drivetrain gaining 185–259 kN), while belts
/// turning steadily are held by the gear, whose limit a lower one raises (MEASURED, standing heap
/// at full pedal, stop-go: fourth spun its belts at a steady 2.175 m/s, its crank governed at
/// 434 N m through the clutch, under a hull at rest for 1226 ticks).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stall {
    /// The drive direction pulled along, ±1 along the heading.
    pub dir: f32,
    /// The largest push along `dir` that has failed to move the hull (N).
    pub push: f32,
    /// The lowest gear whose pull has failed to move it.
    pub gear: u8,
}

/// A gear the Driver left downward, and what it delivered while failing: it is not retried while
/// the hull meets at least that push, under no more pedal and in the same detent. A detent's
/// steering takes its own share of the gear's work, so a push delivered in a turn says nothing of
/// the gear straight (MEASURED, flat ground at a third of the pedal: fourth left in a tight turn
/// delivering 22.6 kN against `R` of 29.0 kN, then priced at 53.9 kN straight and held back).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Failed {
    /// The ladder and gear left.
    pub gear: Engaged,
    /// What the gear sustained along its ladder when it was left (N): the resistance plus the
    /// drivetrain's gain over the window it was left in.
    pub push: f32,
    /// The pedal it failed under.
    pub command: f32,
    /// The steering detent it failed in.
    pub steer_step: u8,
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
    /// Both sprockets' force on the latest driven tick (N, forward positive): what the engaged
    /// gear delivers now ([`TransmissionReport::forces`]).
    pub drive: f32,
    /// The drivetrain's gain over the window as a force (N): hull mass times the belts' mean
    /// acceleration. Through a locked clutch the belts turn with the crank, so this is the revs
    /// rising or falling under load. The hull rings on its patches' elastic shear after a clutch
    /// takes up, and the belts, carrying the crank's inertia, do not (MEASURED, flat ground at a
    /// third of the pedal, 4th to 5th: the hull's speed swung 2.235 to 2.201 m/s over six ticks
    /// while the belts rose 2.212 to 2.227 m/s).
    pub drive_gain: f32,
    /// The sampled patches' mean grip budget over the window (N): the most traction any gear can
    /// put into the ground along the belts, which they carry when every patch slides along them.
    /// `None` when a solver contact pushed within the window: the solver's friction is Coulomb
    /// too, but its coefficient is not read back, so its grip limit is unmeasured.
    pub grip: Option<f32>,
    /// The bound a stall gives: `None` once the hull moves or no pull is asked.
    pub stall: Option<Stall>,
    /// The latest gear left downward while its memory holds.
    pub failed: Option<Failed>,
    /// The engaged gear's clutch sent drive through every sample in the window: a stall under it
    /// is the gear's own. A crank spooling to its launch rpm sends none, so its ticks try nothing
    /// (MEASURED on flat ground: a window of them read fourth as failed at rest).
    pub tried: bool,
    /// The engaged gear pulled every sample in the window through a locked clutch: the drive and
    /// the drivetrain's gain are that gear's alone. A paid cut, another gear or a tick without
    /// pull starts the run again, and a slipping clutch its locked part: a clutch taking up sends
    /// its capacity and the crank's stored speed, whatever the gear (MEASURED, flat ground at a
    /// third of the pedal, 4th to 5th: four ticks at clutch capacity lift the push to 214 kN
    /// against 35 kN once locked).
    pub observed: bool,
}

/// The belts' contacts on the hull at one driving tick (N, world frame).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Contacts {
    /// The traction this tick's sampled contacts apply to the hull, `[left, right]`.
    pub(crate) soft: [Vec3; 2],
    /// Those contacts' grip budget, both sides ([`super::forces::SideReport::grip_budget`]).
    pub(crate) grip: f32,
    /// The solver contacts over the physics step that just ran, `None` where a side had none.
    pub(crate) hard: [Option<HardReaction>; 2],
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
    failed: Option<Failed>,
    /// The gear asked to pull the latest samples, and how many of the latest of those its clutch
    /// drove and held.
    run: Option<(Engaged, usize, usize)>,
    /// The reading this tick's choice used: telemetry and probes read it.
    pub(crate) reading: Option<Sensed>,
}

impl DriverSense {
    /// An empty ledger averaging one cut of `shift_ticks`.
    pub fn for_cut(shift_ticks: NonZeroU8) -> Self {
        let window = usize::from(shift_ticks.get());
        Self {
            window,
            samples: VecDeque::with_capacity(window),
            open: None,
            stall: None,
            failed: None,
            run: None,
            reading: None,
        }
    }

    /// One driving tick. `velocity` is the hull's velocity now (m/s, world), `heading` its unit
    /// forward axis, `belts` both belts' mean speed now (m/s, forward positive) and `mass` the
    /// hull's mass (kg). A contact's traction counts as push only along the
    /// sample's heading: a contact's support, and its traction's other components, are what the
    /// hull meets. The step that just ran carried last tick's sampled push and this tick's `hard`
    /// push from last tick's velocity to `velocity`, so that sample closes here.
    pub(crate) fn tick(
        &mut self,
        velocity: Vec3,
        heading: Vec3,
        belts: f32,
        mass: f32,
        contacts: Contacts,
        dt: f32,
    ) -> Option<Sensed> {
        let Contacts { soft, grip, hard } = contacts;
        if let Some(open) = self.open.take() {
            let mut push = open.push;
            let drive = open
                .drive
                .expect("the step that drove an opened tick reports its sprockets' force");
            // No solver contact pushed a side without one.
            for (push, hard) in push.iter_mut().zip(hard) {
                if let Some(hard) = hard {
                    *push += hard.friction.dot(open.heading);
                }
            }
            let grip = hard.iter().all(Option::is_none).then_some(open.grip);
            let gain = (velocity - open.velocity).dot(open.heading);
            let at_rest = |velocity: Vec3| velocity.dot(open.heading).abs() < PARK_ENGAGE_SPEED;
            if !at_rest(velocity) {
                self.stall = None; // The hull moves: no push has failed to move it.
            } else if let (Some(dir), Some((_, gear)), true) =
                (open.pull, open.pulled_by, open.driving)
                && at_rest(open.velocity)
                && dir * gain <= 0.0
                && (belts.abs() < PARK_ENGAGE_SPEED || dir * (belts - open.belts) <= 0.0)
            {
                let failed = dir * (push[0] + push[1]);
                self.stall = Some(match self.stall {
                    Some(stall) if stall.dir == dir => Stall {
                        dir,
                        push: stall.push.max(failed),
                        gear: stall.gear.min(gear),
                    },
                    _ => Stall {
                        dir,
                        push: failed,
                        gear,
                    },
                });
            }
            let [driving, held] = [open.driving, open.locked].map(usize::from);
            self.run = match (open.pulled_by, self.run) {
                (Some(by), Some((gear, drove, locked))) if by == gear => {
                    Some((gear, (drove + 1) * driving, (locked + 1) * held))
                }
                (by, _) => by.map(|by| (by, driving, held)),
            };
            if self.samples.len() == self.window {
                self.samples.pop_front();
            }
            self.samples.push_back(Sample {
                push,
                grip,
                gain,
                belt_gain: belts - open.belts,
                drive,
            });
        }
        self.open = Some(Open {
            velocity,
            heading,
            belts,
            push: soft.map(|soft| soft.dot(heading)),
            grip,
            pull: None,
            pulled_by: None,
            driving: false,
            locked: false,
            drive: None,
        });
        self.reading = (!self.samples.is_empty()).then(|| {
            assert!(
                mass.is_finite() && mass > 0.0 && dt > 0.0,
                "a driven hull has finite mass and its tick a positive step"
            );
            let count = self.samples.len() as f32;
            let [left, right, gain, belt_gain] =
                self.samples.iter().fold([0.0; 4], |[l, r, g, b], s| {
                    [l + s.push[0], r + s.push[1], g + s.gain, b + s.belt_gain]
                });
            let latest = self.samples.back().expect("the reading has a sample");
            let grip = self.samples.iter().map(|s| s.grip).sum::<Option<f32>>();
            Sensed {
                speed: velocity.dot(heading),
                resistance: (left + right) / count - mass * gain / (count * dt),
                push_difference: (left - right) / count,
                drive: latest.drive,
                drive_gain: mass * belt_gain / (count * dt),
                grip: grip.map(|grip| grip / count),
                stall: self.stall,
                failed: self.failed,
                tried: self.run.is_some_and(|(_, drove, _)| drove >= self.window),
                observed: self.run.is_some_and(|(_, _, locked)| locked >= self.window),
            }
        });
        self.reading
    }

    /// This tick's step pays a shift cut: the drive does not reach the belts, so the tick is not
    /// felt and its sample is dropped.
    pub(crate) fn cut(&mut self) {
        self.open = None;
        self.run = None;
    }

    /// This tick's step as delivery ran it: a paid cut is not felt, a step that asked no pull
    /// withdraws the push a stall's bound was read from, a cut to a lower gear remembers the gear
    /// it left, and a driven step records its clutch and its sprockets' force.
    pub(crate) fn drove(&mut self, report: &TransmissionReport) {
        let dir = if report.reverse { -1.0 } else { 1.0 };
        let pull = report.pulling.then_some(dir);
        if self.stall.is_some_and(|stall| pull != Some(stall.dir)) {
            self.stall = None;
        }
        let engaged = (report.reverse, report.gear);
        // Engaged again, on the other ladder, released, pressed harder or steered otherwise, the
        // gear is tried anew.
        if self.failed.is_some_and(|failed| {
            !report.pulling
                || failed.gear.0 != engaged.0
                || (failed.gear.1 <= engaged.1 && !report.shifting)
                || report.command > failed.command
                || report.steer_step != failed.steer_step
        }) {
            self.failed = None;
        }
        if let (true, Some((left, _, _)), Some(reading)) = (report.shifting, self.run, self.reading)
            && left.0 == engaged.0
            && engaged.1 < left.1
        {
            self.failed = Some(Failed {
                gear: left,
                push: dir * (reading.resistance + reading.drive_gain),
                command: report.command,
                steer_step: report.steer_step,
            });
        }
        if report.shifting {
            self.cut();
        } else if let Some(open) = &mut self.open {
            open.pull = pull;
            open.pulled_by = report.pulling.then_some((report.reverse, report.gear));
            open.driving = report.driving;
            open.locked = report.locked;
            open.drive = Some(report.forces.iter().sum());
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
    const CUT: NonZeroU8 = NonZeroU8::new(20).expect("a fixture cut of twenty ticks");
    const CUT_START: usize = 60;

    fn push(tick: usize) -> f32 {
        if tick < CUT_START {
            45_000.0
        } else if tick < CUT_START + usize::from(CUT.get()) {
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
                let reading = sense.tick(forward * speed, forward, speed, MASS, contacts, DT);
                if push == 0.0 {
                    sense.cut();
                } else {
                    sense.drove(&TransmissionReport::default());
                }
                speed += (push - resist(push)) / MASS * DT;
                reported = [
                    Some(HardReaction {
                        belt: push * 0.25,
                        friction: forward * push * 0.25,
                    }),
                    None,
                ];
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
        let end = CUT_START + usize::from(CUT.get());
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
            driving: true,
            ..Default::default()
        };
        let released = TransmissionReport::default();
        let mut feel = |speed: f32, push: f32, report: &TransmissionReport| {
            let contacts = Contacts {
                soft: [forward * push * 0.5; 2],
                grip: 0.0,
                hard: [None; 2],
            };
            let reading = sense.tick(forward * speed, forward, speed, MASS, contacts, DT);
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

    /// A gear left downward is remembered only while what it failed under holds: the same ladder,
    /// no more pedal, the same detent, pull still asked and that gear not engaged again. The
    /// Driver holds a gear back on the memory, so a memory outliving any of these holds it back
    /// unseen. Fixture: third gear pulling at rest at a third of the pedal, then a cut to first.
    #[test]
    fn a_left_gear_is_remembered_while_its_failure_holds() {
        let forward = Vec3::NEG_Z;
        let pull = |gear: u8, command: f32, steer_step: u8| TransmissionReport {
            gear,
            command,
            steer_step,
            pulling: true,
            driving: true,
            ..Default::default()
        };
        let after = |report: TransmissionReport| {
            let mut sense = DriverSense::for_cut(CUT);
            let contacts = Contacts {
                soft: [forward * 10_000.0; 2],
                grip: 0.0,
                hard: [None; 2],
            };
            let cut = TransmissionReport {
                shifting: true,
                ..pull(1, 0.3, 0)
            };
            for report in [pull(3, 0.3, 0), pull(3, 0.3, 0), cut, report] {
                sense.tick(Vec3::ZERO, forward, 0.0, MASS, contacts, DT);
                sense.drove(&report);
            }
            sense
                .tick(Vec3::ZERO, forward, 0.0, MASS, contacts, DT)
                .and_then(|reading| reading.failed)
                .map(|failed| failed.gear)
        };
        assert_eq!(after(pull(1, 0.3, 0)), Some((false, 3)), "first, as it was");
        for (report, case) in [
            (pull(1, 0.5, 0), "pressed harder"),
            (pull(1, 0.3, 1), "steered"),
            (pull(3, 0.3, 0), "third engaged again"),
            (TransmissionReport::default(), "released"),
        ] {
            assert_eq!(after(report), None, "{case}");
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
            let reading = sense.tick(velocity, heading, velocity.dot(heading), MASS, contacts, DT);
            sense.drove(&TransmissionReport::default());
            if let Some(reading) = reading {
                // DERIVED: no push and no change of velocity close every sample at zero.
                assert_eq!(reading.resistance, 0.0, "tick {tick}");
            }
        }
    }
}
