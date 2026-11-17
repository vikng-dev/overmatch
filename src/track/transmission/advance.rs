//! Tick orders and impulse-weighted reporting around the one mechanical integrator.

use super::*;

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
    pub(crate) orders: Orders,
    last: Option<TransmissionReport>,
    steps: u32,
    elapsed: f64,
    impulse: [f64; 2],
    power_time: f64,
    held: [bool; 2],
    locked: bool,
    driving: bool,
    brakes_slide: bool,
    assignments: usize,
    rows: usize,
    #[cfg(feature = "bitprobe")]
    max_rows: usize,
    #[cfg(feature = "bitprobe")]
    max_assignments: usize,
    #[cfg(feature = "bitprobe")]
    belt_work: crate::bitprobe::BeltWorkProbe,
}

impl DriveTick {
    pub fn new(
        fp: &DriveConstruction,
        tp: &TransmissionParams,
        mut state: DriveState,
        input: &TransmissionInput,
    ) -> Self {
        let orders = prepare_orders(fp, tp, &mut state.transmission, input);
        Self {
            state,
            orders,
            last: None,
            steps: 0,
            elapsed: 0.0,
            impulse: [0.0; 2],
            power_time: 0.0,
            held: [true; 2],
            locked: true,
            driving: false,
            brakes_slide: false,
            assignments: 0,
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
        let report = super::advance(fp, tp, &mut self.state, &self.orders, h);
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
        self.assignments += report.assignments_tried;
        self.rows += report.rows;
        #[cfg(feature = "bitprobe")]
        {
            self.max_assignments = self.max_assignments.max(report.assignments_tried);
            self.max_rows = self.max_rows.max(report.rows);
            for i in 0..2 {
                // Work uses the mean speed of this interval: integral of a linear speed ramp.
                let travel = (f64::from(report.bitprobe.speeds[i])
                    + f64::from(report.next_speeds[i]))
                    * 0.5
                    * dt;
                self.belt_work.travel_m[i] += travel;
                self.belt_work.sprocket_work_j[i] += f64::from(report.forces[i]) * travel;
            }
        }
        self.last = Some(report);
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
        report.assignments_tried = self.assignments;
        report.rows = self.rows;
        #[cfg(feature = "bitprobe")]
        {
            report.bitprobe.assignments_tried = self.assignments + self.orders.candidates;
            report.bitprobe.mechanical_assignments = self.assignments;
            report.bitprobe.mechanical_rows = self.rows;
            report.bitprobe.mechanical_advances = self.steps as usize;
            report.bitprobe.max_substep_assignments = self.max_assignments;
            report.bitprobe.max_substep_rows = self.max_rows;
            report.bitprobe.belt_work = self.belt_work;
        }
        report
    }
}
