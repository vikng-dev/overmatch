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
        Self {
            state,
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
                self.belt_work.travel_m[i] += travel;
                self.belt_work.sprocket_work_j[i] += f64::from(report.forces[i]) * travel;
            }
        }
        self.last = Some(report);
    }

    /// Contact impulses update the same scalar coordinates the next free step reads. Clutch
    /// readout follows their final relative speed; a pre-contact stop no longer holds a moving
    /// belt. The motor and contact rows retain Avian's substep splitting.
    pub(crate) fn accept_contact_speeds(&mut self, tp: &TransmissionParams, speeds: [f32; 2]) {
        if speeds == self.state.speeds {
            return;
        }
        self.state.speeds = speeds;
        let controls = self.orders.controls;
        let st = &mut self.state.transmission;
        let k = controls.dir * controls.ratio / tp.sprocket_radius;
        let locked = !controls.clutch_open
            && tp
                .couplings
                .iter()
                .filter_map(|c| match c {
                    Coupling::Clutch(output) if !output.inner(controls.input.steer) => {
                        Some(*output)
                    }
                    _ => None, // Only closed drive outputs constrain the crank.
                })
                .all(|output| {
                    let weights = output.weights();
                    let shaft = f64::from(k)
                        * (weights[0] * f64::from(speeds[0]) + weights[1] * f64::from(speeds[1]));
                    // Resolution: two f32 roundings carry the block's crank and belt results to state.
                    (f64::from(st.omega_e) - shaft).abs()
                        <= 2.0
                            * f64::from(f32::EPSILON)
                            * (f64::from(st.omega_e).abs() + shaft.abs())
                });
        st.clutch = if controls.clutch_open {
            ClutchState::Out
        } else if locked {
            ClutchState::In
        } else {
            ClutchState::Slip
        };
        self.locked &= locked;
        for (held, speed) in self.held.iter_mut().zip(speeds) {
            *held &= speed == 0.0;
        }
        self.brakes_slide |= (st.park || controls.service) && speeds != [0.0; 2];
        let report = self
            .last
            .as_mut()
            .expect("a contact exchange follows its substep's motor advance");
        report.next_speeds = speeds;
        report.locked = locked;
        #[cfg(feature = "bitprobe")]
        {
            report.bitprobe.next_speeds = speeds;
            report.bitprobe.locked = locked;
        }
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
