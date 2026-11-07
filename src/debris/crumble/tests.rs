//! Crumble and rubble-routing invariants in the game's environment destruction model (game
//! state only). Asset-backed fixtures select members by construction properties; inline
//! fixtures state their geometry and material inputs.

use super::*;
use crate::construction::tests::shipped_house;
use crate::contact_transfer::ContactTransfer;

/// A fixture reading's entity: the moving group, another body, and a chunk's collider at `c + 1`.
fn entity(raw: u32) -> Entity {
    Entity::from_raw_u32(raw).expect("a fixture id")
}

const GROUP: u32 = 1_000_000;
const GROUND: u32 = 1_000_001;

fn collider(c: usize) -> Entity {
    entity(u32::try_from(c).unwrap() + 1)
}

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

    /// Its lowest course: every chunk (by index into the group) whose lowest point lies within a
    /// millimetre of the part's.
    fn course(&self) -> Vec<usize> {
        let floor = self.low.iter().copied().fold(f32::INFINITY, f32::min);
        (0..self.group.len())
            .filter(|&i| self.low[i] - floor < 1e-3)
            .collect()
    }

    /// A ground contact point on chunk `group[i]` at its lowest point, closing at `speed` m/s with
    /// effective mass `mass` kg, the solver having moved `moved` N·s into the ground through it.
    fn point(&self, i: usize, speed: f32, mass: f32, moved: f32) -> ContactTransfer {
        let c = self.group[i];
        let centroid = self.structure.chunks[c].centroid;
        ContactTransfer {
            structure: entity(GROUP),
            structure_collider: collider(c),
            body: entity(GROUND),
            body_collider: Some(entity(GROUND)),
            point: Vec3::new(centroid.x, self.low[i], centroid.z),
            normal: Vec3::NEG_Y,
            impulse: Vec3::NEG_Y * moved,
            effective_mass: mass,
            closing_speed: speed,
        }
    }

    /// The touches and energy the production adapter ([`landings`], [`touches`]) reads from
    /// `transfers` on the part, as a moving group at rest speed zero, against the ground.
    fn read(&self, transfers: &ContactTransfers) -> (Vec<Touch>, f32) {
        let ids: BTreeMap<Entity, usize> = self.group.iter().map(|&c| (collider(c), c)).collect();
        let landings = landings(transfers, |body| (body == entity(GROUP)).then_some(0.0));
        let landing = &landings[&entity(GROUP)];
        touches(
            &self.structure,
            Transform::IDENTITY,
            landing,
            |collider| ids[&collider],
            |_| Contactor::ground(),
        )
    }

    /// A flat landing on `against` at `speed` m/s, as the solver reads a plastic stop: the part's
    /// momentum `M v` as one blow, shared by its lowest course ([`Self::course`]) at their lowest
    /// points, upward, through the landing's force law ([`landing_force`]). With the kinetic
    /// energy it dissipates, `½ M v²`.
    fn landing_on(&self, speed: f32, against: Contactor) -> (Vec<Touch>, f32) {
        let course = self.course();
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

/// A LANDING CRACKS ALIKE AT ANY PHYSICS STEP: the production adapter reads one plastic stop as
/// one blow `J = M v` and the held weight as a press, whatever its step, and the landing's force
/// comes from the contact's stiffness, not the step. So the same far landing read at 1/32, 1/64
/// and 1/128 s cracks the same bonds with the same energy. A force of `J / Δt` (the step's)
/// doubles with each halving and cracks more. Fixture: the lowest course's points share the
/// part's mass evenly, as a flat face's do.
#[test]
fn a_landing_cracks_alike_at_any_physics_step() {
    let part = released_part();
    let speed = 2.0 * part.threshold(Contactor::ground());
    let (mass, g) = (part.mass(), crate::physical_laws::gravity_mps2());
    let course = part.course();
    let share = mass / course.len() as f32;
    let read: Vec<(Vec<usize>, f32, Vec3)> = [1.0 / 32.0, 1.0 / 64.0, 1.0 / 128.0]
        .into_iter()
        .map(|dt: f32| {
            let points = course
                .iter()
                .map(|&i| part.point(i, speed, share, share * (speed + g * dt)))
                .collect();
            let (touches, energy) = part.read(&ContactTransfers::fixture(vec![points], dt));
            let force = touches.iter().map(|touch| touch.force).sum::<Vec3>();
            (part.crumble(&touches, energy).cracked, energy, force)
        })
        .collect();
    let (cracked, energy, force) = &read[0];
    assert!(!cracked.is_empty(), "the landing cracks");
    for (other, spent, pushed) in &read {
        assert_eq!(other, cracked);
        assert!((spent - energy).abs() <= 1e-4 * energy);
        assert!((*pushed - *force).length() <= 1e-4 * force.length());
    }
}

/// ONE BLOW, ONE PEAK, HOWEVER SAMPLED: one blow on one chunk, read as one point or split evenly
/// among four about the same centre, is one patch to the adapter, so it puts the same force there
/// and cracks the same bonds. A mutant converting each point's share alone raises the summed peak
/// by `n^(2/5)` (DERIVED from `P ∝ m^(3/5)`) and cracks more. Fixture: the lowest course's first
/// chunk, struck with the part's whole momentum.
#[test]
fn one_blow_cracks_alike_however_many_points_sample_it() {
    let part = released_part();
    let speed = 4.0 * part.threshold(Contactor::ground());
    let mass = part.mass();
    let i = part.course()[0];
    let read = |n: usize| {
        let points = (0..n)
            .map(|k| {
                let mut point = part.point(i, speed, mass, mass * speed / n as f32);
                if n > 1 {
                    let angle = std::f32::consts::TAU * k as f32 / n as f32;
                    point.point += 0.05 * Vec3::new(angle.cos(), 0.0, angle.sin());
                }
                point
            })
            .collect();
        part.read(&ContactTransfers::fixture(vec![points], 1.0 / 64.0))
    };
    let ((one, energy), (four, split)) = (read(1), read(4));
    let force = |touches: &[Touch]| touches.iter().map(|touch| touch.force).sum::<Vec3>();
    assert!((force(&one) - force(&four)).length() <= 1e-4 * force(&one).length());
    assert!((energy - split).abs() <= 1e-4 * energy);
    let cracked = part.crumble(&one, energy).cracked;
    assert!(!cracked.is_empty(), "the blow cracks");
    assert_eq!(cracked, part.crumble(&four, split).cracked);
}

/// ONE COLLISION, ONE BUDGET: two equal moving groups closing on each other each read the
/// collision from their own side, with its one effective mass `μ` and closing speed `v`; together
/// they may spend at most the energy it dissipated, `½ μ v²`, each its compliance's share, half.
/// A mutant giving each side the whole budget spends it twice. Fixture: the part's first chunk on
/// both sides, `μ` half the part's mass (two equal bodies), closing at 3 m/s.
#[test]
fn two_groups_colliding_spend_one_collisions_energy() {
    let part = released_part();
    let c = part.group[0];
    let (mass, speed) = (0.5 * part.mass(), 3.0);
    let (a, b, ca, cb) = (
        entity(GROUP),
        entity(GROUND),
        collider(c),
        entity(GROUND + 1),
    );
    let side = |structure, own, body, theirs, normal: Vec3| ContactTransfer {
        structure,
        structure_collider: own,
        body,
        body_collider: Some(theirs),
        point: part.structure.chunks[c].centroid,
        normal,
        impulse: normal * mass * speed,
        effective_mass: mass,
        closing_speed: speed,
    };
    let transfers = ContactTransfers::fixture(
        vec![
            vec![side(a, ca, b, cb, Vec3::X)],
            vec![side(b, cb, a, ca, Vec3::NEG_X)],
        ],
        1.0 / 64.0,
    );
    let landings = landings(&transfers, |body| (body == a || body == b).then_some(0.0));
    assert_eq!(landings.len(), 2, "each side lands");
    let spent: Vec<f32> = landings
        .values()
        .map(|landing| {
            let chunk = Contactor::chunk(&part.structure, c);
            touches(
                &part.structure,
                Transform::IDENTITY,
                landing,
                |_| c,
                |_| chunk,
            )
            .1
        })
        .collect();
    let dissipated = 0.5 * mass * speed * speed;
    assert!(spent.iter().sum::<f32>() <= dissipated * (1.0 + 1e-5));
    assert!(
        spent
            .iter()
            .all(|e| (e - 0.5 * dissipated).abs() <= 1e-4 * dissipated)
    );
}

/// A LANDING ON AN ELEMENT THAT NEVER BREAKS meets its substance's face, read from the admitted
/// material data: the element has no law chunk to read one from. A mutant reading its first chunk
/// panics on the server. Fixture: the shipped house with its wood made of a steel, which never
/// breaks.
#[test]
fn a_landing_on_an_element_that_never_breaks_meets_its_substance() {
    let kind =
        std::sync::Arc::new(crate::construction::tests::shipped_house_of("Wood", "RHA").unwrap());
    let breakage = Breakage::new(kind.clone(), 1);
    let steel = (0..kind.asset.elements.len())
        .find(|&k| kind.layout.elements[k].is_none())
        .expect("the steel elements never break");
    let element = Element(u16::try_from(steel).unwrap());
    assert_eq!(
        meets(None, Some(&element), false, Some(&breakage)),
        kind.element_face(steel)
    );
}

/// SOFT GROUND CRACKS LESS: the Hertz peak scales as `E*^(2/5)` (DERIVED), and the ground's
/// stiffness data is below a rigid body's, so its onset speed ([`Part::threshold`]) lies above the
/// rigid floor's; a landing between the two, their geometric mean, cracks bonds on the rigid floor
/// and none on the ground. A mutant ignoring the other side's stiffness cracks both alike.
#[test]
fn a_landing_on_soft_ground_cracks_less_than_on_stiff() {
    let part = released_part();
    let chunk = Contactor::chunk(&part.structure, part.group[0]);
    let ground = Contactor::ground();
    let ratio = ((chunk.compliance + Contactor::RIGID.compliance)
        / (chunk.compliance + ground.compliance))
        .powf(0.4);
    assert!(ratio < 1.0, "the ground is softer than a rigid floor");
    let (rigid, soft) = (part.threshold(Contactor::RIGID), part.threshold(ground));
    assert!(
        soft > rigid,
        "the ground's onset {soft} m/s, the rigid floor's {rigid} m/s"
    );
    let speed = (rigid * soft).sqrt();
    let (stiff, energy) = part.landing_on(speed, Contactor::RIGID);
    let (yielding, _) = part.landing_on(speed, ground);
    let peak = |touches: &[Touch]| touches[0].force.length();
    assert!((peak(&yielding) / peak(&stiff) - ratio as f32).abs() < 1e-3);
    assert!(!part.crumble(&stiff, energy).cracked.is_empty());
    assert!(part.crumble(&yielding, energy).cracked.is_empty());
}

/// THE ENERGY LAW: the cracks absorb at most the energy the landing dissipated, the most
/// overloaded first, and stop at the first the rest cannot pay; with enough energy every failure
/// of the field happens. Each crack is priced here from the seam data alone, `G_I A` for tension
/// and `G_II A` for shear (a parting costs nothing), so a mutant overspending, undercharging,
/// swapping the modes or judging cheapest first breaks it.
#[test]
fn the_cracks_absorb_at_most_the_energy_the_landing_dissipated() {
    let part = released_part();
    let price = |failure: &Failure| {
        let joint = &part.structure.joints[failure.joint];
        let seam = joint
            .seam()
            .expect("a failing joint of a bonded part is a bond");
        match failure.cause {
            Cause::Tension => seam.fracture_energy * joint.area,
            Cause::Shear => seam.shear.energy * joint.area,
            _ => 0.0,
        }
    };
    let seam = part.structure.joints[0].seam().expect("a bonded part");
    assert_ne!(
        seam.fracture_energy, seam.shear.energy,
        "the modes price apart"
    );
    let (touches, _) = part.landing(4.0 * part.threshold(Contactor::ground()));
    let mut failures = part.failures(&touches);
    failures.sort_by(|a, b| {
        overload(b)
            .total_cmp(&overload(a))
            .then(a.joint.cmp(&b.joint))
    });
    let total: f32 = failures.iter().map(price).sum();
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
                let work = price(failure);
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
            assert!(
                (result.spent - spent).abs() <= 1e-5 * total,
                "{share}: spent {} J, priced {spent} J",
                result.spent
            );
        } else {
            assert_eq!(result.cracked[..expected.len()], expected[..], "{share}");
        }
    }
}

