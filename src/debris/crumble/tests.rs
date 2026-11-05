//! The crumble's invariants, in the game's environment destruction model (game state only), on
//! the shipped house's data. Which chunks a test reads is chosen by a class rule (a breakable
//! element's chunks, its lowest course, every chunk ordered by mass), never by number.

use super::*;
use crate::construction::tests::shipped_house;

/// A released part: the shipped house's breakable element with the most internal joints, every
/// joint leaving it parted (as a release leaves it), and each chunk's lowest point (m).
struct Part {
    structure: Structure,
    state: State,
    group: Vec<usize>,
    low: Vec<f32>,
}

fn released_part() -> Part {
    let breakage = Breakage::new(shipped_house(), 1);
    let structure = (*breakage.live.structure).clone();
    let layout = &breakage.kind.layout;
    let internal = |e: usize, joint: &crate::construction::fracture::Joint| {
        matches!(joint.b, Side::Chunk(b)
            if layout.element_of[joint.a] as usize == e && layout.element_of[b] as usize == e)
    };
    let element = (0..layout.elements.len())
        .max_by_key(|&e| structure.joints.iter().filter(|j| internal(e, j)).count())
        .expect("the house has elements");
    let group: Vec<usize> = (0..structure.chunks.len())
        .filter(|&c| layout.element_of[c] as usize == element && !structure.chunks[c].buried)
        .collect();
    let mut state = breakage.live.state.clone();
    state.retain_group(&group);
    for (j, joint) in structure.joints.iter().enumerate() {
        if !internal(element, joint) {
            state.separated[j] = true;
        }
    }
    let low = group
        .iter()
        .map(|&c| {
            layout.blocks[c]
                .points
                .iter()
                .map(|p| p.y)
                .fold(f32::INFINITY, f32::min)
        })
        .collect();
    Part {
        structure,
        state,
        group,
        low,
    }
}

impl Part {
    fn mass(&self) -> f32 {
        self.group
            .iter()
            .map(|&c| self.structure.chunks[c].mass)
            .sum()
    }

    /// A flat landing on the ground at `speed` m/s ([`Self::landing_on`]).
    fn landing(&self, speed: f32) -> (Vec<Touch>, f32) {
        self.landing_on(speed, Contactor::ground())
    }

    /// A flat landing on `against` at `speed` m/s, as the solver reads a plastic stop: the part's
    /// momentum `M v` as one blow, shared by its lowest course (every chunk whose lowest point lies
    /// within a millimetre of the part's) at their lowest points, upward, through the landing's
    /// force law ([`landing_force`]). With the kinetic energy it dissipates, `½ M v²`.
    fn landing_on(&self, speed: f32, against: Contactor) -> (Vec<Touch>, f32) {
        let floor = self.low.iter().copied().fold(f32::INFINITY, f32::min);
        let course: Vec<usize> = (0..self.group.len())
            .filter(|&i| self.low[i] - floor < 1e-3)
            .collect();
        let blow = Vec3::Y * self.mass() * speed / course.len() as f32;
        let touches = course
            .iter()
            .map(|&i| {
                let c = self.group[i];
                let centroid = self.structure.chunks[c].centroid;
                Touch {
                    chunk: c,
                    at: Vec3::new(centroid.x, self.low[i], centroid.z),
                    force: landing_force(&self.structure, c, against, (blow, Vec3::ZERO, speed)),
                }
            })
            .collect();
        (touches, 0.5 * self.mass() * speed * speed)
    }

    fn crumble(&self, touches: &[Touch], energy: f32) -> Crumble {
        crumble(&self.structure, &self.state, &self.group, touches, energy)
    }

    /// The kernel's failures under `touches`, before any energy is judged.
    fn failures(&self, touches: &[Touch]) -> Vec<Failure> {
        Kernel::of(&self.structure, &self.state, &self.group)
            .hold(
                &self.structure,
                &loads(&self.structure, &self.group, touches),
            )
            .failures
    }

