//! Tick orders and impulse-weighted reporting around the one mechanical integrator.

use super::*;
use crate::track::drive::{DriveAxes, DriveTarget};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct DriveState {
    pub transmission: TransmissionState,
    pub speeds: [f32; 2],
    pub phases: [f64; 2],
}

/// Server-local orders retain the sampled law's reference speeds for the whole tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Orders {
    pub(super) controls: Controls,
    #[cfg(feature = "bitprobe")]
    candidates: usize,
}

pub(crate) fn prepare_orders(
    fp: &DriveConstruction,
    tp: &TransmissionParams,
    state: &mut TransmissionState,
    input: &TransmissionInput,
) -> Orders {
    #[cfg(feature = "bitprobe")]
    super::super::block::take_assignment_work();
    let controls = driver(fp, tp, state, input);
    Orders {
        controls,
        #[cfg(feature = "bitprobe")]
        candidates: super::super::block::take_assignment_work(),
    }
}

/// Each substep depends on this owner's preceding speeds. Reports average delivered impulses
/// over elapsed time; a tick is locked/held only if every substep was, and drives if any did.
pub(crate) struct DriveTick {
    pub state: DriveState,
    initial: DriveState,
    contacts_started: bool,
    pub(crate) orders: Orders,
    pub(crate) warm: super::super::block::WarmStart,
    last: Option<TransmissionReport>,
    steps: u32,
    elapsed: f64,
    impulse: [f64; 2],
    power_time: f64,
    held: [bool; 2],
    locked: bool,
    driving: bool,
    brakes_slide: bool,
    #[cfg(feature = "bitprobe")]
    assignments: usize,
    #[cfg(feature = "bitprobe")]
    rows: usize,
    #[cfg(feature = "bitprobe")]
    max_rows: usize,
    #[cfg(feature = "bitprobe")]
    max_assignments: usize,
    #[cfg(feature = "bitprobe")]
    pub(crate) belt_work: crate::bitprobe::BeltWorkProbe,
}

impl DriveTick {
    pub fn new(
        fp: &DriveConstruction,
        tp: &TransmissionParams,
        mut state: DriveState,
        input: &TransmissionInput,
    ) -> Self {
        let orders = prepare_orders(fp, tp, &mut state.transmission, input);
        Self::from_orders(state, orders)
    }

    fn from_orders(state: DriveState, orders: Orders) -> Self {
        Self {
            state,
            initial: state,
            contacts_started: false,
            orders,
            warm: super::super::block::WarmStart::default(),
            last: None,
            steps: 0,
            elapsed: 0.0,
            impulse: [0.0; 2],
            power_time: 0.0,
            held: [true; 2],
            locked: true,
            driving: false,
            brakes_slide: false,
            #[cfg(feature = "bitprobe")]
            assignments: 0,
            #[cfg(feature = "bitprobe")]
            rows: 0,
            #[cfg(feature = "bitprobe")]
            max_rows: 0,
            #[cfg(feature = "bitprobe")]
            max_assignments: 0,
            #[cfg(feature = "bitprobe")]
            belt_work: crate::bitprobe::BeltWorkProbe {
                start_speeds: state.speeds,
                ..Default::default()
            },
        }
    }

    pub fn advance(&mut self, fp: &DriveConstruction, tp: &TransmissionParams, h: f32) {
        let report = super::advance(fp, tp, &mut self.state, &self.orders, &mut self.warm, h);
        self.record(report, h);
    }

    fn record(&mut self, report: TransmissionReport, h: f32) {
        let dt = f64::from(h);
        self.elapsed += dt;
        self.steps += 1;
        for (i, force) in report.forces.into_iter().enumerate() {
            self.impulse[i] += f64::from(force) * dt;
            self.held[i] &= report.held[i];
        }
        self.power_time += f64::from(report.power_available) * dt;
        self.locked &= report.locked;
        self.driving |= report.driving;
        self.brakes_slide |= report.brakes_slide;
        #[cfg(feature = "bitprobe")]
        {
            self.assignments += report.assignments_tried;
            self.rows += report.rows;
            self.max_assignments = self.max_assignments.max(report.assignments_tried);
            self.max_rows = self.max_rows.max(report.rows);
            for i in 0..2 {
                // Work uses the mean speed of this interval: integral of a linear speed ramp.
                let travel = (f64::from(report.bitprobe.speeds[i])
                    + f64::from(report.next_speeds[i]))
                    * 0.5
                    * dt;
                self.belt_work.sprocket_work_j[i] += f64::from(report.forces[i]) * travel;
            }
        }
        self.last = Some(report);
    }

