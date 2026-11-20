//! Drivetrain adapter for finite moving surfaces. Each solve starts from the same interval's
//! free momentum and bounds; only accumulated contact impulses change. The block owns regimes.

use super::{
    block::{FaceResponse, Solution, WarmStart},
    transmission::{self, PreparedDrive},
};
use crate::avian_ext::surface_motion::{self, SurfaceModel};
use avian3d::dynamics::solver::solver_body::SolverBody;
use std::sync::OnceLock;

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

    fn speed(&self, index: usize) -> f64 {
        f64::from(self.0[index])
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Coordinates {
    Drivetrain(Box<Drivetrain>),
    Prescribed(Prescribed),
}

#[derive(Clone, Debug)]
pub(crate) struct Drivetrain {
    prepared: PreparedDrive,
    impulses: [f64; 2],
    seed: WarmStart,
    current: OnceLock<Current>,
}

#[derive(Clone, Debug)]
struct Current {
    solved: Solution,
    response: FaceResponse,
    warm: WarmStart,
}

impl Drivetrain {
    /// Warm application preserves summation order but needs no intermediate solution. The first
    /// read settles all accumulated warm impulses; graph colouring gives the owner exclusive use.
    /// That read runs inside the contact colour loop, serial at small populations; against an
    /// owner-parallel settle pass after warm start, its cost is unmeasured.
    fn current(&self) -> &Current {
        self.current.get_or_init(|| {
            let mut warm = self.seed.clone();
            let mechanical = &self.prepared.mechanical;
            let solved = mechanical.solve(self.impulses, &mut warm);
            let response =
                solved.response(&mechanical.speeds, &mechanical.rows, &mechanical.limits);
            Current {
                solved,
                response,
                warm,
            }
        })
    }

    fn push(&mut self, index: usize, impulse: f64) -> bool {
        self.current();
        let total = self
            .impulses
            .get_mut(index)
            .expect("a contact binds its owner's prepared coordinate");
        if impulse == 0.0 {
            return false; // The current solution already certifies unchanged totals.
        }
        *total += impulse;
        let Current {
            solved,
            response,
            warm,
        } = self
            .current
            .get_mut()
            .expect("the current totals were just settled");
        let mechanical = &self.prepared.mechanical;
        if let Some(next) = solved.step(
            response,
            &mechanical.rows,
            &mechanical.limits,
            index + 1,
            impulse,
        ) {
            *solved = next;
            return false;
        }
        let next = mechanical.solve(self.impulses, warm);
        let changed = !solved.same_face(&next);
        if changed {
            *response = next.response(&mechanical.speeds, &mechanical.rows, &mechanical.limits);
        }
        *solved = next;
        changed
    }
}

impl Coordinates {
    #[cfg(test)]
    pub(crate) fn kinetic_energy(&self) -> f64 {
        let Self::Drivetrain(drive) = self else {
            panic!("the energy witness prepares dynamic mechanical coordinates");
        };
        drive
            .prepared
            .mechanical
            .speeds
            .iter()
            .zip(&drive.current().solved.speeds)
            .map(|(s, v)| 0.5 * s.inertia * v * v)
            .sum()
    }

    pub(crate) fn drivetrain(prepared: PreparedDrive, seed: WarmStart) -> Self {
        Self::Drivetrain(Box::new(Drivetrain {
            prepared,
            impulses: [0.0; 2],
            seed,
            current: OnceLock::new(),
        }))
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
        let Self::Drivetrain(drive) = self else {
            panic!(
                "a dynamic drivetrain finishes its own mechanical coordinates, never a replica's"
            );
        };
        // An owner no solve-stage visit read (no belt contact, or warm application only) settles
        // its totals here; otherwise this commits the solution the contacts exchanged against.
        let current = drive.current();
        warm.clone_from(&current.warm);
        transmission::finish_advance(tp, state, &drive.prepared, &current.solved, h)
    }

    #[cfg(test)]
    pub(crate) fn prepared(&self) -> &PreparedDrive {
        let Self::Drivetrain(drive) = self else {
            panic!("the witness prepares a dynamic drivetrain")
        };
        &drive.prepared
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
        let Self::Drivetrain(drive) = self else {
            panic!("the witness exchanges dynamic momentum")
        };
        drive.impulses
    }
    #[cfg(test)]
    pub(crate) fn add_impulses(&mut self, impulses: [f64; 2]) {
        for (i, p) in impulses.into_iter().enumerate() {
            self.push(i, p);
        }
    }
    #[cfg(feature = "bitprobe")]
    pub(crate) fn work(&self) -> f64 {
        let Self::Drivetrain(drive) = self else {
            return 0.0;
        }; // Replicas integrate no local work.
        drive.prepared.mechanical.speeds[1..]
            .iter()
            .zip(&drive.current().solved.speeds[1..])
            .zip(drive.impulses)
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
            Self::Drivetrain(drive) => drive.current().solved.speeds[index + 1],
            Self::Prescribed(p) => p.speed(index),
        }
    }
    fn mobility(&self, index: usize) -> f64 {
        match self {
            Self::Drivetrain(drive) => drive.current().response.mobility[index + 1][index + 1],
            Self::Prescribed(_) => 0.0,
        }
    }
    fn free_mobility(&self, index: usize) -> f64 {
        match self {
            Self::Drivetrain(drive) => 1.0 / drive.prepared.mechanical.speeds[index + 1].inertia,
            Self::Prescribed(_) => 0.0,
        }
    }
    fn warm_push(&mut self, index: usize, impulse: f64) {
        assert!(
            impulse.is_finite(),
            "a warm exchange supplies a finite generalized impulse"
        );
        match self {
            Self::Drivetrain(drive) => {
                *drive
                    .impulses
                    .get_mut(index)
                    .expect("a contact binds its owner's prepared coordinate") += impulse;
                drive.current.take();
            }
            Self::Prescribed(_) => {} // A replica's speeds are streamed.
        }
    }
    fn push(&mut self, index: usize, impulse: f64) -> bool {
        assert!(
            impulse.is_finite(),
            "a surface exchange supplies a finite generalized impulse"
        );
        match self {
            Self::Drivetrain(drive) => drive.push(index, impulse),
            Self::Prescribed(_) => false,
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

/// Fixture: an unpowered drivetrain whose bounded elements have zero capacity. Its contact
/// response is bare belt inertia, through the same adapter and block solver as a shipped tank.
#[cfg(test)]
pub(crate) fn free_fixture(speeds: [f32; 2], inertia: f32) -> Coordinates {
    use super::drive_fixture::{construction, lab_fp, lab_tp, unpowered_contact_tick};
    let mut fp = construction(&lab_fp());
    fp.belt_inertia = inertia;
    let mut tp = lab_tp();
    tp.brake_capacity_n = 0.0;
    tp.clutch_capacity = 0.0;
    tp.steer_capacity_n = 0.0;
    let state = transmission::TransmissionState {
        park: false,
        ..transmission::TransmissionState::stopped()
    };
    let mut tick = unpowered_contact_tick(&fp, &tp, state, speeds, 0.0, 1.0);
    tick.prepare_contact(&fp, &tp, 1.0, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Opposing contacts may exceed static capacity at an intermediate iterate. The committed
    /// regime must depend on their net total, so contact ordering cannot latch a brake open.
    #[test]
    fn final_contact_totals_redetermine_the_brake_regime() {
        use crate::track::drive_fixture::{
            class_drivetrains, construction, unpowered_contact_tick,
        };
        for (fp, tp) in class_drivetrains() {
            let fp = construction(&fp);
            let h = 1.0 / 384.0; // Fixture interval, s.
            let capacity = f64::from(tp.brake_capacity_n * tp.brake_static_factor * h);
            for sign in [-1.0, 1.0] {
                let state = transmission::TransmissionState::stopped();
                let mut tick = unpowered_contact_tick(&fp, &tp, state, [0.0; 2], 0.0, h);
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

    /// Every contact visit reads its owner's speeds after a push, and readback commits them. A
    /// step along the current face must reach the face and speeds a fresh search finds from the
    /// same totals, and hold each brake the search holds exactly at its target: the next
    /// interval offers static capacity only then. Fixture: class drivetrains driven, pivoting
    /// and braked, parked or not, from rest and moving, with seeded pushes of up to five brake
    /// capacities that cross held, sliding and limited faces.
    #[test]
    fn face_steps_agree_with_a_fresh_search() {
        use crate::track::drive::{DriveAxes, DriveIntent, DriveTarget};
        use crate::track::drive_fixture::{class_drivetrains, construction};
        use transmission::{DriveState, DriveSupply, DriveTick, TransmissionState};
        let (mut stepped, mut searched, mut held, mut worst) = (0, 0, 0, 0.0_f64);
        for (fp, tp) in class_drivetrains() {
            let fp = construction(&fp);
            let h = 1.0 / 384.0; // Fixture interval, s.
            let axes = DriveIntent::Axes(DriveAxes {
                throttle: 0.5,
                steer: 0.4,
            });
            let pivot = DriveIntent::Axes(DriveAxes {
                throttle: 0.0,
                steer: 0.4,
            });
            for intent in [axes, pivot, DriveIntent::Brake] {
                for park in [false, true] {
                    for speeds in [[0.0; 2], [1.0, -2.0], [4.0; 2]] {
                        let state = DriveState {
                            transmission: TransmissionState {
                                omega_e: 100.0,
                                park,
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
                            DriveTarget {
                                supply: DriveSupply::WHOLE,
                                intent,
                            },
                            h,
                        );
                        let mut coordinates = tick.prepare_contact(&fp, &tp, h, false);
                        let scale = 10.0 * f64::from(tp.brake_capacity_n * h);
                        let mut random = 0x1234_5678_u32;
                        for i in 0..100 {
                            random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                            let impulse = (f64::from(random) / f64::from(u32::MAX) - 0.5) * scale;
                            let index = i % 2;
                            {
                                let Coordinates::Drivetrain(drive) = &coordinates else {
                                    panic!("the fixture prepares a dynamic drivetrain")
                                };
                                let current = drive.current();
                                let mechanical = &drive.prepared.mechanical;
                                let mut totals = drive.impulses;
                                totals[index] += impulse;
                                let fresh = mechanical.solve(totals, &mut current.warm.clone());
                                match current.solved.step(
                                    &current.response,
                                    &mechanical.rows,
                                    &mechanical.limits,
                                    index + 1,
                                    impulse,
                                ) {
                                    Some(step) => {
                                        stepped += 1;
                                        assert!(
                                            step.same_face(&fresh),
                                            "a certified step keeps the face"
                                        );
                                        for (a, b) in step.speeds.iter().zip(&fresh.speeds) {
                                            worst = worst.max((a - b).abs() / (1.0 + b.abs()));
                                        }
                                        for row in mechanical.rows.iter().filter(|r| {
                                            r.dynamic.is_some() && !r.starts_sliding(&fresh.speeds)
                                        }) {
                                            held += 1;
                                            assert!(
                                                !row.starts_sliding(&step.speeds),
                                                "a step holds a brake exactly where the search does"
                                            );
                                        }
                                    }
                                    None => searched += 1,
                                }
                            }
                            coordinates.push(index, impulse);
                        }
                    }
                }
            }
        }
        println!(
            "FACE_STEP stepped={stepped} searched={searched} held={held} worst_relative={worst:e}"
        );
        assert!(
            stepped > 0 && searched > 0 && held > 0,
            "the fixture must step along held faces and cross faces"
        );
        // Resolution: the contact stores impulses in f32; a step must agree below that rounding.
        assert!(
            worst <= f64::from(f32::EPSILON),
            "worst relative speed gap {worst:e}"
        );
    }

    /// Contact iteration must not allocate for a block that fits its inline storage.
    #[test]
    fn small_contact_push_stays_on_the_stack() {
        // Fixture: free moving coordinates, inertia and one finite contact impulse.
        let mut coordinates = free_fixture([1.0, -1.0], 1000.0);
        let allocations = crate::tests::alloc_count::allocations_in(|| {
            coordinates.push(0, 10.0);
        });
        assert_eq!(
            allocations, 0,
            "small-block contact iteration must not enter the allocator"
        );
    }
}
