//! Drivetrain adapter for finite moving surfaces. Each solve starts from the same interval's
//! free momentum and bounds; only accumulated contact impulses change. The block owns regimes.

use super::{
    block::{FaceResponse, Solution, WarmStart},
    transmission::{self, PreparedDrive},
};
use crate::avian_ext::surface_motion::{self, SurfaceModel};
use avian3d::dynamics::solver::solver_body::SolverBody;

/// Replicas consume streamed speeds; the authority owns their momentum exchange (ADR-0037).
#[derive(Clone, Debug)]
pub(crate) struct Prescribed([f32; 2]);
impl Prescribed {
    pub(crate) fn new(speeds: [f32; 2]) -> Self {
        assert!(
            speeds.iter().all(|s| s.is_finite()),
            "streamed surface coordinates have finite speeds"
        );
        Self(speeds)
    }
}

impl SurfaceModel for Prescribed {
    fn clone_model(&self) -> Box<dyn SurfaceModel> {
        Box::new(self.clone())
    }
    fn speed(&self, index: usize) -> f64 {
        f64::from(self.0[index])
    }
    fn mobility(&self, _index: usize) -> f64 {
        0.0
    }
    fn free_mobility(&self, _index: usize) -> f64 {
        0.0
    }
    fn push(&mut self, _index: usize, _impulse: f64) -> bool {
        false
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Coordinates {
    Drivetrain {
        prepared: Box<PreparedDrive>,
        impulses: [f64; 2],
        solved: Solution,
        response: FaceResponse,
        warm: WarmStart,
    },
    Prescribed(Prescribed),
}

impl Coordinates {
    #[cfg(test)]
    pub(crate) fn kinetic_energy(&self) -> f64 {
        let Self::Drivetrain {
            prepared, solved, ..
        } = self
        else {
            panic!("the energy witness prepares dynamic mechanical coordinates");
        };
        prepared
            .mechanical
            .speeds
            .iter()
            .zip(&solved.speeds)
            .map(|(s, v)| 0.5 * s.inertia * v * v)
            .sum()
    }

    pub(crate) fn drivetrain(prepared: PreparedDrive) -> Self {
        let mut warm = WarmStart::default();
        let mechanical = &prepared.mechanical;
        let solved = mechanical.solve([0.0; 2], &mut warm);
        let response = solved.response(&mechanical.speeds, &mechanical.rows, &mechanical.limits);
        Self::Drivetrain {
            prepared: Box::new(prepared),
            impulses: [0.0; 2],
            solved,
            response,
            warm,
        }
    }

    pub(crate) fn install(self, body: &mut SolverBody) {
        surface_motion::install(self, body);
    }

    pub(crate) fn finish(
        &self,
        tp: &transmission::TransmissionParams,
        state: &mut transmission::DriveState,
        warm: &mut WarmStart,
        h: f32,
    ) -> transmission::TransmissionReport {
        let Self::Drivetrain {
            prepared, solved, ..
        } = self
        else {
            panic!(
                "a dynamic drivetrain finishes its own mechanical coordinates, never a replica's"
            );
        };
        warm.remember(
            &prepared.mechanical.rows,
            &prepared.mechanical.limits,
            solved,
        );
        transmission::finish_advance(tp, state, prepared, solved, h)
    }

    #[cfg(test)]
    pub(crate) fn prepared(&self) -> &PreparedDrive {
        let Self::Drivetrain { prepared, .. } = self else {
            panic!("the witness prepares a dynamic drivetrain")
        };
        prepared
    }

    #[cfg(test)]
    pub(crate) fn initial_speeds(&self) -> [f32; 2] {
        [0, 1].map(|i| self.prepared().mechanical.speeds[i + 1].value as f32)
    }

    #[cfg(test)]
    pub(crate) fn speeds(&self) -> [f32; 2] {
        [0, 1].map(|i| self.speed(i) as f32)
    }

    #[cfg(test)]
    pub(crate) fn contact_impulses(&self) -> [f64; 2] {
        let Self::Drivetrain { impulses, .. } = self else {
            panic!("the witness exchanges dynamic momentum")
        };
        *impulses
    }

    #[cfg(test)]
    pub(crate) fn add_impulses(&mut self, impulses: [f64; 2]) {
        for (i, p) in impulses.into_iter().enumerate() {
            self.push(i, p);
        }
    }

    #[cfg(feature = "bitprobe")]
    pub(crate) fn work(&self) -> f64 {
        let Self::Drivetrain {
            prepared,
            impulses,
            solved,
            ..
        } = self
        else {
            return 0.0;
        }; // Replicas integrate no local work.
        prepared.mechanical.speeds[1..]
            .iter()
            .zip(&solved.speeds[1..])
            .zip(impulses)
            .map(|((from, to), p)| p * (from.value + to) * 0.5)
            .sum()
    }
}

impl SurfaceModel for Coordinates {
    fn clone_model(&self) -> Box<dyn SurfaceModel> {
        Box::new(self.clone())
    }
    fn speed(&self, index: usize) -> f64 {
        match self {
            Self::Drivetrain { solved, .. } => solved.speeds[index + 1],
            Self::Prescribed(p) => p.speed(index),
        }
    }
    fn mobility(&self, index: usize) -> f64 {
        match self {
            Self::Drivetrain { response, .. } => response.mobility[index + 1][index + 1],
            Self::Prescribed(p) => p.mobility(index),
        }
    }
    fn free_mobility(&self, index: usize) -> f64 {
        match self {
            Self::Drivetrain { prepared, .. } => {
                1.0 / prepared.mechanical.speeds[index + 1].inertia
            }
            Self::Prescribed(p) => p.free_mobility(index),
        }
    }
    fn push(&mut self, index: usize, impulse: f64) -> bool {
        assert!(
            impulse.is_finite(),
            "a surface exchange supplies a finite generalized impulse"
        );
        match self {
            Self::Drivetrain {
                prepared,
                impulses,
                solved,
                response,
                warm,
            } => {
                let total = impulses
                    .get_mut(index)
                    .expect("a contact binds its owner's prepared coordinate");
                if impulse == 0.0 {
                    return false; // The current solution already certifies unchanged totals.
                }
                *total += impulse;
                let mechanical = &prepared.mechanical;
                let next = mechanical.solve(*impulses, warm);
                let changed = !solved.same_face(&next);
                if changed {
                    *response =
                        next.response(&mechanical.speeds, &mechanical.rows, &mechanical.limits);
                }
                *solved = next;
                changed
            }
            Self::Prescribed(p) => p.push(index, impulse),
        }
    }
}

pub(crate) fn surface(body: &SolverBody) -> &Coordinates {
    let data: &dyn core::any::Any = surface_motion::surface(body);
    data.downcast_ref()
        .expect("a belt owner installs its drivetrain or prescribed replica coordinates")
}

#[cfg(test)]
pub(crate) fn surface_mut(body: &mut SolverBody) -> &mut Coordinates {
    let data: &mut dyn core::any::Any = surface_motion::surface_mut(body);
    data.downcast_mut()
        .expect("the witness installs drivetrain coordinates")
}

pub(crate) fn read_back(
    body: &SolverBody,
    tick: &mut transmission::DriveTick,
    tp: &transmission::TransmissionParams,
    h: f32,
) {
    tick.finish_contact(tp, surface(body), h);
}

/// Fixture: an unpowered drivetrain whose bounded elements have zero capacity. Its contact
/// response is bare belt inertia, through the same adapter and block solver as a shipped tank.
#[cfg(test)]
pub(crate) fn free_fixture(speeds: [f32; 2], inertia: f32) -> Coordinates {
    use super::drive_fixture::{construction, lab_fp, lab_tp};
    use transmission::{DriveState, DriveSupply, DriveTick, TransmissionState};
    let mut fp = construction(&lab_fp());
    fp.belt_inertia = inertia;
    let mut tp = lab_tp();
    tp.brake_capacity_n = 0.0;
    tp.clutch_capacity = 0.0;
    tp.steer_capacity_n = 0.0;
    let state = DriveState {
        transmission: TransmissionState {
            omega_e: 0.0,
            park: false,
            mean_hold: false,
            ..TransmissionState::from_spec(&tp)
        },
        speeds,
        phases: [0.0; 2],
    };
    let mut tick = DriveTick::contact_only(
        &fp,
        &tp,
        state,
        super::drive::DriveTarget {
            supply: DriveSupply {
                control_authority: false,
                task_rate: 0.0,
                engine_in_action: false,
                transmission_condition: 0.0,
            },
            intent: super::drive::DriveIntent::Axes(super::drive::DriveAxes::default()),
        },
        1.0,
    );
    tick.prepare_contact(&fp, &tp, 1.0, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opposing contacts may exceed static capacity at an intermediate iterate. The committed
    /// regime must depend on their net total, so contact ordering cannot latch a brake open.
    #[test]
    fn final_contact_totals_redetermine_the_brake_regime() {
        use crate::track::drive_fixture::{class_drivetrains, construction};
        use transmission::{DriveState, DriveSupply, DriveTick, TransmissionState};
        for (fp, tp) in class_drivetrains() {
            let fp = construction(&fp);
            let h = 1.0 / 384.0; // Fixture interval, s.
            let capacity = f64::from(tp.brake_capacity_n * tp.brake_static_factor * h);
            for sign in [-1.0, 1.0] {
                let state = DriveState {
                    transmission: TransmissionState {
                        omega_e: 0.0,
                        park: true,
                        mean_hold: false,
                        ..TransmissionState::from_spec(&tp)
                    },
                    speeds: [0.0; 2],
                    phases: [0.0; 2],
                };
                let mut tick = DriveTick::contact_only(
                    &fp,
                    &tp,
                    state,
                    crate::track::drive::DriveTarget {
                        supply: DriveSupply {
                            control_authority: false,
                            task_rate: 0.0,
                            engine_in_action: false,
                            transmission_condition: 0.0,
                        },
                        intent: crate::track::drive::DriveIntent::Axes(
                            crate::track::drive::DriveAxes::default(),
                        ),
                    },
                    h,
                );
                let mut coordinates = tick.prepare_contact(&fp, &tp, h, false);
                let impulse = sign * 4.0 * capacity; // Fixture: exceeds every static side-stop budget.
                coordinates.push(0, impulse);
                assert!(
                    coordinates.speed(0).abs() > 0.0,
                    "the intermediate iterate must release the brake"
                );
                // DERIVED: this final demand is above kinetic but below static capacity.
                let final_load = sign * 0.5 * (capacity + f64::from(tp.brake_capacity_n * h));
                coordinates.push(0, final_load - impulse);
                assert_eq!(coordinates.speeds(), [0.0; 2]);
                tick.finish_contact(&tp, &coordinates, h);
                assert!(
                    !tick.report(1).brakes_slide && tick.report(1).held == [true; 2],
                    "the final within-static demand must restore hold"
                );
            }
        }
    }
}