    /// The least speed at which a flat landing on `against` fails a bond, m/s (DERIVED): the free
    /// part's field is linear in the contact force (uniform gravity cancels), so at a reference
    /// speed every failing joint's tension, shear and compression scale with the force. A tension
    /// crack starts at the force share `k = σ_t / σ`; a shear crack, where `τ = c + μ p`, at
    /// `k = c / (τ − μ p)`, `μ p` the capacity's friction share. The Hertz peak grows as
    /// `(m v²)^(3/5)`, so the speed scales by `k^(5/6)`. The seams' strengths and the two sides'
    /// stiffness come from the data.
    fn threshold(&self, against: Contactor) -> f32 {
        let reference = 100.0;
        let (touches, _) = self.landing_on(reference, against);
        let share = self
            .failures(&touches)
            .iter()
            .map(|failure| {
                let seam = self.structure.joints[failure.joint]
                    .seam()
                    .expect("a loaded internal joint of a breakable part is a bond");
                match failure.cause {
                    Cause::Tension => failure.capacity / failure.demand,
                    Cause::Shear => {
                        let cohesion = seam.shear.cohesion;
                        let friction = failure.capacity - cohesion;
                        cohesion / (failure.demand - friction)
                    }
                    cause => unreachable!("a bonded part fails by tension or shear, not {cause:?}"),
                }
            })
            .fold(f32::INFINITY, f32::min);
        reference * share.powf(5.0 / 6.0)
    }
}

/// A SHORT DROP LANDS WHOLE: below the derived threshold ([`Part::threshold`], from the seams'
/// strength data) no bond cracks and no joint parts, however much energy the drop brings; just
/// above it a bond cracks. A mutant that spends a landing's energy on seams the field does not
/// overload (the cheapest anywhere) cracks the short drop.
#[test]
fn a_short_drop_lands_whole() {
    let part = released_part();
    let speed = part.threshold(Contactor::ground());
    let g = crate::physical_laws::gravity_mps2();
    let height = speed * speed / (2.0 * g);
    println!(
        "MEASURED threshold: a {:.0} kg part of {} chunks cracks from a {height:.3} m drop ({speed:.3} m/s, DERIVED)",
        part.mass(),
        part.group.len()
    );
    assert!(speed.is_finite() && speed > 0.0, "the part has a threshold");
    let (short, energy) = part.landing(0.9 * speed);
    let result = part.crumble(&short, energy);
    assert!(result.cracked.is_empty() && result.separated.is_empty());
    assert_eq!(result.fragments, vec![part.group.clone()]);
    let (over, energy) = part.landing(1.1 * speed);
    assert!(!part.crumble(&over, energy).cracked.is_empty());
}

/// A FAR DROP CRACKS WHERE THE FORCE RUNS: the landing's first field cracks every bond it
/// overloads, and bonds no field reaches stay shut although the energy left would pay for each.
/// A mutant cracking by cost alone opens every seam; one ignoring the field cracks nothing.
#[test]
fn a_far_drop_cracks_only_where_the_force_runs() {
    let part = released_part();
    let (touches, energy) = part.landing(4.0 * part.threshold(Contactor::ground()));
    let result = part.crumble(&touches, energy);
    let first: Vec<usize> = part
        .failures(&touches)
        .iter()
        .filter(|failure| matches!(failure.cause, Cause::Tension | Cause::Shear))
        .map(|failure| failure.joint)
        .collect();
    assert!(!first.is_empty(), "a far drop overloads bonds");
    assert!(first.iter().all(|j| result.cracked.contains(j)));
    let left = energy - result.spent;
    let shut = part
        .structure
        .joints
        .iter()
        .enumerate()
        .filter(|(j, joint)| {
            matches!(joint.b, Side::Chunk(b) if part.group.contains(&joint.a) && part.group.contains(&b))
                && joint.seam().is_some()
                && !result.cracked.contains(j)
                && crack_work(&part.structure, *j, Cause::Shear)
                    .max(crack_work(&part.structure, *j, Cause::Tension))
                    <= left
        })
        .count();
    assert!(shut > 0, "bonds off the load path stay shut");
    println!(
        "MEASURED far drop: {} cracks, {} partings, {} fragments, {:.1} J of {energy:.0} J, work {}",
        result.cracked.len(),
        result.separated.len(),
        result.fragments.len(),
        result.spent,
        result.work
    );
}