/// The game's environment destruction keeps a fragment on the server when its size, the cube
/// root of its volume, reaches the server's smallest body, and only then, whatever its mass: a
/// mutant that compares `>`, or that lets a small heavy fragment stay by its mass, routes one row
/// wrong.
#[test]
fn a_fragment_stays_a_server_body_by_its_size_alone() {
    use crate::construction::fracture::{Chunk, Material};
    let floor = PhysicalLaws::shipped().server_rubble_min_size_m;
    let registry = crate::substances::SubstanceRegistry::shipped_static();
    let material = Material::of(registry, "Brick").unwrap().unwrap();
    // Fixture volumes (m³) and masses (kg), set from the threshold.
    for (volume, mass, server) in [
        (floor.powi(3), 1e-3, true),
        ((2.0 * floor).powi(3), 1e-3, true),
        ((0.9 * floor).powi(3), 1e6, false),
    ] {
        let structure = Structure {
            geometry: crate::construction::fracture::bondsolve::Geometry::CircularFixture,
            part_of: vec![0],
            chunks: vec![Chunk {
                volume,
                mass,
                material,
                centroid: Vec3::ZERO,
                buried: false,
            }],
            joints: Vec::new(),
        };
        assert_eq!(
            server_body(&structure, &[0]),
            server,
            "{volume} m³, {mass} kg"
        );
    }
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
        ReleasedMotion {
            velocity,
            spin,
            centre,
        },
        9,
    );
    let at = frame.transform_point(part.structure.chunks[fragment[0]].centroid);
    let expected = velocity + spin.cross(at - centre);
    assert!((Vec3::from_array(shed.velocity) - expected).length() < 1e-4);
    assert_eq!(shed.spin, spin.to_array());
    assert_eq!((shed.construction, shed.tick), (5, 9));
    assert_eq!(shed.chunks, vec![fragment[0] as u16]);
}