    pub(crate) fn prepare_contact(
        &mut self,
        fp: &DriveConstruction,
        tp: &TransmissionParams,
        h: f32,
        resting: bool,
    ) -> super::super::surface_motion::Coordinates {
        if !self.contacts_started {
            if resting {
                // The sleeping trial is a prediction. A real wake replays that interval once.
                let warm = std::mem::take(&mut self.warm);
                *self = Self::from_orders(self.initial, self.orders);
                self.warm = warm;
            }
            self.contacts_started = true;
        }
        super::super::surface_motion::Coordinates::drivetrain(super::prepare_advance(
            fp,
            tp,
            &self.state,
            &self.orders,
            h,
        ))
    }

    pub(crate) fn finish_contact(
        &mut self,
        tp: &TransmissionParams,
        coordinates: &super::super::surface_motion::Coordinates,
        h: f32,
    ) {
        let report = coordinates.finish(tp, &mut self.state, &mut self.warm, h);
        self.record(report, h);
        #[cfg(feature = "bitprobe")]
        {
            self.belt_work.solver_work_j += coordinates.work();
        }
    }

    /// An unsampled interval retains real supply and intent; only the sampled ground load is absent.
    pub(crate) fn contact_only(
        fp: &DriveConstruction,
        tp: &TransmissionParams,
        state: DriveState,
        target: DriveTarget,
        h: f32,
    ) -> Self {
        let axes = match target.intent {
            DriveIntent::Axes(axes) => axes,
            DriveIntent::Brake => DriveAxes::default(),
        };
        Self::new(
            fp,
            tp,
            state,
            &TransmissionInput {
                supply: target.supply,
                intent: target.intent,
                throttle: axes.throttle,
                steer: axes.steer,
                side_commands: axes.side_commands(),
                speeds: state.speeds,
                reactions: [0.0; 2],
                reaction_slopes: [0.0; 2],
                sensed: None,
                dt: h,
            },
        )
    }

    pub fn report(&self, substeps: u32) -> TransmissionReport {
        assert_eq!(
            self.steps, substeps,
            "a prepared drivetrain advances exactly once per physics substep"
        );
        let mut report = self.last.expect("a physics tick has at least one substep");
        report.forces = self.impulse.map(|impulse| (impulse / self.elapsed) as f32);
        report.power_available = (self.power_time / self.elapsed) as f32;
        report.held = self.held;
        report.locked = self.locked;
        report.driving = self.driving;
        report.brakes_slide = self.brakes_slide;
        #[cfg(feature = "bitprobe")]
        {
            report.assignments_tried = self.assignments;
            report.rows = self.rows;
            report.bitprobe.assignments_tried = self.assignments + self.orders.candidates;
            report.bitprobe.mechanical_assignments = self.assignments;
            report.bitprobe.mechanical_rows = self.rows;
            report.bitprobe.mechanical_advances = self.steps as usize;
            report.bitprobe.max_substep_assignments = self.max_assignments;
            report.bitprobe.max_substep_rows = self.max_rows;
            report.bitprobe.belt_work = self.belt_work;
            report.bitprobe.driving = self.driving;
            report.bitprobe.brakes_slide = self.brakes_slide;
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A physics wake without sampling still receives the real operator and power supply.
    /// Scheduling may erase only the absent sampled ground load, never throttle or task service.
    #[test]
    fn contact_only_preserves_supply_and_intent_and_omits_only_sampled_load() {
        use crate::track::drive_fixture::{class_drivetrains, construction};
        for (fp, tp) in class_drivetrains() {
            let fp = construction(&fp);
            let axes = DriveAxes {
                throttle: 0.7,
                steer: 0.4,
            }; // Fixture operator axes.
            let target = DriveTarget {
                supply: DriveSupply::WHOLE,
                intent: DriveIntent::Axes(axes),
            };
            let state = DriveState {
                transmission: TransmissionState::from_spec(&tp),
                speeds: [0.0; 2],
                phases: [0.0; 2],
            };
            let tick = DriveTick::contact_only(&fp, &tp, state, target, 1.0 / 384.0);
            let input = tick.orders.controls.input;
            assert_eq!(input.supply, target.supply);
            assert_eq!(input.intent, target.intent);
            assert_eq!(input.throttle, axes.throttle);
            assert_eq!(input.steer, axes.steer);
            assert_eq!(input.side_commands, axes.side_commands());
            assert_eq!(input.reactions, [0.0; 2]);
            assert_eq!(input.reaction_slopes, [0.0; 2]);
            assert!(input.sensed.is_none());
        }
    }
}
