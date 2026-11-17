//! The Driver (the game's tank driving model): what it feels of the hull, and the gear it chooses
//! on that. Speeds are m/s and forces N, along the hull's heading with forward positive. The
//! design, the measurements behind each clause and the rejected forms are
//! `.agents/docs/design/track-model/architecture.md` section 5a.
//!
//! [`DriverSense`] is the ledger, built empty at spawn from the tank's cut length. Each driving
//! tick [`DriverSense::tick`] closes the last tick's sample and returns the reading, [`Sensed`];
//! the transmission's step hands it to [`choose`], and [`DriverSense::drove`] records how the step
//! ran. Outside the track model, probes read a reading's speed, resistance and push difference;
//! its other fields are the track model's.
//!
//! The ledger feels the hull's speed along its heading and the resistance it meets, averaged over
//! one shift cut of driven ticks. Resistance closes the hull's force ledger from its own side: the
//! belts' traction on the hull along its heading, less hull mass times the hull's gain in velocity
//! along it. Whatever else acts along the heading (grade, rolling, a contact's support, an
//! obstacle's reaction, a tow, recoil) is in it, so no load model is needed; a gear change does not
//! move it, because the push the hull receives is what the ground returned, whatever gear sent it.
//! The window is one shift cut, and no fewer than [`MIN_SAMPLES`] ticks: the landing forecast needs
//! exactly the speed the world takes from the hull in one cut, and a single-tick impulse must not
//! decide alone.
//!
//! Each tick's sample is taken along the heading that tick opened with, where its sampled contacts
//! were evaluated: speeds along two headings differ by `V·Δh` besides the gain (DERIVED:
//! `d(V·h)/dt = a·h + V·dh/dt`), which a pitching or yawing hull would read as resistance.
//!
//! A paid cut's ticks are not felt: with the drive interrupted there is nothing for it to meet, and
//! an obstacle's reaction, which exists only under push, vanishes with it. Sampling the cut would
//! read a stalled hull's obstacle as gone at the cut's end, where the free choice at rest reads it.
//!
//! Each tick also reads the terms of the load a gear carries ([`Evidence`]), so a reading knows its
//! own error.

use std::collections::VecDeque;
use std::num::NonZeroU8;

use bevy::prelude::*;

use super::stop_contacts::HardReaction;
use super::transmission::{
    ClutchState, DriveConstruction, LaunchState, TransmissionInput, TransmissionParams,
    TransmissionReport, TransmissionState, candidate_pull, coupled_mass, force_margin,
    landing_pull, park_engage_speed, steer_ratio, viable_pull,
};

/// Resolution: the fewest driven ticks a reading spans, since one gives a mean but no sample
/// standard error.
const MIN_SAMPLES: usize = 2;

/// One driven tick of the ledger: the belts' traction on the hull along its heading through it,
/// `[left, right]` (N), what resisted forward travel over it (N: that traction less hull mass times
/// the hull's gain in speed along its heading, over the tick), its contacts' grip budget and their
/// rolling resistance against forward travel, whole and as delivered (N), the belts' mean gain in
/// speed (m/s), and both sprockets' force as the tick's step applied it (N).
#[derive(Clone, Copy, Debug)]
struct Sample {
    push: [f32; 2],
    resistance: f32,
    grip: f32,
    rolling: f32,
    rolled: f32,
    belt_gain: f32,
    drive: f32,
}

impl Sample {
    /// The tick's terms of the load a gear carries ([`Evidence`]).
    fn terms(&self) -> [f64; 3] {
        [
            self.resistance - self.rolled,
            self.rolling,
            self.push[0] - self.push[1],
        ]
        .map(f64::from)
    }
}

/// Driven ticks' load terms: their count, mean and co-moment `Σ (x − x̄)(x − x̄)ᵀ`, gathered one
/// tick at a time (Welford's update).
#[derive(Clone, Copy, Debug, Default)]
struct Moments {
    count: f64,
    mean: [f64; 3],
    co: [[f64; 3]; 3],
}

impl Moments {
    fn add(&mut self, x: [f64; 3]) {
        self.count += 1.0;
        let before = self.mean;
        for (mean, x) in self.mean.iter_mut().zip(x) {
            *mean += (x - *mean) / self.count;
        }
        for (i, row) in self.co.iter_mut().enumerate() {
            for (j, co) in row.iter_mut().enumerate() {
                *co += (x[i] - before[i]) * (x[j] - self.mean[j]);
            }
        }
    }

    fn evidence(&self) -> Evidence {
        let n = self.count;
        assert!(
            n >= MIN_SAMPLES as f64,
            "a reading's error is read over `MIN_SAMPLES` ticks or more"
        );
        Evidence {
            mean: self.mean.map(|mean| mean as f32),
            spread: self
                .co
                .map(|row| row.map(|co| (co / (n * (n - 1.0))) as f32)),
        }
    }
}

/// What a set of driven ticks tells of the load a gear carries, `dir·(R − rolled) + rolling +
/// share·ΔR` (N): each tick reads its own resistance `R`, the rolling its contacts delivered and
/// their whole rolling past the fade ([`Sensed::rolled`], [`Sensed::rolling`]) and its belts' push
/// difference `ΔR`, so the set gives each term's mean and that mean's error with the terms'
/// covariance kept: rolling delivered and whole move together in a crawl, and `R` holds the
/// delivered share.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Evidence {
    /// The mean over the ticks of `(R − rolled, rolling, ΔR)` (N).
    mean: [f32; 3],
    /// The squared standard errors of those means and their covariances (N²): the terms' sample
    /// covariance over the `n` ticks, `Σ (x − x̄)(x − x̄)ᵀ / (n − 1)`, over `n`.
    spread: [[f32; 3]; 3],
}

impl Evidence {
    /// The standard error of the load's mean (N), `√(aᵀ S a)` for the spread `S` and `a = (dir, 1,
    /// share)`: the sample standard error of the per-tick load, `√(Σ (L_i − L̄)² / (n (n − 1)))`.
    /// It takes the ticks as independent: a load trending across them reads as scatter, and a
    /// ripple the set spans whole reads as scatter its mean does not carry.
    fn error(&self, dir: f32, share: f32) -> f32 {
        let a = [dir, 1.0, share];
        let form: f32 = (0..3)
            .flat_map(|i| (0..3).map(move |j| (i, j)))
            .map(|(i, j)| a[i] * self.spread[i][j] * a[j])
            .sum();
        // A covariance's form is non-negative; its rounding need not be.
        form.max(0.0).sqrt()
    }
}

/// This tick's opened sample: the hull's start velocity (m/s, world) and heading, the belts' mean
/// start speed (m/s), the sampled-contact push along that heading, those contacts' grip budget and
/// their rolling resistance against forward travel along it, whole and as delivered (N), and what
/// this tick's step reported: the drive direction it pulled along (±1 along the heading; `None`
/// when it asked no pull), the ladder and gear asked, whether its clutch sent drive and whether it
/// held, and both sprockets' force (N; `None` until it reports).
#[derive(Clone, Copy, Debug)]
struct Open {
    velocity: Vec3,
    heading: Vec3,
    belts: f32,
    push: [f32; 2],
    grip: f32,
    rolling: f32,
    rolled: f32,
    pull: Option<f32>,
    pulled_by: Option<Engaged>,
    driving: bool,
    locked: bool,
    drive: Option<f32>,
}

/// A ladder and gear: `(reverse, gear)`.
type Engaged = (bool, u8);

/// What a stalled hull's force balance bounds: at rest under a push that did not move it, the
/// world returned at least that push, and how much more it would return is unknown. Of it, the
/// rolling the contacts delivered at that tick is rolling, so the bound is on the rest. A stall is
/// read only under belts the push did not move either, at rest or not gaining: belts gaining under
/// a stalled hull slide at the ground's limit, which no gear raises, while belts turning steadily
/// are held by the gear, whose limit a lower one raises.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Stall {
    /// The drive direction pulled along, ±1 along the heading.
    dir: f32,
    /// The largest push along `dir` that has failed to move the hull, less the rolling its
    /// contacts delivered (N): a floor on what the hull meets besides delivered rolling.
    bound: f32,
}