/// The game's environment destruction keeps absorbed matter at rubble resolution. Only a joint
/// parted in this event may absorb new matter, the largest first; later evaluations must not undo
/// that absorption. A fragment left alone below it goes to decor and the record as dust.
#[test]
fn a_small_parted_neighbour_joins_one_server_body_and_stays_with_it() {
    use crate::construction::fracture::{Bond, Joint};
    let mut structure = released_part().structure;
    structure.chunks.truncate(3);
    structure.part_of.truncate(3);
    let laws = PhysicalLaws::shipped();
    // Fixture: one fragment below the floor between two resolved fragments; the joint to chunk 2
    // has the larger area.
    for (chunk, scale) in structure.chunks.iter_mut().zip([0.5_f32, 2.0, 2.0]) {
        chunk.volume = (laws.server_rubble_min_size_m * scale).powi(3);
    }
    let seam = structure.chunks[0].material.seam;
    structure.joints = [(1, 1.0), (2, 2.0)]
        .map(|(b, area)| Joint {
            a: 0,
            b: Side::Chunk(b),
            area,
            centroid: Vec3::ZERO,
            normal: Vec3::X,
            bond: Bond::Bonded(seam),
        })
        .to_vec();
    let fragments = vec![vec![0], vec![1], vec![2]];
    let mut retained = Vec::new();
    assert_eq!(
        merge_fragments(&structure, fragments.clone(), &[], &mut retained),
        fragments
    );
    let merged = merge_fragments(&structure, fragments.clone(), &[0, 1], &mut retained);
    assert_eq!(
        merged,
        [vec![0, 2], vec![1]],
        "the larger parted joint selects the neighbour"
    );
    assert_eq!(retained, [1]);
    assert_eq!(
        merge_fragments(&structure, fragments.clone(), &[], &mut retained),
        merged
    );
    let at_rest = |light: &[Vec<usize>]| {
        vec![
            ReleasedMotion {
                velocity: Vec3::ZERO,
                spin: Vec3::ZERO,
                centre: Vec3::ZERO,
            };
            light.len()
        ]
    };
    let mut record = Record::default();
    let (bodies, decor) = route(
        &structure,
        (1, Transform::IDENTITY),
        merged,
        at_rest,
        1,
        &mut record,
    );
    assert_eq!(bodies, [vec![0, 2], vec![1]]);
    assert!(decor.is_empty() && record.entries.is_empty());
    let (bodies, decor) = route(
        &structure,
        (1, Transform::IDENTITY),
        fragments,
        at_rest,
        1,
        &mut record,
    );
    assert_eq!(bodies, [vec![1], vec![2]]);
    assert_eq!(
        decor.iter().map(|shed| &shed.chunks).collect::<Vec<_>>(),
        [&[0]]
    );
    assert_eq!(
        record.entries,
        [Entry {
            construction: 1,
            event: Event::Dust(0),
            tick: 1,
        }],
        "the clients' ledger shows the decor chunk gone"
    );
    // A neighbour which remains standing cannot absorb the leaving fragment.
    assert_eq!(
        merge_fragments(&structure, vec![vec![0]], &[0], &mut Vec::new()),
        [vec![0]]
    );
}