/// A LANDING CRACKS ALIKE AT ANY PHYSICS STEP: the solver reads one plastic stop as one blow
/// `J = M v` and the held weight as a press, whatever its step, and the landing's force comes
/// from the contact's stiffness, not the step. So the same far landing read at 1/32, 1/64 and
/// 1/128 s cracks the same bonds. A force of `J / Δt` (the step's) doubles with each halving and
/// cracks more.
#[test]
fn a_landing_cracks_alike_at_any_physics_step() {
    let part = released_part();
    let speed = 2.0 * part.threshold(Contactor::ground());
    let weight = Vec3::Y * part.mass() * crate::physical_laws::gravity_mps2();
    let cracks: Vec<Vec<usize>> = [1.0 / 32.0, 1.0 / 64.0, 1.0 / 128.0]
        .iter()
        .map(|_step: &f32| {
            let (mut touches, energy) = part.landing(speed);
            let share = weight / touches.len() as f32;
            for touch in &mut touches {
                touch.force += share; // The press: what the contact holds, in newtons, per step.
            }
            part.crumble(&touches, energy).cracked
        })
        .collect();
    assert!(!cracks[0].is_empty(), "the landing cracks");
    assert!(cracks.iter().all(|c| *c == cracks[0]));
}

/// SOFT GROUND CRACKS LESS: the Hertz peak scales as `E*^(2/5)` (DERIVED), and the ground's
/// stiffness data is far below a rigid body's, so a landing that cracks bonds on a rigid floor
/// cracks none of them on the ground. A mutant ignoring the other side's stiffness cracks both
/// alike.
#[test]
fn a_landing_on_soft_ground_cracks_less_than_on_stiff() {
    let part = released_part();
    let chunk = Contactor::chunk(&part.structure, part.group[0]);
    let ground = Contactor::ground();
    let ratio = ((chunk.compliance + Contactor::RIGID.compliance)
        / (chunk.compliance + ground.compliance))
        .powf(0.4);
    assert!(ratio < 1.0, "the ground is softer than a rigid floor");
    let speed = 1.5 * part.threshold(Contactor::RIGID);
    let (stiff, energy) = part.landing_on(speed, Contactor::RIGID);
    let (soft, _) = part.landing_on(speed, ground);
    let peak = |touches: &[Touch]| touches[0].force.length();
    assert!((peak(&soft) / peak(&stiff) - ratio as f32).abs() < 1e-3);
    let (stiff, soft) = (
        part.crumble(&stiff, energy).cracked.len(),
        part.crumble(&soft, energy).cracked.len(),
    );
    assert!(soft < stiff, "soft {soft}, stiff {stiff}");
}