/// The Driver's reading for one tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sensed {
    /// The hull's speed along its heading at this tick's start (m/s).
    pub(crate) speed: f32,
    /// What resisted forward travel over the window (N): the belts' mean push less hull mass
    /// times the hull's mean acceleration along its heading.
    pub(crate) resistance: f32,
    /// The window's [`Evidence::spread`]: its means are [`Self::window`]'s.
    spread: [[f32; 3]; 3],
    /// The left belt's mean push less the right's over the window (N).
    pub(crate) push_difference: f32,
    /// Both sprockets' mean force over the window (N, forward positive): what the engaged gear
    /// delivers ([`TransmissionReport::forces`]). Under [`Self::observed`] the window is the
    /// gear's locked run, so no single tick decides.
    drive: f32,
    /// The contacts' mean grip budget over the window (N): the most traction any gear can put into
    /// the ground along the belts, which they carry when every contact slides along them. The
    /// sampled patches' budget and the solver contacts' Coulomb limit, its coefficient times each
    /// point's normal force ([`HardReaction::grip`]).
    grip: f32,
    /// The sampled contacts' mean rolling resistance over the window once the hull travels past
    /// the fade ([`super::forces::ROLLING_FADE_SPEED`]), N against travel along the heading
    /// either way. Solver contacts carry none.
    rolling: f32,
    /// The rolling the contacts delivered over the window, inside [`Self::resistance`] (N, against
    /// forward travel positive): none at rest, all of [`Self::rolling`] past the fade forward.
    rolled: f32,
    /// The bound a stall gives: `None` once the hull moves or no pull is asked. It is a floor on
    /// what the hull meets, and outlives the stall: whether the hull stalls now is
    /// [`Self::stalled`].
    stall: Option<Stall>,
    /// The hull stalls now under the engaged gear: a stall's bound holds, the gear drove every
    /// sample of the window with the hull at rest, and over the window its belts came to rest or
    /// did not gain. The stall is the gear's own only where its clutch sent drive throughout: a
    /// crank spooling to its launch rpm sends none.
    stalled: bool,
    /// The engaged gear pulled every sample in the window: the window is its own. A paid cut,
    /// another gear or a tick without pull starts the run again.
    pulled: bool,
    /// The engaged gear pulled every sample in the window through a locked clutch: the drive is
    /// that gear's alone. A slipping clutch starts the run again: a clutch taking up sends its
    /// capacity and the crank's stored speed, whatever the gear.
    observed: bool,
}

impl Sensed {
    /// Fixture reading: a hull riding its belts at `speed` (m/s) and meeting `load` (N) along its
    /// heading, read without error over a window its gear pulled whole through a held clutch,
    /// delivering `load`, its contacts' grip budget `grip` (N).
    #[cfg(test)]
    pub(crate) fn steady(speed: f32, load: f32, grip: f32) -> Self {
        Self {
            speed,
            resistance: load,
            spread: [[0.0; 3]; 3],
            push_difference: 0.0,
            drive: load,
            grip,
            rolling: 0.0,
            rolled: 0.0,
            stall: None,
            stalled: false,
            pulled: true,
            observed: true,
        }
    }

    /// The window's evidence on the load a gear carries.
    fn window(&self) -> Evidence {
        Evidence {
            mean: [
                self.resistance - self.rolled,
                self.rolling,
                self.push_difference,
            ],
            spread: self.spread,
        }
    }
}

/// The belts' contacts on the hull at one driving tick (N, world frame).
#[derive(Clone, Copy, Debug)]
pub(super) struct Contacts {
    /// The traction this tick's sampled contacts apply to the hull, `[left, right]`.
    pub(super) soft: [Vec3; 2],
    /// Those contacts' grip budget, both sides ([`super::forces::SideReport::grip_budget`]).
    pub(super) grip: f32,
    /// The rolling resistance those contacts apply to the hull, both sides
    /// ([`super::forces::BeltContact::rolling_force`]).
    pub(super) rolling: Vec3,
    /// What they apply past the fade to forward travel, both sides
    /// ([`super::forces::SideReport::rolling_budget`]).
    pub(super) rolling_budget: Vec3,
    /// The solver contacts over the physics step that just ran, `None` where a side had none.
    pub(super) hard: [Option<HardReaction>; 2],
}

/// The Driver's ledger: server-side driving state, built empty at spawn from the tank's cut
/// length. Nothing is sensed until [`MIN_SAMPLES`] driven ticks have closed.
#[derive(Component, Clone, Debug)]
pub struct DriverSense {
    window: usize,
    samples: VecDeque<Sample>,
    /// The solver's push for this tick is read on the next one, so the sample closes there.
    open: Option<Open>,
    stall: Option<Stall>,
    /// The gear asked to pull the latest samples, how many of them it pulled, and how many of the
    /// latest of those it drove with the hull at rest and its clutch held.
    run: Option<(Engaged, usize, usize, usize)>,
    /// The reading this tick's choice used, for probes.
    pub(crate) reading: Option<Sensed>,
}