/// THE ENERGY LAW: the cracks absorb at most the energy the landing dissipated, the most
/// overloaded first, and stop at the first the rest cannot pay; with enough energy every failure
/// of the field happens. A mutant overspending, or judging cheapest first, breaks it.
#[test]
fn the_cracks_absorb_at_most_the_energy_the_landing_dissipated() {
    let part = released_part();
    let (touches, _) = part.landing(4.0 * part.threshold(Contactor::ground()));
    let mut failures = part.failures(&touches);
    failures.sort_by(|a, b| {
        overload(b)
            .total_cmp(&overload(a))
            .then(a.joint.cmp(&b.joint))
    });
    let total: f32 = failures
        .iter()
        .map(|failure| crack_work(&part.structure, failure.joint, failure.cause))
        .sum();
    assert!(total > 0.0, "the field cracks bonds");
    for share in [0.0, 0.25, 0.5, 1.0, 2.0] {
        let energy = total * share;
        let result = part.crumble(&touches, energy);
        assert!(
            result.spent <= energy,
            "{share}: {} of {energy} J",
            result.spent
        );
        let mut spent = 0.0;
        let expected: Vec<usize> = failures
            .iter()
            .map_while(|failure| {
                let work = crack_work(&part.structure, failure.joint, failure.cause);
                (spent + work <= energy).then(|| {
                    spent += work;
                    failure
                })
            })
            .filter(|failure| matches!(failure.cause, Cause::Tension | Cause::Shear))
            .map(|failure| failure.joint)
            .collect();
        // Within the landing's first field the cracks are its failures' prefix; energy left over
        // past it pays the cracks of the field that moves off them, after it.
        if share <= 1.0 {
            assert_eq!(
                result.cracked, expected,
                "{share}: the most overloaded first"
            );
        } else {
            assert_eq!(result.cracked[..expected.len()], expected[..], "{share}");
        }
    }
}

/// ROUTING: a fragment stays a server body exactly when its mass reaches the cut-off, read from its
/// owner ([`PhysicalLaws::server_fragment_kg`]); single chunks and growing runs of them, by mass,
/// straddle it. A mutant that compares volume, counts chunks or flips the comparison breaks it.
#[test]
fn fragments_at_the_cut_off_stay_server_bodies_and_lighter_ones_shed() {
    let structure = released_part().structure;
    let cut = PhysicalLaws::shipped().server_fragment_kg;
    let mut chunks: Vec<usize> = (0..structure.chunks.len()).collect();
    chunks.sort_by(|&a, &b| {
        structure.chunks[a]
            .mass
            .total_cmp(&structure.chunks[b].mass)
    });
    let (mut light, mut heavy) = (0, 0);
    for size in 1..=8 {
        for start in (0..chunks.len()).step_by(97) {
            let fragment: Vec<usize> = chunks.iter().skip(start).take(size).copied().collect();
            let mass: f32 = fragment.iter().map(|&c| structure.chunks[c].mass).sum();
            assert_eq!(server_body(&structure, &fragment), mass >= cut, "{mass} kg");
            if mass >= cut {
                heavy += 1;
            } else {
                light += 1;
            }
        }
    }
    assert!(light > 0 && heavy > 0, "the fixture straddles the cut-off");
}

/// A shed fragment leaves with the rigid velocity field of the group it left, at its own centre:
/// `v + ω × (c_f − c)`, and the group's spin. A mutant that hands it the group's centre velocity
/// throws a spinning part's tips the wrong way.
#[test]
fn a_shed_fragment_leaves_with_its_groups_rigid_motion() {
    let part = released_part();
    let frame = Transform::from_xyz(3.0, 2.0, -1.0).with_rotation(Quat::from_rotation_y(0.4));
    let (velocity, spin, centre) = (
        Vec3::new(1.0, -2.0, 0.5),
        Vec3::new(0.0, 3.0, 1.0),
        Vec3::ZERO,
    );
    let fragment = &part.group[..1];
    let shed = Shed::of(
        &part.structure,
        (5, frame),
        fragment,
        (velocity, spin, centre),
        9,
    );
    let at = frame.transform_point(part.structure.chunks[fragment[0]].centroid);
    let expected = velocity + spin.cross(at - centre);
    assert!((Vec3::from_array(shed.velocity) - expected).length() < 1e-4);
    assert_eq!(shed.spin, spin.to_array());
    assert_eq!((shed.construction, shed.tick), (5, 9));
    assert_eq!(shed.chunks, vec![fragment[0] as u16]);
}