impl DriverSense {
    /// An empty ledger averaging one cut of `shift_ticks`, and no fewer than [`MIN_SAMPLES`].
    pub(crate) fn for_cut(shift_ticks: NonZeroU8) -> Self {
        let window = usize::from(shift_ticks.get()).max(MIN_SAMPLES);
        Self {
            window,
            samples: VecDeque::with_capacity(window),
            open: None,
            stall: None,
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
    pub(super) fn tick(
        &mut self,
        velocity: Vec3,
        heading: Vec3,
        belts: f32,
        mass: f32,
        contacts: Contacts,
        dt: f32,
    ) -> Option<Sensed> {
        let Contacts {
            soft,
            grip,
            rolling,
            rolling_budget,
            hard,
        } = contacts;
        if let Some(open) = self.open.take() {
            assert!(
                mass.is_finite() && mass > 0.0 && dt > 0.0,
                "a driven hull has finite mass and its tick a positive step"
            );
            let mut push = open.push;
            let drive = open
                .drive
                .expect("the step that drove an opened tick reports its sprockets' force");
            for (push, hard) in push.iter_mut().zip(hard) {
                if let Some(hard) = hard {
                    *push += hard.friction.dot(open.heading);
                }
            }
            let grip = open.grip + hard.iter().flatten().map(|hard| hard.grip).sum::<f32>();
            let gain = (velocity - open.velocity).dot(open.heading);
            let at_rest = |velocity: Vec3| velocity.dot(open.heading).abs() < park_engage_speed();
            let stalled = open.pull.filter(|&dir| {
                open.driving
                    && at_rest(open.velocity)
                    && at_rest(velocity)
                    && dir * gain <= 0.0
                    && (belts.abs() < park_engage_speed() || dir * (belts - open.belts) <= 0.0)
            });
            if !at_rest(velocity) {
                self.stall = None; // The hull moves: no push has failed to move it.
            } else if let Some(dir) = stalled {
                let failed = dir * (push[0] + push[1] - open.rolled);
                let bound = match self.stall {
                    Some(stall) if stall.dir == dir => stall.bound.max(failed),
                    _ => failed,
                };
                self.stall = Some(Stall { dir, bound });
            }
            let resting = open.driving && at_rest(open.velocity) && at_rest(velocity);
            let [resting, held] = [resting, open.locked].map(usize::from);
            self.run = match (open.pulled_by, self.run) {
                (Some(by), Some((gear, pulled, rested, locked))) if by == gear => Some((
                    gear,
                    pulled + 1,
                    (rested + 1) * resting,
                    (locked + 1) * held,
                )),
                (by, _) => by.map(|by| (by, 1, resting, held)),
            };
            if self.samples.len() == self.window {
                self.samples.pop_front();
            }
            self.samples.push_back(Sample {
                push,
                resistance: push[0] + push[1] - mass * gain / dt,
                grip,
                rolling: open.rolling,
                rolled: open.rolled,
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
            rolling: -rolling_budget.dot(heading),
            rolled: -rolling.dot(heading),
            pull: None,
            pulled_by: None,
            driving: false,
            locked: false,
            drive: None,
        });
        self.reading = (self.samples.len() >= MIN_SAMPLES).then(|| {
            let count = self.samples.len() as f32;
            let [left, right, resistance, belt_gain, drive] =
                self.samples.iter().fold([0.0; 5], |[l, r, e, b, d], s| {
                    [
                        l + s.push[0],
                        r + s.push[1],
                        e + s.resistance,
                        b + s.belt_gain,
                        d + s.drive,
                    ]
                });
            let [grip, rolling, rolled] = self.samples.iter().fold([0.0; 3], |[g, r, d], s| {
                [g + s.grip, r + s.rolling, d + s.rolled]
            });
            let window = self
                .samples
                .iter()
                .fold(Moments::default(), |mut window, s| {
                    window.add(s.terms());
                    window
                });
            Sensed {
                speed: velocity.dot(heading),
                resistance: resistance / count,
                spread: window.evidence().spread,
                push_difference: (left - right) / count,
                drive: drive / count,
                grip: grip / count,
                rolling: rolling / count,
                rolled: rolled / count,
                stall: self.stall,
                // The window's belts at rest or not gaining under the stall's pull.
                stalled: self.stall.is_some_and(|stall| {
                    self.run
                        .is_some_and(|(_, _, rested, _)| rested >= self.window)
                        && (belts.abs() < park_engage_speed() || stall.dir * belt_gain <= 0.0)
                }),
                pulled: self
                    .run
                    .is_some_and(|(_, pulled, ..)| pulled >= self.window),
                observed: self.run.is_some_and(|(.., locked)| locked >= self.window),
            }
        });
        self.reading
    }

    /// This tick's step as delivery ran it: a paid cut is not felt, a step that asked no pull
    /// withdraws the push a stall's bound was read from, and a driven step records its clutch and
    /// its sprockets' force.
    pub(crate) fn drove(&mut self, report: &TransmissionReport) {
        let dir = if report.reverse { -1.0 } else { 1.0 };
        let pull = report.pulling.then_some(dir);
        if self.stall.is_some_and(|stall| pull != Some(stall.dir)) {
            self.stall = None;
        }
        if report.shifting {
            self.open = None;
            self.run = None;
        } else {
            let open = self
                .open
                .as_mut()
                .expect("every non-shifting delivery follows the tick it drove, opened by `tick`");
            open.pull = pull;
            open.pulled_by = report.pulling.then_some((report.reverse, report.gear));
            open.driving = report.driving;
            open.locked = report.locked;
            open.drive = Some(report.forces.iter().sum());
        }
    }
}

/// One Driver decision on what it felt, at the requested pedal `command`, inside the transmission's
/// step.
///
/// Speed `v` (m/s) and resistance `R` (N) read along the engaged ladder; rollback retains its
/// negative speed. A gear carries the load `L`: `R` less the rolling the contacts delivered, at a stall no less than
/// the stall's bound ([`Stall::bound`]), plus the contacts' whole rolling ([`Sensed::rolling`]) and
/// the locked gear's detent share of the belt push difference, `s·κ·ΔR`, from the crank tie.
/// A gear *holds* what it pulls on the pedal's own fuel,
/// capped by the contacts' grip budget ([`Sensed::grip`]).
///
/// `L` is a mean over driven ticks, known to their sample standard error `u`
/// ([`Evidence::error`]). A gear is *viable* when it holds at least `L + u` and *short* when it
/// holds less than `L − u`; between them its verdict is open, and an engaged gear whose verdict is
/// open is not stepped down. The engaged gear is judged as it runs, at `v` with its clutch as it
/// is, over a window it pulled whole ([`Sensed::pulled`]), holding the most it nets lugging down
/// ([`viable_pull`]). Any other gear is judged where its clutch closes from out, at the crank speed
/// it lands on ([`landing_pull`]): where the cut lands, `v − R·t_cut/M_c` and no lower than rest,
/// or at `v` for a launch. No clause admits a gear whose crank is forecast to land past the
/// engine's crank limit. Reach is `max_step` either way, within the ladder.
///
/// 1. Clutch ordered open: free choice among safe gears, restricted to the launch range at
///    rest or rollback; highest viable, otherwise strongest, including a stall's strongest.
/// 2. The hull stalls now ([`Sensed::stalled`]), its load only bounded from below:
///    the gear in reach that holds most. The engaged gear short: the highest viable gear in reach,
///    else the one that holds most.
/// 3. Straight, clutch in, a held window and the hull moving where the cut lands: of the higher
///    viable gears in reach that pull more than the engaged gear's drive by M ([`force_margin`])
///    where they land ([`candidate_pull`], capped by grip), the one that pulls hardest, the higher
///    on a tie.
///
/// Clauses 2 and 3 pay the cut, and only for a gear that pulls where it lands: one the pedal's own
/// fuel cannot drive at the crank it lands on only brakes less than the engaged gear. A cut to the
/// gear that holds most is paid only where it holds more than the engaged gear; among launch gears
/// a tie goes to the higher. A cut in flight or no pull asked choose nothing.
pub(super) fn choose(
    st: &mut TransmissionState,
    tp: &TransmissionParams,
    fp: &DriveConstruction,
    inp: &TransmissionInput,
    command: f32,
    sensed: Sensed,
) {
    if st.shift_ticks != 0 || command <= 0.0 {
        return;
    }
    let (ladder, dir) = if st.reverse {
        (&tp.gears_rev, -1.0)
    } else {
        (&tp.gears_fwd, 1.0)
    };
    let top = ladder.len() as u8;
    let speed = dir * sensed.speed;
    let stall = sensed.stall.filter(|stall| stall.dir == dir);
    // A stalled hull meets at least the push that failed to move it.
    let at_least = |felt: f32| stall.map_or(felt, |stall| felt.max(stall.bound));
    let resistance = at_least(dir * sensed.resistance);
    // A gear that moves the hull past the rolling fade meets the contacts' whole rolling, not the
    // share the ticks delivered: none at rest, part of it in a crawl. The stall's bound is net of
    // the rolling delivered where it was read, so it floors the share that is not rolling.
    let straight = |[net, rolling, _]: [f32; 3]| at_least(dir * net) + rolling;
    // The load `gear` carries by `evidence`, and its standard error.
    let carried = |evidence: &Evidence, gear: u8| {
        let share = steer_ratio(tp, st, inp, gear);
        (
            straight(evidence.mean) + share * evidence.mean[2],
            evidence.error(dir, share),
        )
    };
    let window = sensed.window();
    let ratio = |gear: u8| ladder[usize::from(gear - 1)];
    let lands = |gear: u8, at: f32| {
        landing_pull(tp, inp.supply, at, ratio(gear), command).map(|pull| pull.min(sensed.grip))
    };
    let viable = |gear: u8, holds: Option<f32>| {
        holds.is_some_and(|holds| verdict(holds, carried(&window, gear)) == Verdict::Viable)
    };
    // The gear of `gears` that holds most, the higher on a tie, and what it holds; `None` where
    // none lands.
    let strongest = |gears: std::ops::RangeInclusive<u8>, holds: &dyn Fn(u8) -> Option<f32>| {
        gears
            .filter_map(|gear| Some((gear, holds(gear)?)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
    };
    if st.launch == LaunchState::Unchosen {
        // A rolling open clutch may select any safe gear for free. Only a start or rollback
        // uses the construction's launch range; the signed shaft price is retained in both.
        let top_free = if speed <= 0.0 {
            tp.launch_gears.min(top)
        } else {
            top
        };
        let gears = 1..=top_free;
        let chosen = if stall.is_some() {
            None
        } else {
            gears
                .clone()
                .rev()
                .find(|&gear| viable(gear, lands(gear, speed)))
        }
        .or_else(|| strongest(gears, &|gear| lands(gear, speed)).map(|(gear, _)| gear));
        if let Some(gear) = chosen {
            st.gear = gear;
            st.launch = LaunchState::TakingUp;
        }
        // No safe landing is a real overrun state: retain the gear. The open clutch isolates
        // the crank until a ratio can be selected; the ceiling brake is absent while open.
        return;
    }
    // A hull the cut brings to rest is held there by the park latch, so no cut lands below rest.
    let landing =
        (speed - resistance * f32::from(tp.shift_ticks.get()) * inp.dt / coupled_mass(fp)).max(0.0);
    let reach =
        st.gear.saturating_sub(tp.max_step).max(1)..=st.gear.saturating_add(tp.max_step).min(top);
    let held =
        viable_pull(tp, inp.supply, speed, ratio(st.gear), command, st.clutch).min(sensed.grip);
    // The design's "A cut buys drive, never less braking" (section 5a) says why.
    let buys = |gear: u8| lands(gear, landing).filter(|&pull| pull > 0.0);
    let offer = |gear: u8| {
        if gear == st.gear {
            Some(held)
        } else {
            buys(gear)
        }
    };
    // The gear in reach that holds most, where it holds more than the engaged gear: on equal
    // capacity a cut buys nothing.
    let stronger = || {
        let (best, holds) =
            strongest(reach.clone(), &offer).expect("a reach holds the engaged gear");
        (holds > held).then_some(best)
    };
    // A stall bounds the load from below only, so no gear can be shown viable against it.
    let stalled = stall.is_some() && sensed.stalled;
    let short =
        sensed.pulled && !stalled && verdict(held, carried(&window, st.gear)) == Verdict::Short;
    let chosen = if stalled {
        stronger()
    } else if short {
        reach
            .clone()
            .rev()
            .filter(|&gear| gear != st.gear)
            .find(|&gear| viable(gear, buys(gear)))
            .or_else(stronger)
    } else if sensed.observed
        && st.steer_step == 0
        && st.clutch == ClutchState::In
        && landing >= park_engage_speed()
        && st.gear < top
    {
        let margin = force_margin(tp);
        // A detent turn's radius belongs to its gear: no upshift changes it.
        (st.gear + 1..=*reach.end())
            .filter(|&gear| viable(gear, buys(gear)))
            .map(|gear| {
                let pull = candidate_pull(
                    tp,
                    fp,
                    inp.supply,
                    landing,
                    ratio(gear),
                    command,
                    carried(&window, gear).0,
                    ClutchState::Out,
                );
                (gear, pull.min(sensed.grip))
            })
            .filter(|&(_, pull)| pull > dir * sensed.drive + margin)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(gear, _)| gear)
    } else {
        None
    };
    if let Some(gear) = chosen {
        st.gear = gear;
        st.shift_ticks = tp.shift_ticks.get();
        st.launch = LaunchState::TakingUp;
    }
}

/// What a gear's hold says of the load it carries, read to that load's standard error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Viable,
    Short,
    Open,
}

fn verdict(holds: f32, (load, error): (f32, f32)) -> Verdict {
    if holds >= load + error {
        Verdict::Viable
    } else if holds < load - error {
        Verdict::Short
    } else {
        Verdict::Open
    }
}

#[cfg(test)]
mod tests {
    use super::super::drive_fixture::*;
    use super::super::transmission::{
        DriveSupply, RPM_TO_RAD, TransmissionAuthoring, price_driver_force,
    };
    use super::*;

    /// Fixture: a 40 t point hull at the shipped tick, a 20-tick cut from tick 60, and a push of
    /// 45 kN before it and 60 kN after.
    const MASS: f32 = 40_000.0;
    const DT: f32 = 1.0 / 64.0;
    const CUT: NonZeroU8 = NonZeroU8::new(20).expect("a fixture cut of twenty ticks");
    const CUT_START: usize = 60;
    /// Fixture: the hull's forward axis.
    const FORWARD: Vec3 = Vec3::NEG_Z;

    /// Fixture contacts pushing the hull forward by `push` (N), half on each belt, with no grip
    /// budget, no rolling and no solver contact.
    fn pushing(push: f32) -> Contacts {
        Contacts {
            soft: [FORWARD * push * 0.5; 2],
            grip: 0.0,
            rolling: Vec3::ZERO,
            rolling_budget: Vec3::ZERO,
            hard: [None; 2],
        }
    }

    /// Fixture: a step that asked forward pull and whose clutch sent drive.
    fn pulling() -> TransmissionReport {
        TransmissionReport {
            pulling: true,
            driving: true,
            ..Default::default()
        }
    }

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
        (0..200)
            .map(|tick| {
                let push = push(tick);
                let contacts = Contacts {
                    soft: [push * 0.25, push * 0.5].map(|push| FORWARD * push),
                    hard: reported,
                    ..pushing(0.0)
                };
                let reading = sense.tick(FORWARD * speed, FORWARD, speed, MASS, contacts, DT);
                sense.drove(&TransmissionReport {
                    shifting: push == 0.0,
                    ..Default::default()
                });
                speed += (push - resist(push)) / MASS * DT;
                reported = [
                    Some(HardReaction {
                        belt: push * 0.25,
                        friction: FORWARD * push * 0.25,
                        grip: f32::INFINITY, // Fixture: ground that grips beyond any push.
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
        assert!(
            grade[..MIN_SAMPLES].iter().all(Option::is_none),
            "nothing is sensed before `MIN_SAMPLES` ticks close"
        );
        for (tick, reading) in grade.iter().enumerate().skip(MIN_SAMPLES) {
            let read = reading
                .expect("every later tick reads the ledger")
                .resistance;
            assert!(near(read, 30_000.0), "grade, tick {tick}: read {read}");
        }
        let obstacle = readings(|push| push);
        let cut = usize::from(CUT.get());
        let end = CUT_START + cut;
        for (tick, want) in [
            (CUT_START, 45_000.0),
            (end, 45_000.0),
            (end + cut, 60_000.0),
        ] {
            let read = obstacle[tick]
                .expect("the ledger has closed ticks")
                .resistance;
            assert!(
                near(read, want),
                "obstacle, tick {tick}: read {read}, want {want}"
            );
        }
    }

    /// A gearbox's cut of one tick still feels: its window holds [`MIN_SAMPLES`], so the load gate
    /// admits every cut of a tick or more. Mutant: a window of one cut never reads.
    #[test]
    fn a_one_tick_cut_reads_over_the_fewest_samples() {
        let mut sense = DriverSense::for_cut(NonZeroU8::MIN);
        let read = (0..=MIN_SAMPLES).map(|_| {
            let reading = sense.tick(FORWARD * 3.0, FORWARD, 3.0, MASS, pushing(40_000.0), DT);
            sense.drove(&pulling());
            reading.is_some()
        });
        assert!(
            read.last().unwrap(),
            "the reading closing `MIN_SAMPLES` ticks"
        );
    }

    /// A stalled hull bounds what it meets by the largest push that failed to move it, only while
    /// it stays stalled under that pull: the Driver's choice at a stall relies on the bound, and on
    /// its dropping the moment the hull moves or the pull is withdrawn. Fixture: a hull held at rest
    /// under a push of 30, 60 then 15 kN, then moving at 1 m/s, stalled again, and released; the
    /// first reading closes two ticks.
    #[test]
    fn a_stall_bounds_resistance_by_its_largest_failed_push() {
        let mut sense = DriverSense::for_cut(CUT);
        let (pulling, released) = (pulling(), TransmissionReport::default());
        let mut feel = |speed: f32, push: f32, report: &TransmissionReport| {
            let reading = sense.tick(FORWARD * speed, FORWARD, speed, MASS, pushing(push), DT);
            sense.drove(report);
            reading
                .and_then(|reading| reading.stall)
                .map(|stall| stall.bound)
        };
        let script = [
            (0.0, 30_000.0, &pulling, None),
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

    /// A stall's bound is what the hull meets besides the rolling its contacts delivered: the
    /// Driver adds the contacts' whole rolling to it, so a bound that kept the delivered share
    /// would count it twice. Fixture: a hull creeping at half the rest tolerance
    /// ([`park_engage_speed()`]) under 30 kN while its contacts deliver 2 kN of rolling.
    #[test]
    fn a_stall_bound_is_net_of_delivered_rolling() {
        let mut sense = DriverSense::for_cut(CUT);
        let contacts = Contacts {
            rolling: -FORWARD * 2_000.0,
            rolling_budget: -FORWARD * 20_000.0,
            ..pushing(30_000.0)
        };
        let mut reading = None;
        for _ in 0..3 {
            let creep = FORWARD * 0.5 * park_engage_speed();
            reading = sense.tick(creep, FORWARD, 0.0, MASS, contacts, DT);
            sense.drove(&pulling());
        }
        let stall = reading
            .and_then(|reading| reading.stall)
            .expect("a held push bounds the stall");
        assert_eq!(stall.bound, 28_000.0);
    }

    /// A stall's bound outlives the stall, but the hull stalls now only under a window the gear
    /// drove throughout with the hull at rest and whose belts did not gain over it: the Driver
    /// steps down under it, and belts gaining under a hull at rest slide at the ground's limit,
    /// which no lower gear raises. Fixture: a hull held at rest under 30 kN, one tick with its
    /// belts held, a window with them gaining, then a window with them held.
    #[test]
    fn the_hull_stalls_now_under_drive_at_rest_and_no_belt_gain() {
        let window = usize::from(CUT.get());
        let mut sense = DriverSense::for_cut(CUT);
        let mut feel = |belts: f32| {
            let reading = sense.tick(Vec3::ZERO, FORWARD, belts, MASS, pushing(30_000.0), DT);
            sense.drove(&pulling());
            reading
        };
        feel(0.0);
        feel(0.0);
        let held = feel(0.0).expect("two closed samples");
        assert!(held.stall.is_some(), "a held tick bounds the stall");
        let gaining = (1..=window)
            .map(|tick| feel(0.2 + 0.01 * tick as f32))
            .last()
            .flatten()
            .expect("a closed window");
        assert!(gaining.stall.is_some(), "the bound outlives the stall");
        assert!(!gaining.stalled, "belts gaining under a hull at rest");
        // The first held reading closes the last gaining tick's sample.
        let stalled = (0..=window)
            .map(|_| feel(0.5))
            .last()
            .flatten()
            .expect("a closed window");
        assert!(stalled.stalled, "driven at rest, the belts held");
    }

    /// A reading's error is the sample standard error of the load a gear carries, `R − rolled +
    /// rolling`: none for a steady load, and `a/√(n − 1)` for one alternating `±a` over an even `n`
    /// ticks (DERIVED: the sample variance `n a²/(n − 1)`, over `n`). The Driver decides a gear
    /// against that load only beyond it, so the ride's scatter, not an authored band, sets the
    /// error. Mutants: a floor on the error reads a steady load as uncertain; the scatter of `R`
    /// alone reads none here; the population deviation reads `a/√n`. Fixture: a hull coasting at
    /// 3 m/s under a steady 100 kN push, its contacts delivering a steady 10 kN of a whole 20 kN,
    /// then 10 and 20 kN of a whole 20 and 40 kN on alternate ticks.
    #[test]
    fn the_error_is_the_sample_standard_error_of_the_load_a_gear_carries() {
        let window = usize::from(CUT.get());
        let read = |rolling: &dyn Fn(usize) -> (f32, f32)| {
            let mut sense = DriverSense::for_cut(CUT);
            let mut reading = None;
            for tick in 0..=window {
                let (rolled, whole) = rolling(tick);
                let contacts = Contacts {
                    rolling: -FORWARD * rolled,
                    rolling_budget: -FORWARD * whole,
                    ..pushing(100_000.0)
                };
                reading = sense.tick(FORWARD * 3.0, FORWARD, 3.0, MASS, contacts, DT);
                sense.drove(&TransmissionReport::default());
            }
            reading.expect("a closed window").window()
        };
        let steady = read(&|_| (10_000.0, 20_000.0));
        assert_eq!(steady.error(1.0, 0.0), 0.0, "a steady load");
        let alternating = read(&|tick| {
            if tick.is_multiple_of(2) {
                (10_000.0, 20_000.0)
            } else {
                (20_000.0, 40_000.0)
            }
        });
        // The load alternates 110 and 120 kN.
        let want = 5_000.0 / ((window - 1) as f32).sqrt();
        let error = alternating.error(1.0, 0.0);
        assert!(
            (error - want).abs() < want * 1e-4,
            "read {error} N against {want} N"
        );
        let mean = alternating.mean[0] + alternating.mean[1];
        assert!((mean - 115_000.0).abs() < 1.0, "mean load {mean} N");
    }

    /// The engaged gear's run starts again on a paid cut and on another gear, so a reading is the
    /// engaged gear's own only once that gear has pulled a whole window since: the Driver judges
    /// the engaged gear on such a window alone. Mutants: a run that carries across a gear change
    /// judges fourth on third's ticks; one that carries across a cut judges fourth on the ticks
    /// before it. Fixture: a hull coasting at 3 m/s under 40 kN, pulled in third, then fourth,
    /// then fourth again after a cut; each reading closes the tick before it.
    #[test]
    fn the_engaged_gears_run_starts_again_on_a_cut_or_another_gear() {
        let window = usize::from(CUT.get());
        let mut sense = DriverSense::for_cut(CUT);
        let report = |gear: u8, shifting: bool| TransmissionReport {
            gear,
            shifting,
            ..pulling()
        };
        // Whether each of `ticks` readings read its window pulled whole, every tick driven as
        // `report` says.
        let mut pulled = |ticks: usize, report: TransmissionReport| -> Vec<bool> {
            (0..ticks)
                .map(|_| {
                    let reading =
                        sense.tick(FORWARD * 3.0, FORWARD, 3.0, MASS, pushing(40_000.0), DT);
                    sense.drove(&report);
                    reading.is_some_and(|reading| reading.pulled)
                })
                .collect()
        };
        let whole_at_last = |pulls: &[bool], from: usize| {
            pulls[from..window].iter().all(|pulled| !pulled) && pulls[window]
        };
        let third = pulled(window + 1, report(3, false));
        assert!(whole_at_last(&third, 0), "third: {third:?}");
        // Fourth's first reading closes third's last tick.
        let fourth = pulled(window + 1, report(4, false));
        assert!(whole_at_last(&fourth, 1), "fourth: {fourth:?}");
        pulled(2, report(4, true));
        let after_cut = pulled(window + 1, report(4, false));
        assert!(whole_at_last(&after_cut, 0), "after the cut: {after_cut:?}");
    }

    /// The engaged gear's drive reads its whole window, not its latest tick: the upshift compares a
    /// candidate against it over the same held window, so one tick's dip must not decide. Fixture:
    /// two locked windows of the same mean force, 50 kN, one steady and one opening on 65 kN and
    /// closing on 35 kN.
    #[test]
    fn the_engaged_drive_reads_its_whole_window() {
        let window = usize::from(CUT.get());
        let read = |force: &dyn Fn(usize) -> f32| {
            let mut sense = DriverSense::for_cut(CUT);
            let mut reading = None;
            // The reading on tick `window` closes the samples of ticks 0 to `window - 1`.
            for tick in 0..=window {
                reading = sense.tick(FORWARD * 3.0, FORWARD, 3.0, MASS, pushing(50_000.0), DT);
                sense.drove(&TransmissionReport {
                    locked: true,
                    forces: [force(tick) * 0.5; 2],
                    ..pulling()
                });
            }
            reading.expect("a window of closed samples")
        };
        let steady = read(&|_| 50_000.0);
        let dipped = read(&|tick| match tick {
            0 => 65_000.0,
            tick if tick == window - 1 => 35_000.0,
            _ => 50_000.0,
        });
        for reading in [steady, dipped] {
            assert!(reading.observed, "the fixture window is locked whole");
            assert!(
                (reading.drive - 50_000.0).abs() < 1.0,
                "drive {} over a 50 kN window",
                reading.drive
            );
        }
    }

    /// The ledger reads the contacts' whole rolling against forward travel and the share they
    /// delivered, which the Driver's price swaps for the whole. Fixture: contacts of 20 kN whole
    /// rolling delivering none, a fifth, then all of it.
    #[test]
    fn the_ledger_reads_whole_and_delivered_rolling() {
        let whole = -FORWARD * 20_000.0;
        for share in [0.0, 0.2, 1.0] {
            let mut sense = DriverSense::for_cut(CUT);
            let contacts = Contacts {
                rolling: whole * share,
                rolling_budget: whole,
                ..pushing(0.0)
            };
            let mut reading = None;
            for _ in 0..3 {
                reading = sense.tick(Vec3::ZERO, FORWARD, 0.0, MASS, contacts, DT);
                sense.drove(&TransmissionReport::default());
            }
            let reading = reading.expect("two closed samples");
            assert_eq!(
                [reading.rolling, reading.rolled],
                [20_000.0, 20_000.0 * share],
                "delivering {share} of it"
            );
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
            let heading = Quat::from_axis_angle(axis, 0.5 * DT * tick as f32) * FORWARD;
            let belts = velocity.dot(heading);
            let reading = sense.tick(velocity, heading, belts, MASS, pushing(0.0), DT);
            sense.drove(&TransmissionReport::default());
            if let Some(reading) = reading {
                // DERIVED: no push and no change of velocity close every sample at zero.
                assert_eq!(reading.resistance, 0.0, "tick {tick}");
            }
        }
    }

    /// Fixture: the lab ladder's top gear crawling locked at a quarter pedal over a window it pulled
    /// whole, the hull meeting what that gear delivers on the pedal and the idle catch together, on
    /// ground that grips beyond any gear's pull. Its crank turns a whole rpm below the first whole rpm
    /// under idle where the catch carries that crawl past what the pedal alone holds, so the catch's
    /// ramp sets the shortfall, not rounding.
    fn carried_top_gear(tp: &TransmissionParams) -> (TransmissionState, TransmissionInput) {
        let top = tp.gears_fwd.len() as u8;
        let ratio = tp.gears_fwd[usize::from(top - 1)];
        let at = |rpm: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
        let (whole, locked) = (DriveSupply::WHOLE, ClutchState::In);
        let crawl = |rpm: f32| price_driver_force(tp, whole, at(rpm), ratio, 0.25, locked);
        let pedal = |rpm: f32| viable_pull(tp, whole, at(rpm), ratio, 0.25, locked);
        let carried = (1..tp.engine.idle_rpm as u16)
            .map(|below| tp.engine.idle_rpm - f32::from(below))
            .find(|&rpm| crawl(rpm) > pedal(rpm))
            .expect("the idle catch carries a quarter pedal's crawl below idle");
        let rpm = carried - 1.0;
        let inp = driver_input(at(rpm), crawl(rpm), 0.25);
        let state = TransmissionState {
            gear: top,
            clutch: ClutchState::In,
            launch: LaunchState::Locked,
            omega_e: rpm * RPM_TO_RAD,
            ..fresh(tp)
        };
        (state, inp)
    }

    /// The game's driving model: a gear is viable only on the pedal's own fuel, so a gear the idle
    /// catch carries is not, and steps to the highest viable gear in reach where the cut lands; the
    /// Driver relies on it to leave a crawl on torque no pedal asked for. Mutant: pricing the engaged
    /// gear with the catch keeps the crawl. Fixture: the lab ladder's top gear (fifth) carried by the
    /// catch, fourth viable where it lands.
    #[test]
    fn a_gear_the_idle_catch_carries_steps_to_the_highest_viable_gear() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let (mut state, inp) = carried_top_gear(&tp);
        let sensed = felt(&inp);
        let top = state.gear;
        let landing = sensed.speed
            - sensed.resistance * f32::from(tp.shift_ticks.get()) * inp.dt
                / coupled_mass(&construction(&fp));
        let ratio = tp.gears_fwd[usize::from(top - 2)];
        assert!(
            landing_pull(&tp, DriveSupply::WHOLE, landing, ratio, 0.25)
                .is_some_and(|pull| pull >= sensed.resistance),
            "the fixture's next gear down is viable where it lands"
        );
        choose(&mut state, &tp, &construction(&fp), &inp, 0.25, sensed);
        assert_eq!(
            (state.gear, state.shift_ticks > 0),
            (top - 1, true),
            "the carried gear steps to the highest viable gear"
        );
    }

    /// Fixture evidence of a straight load `load` (N) read to a standard error `error` (N).
    fn read_to(load: f32, error: f32) -> Evidence {
        Evidence {
            mean: [load, 0.0, 0.0],
            spread: [[error * error, 0.0, 0.0], [0.0; 3], [0.0; 3]],
        }
    }

    /// The game's driving model: the Driver decides a gear against its load only beyond the reading's
    /// standard error: a shortfall within it does not step the engaged gear down, and a gear is
    /// admitted only holding the load plus it. The Driver relies on both not to step on a load reading
    /// that wobbles across a gear's pull. Mutants: dropping the error from the verdict steps a gear
    /// down on a shortfall the reading cannot resolve; dropping it from admission launches a gear the
    /// load may exceed. Fixture: the lab ladder's top gear carried by the idle catch, its shortfall
    /// read to just above and just below it; a launch against 0.999 of third's launch pull, read
    /// exactly and to a hundredth.
    #[test]
    fn a_verdict_within_the_readings_error_neither_steps_down_nor_admits() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let construction = construction(&fp);
        let (engaged, inp) = carried_top_gear(&tp);
        let sensed = felt(&inp);
        let ratio = tp.gears_fwd[usize::from(engaged.gear - 1)];
        let held = viable_pull(&tp, inp.supply, sensed.speed, ratio, 0.25, ClutchState::In);
        let short = sensed.resistance - held;
        assert!(short > 0.0, "the fixture's gear is short of its load");
        for (error, steps) in [(1.01 * short, false), (0.99 * short, true)] {
            let mut st = engaged;
            let sensed = Sensed {
                spread: read_to(sensed.resistance, error).spread,
                ..sensed
            };
            choose(&mut st, &tp, &construction, &inp, 0.25, sensed);
            assert_eq!(
                st.shift_ticks > 0,
                steps,
                "short by {short} N, read to {error} N"
            );
        }
        let load = 0.999 * launch_force(&tp, 3);
        let inp = driver_input(0.0, load, 1.0);
        for (error, launched) in [(0.0, 3), (0.01 * load, 2)] {
            let mut st = fresh(&tp);
            let sensed = Sensed {
                spread: read_to(load, error).spread,
                ..felt(&inp)
            };
            choose(&mut st, &tp, &construction, &inp, 1.0, sensed);
            assert_eq!(
                st.gear, launched,
                "launching against {load} N read to {error} N"
            );
        }
    }

    /// The game's driving model: an open verdict keeps the engaged gear against a step down only; an
    /// upshift stands on its candidate's own verdict and pull. The Driver relies on it to leave a gear
    /// lugging at its load when a higher gear, slipping where it lands, is viable and pulls past the
    /// drive. Mutant: refusing the upshift while the engaged verdict is open holds the lugging gear.
    /// Fixture: the lab curve with its idle torque cut to a quarter of its peak, second gear lugging
    /// locked at 700 rpm at full pedal against what it holds there, read to a hundredth of that.
    #[test]
    fn an_open_verdict_refuses_no_stronger_viable_upshift() {
        let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
            torque_nm: &[
                (600.0, 550.0),
                (1100.0, 2200.0),
                (1700.0, 1950.0),
                (1800.0, 0.0),
            ],
            ..lab_authoring()
        })
        .expect("a lab curve climbing fourfold from idle to its peak is admitted");
        let (gear, rpm) = (2, 700.0);
        let ratio = tp.gears_fwd[usize::from(gear - 1)];
        let speed = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
        let held = viable_pull(&tp, DriveSupply::WHOLE, speed, ratio, 1.0, ClutchState::In);
        let inp = driver_input(speed, held, 1.0);
        let sensed = Sensed {
            spread: read_to(held, 0.01 * held).spread,
            ..felt(&inp)
        };
        let mut st = TransmissionState {
            gear,
            clutch: ClutchState::In,
            launch: LaunchState::Locked,
            omega_e: rpm * RPM_TO_RAD,
            ..fresh(&tp)
        };
        choose(&mut st, &tp, &construction(&lab_fp()), &inp, 1.0, sensed);
        assert_eq!(
            (st.gear, st.shift_ticks > 0),
            (gear + 1, true),
            "second gear, open at {held} N, steps to third, which slips where it lands"
        );
    }

    /// The game's driving model: a cut to the gear that holds most is paid only for more than the
    /// engaged gear holds; on equal capacity the engaged gear stays. The Driver relies on it not to
    /// cycle a stall between tied gears, each cut reopening the launch. Mutant: a tie going to the
    /// higher gear steps a stalled gear up and pays a cut for nothing. Fixture: the lab ladder's second
    /// gear stalled at rest and slipping, on ground whose grip caps every gear below the load.
    #[test]
    fn a_stall_on_equal_capacity_keeps_the_gear() {
        let (fp, tp) = (lab_fp(), lab_tp());
        let grip = 0.5 * launch_force(&tp, tp.gears_fwd.len() as u8);
        let mut inp = stalled(driver_input(0.0, 2.0 * grip, 1.0));
        inp.sensed.as_mut().expect("a Driver-policy reading").grip = grip;
        let mut st = TransmissionState {
            gear: 2,
            clutch: ClutchState::Slip,
            launch: LaunchState::TakingUp,
            ..fresh(&tp)
        };
        choose(&mut st, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
        assert_eq!(
            (st.gear, st.shift_ticks),
            (2, 0),
            "every gear holds the grip"
        );
    }

    /// Fixture: `inp`'s hull held at rest under a forward pull of the resistance it feels, which is
    /// what the ledger reads of a pull that does not move the hull.
    fn stalled(mut inp: TransmissionInput) -> TransmissionInput {
        let sensed = inp.sensed.as_mut().expect("a Driver-policy reading");
        sensed.stall = Some(Stall {
            dir: 1.0,
            bound: sensed.resistance,
        });
        sensed.stalled = true;
        inp
    }

    /// The declared skip limit and the detent prohibition constrain the same choice path: a stall
    /// steps to the gear in reach that holds most, here the lowest, never past it. Fixture: a stalled
    /// third gear meeting a resistance only first gear's launch can hold, then a straight fourth gear
    /// at its governed speed, where fifth has more to spare.
    #[test]
    fn driver_respects_the_shift_step_and_turn_contracts() {
        let (fp, mut tp) = (lab_fp(), lab_tp());
        let resistance = (launch_force(&tp, 1) + launch_force(&tp, 2)) * 0.5;
        for (max_step, landed) in [(1, 2), (2, 1)] {
            tp.max_step = max_step;
            let mut state = TransmissionState {
                gear: 3,
                clutch: ClutchState::Slip,
                launch: LaunchState::TakingUp,
                ..fresh(&tp)
            };
            let inp = stalled(driver_input(0.0, resistance, 1.0));
            choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
            assert_eq!(
                state.gear, landed,
                "a step down reaches no further than max_step {max_step}"
            );
        }
        let speed = tp.engine.governed_rpm * RPM_TO_RAD * tp.sprocket_radius / tp.gears_fwd[3];
        for detent in [0, 1, 2] {
            let mut state = TransmissionState {
                gear: 4,
                clutch: ClutchState::In,
                launch: LaunchState::Locked,
                steer_step: detent,
                omega_e: (tp.engine.governed_rpm - 50.0) * RPM_TO_RAD, // Fixture: fuel beats drag.
                ..fresh(&tp)
            };
            let inp = driver_input(speed, 1_000.0, 1.0);
            choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
            assert_eq!(
                state.gear,
                if detent == 0 { 5 } else { 4 },
                "a detent turn's radius belongs to its gear"
            );
        }
    }

    /// The top of any admitted ladder chooses no gear above it: the upshift clause relies on a
    /// higher gear existing before it forms the range above the engaged one. Fixture: a ladder of the
    /// most gears the u8 index admits, cruising straight in its top gear on ample grip.
    #[test]
    fn the_top_of_the_longest_admitted_ladder_chooses_nothing_above_it() {
        let speeds: Vec<f32> = (1..=u8::MAX).map(|gear| f32::from(gear) * 0.5).collect();
        let radii: Vec<_> = speeds.iter().map(|v| (20.0 * v, 60.0 * v)).collect();
        let tp = TransmissionParams::from_authoring(&TransmissionAuthoring {
            forward_speeds_kmh: &speeds,
            steer_radii_m: &radii,
            reverse_steer_radii_m: &[(56.0, 168.0), (86.0, 258.0), (124.0, 372.0), (184.0, 552.0)],
            ..tiger_authoring()
        })
        .expect("a ladder of u8::MAX gears is admitted");
        let top = u8::MAX;
        let speed = 0.5 * tp.governed_speed(false);
        let mut state = TransmissionState {
            gear: top,
            clutch: ClutchState::In,
            launch: LaunchState::Locked,
            omega_e: (tp.engine.governed_rpm - 50.0) * RPM_TO_RAD, // Fixture: fuel beats drag.
            ..fresh(&tp)
        };
        let inp = driver_input(speed, 1_000.0, 1.0);
        choose(
            &mut state,
            &tp,
            &construction(&lab_fp()),
            &inp,
            1.0,
            felt(&inp),
        );
        assert_eq!(state.gear, top);
    }

    /// Fixture launch price of forward `gear` at full pedal: what a slipping clutch delivers.
    fn launch_force(tp: &TransmissionParams, gear: u8) -> f32 {
        price_driver_force(
            tp,
            DriveSupply::WHOLE,
            0.0,
            tp.gears_fwd[usize::from(gear - 1)],
            1.0,
            ClutchState::Slip,
        )
    }

    /// No gear is priced above the grip its belts can carry: belts sliding along their travel at
    /// their grip limit, carrying the whole resistance in a gear that holds it on its own fuel, change
    /// no gear, where the same reading on ample grip changes gears. A Driver that shifted on an
    /// ungranted pull would cut the drive for nothing. Fixture: each class drivetrain moving in each
    /// forward gear with the clutch in, the crank from idle to governed and resistance up to first
    /// gear's launch pull, every patch sliding along the belt so that its grip budget is the
    /// resistance; a gear that cannot hold the resistance is not viable on any grip and steps down.
    #[test]
    fn belts_at_their_grip_limit_change_no_gear() {
        for (fp, tp) in class_drivetrains() {
            let construction = construction(&fp);
            let mut changed = 0;
            for gear in 1..=tp.gears_fwd.len() as u8 {
                let ratio = tp.gears_fwd[usize::from(gear - 1)];
                for step in 0..=24 {
                    let span = tp.engine.governed_rpm - tp.engine.idle_rpm;
                    let rpm = tp.engine.idle_rpm + span * step as f32 / 24.0;
                    let speed = rpm * RPM_TO_RAD * tp.sprocket_radius / ratio;
                    for share in 0..=20 {
                        let resistance = launch_force(&tp, 1) * share as f32 / 20.0;
                        let mut inp = driver_input(speed, resistance, 1.0);
                        let held = viable_pull(&tp, inp.supply, speed, ratio, 1.0, ClutchState::In);
                        if held < resistance {
                            continue;
                        }
                        let engaged = TransmissionState {
                            gear,
                            clutch: ClutchState::In,
                            launch: LaunchState::Locked,
                            omega_e: rpm * RPM_TO_RAD,
                            ..fresh(&tp)
                        };
                        let mut st = engaged;
                        choose(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
                        changed += usize::from(st.gear != gear);
                        let sliding = inp.sensed.as_mut().expect("a Driver-policy reading");
                        sliding.grip = resistance;
                        let mut st = engaged;
                        choose(&mut st, &tp, &construction, &inp, 1.0, felt(&inp));
                        assert_eq!(
                            st.gear, gear,
                            "gear {gear} at {rpm} rpm against {resistance} N, sliding"
                        );
                    }
                }
            }
            assert!(changed > 0, "the fixture's ample grip changes gears");
        }
    }

    /// The free choice at hull rest is the highest launch gear that holds what the Driver feels,
    /// and costs no cut: the launch relies on it. Fixture: every starting gear against no
    /// resistance, a resistance between the launch prices, and one no launch gear holds.
    #[test]
    fn the_free_choice_at_rest_launches_in_the_highest_launch_gear_that_holds() {
        for (fp, tp) in class_drivetrains() {
            let top = tp.gears_fwd.len() as u8;
            let lowest = tp.launch_gears.min(top);
            for resistance in [
                0.0,
                (launch_force(&tp, 1) + launch_force(&tp, lowest)) * 0.5,
                launch_force(&tp, 1) * 1.5,
            ] {
                let inp = driver_input(0.0, resistance, 1.0);
                let expected = affordable_launch(&tp, &inp, 1.0).unwrap_or(1);
                for start in 1..=top {
                    let mut state = TransmissionState {
                        gear: start,
                        ..fresh(&tp)
                    };
                    choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
                    assert_eq!(state.gear, expected, "R={resistance} from gear {start}");
                    assert_eq!(state.shift_ticks, 0, "the choice at rest is free");
                }
            }
        }
    }

    /// The game's driving model: a launch prices each gear against the rolling the hull meets once it
    /// moves, not the share faded at rest, where a gear that clears only the faded share creeps at the
    /// speed its pull meets the fading rolling. Mutant: pricing the resistance felt at rest launches
    /// the highest launch gear. Fixture: each class at rest against half the highest launch gear's
    /// price, its contacts' rolling lifting the load to half way to first gear's.
    #[test]
    fn a_launch_prices_the_rolling_the_hull_meets_moving() {
        for (fp, tp) in class_drivetrains() {
            let highest = tp.launch_gears.min(tp.gears_fwd.len() as u8);
            let (first, top) = (launch_force(&tp, 1), launch_force(&tp, highest));
            let at_rest = top * 0.5;
            let mut inp = driver_input(0.0, at_rest, 1.0);
            inp.sensed
                .as_mut()
                .expect("a Driver-policy reading")
                .rolling = (first + top) * 0.5 - at_rest;
            assert_eq!(
                affordable_launch(&tp, &inp, 1.0),
                Some(highest),
                "the fixture's highest launch gear clears the resistance at rest"
            );
            let moving = driver_input(0.0, (first + top) * 0.5, 1.0);
            let mut state = TransmissionState {
                gear: highest,
                ..fresh(&tp)
            };
            choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
            assert_eq!(
                Some(state.gear),
                affordable_launch(&tp, &moving, 1.0),
                "the launch holds against the rolling it meets moving"
            );
            assert!(
                state.gear < highest,
                "the fixture's load passes the highest launch gear"
            );
        }
    }

    /// A hull at rest with its clutch engaged never upshifts: every higher gear launches with less
    /// force. A stalled push relies on it to step down, never up. Fixture: each class and gear,
    /// slipping or engaged, against no resistance, half and all of first gear's launch.
    #[test]
    fn no_upshift_at_hull_rest() {
        for (fp, tp) in class_drivetrains() {
            for start in 1..=tp.gears_fwd.len() as u8 {
                for clutch in [ClutchState::Slip, ClutchState::In] {
                    for share in [0.0, 0.5, 1.0] {
                        let inp = driver_input(0.0, share * launch_force(&tp, 1), 1.0);
                        let mut state = TransmissionState {
                            gear: start,
                            clutch,
                            launch: if clutch == ClutchState::In {
                                LaunchState::Locked
                            } else {
                                LaunchState::TakingUp
                            },
                            ..fresh(&tp)
                        };
                        choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
                        assert!(
                            state.gear <= start,
                            "{clutch:?} gear {start} at rest rose to {}",
                            state.gear
                        );
                    }
                }
            }
        }
    }

    /// A full-pedal push held at rest by an obstacle steps down to first gear and never up: the
    /// stalled hull's push is what it feels, and every lower gear launches with more. Fixture: hull
    /// and belts held at rest, the obstacle returning the push the Driver felt a tick earlier.
    #[test]
    fn a_stalled_full_clutch_push_steps_down() {
        for (fp, tp) in class_drivetrains() {
            let construction = construction(&fp);
            let mass = construction.weight_n() / super::super::derive::G;
            let mut sense = DriverSense::for_cut(tp.shift_ticks);
            let mut state = TransmissionState {
                gear: tp.launch_gears.min(tp.gears_fwd.len() as u8),
                clutch: ClutchState::Slip,
                launch: LaunchState::TakingUp,
                ..fresh(&tp)
            };
            let mut pushed = [0.0_f32; 2];
            let mut gears = vec![state.gear];
            // Fixture: ten seconds for every step down.
            for _ in 0..64 * 10 {
                let mut inp = input(1.0, 0.0, [0.0; 2], pushed);
                let hard = pushed.map(|push| {
                    Some(HardReaction {
                        belt: push,
                        friction: FORWARD * push,
                        grip: f32::INFINITY, // Fixture: an obstacle gripping beyond any gear's pull.
                    })
                });
                inp.sensed = sense.tick(
                    Vec3::ZERO,
                    FORWARD,
                    0.0,
                    mass,
                    Contacts {
                        soft: [Vec3::ZERO; 2],
                        grip: 0.0,
                        rolling: Vec3::ZERO,
                        rolling_budget: Vec3::ZERO,
                        hard,
                    },
                    inp.dt,
                );
                let report = step(&fp, &tp, &mut state, &inp);
                sense.drove(&report);
                pushed = report.forces;
                if gears.last() != Some(&state.gear) {
                    assert!(
                        state.gear < *gears.last().unwrap(),
                        "a stalled push rose: {gears:?}"
                    );
                    gears.push(state.gear);
                }
            }
            println!("MEASURED stalled push gears={gears:?}");
            assert_eq!(
                state.gear, 1,
                "a stalled push ends in first gear: {gears:?}"
            );
        }
    }

    /// The game's driving model: no clause admits a gear whose crank is forecast to land past the
    /// engine's crank limit or on a crank its fuel cannot drive, so a climb that slows the hull out
    /// of top gear holds that gear until a lower one lands where its pedal pulls. The Driver relies
    /// on it not to brake a climbing hull at the ceiling. Mutants: crediting a landing gear the
    /// most it nets at a slower crank closes a clutch on a crank its fuel cannot drive, at once,
    /// and bounded at the limit later in the climb. Fixture: each class drivetrain's top gear at
    /// its governed speed and full pedal, against a load half way between what top gear and the
    /// next gear down hold at their governed speeds, read without error, for 60 s; the hull rides
    /// its belts.
    #[test]
    fn a_climb_out_of_top_gear_engages_no_gear_past_the_crank_limit() {
        for (fp, tp) in class_drivetrains() {
            let construction = construction(&fp);
            let top = tp.gears_fwd.len() as u8;
            let ratio = |gear: u8| tp.gears_fwd[usize::from(gear - 1)];
            let governed = tp.engine.governed_rpm * RPM_TO_RAD;
            let speed_at = |gear: u8, omega: f32| omega * tp.sprocket_radius / ratio(gear);
            let hold = |gear: u8| {
                let at = speed_at(gear, governed);
                viable_pull(
                    &tp,
                    DriveSupply::WHOLE,
                    at,
                    ratio(gear),
                    1.0,
                    ClutchState::In,
                )
            };
            let load = 0.5 * (hold(top) + hold(top - 1));
            let hull = construction.weight_n() / super::super::derive::G;
            let limit = tp.engine.crank_limit_rpm * RPM_TO_RAD;
            let mut st = TransmissionState {
                gear: top,
                clutch: ClutchState::In,
                launch: LaunchState::Locked,
                omega_e: governed,
                ..fresh(&tp)
            };
            let mut speed = speed_at(top, governed);
            let mut gears = vec![top];
            for tick in 0..64 * 60 {
                let mut inp = driver_input(speed, load, 1.0);
                // Each belt meets half the load and carries half the hull, which moves with it.
                inp.reactions = [0.5 * load; 2];
                inp.reaction_slopes = [0.5 * hull / inp.dt; 2];
                let prior = st.clutch;
                let report = step(&fp, &tp, &mut st, &inp);
                speed = 0.5 * (report.next_speeds[0] + report.next_speeds[1]);
                let rpm = st.omega_e / RPM_TO_RAD;
                assert!(
                    st.omega_e <= limit,
                    "tick {tick}: gear {} turns the crank past the limit, {rpm} rpm",
                    st.gear
                );
                if prior == ClutchState::Out && st.clutch != ClutchState::Out {
                    // What the pedal nets through the gear with its shaft turning the crank at `rpm`.
                    let shaft = st.omega_e * tp.sprocket_radius / ratio(st.gear);
                    let (whole, held) = (DriveSupply::WHOLE, ClutchState::In);
                    assert!(
                        price_driver_force(&tp, whole, shaft, ratio(st.gear), 1.0, held) > 0.0,
                        "tick {tick}: gear {} closes on {rpm} rpm, past fuel",
                        st.gear
                    );
                    gears.push(st.gear);
                }
            }
            assert!(
                gears.len() > 1 && speed >= park_engage_speed(),
                "the climb leaves top gear and moves on: {gears:?} at {speed} m/s"
            );
        }
    }

    /// The game's driving model: on overrun, the hull turning the engaged crank past where the
    /// pedal's fuel meets drag, the Driver judges each gear at its own crank. Under a pedal that
    /// drives a higher gear where it lands, it shifts up; under one that drives no gear, it pays no
    /// cut and the engaged gear keeps braking. A full-pedal descent relies on the first to leave a
    /// low gear the slope has pushed past the governor's cut; a lift-off on the second not to walk
    /// up the ladder shedding engine braking. Mutants: reading fuel against drag at the engaged
    /// crank holds the low gear at full pedal; cutting to a gear that only brakes less shifts up at
    /// the light pedal. Fixture: each class drivetrain locked in first gear, steady at the first
    /// whole rpm past its governed speed where full pedal nets nothing at the crank, the slope
    /// carrying exactly the drive the engaged gear delivers there, read without error on ample
    /// grip; the light pedal [`braking_pedal`], which nets nothing at any crank.
    #[test]
    fn an_overrun_gear_shifts_up_only_to_a_gear_the_pedal_drives() {
        for (fp, tp) in class_drivetrains() {
            let first = tp.gears_fwd[0];
            let at = |rpm: f32| rpm * RPM_TO_RAD * tp.sprocket_radius / first;
            let nets = |rpm: f32, pedal: f32| {
                price_driver_force(
                    &tp,
                    DriveSupply::WHOLE,
                    at(rpm),
                    first,
                    pedal,
                    ClutchState::In,
                )
            };
            let overrun = (1_u16..)
                .map(|past| tp.engine.governed_rpm + f32::from(past))
                .find(|&rpm| nets(rpm, 1.0) <= 0.0)
                .expect("full pedal nets nothing at the crank past the governor's cut");
            for (pedal, shifts) in [(1.0, true), (braking_pedal(&tp), false)] {
                let inp = driver_input(at(overrun), nets(overrun, pedal), pedal);
                let mut st = TransmissionState {
                    gear: 1,
                    clutch: ClutchState::In,
                    launch: LaunchState::Locked,
                    omega_e: overrun * RPM_TO_RAD,
                    ..fresh(&tp)
                };
                choose(&mut st, &tp, &construction(&fp), &inp, pedal, felt(&inp));
                assert_eq!(
                    (st.gear > 1, st.shift_ticks > 0),
                    (shifts, shifts),
                    "first gear overrun at {overrun} rpm under {pedal} pedal chose gear {}",
                    st.gear
                );
            }
        }
    }

    /// A raw peak beyond fuel cut is unavailable to launch. Both classes must select a capable
    /// gear inside their declared launch domain instead of retaining an out-of-domain cruise gear.
    #[test]
    fn launch_targets_the_peak_available_after_fuel_cut() {
        for mut spec in [compact_spec(), super::super::rig_geom::tiger_spec()] {
            let tr = &mut spec.track.powertrain.transmission;
            let peak = tr
                .engine
                .torque_curve
                .iter()
                .map(|point| point.1)
                .fold(0.0_f32, f32::max);
            // Fixture: compact's tail becomes 900 N m at 2600 rpm, beyond its 2200 rpm governor.
            tr.engine.torque_curve.last_mut().unwrap().1 = peak * 1.125;
            let mut fp = lab_fp();
            fp.inertia = 3000.0; // Fixture arithmetic coupling.
            fp.grip.side_weight = spec.mass * super::super::derive::G / 2.0;
            let tp = tr.params(0.3, 1.0, fp.inertia, spec.mass).unwrap(); // Fixture sprocket radius and half-tread.
            let inp = driver_input(0.0, 0.0, 1.0);
            let selected = affordable_launch(&tp, &inp, 1.0)
                .expect("fuel-governed peak must select a capable launch");
            assert!(tp.torque_at(tp.launch_omega(1.0) / RPM_TO_RAD) > 0.0);
            let mut state = TransmissionState {
                gear: tp.gears_fwd.len() as u8,
                park: true,
                ..fresh(&tp)
            };
            choose(&mut state, &tp, &construction(&fp), &inp, 1.0, felt(&inp));
            assert_eq!(state.gear, selected);
        }
    }
}
