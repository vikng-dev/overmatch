//! Hand-checkable gates for the part-scale verdict of the game's environment destruction model
//! (game state only). Fixture geometry and strengths are labelled fixture data: they select
//! which law term governs, never a shipped value. Each analytical expectation is DERIVED beside it.

use super::*;
use crate::construction::fracture::{Bond, Chunk, Joint, Material, Origin, Seam, bondsolve};
use bevy::math::Vec3;

/// A fixture physics step, s: the game's 64 Hz tick.
const STEP: f32 = 1.0 / 64.0;

fn g() -> f64 {
    f64::from(crate::physical_laws::gravity_mps2())
}

fn brick() -> Material {
    Material::of(
        crate::substances::SubstanceRegistry::shipped_static(),
        "Brick",
    )
    .expect("the shipped registry holds Brick")
    .expect("Brick states a seam strength")
}

/// Fixture seam: tensile strength `tensile` Pa, cohesion `cohesion` Pa, the brick's works.
fn seam(tensile: f32, cohesion: f32) -> Seam {
    let mut seam = brick().seam;
    seam.tensile = tensile;
    seam.shear.cohesion = cohesion;
    seam
}

/// A fixture cube chunk of side `side` m centred at `centre`.
fn cube(centre: Vec3, side: f32, buried: bool) -> Chunk {
    let material = brick();
    let volume = side * side * side;
    Chunk {
        volume,
        centroid: centre,
        mass: material.density * volume,
        material,
        buried,
    }
}

fn joint(a: usize, b: Side, centroid: Vec3, normal: Vec3, area: f32, bond: Bond) -> Joint {
    Joint {
        a,
        b,
        area,
        centroid,
        normal,
        bond,
    }
}

fn structure(chunks: Vec<Chunk>, part_of: Vec<usize>, joints: Vec<Joint>) -> Structure {
    Structure {
        chunks,
        part_of,
        joints,
        geometry: bondsolve::Geometry::CircularFixture,
    }
}

fn press(chunk: usize, at: Vec3, force: Vec3) -> Load {
    Load {
        chunk,
        at,
        energy: 0.0,
        crushed: 0.0,
        blow: Vec3::ZERO,
        press: force,
        peak: None,
        origin: Origin::Field,
    }
}

/// The force part `a` hands across each interface between `a` and `b`, summed over its joints.
fn reaction(assessment: &Assessment, a: usize, b: Option<usize>) -> DVec3 {
    let graph = &assessment.graph;
    let n = graph.parts.len();
    graph
        .interfaces
        .iter()
        .filter(|i| (i.a, i.b) == (a, b) || (Some(i.a), i.b.map(Some)) == (b, Some(Some(a))))
        .flat_map(|i| i.bonded.iter().chain(&i.push))
        .map(|&e| {
            let edge = &graph.edges[e];
            let sign = if edge.a == a { 1.0 } else { -1.0 };
            edge.resultant(&graph.nodes, &assessment.answer[n..]).linear * sign
        })
        .sum()
}

fn close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= 1e-6 * expected.abs().max(1.0),
        "{what}: {actual} against {expected}"
    );
}

/// A LINTEL ON TWO PIERS SHARES ITS LOAD BY THE LEVER OF ITS TWO JOINTS. The piers are buried
/// parts; the lintel is one rigid part on two equal disk joints at x = ±s. DERIVED: vertical
/// equilibrium gives each joint P/2 from the centred weight; a point load P at x = a adds
/// ±P a s / (2 s² + r²/2), the disks' own rocking stiffness (r²/4 each) beside the lever.
/// The pier under the load carries more, and the two shares sum to the load.
#[test]
fn a_lintel_on_two_piers_shares_its_load() {
    let (s, side, area) = (1.0_f32, 0.4_f32, 0.04_f32);
    let bond = Bond::Bonded(seam(1e9, 1e9));
    let chunks = vec![
        cube(Vec3::new(-s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(0.0, side / 2.0, 0.0), side, false),
    ];
    let joints = vec![
        joint(
            2,
            Side::Chunk(0),
            Vec3::new(-s, 0.0, 0.0),
            Vec3::Y,
            area,
            bond,
        ),
        joint(
            2,
            Side::Chunk(1),
            Vec3::new(s, 0.0, 0.0),
            Vec3::Y,
            area,
            bond,
        ),
    ];
    let structure = structure(chunks, vec![0, 1, 2], joints);
    let weight = f64::from(structure.chunks[2].mass) * g();
    let (load, at) = (2000.0_f64, 0.5_f64);
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    let (failures, _) = assessment.advance(
        &structure,
        &mut state,
        &[press(
            2,
            Vec3::new(at as f32, side / 2.0, 0.0),
            Vec3::NEG_Y * load as f32,
        )],
        Vec3::ZERO,
        STEP,
        false,
    );
    assert!(failures.is_empty(), "{failures:?}");
    let radius_squared = f64::from(area) / std::f64::consts::PI;
    let s = f64::from(s);
    let moment_share = load * at * s / (2.0 * s * s + radius_squared / 2.0);
    let left = -reaction(&assessment, 2, Some(0)).y;
    let right = -reaction(&assessment, 2, Some(1)).y;
    // The lintel hands its load down: a compressive, downward force on each pier.
    close(left, (weight + load) / 2.0 - moment_share, "left pier");
    close(right, (weight + load) / 2.0 + moment_share, "right pier");
    close(left + right, weight + load, "sum");
}

fn tick(
    structure: &Structure,
    state: &mut State,
    assessment: &mut Assessment,
    loads: &[Load],
) -> Vec<Failure> {
    assessment
        .advance(structure, state, loads, Vec3::ZERO, STEP, false)
        .0
}

/// A CANTILEVER HELD AT ONE INTERFACE CARRIES THE SUM AND MOMENT OF ITS LOADS, AND CRACKS AT ITS
/// NAVIER LIMIT. A beam cube hangs off a buried wall by one vertical disk joint. DERIVED: the
/// joint carries W + P and the moment W d + P L about its centre; a disk's peak tension is
/// 4 M / (r A), so P* = (f_t r A / 4 − W d) / L. Past it the interface cracks (Tension); next
/// tick the cracked face has no compression to hold the moment, tips, and the beam is released.
#[test]
fn a_cantilever_carries_its_load_and_falls_past_its_navier_limit() {
    let (side, area, tensile) = (0.4_f32, 0.04_f32, 1.0e6_f32);
    let chunks = vec![
        cube(Vec3::new(-side / 2.0, 0.0, 0.0), side, true),
        cube(Vec3::new(side / 2.0, 0.0, 0.0), side, false),
    ];
    let joints = vec![joint(
        1,
        Side::Chunk(0),
        Vec3::ZERO,
        Vec3::X,
        area,
        Bond::Bonded(seam(tensile, 1e9)),
    )];
    let structure = structure(chunks, vec![0, 1], joints);
    let weight = f64::from(structure.chunks[1].mass) * g();
    let (d, lever) = (f64::from(side) / 2.0, f64::from(side));
    let radius = (f64::from(area) / std::f64::consts::PI).sqrt();
    let limit = (f64::from(tensile) * radius * f64::from(area) / 4.0 - weight * d) / lever;
    assert!(limit > 0.0, "the fixture beam holds its own weight");
    let tip = |factor: f64| {
        press(
            1,
            Vec3::new(side, 0.0, 0.0),
            Vec3::NEG_Y * (factor * limit) as f32,
        )
    };
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &[tip(0.98)]).is_empty());
    let n = assessment.graph.parts.len();
    let edge = &assessment.graph.edges[0];
    let carried = edge.resultant(&assessment.graph.nodes, &assessment.answer[n..]);
    close(-carried.linear.y, weight + 0.98 * limit, "shear");
    close(
        carried.angular.length(),
        weight * d + 0.98 * limit * lever,
        "moment",
    );
    let failures = tick(&structure, &mut state, &mut assessment, &[tip(1.02)]);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].cause, Cause::Tension);
    assert!(assessment.support.is_empty(), "a cracked face still bears");
    let failures = tick(&structure, &mut state, &mut assessment, &[tip(1.02)]);
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].cause, Cause::Tip);
    assert_eq!(assessment.support, vec![vec![1]]);
}

/// DAMAGE INSIDE A PART LOWERS ITS INTERFACE'S CAPACITY ONLY THROUGH THE JOINT AREA LEFT. The
/// two-cube wall part below loses one cube to wear; its base interface keeps one ground disk, under
/// the remaining cube's centre. DERIVED single-disk capacity, pushed at height z: P* z =
/// (f_t + W₁ / A) A r / 4, below the intact two-disk capacity.
#[test]
fn damage_inside_a_part_lowers_its_base_capacity_through_the_joints_left() {
    let (s, side, area, tensile) = (0.2_f32, 0.4_f32, 0.04_f32, 1.0e5_f32);
    let bond = Bond::Bonded(seam(tensile, 1e9));
    let chunks = vec![
        cube(Vec3::new(-s, side / 2.0, 0.0), side, false),
        cube(Vec3::new(s, side / 2.0, 0.0), side, false),
    ];
    let joints = vec![
        joint(
            0,
            Side::Chunk(1),
            Vec3::new(0.0, side / 2.0, 0.0),
            Vec3::X,
            0.16,
            bond,
        ),
        joint(
            0,
            Side::Ground,
            Vec3::new(-s, 0.0, 0.0),
            Vec3::Y,
            area,
            bond,
        ),
        joint(1, Side::Ground, Vec3::new(s, 0.0, 0.0), Vec3::Y, area, bond),
    ];
    let structure = structure(chunks, vec![0, 0], joints);
    let mut state = State::new(&structure);
    let volume = structure.chunks[0].volume;
    assert!(super::super::wear(&structure, &mut state, 0, volume));
    let weight = f64::from(structure.chunks[1].mass) * g();
    let (a, z) = (f64::from(area), f64::from(side));
    let r = (a / std::f64::consts::PI).sqrt();
    let limit = (f64::from(tensile) + weight / a) * a * r / (4.0 * z);
    let s64 = f64::from(s);
    let intact =
        (f64::from(tensile) + weight / a) * a * (2.0 * s64 * s64 + r * r / 2.0) / ((s64 + r) * z);
    assert!(limit < intact, "{limit} N against the intact {intact} N");
    let push = |factor: f64| {
        press(
            1,
            Vec3::new(s, side, 0.0),
            Vec3::X * (factor * limit) as f32,
        )
    };
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &[push(0.98)]).is_empty());
    let interface = &assessment.graph.interfaces[0];
    assert_eq!((interface.b, interface.bonded.len()), (None, 1));
    let failures = tick(&structure, &mut state, &mut assessment, &[push(1.02)]);
    assert!(
        failures.len() == 1 && failures[0].cause == Cause::Tension,
        "{failures:?}"
    );
}

/// A WALL PART IS PUSHED OVER AT ITS BASE INTERFACE'S CAPACITY. Two cubes are one part on two
/// ground disks at x = ±s. DERIVED Navier capacity of that two-disk section, pushed at height z:
/// P* z = (f_t + W / 2A) A (2 s² + r²/2) / (s + r). Below it nothing changes and a held press
/// costs no solve; past it the base cracks whole, then tips (its moment over its weight leaves
/// the disks' hull) and the part is released.
#[test]
fn a_wall_part_is_pushed_over_at_its_base_capacity() {
    let (s, side, area, tensile) = (0.2_f32, 0.4_f32, 0.04_f32, 1.0e5_f32);
    let bond = Bond::Bonded(seam(tensile, 1e9));
    let chunks = vec![
        cube(Vec3::new(-s, side / 2.0, 0.0), side, false),
        cube(Vec3::new(s, side / 2.0, 0.0), side, false),
    ];
    let joints = vec![
        joint(
            0,
            Side::Chunk(1),
            Vec3::new(0.0, side / 2.0, 0.0),
            Vec3::X,
            0.16,
            bond,
        ),
        joint(
            0,
            Side::Ground,
            Vec3::new(-s, 0.0, 0.0),
            Vec3::Y,
            area,
            bond,
        ),
        joint(1, Side::Ground, Vec3::new(s, 0.0, 0.0), Vec3::Y, area, bond),
    ];
    let structure = structure(chunks, vec![0, 0], joints);
    let weight = 2.0 * f64::from(structure.chunks[0].mass) * g();
    let (a, s, z) = (f64::from(area), f64::from(s), f64::from(side));
    let r2 = a / std::f64::consts::PI;
    let limit = (f64::from(tensile) + weight / (2.0 * a)) * a * (2.0 * s * s + r2 / 2.0)
        / ((s + r2.sqrt()) * z);
    assert!(
        limit * z > weight * (s + r2.sqrt()),
        "the fixture tips once cracked"
    );
    let push = |factor: f64| {
        press(
            1,
            Vec3::new(s as f32, side, 0.0),
            Vec3::X * (factor * limit) as f32,
        )
    };
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &[push(0.98)]).is_empty());
    assert_eq!(
        assessment.graph.parts.len(),
        1,
        "two chunks of one element are one part"
    );
    let (_, solves) = assessment.advance(
        &structure,
        &mut state,
        &[push(0.98)],
        Vec3::ZERO,
        STEP,
        false,
    );
    assert_eq!(
        (solves, assessment.work),
        (0, 0),
        "a held press reuses its field"
    );
    let failures = tick(&structure, &mut state, &mut assessment, &[push(1.02)]);
    assert_eq!(failures.len(), 2, "the base cracks whole: {failures:?}");
    assert!(failures.iter().all(|f| f.cause == Cause::Tension));
    let failures = tick(&structure, &mut state, &mut assessment, &[push(1.02)]);
    assert!(
        failures.len() == 2 && failures.iter().all(|f| f.cause == Cause::Tip),
        "{failures:?}"
    );
    assert_eq!(assessment.support, vec![vec![0, 1]]);
}

/// A PART RESTING ON TWO OTHERS BEARS HALF ITS WEIGHT ON EACH, AND SLIPS PAST FRICTION. A slab
/// rests (no bond) on two buried piers at x = ±s. DERIVED: each rest bears W / 2; a push P in the
/// rests' plane loads each with P / 2 of shear, so both slip once P > μ W, μ the pair friction.
/// Nothing else holds the slab tangentially, so the slip is a mechanism: its rests separate and
/// it is released.
#[test]
fn a_part_resting_on_two_others_bears_and_slips_at_friction() {
    let (s, side, area) = (1.0_f32, 0.4_f32, 0.04_f32);
    let chunks = vec![
        cube(Vec3::new(-s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(0.0, side / 2.0, 0.0), side, false),
    ];
    let joints = vec![
        joint(
            2,
            Side::Chunk(0),
            Vec3::new(-s, 0.0, 0.0),
            Vec3::Y,
            area,
            Bond::Resting,
        ),
        joint(
            2,
            Side::Chunk(1),
            Vec3::new(s, 0.0, 0.0),
            Vec3::Y,
            area,
            Bond::Resting,
        ),
    ];
    let structure = structure(chunks, vec![0, 1, 2], joints);
    let weight = f64::from(structure.chunks[2].mass) * g();
    let mu = bondsolve::pair_friction(&structure, &structure.joints[0]);
    let push = |factor: f64| press(2, Vec3::ZERO, Vec3::X * (factor * mu * weight) as f32);
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &[push(0.98)]).is_empty());
    close(
        -reaction(&assessment, 2, Some(0)).y,
        weight / 2.0,
        "left rest",
    );
    close(
        -reaction(&assessment, 2, Some(1)).y,
        weight / 2.0,
        "right rest",
    );
    let failures = tick(&structure, &mut state, &mut assessment, &[push(1.02)]);
    assert!(
        failures.len() == 2 && failures.iter().all(|f| f.cause == Cause::Slide),
        "{failures:?}"
    );
    assert_eq!(assessment.support, vec![vec![2]]);
}

/// A REST THAT SLIPS CARRIES COULOMB FRICTION AND HANDS THE EXCESS TO ITS PART'S OTHER
/// INTERFACE. A block rests on a slope of angle θ and is fixed to a buried post uphill by a soft
/// fixture fixing (1/1000 of the bulk slope), so the rest carries nearly all of the shear while it
/// sticks.
/// DERIVED: below θ = atan μ the rest sticks; above it the rest slips, carries no tangential
/// stiffness and μN against the slip, and the fixing carries the rest of the downhill weight,
/// W sin θ − μN, by the block's equilibrium. It keeps bearing, nothing separates, and once
/// friction settles the block costs nothing.
#[test]
fn a_slipping_rest_carries_friction_and_hands_the_excess_to_its_fixing() {
    let (side, rest_area, fixing_area) = (0.4_f32, 0.16_f32, 0.01_f32);
    let soft = Bond::Bonded(Seam {
        fixing: Some(super::super::Fixing {
            normal: 5.0e7,
            shear: 5.0e7,
        }),
        ..seam(1e9, 1e9)
    });
    let slope = |theta: f64| {
        let n = Vec3::new(-theta.sin() as f32, theta.cos() as f32, 0.0);
        let u = Vec3::new(theta.cos() as f32, theta.sin() as f32, 0.0);
        let centre = n * side / 2.0;
        structure(
            vec![
                cube(centre, side, false),
                cube(centre + u * side, side, true),
            ],
            vec![0, 1],
            vec![
                joint(0, Side::Ground, Vec3::ZERO, n, rest_area, Bond::Resting),
                joint(
                    0,
                    Side::Chunk(1),
                    centre + u * side / 2.0,
                    u,
                    fixing_area,
                    soft,
                ),
            ],
        )
    };
    let probe = slope(0.0);
    let mu = bondsolve::pair_friction(&probe, &probe.joints[0]);
    let weight = f64::from(probe.chunks[0].mass) * g();
    let carried = |assessment: &Assessment, joint: usize| {
        let graph = &assessment.graph;
        let n = graph.parts.len();
        let e = graph
            .joints
            .iter()
            .position(|&j| j == joint)
            .expect("the joint is an edge");
        let edge = &graph.edges[e];
        (
            edge.resultant(&graph.nodes, &assessment.answer[n..]),
            edge.normal,
        )
    };

    let stick = slope(0.8 * mu.atan());
    let mut state = State::new(&stick);
    let mut assessment = Assessment::default();
    assert!(tick(&stick, &mut state, &mut assessment, &[]).is_empty());
    assert!(
        assessment.slipping.is_empty(),
        "below atan μ the rest sticks"
    );

    let theta = 1.2 * mu.atan();
    let slip = slope(theta);
    let mut state = State::new(&slip);
    let mut assessment = Assessment::default();
    let failures = tick(&slip, &mut state, &mut assessment, &[]);
    assert!(failures.is_empty(), "{failures:?}");
    assert!(
        assessment.slipping.contains_key(&0) && !assessment.slipping.contains_key(&1),
        "the rest slips, the fixing holds"
    );
    let (rest, normal) = carried(&assessment, 0);
    let pressing = -rest.linear.dot(normal);
    assert!(pressing > 0.0, "a slipping rest keeps bearing");
    let friction = (rest.linear + normal * pressing).length();
    assert!(
        (friction - mu * pressing).abs() <= 1e-9 * weight,
        "a slipping rest carries exactly μN: {friction} N against {} N",
        mu * pressing
    );
    let (fixing, _) = carried(&assessment, 1);
    let uphill = DVec3::new(theta.cos(), theta.sin(), 0.0);
    let excess = weight * theta.sin() - friction;
    assert!(
        excess > 0.0,
        "friction alone does not hold the block above atan μ"
    );
    assert!(
        (-fixing.linear.dot(uphill) - excess).abs() <= 1e-6 * weight,
        "the fixing carries the excess: {} N against {excess} N",
        -fixing.linear.dot(uphill)
    );
    let (failures, solves) = assessment.advance(&slip, &mut state, &[], Vec3::ZERO, STEP, false);
    assert!(failures.is_empty());
    assert_eq!(
        (solves, assessment.work),
        (0, 0),
        "a settled slip costs nothing"
    );
}

/// A fixture running-bond wall of the game's environment destruction model: `courses` courses of
/// `bricks` bricks (length 0.24 m, height 0.08 m, thickness 0.12 m, fixture data), one element,
/// every joint bonded by `seam`, course 0 on the ground. Odd courses shift half a brick, with half
/// bricks at their ends. Returns it and each chunk's (course, left x, right x).
fn wall(courses: usize, bricks: usize, seam: Seam) -> (Structure, Vec<(usize, f32, f32)>) {
    let (l, h, t) = (0.24_f32, 0.08_f32, 0.12_f32);
    let material = brick();
    let mut chunks = Vec::new();
    let mut layout = Vec::new();
    for k in 0..courses {
        let cuts: Vec<f32> = if k % 2 == 0 {
            (0..=bricks).map(|i| i as f32 * l).collect()
        } else {
            std::iter::once(0.0)
                .chain((0..bricks).map(|i| (i as f32 + 0.5) * l))
                .chain(std::iter::once(bricks as f32 * l))
                .collect()
        };
        for w in cuts.windows(2) {
            let volume = (w[1] - w[0]) * h * t;
            chunks.push(Chunk {
                volume,
                centroid: Vec3::new((w[0] + w[1]) / 2.0, h * (k as f32 + 0.5), 0.0),
                mass: material.density * volume,
                material,
                buried: false,
            });
            layout.push((k, w[0], w[1]));
        }
    }
    let bond = Bond::Bonded(seam);
    let mut joints = Vec::new();
    for (a, &(k, x0, x1)) in layout.iter().enumerate() {
        let y = h * k as f32;
        if k == 0 {
            joints.push(joint(
                a,
                Side::Ground,
                Vec3::new((x0 + x1) / 2.0, 0.0, 0.0),
                Vec3::Y,
                (x1 - x0) * t,
                bond,
            ));
        }
        for (b, &(kb, y0, y1)) in layout.iter().enumerate() {
            if kb == k && y0 == x1 {
                joints.push(joint(
                    a,
                    Side::Chunk(b),
                    Vec3::new(x1, y + h / 2.0, 0.0),
                    Vec3::X,
                    h * t,
                    bond,
                ));
            }
            let (lo, hi) = (x0.max(y0), x1.min(y1));
            if kb + 1 == k && hi > lo {
                joints.push(joint(
                    a,
                    Side::Chunk(b),
                    Vec3::new((lo + hi) / 2.0, y, 0.0),
                    Vec3::Y,
                    (hi - lo) * t,
                    bond,
                ));
            }
        }
    }
    let part_of = vec![0; chunks.len()];
    (structure(chunks, part_of, joints), layout)
}

/// A HOLE IN A STANDING WALL: THE CHUNKS ITS JOINTS CANNOT HOLD DROP OUT, AND THE REST ARCHES. A
/// fixture running-bond wall loses the middle bricks of its lowest courses. The part's chunk
/// graph is solved for gravity, its joints judged at chunk scale (mortar tension included), and
/// the part law then judges the pieces. Cross-check, CLAIMED (NCMA TEK 17-01D): arching carries
/// the masonry above a 45° triangle over the opening, so at most that triangle's bricks load a
/// lintel. The bricks that drop, by where they sit against that triangle, are printed for the
/// report: observations, never pins. Invariant: the cascade ends with a finite field, and the
/// wall beside the opening, below its head, keeps standing.
#[test]
fn a_hole_in_a_wall_drops_what_its_joints_cannot_hold_and_the_rest_arches() {
    let (courses, bricks) = (12, 12);
    let (l, h) = (0.24_f32, 0.08_f32);
    for (label, mortar) in [
        ("shipped mortar", brick().seam),
        ("no-tension fixture mortar", seam(0.0, 0.0)),
    ] {
        let (structure, layout) = wall(courses, bricks, mortar);
        let mut live = crate::construction::breakage::Live::from_structure(structure, 1);
        let element_of = vec![0_u16; layout.len()];
        let (quiet, causes, _) = settle(&element_of, &mut live, |_| Vec::new());
        assert!(
            causes.is_empty(),
            "{label}: the intact wall stands: {causes:?}"
        );
        let (left, right, head) = (4.0 * l, 8.0 * l, 3.0 * h);
        let opening = |&(k, x0, x1): &(usize, f32, f32)| {
            (k as f32) * h < head && x0 >= left - 1e-4 && x1 <= right + 1e-4
        };
        let hole: Vec<usize> = (0..layout.len()).filter(|&c| opening(&layout[c])).collect();
        for &c in &hole {
            let volume = live.structure.chunks[c].volume;
            assert!(super::super::wear(
                &live.structure,
                &mut live.state,
                c,
                volume
            ));
        }
        let event = crate::construction::breakage::run_laws(
            &element_of,
            &mut live,
            &[],
            Vec::new(),
            Vec3::ZERO,
            STEP,
        );
        let event_work = live.assessment.work;
        let (ticks, mut causes, released) = settle(&element_of, &mut live, |_| Vec::new());
        for failure in &event.failures {
            *causes.entry(format!("{:?}", failure.cause)).or_default() += 1;
        }
        let middle = (left + right) / 2.0;
        let in_triangle = |&(k, x0, x1): &(usize, f32, f32)| {
            let y = (k as f32 + 0.5) * h - head;
            let x = (x0 + x1) / 2.0;
            y > 0.0 && (x - middle).abs() < (right - left) / 2.0 - y
        };
        let dropped: Vec<usize> = (0..layout.len())
            .filter(|&c| !hole.contains(&c) && !live.state.standing(c))
            .collect();
        let triangle = (0..layout.len())
            .filter(|&c| in_triangle(&layout[c]))
            .count();
        let inside = dropped.iter().filter(|&&c| in_triangle(&layout[c])).count();
        println!(
            "HOLE {label}: removed={} quiet_after={quiet} ticks={ticks} failures={causes:?} released={released:?} dropped={} triangle_bricks={triangle} dropped_in_triangle={inside} dropped_outside={} event_work={event_work}",
            hole.len(),
            dropped.len(),
            dropped.len() - inside,
        );
        // Each course from the top, the opening's middle at the centre: o stands, x dropped.
        for k in (0..courses).rev() {
            let row: String = (0..layout.len())
                .filter(|&c| layout[c].0 == k)
                .map(|c| match (hole.contains(&c), dropped.contains(&c)) {
                    (true, _) => ' ',
                    (false, true) => 'x',
                    (false, false) => 'o',
                })
                .collect();
            println!("HOLE {label} course {k:2}: {row}");
        }
        let beside =
            |&(k, x0, x1): &(usize, f32, f32)| (k as f32) * h < head && (x1 <= left || x0 >= right);
        assert!(
            dropped.iter().all(|&c| !beside(&layout[c])),
            "{label}: the piers beside the opening stand"
        );
    }
}

/// The rest normal stiffness of a fixture block's ground rest of `area`, N/m: the unit the
/// reviewer's normalized fixtures state stiffness in.
fn rest_stiffness(block: &Chunk, area: f32) -> f64 {
    let probe = structure(
        vec![*block],
        vec![0],
        vec![joint(
            0,
            Side::Ground,
            block.centroid,
            Vec3::Y,
            area,
            Bond::Resting,
        )],
    );
    bondsolve::joint_edge(&probe, 0, 0, None, true).normal_stiffness
}

/// A fixture block of the game's environment destruction model at the origin, friction ½, on a
/// ground rest and fixed to a buried post by a fixing whose translational stiffness in the x–y
/// plane is `fixing` (in units of the rest's normal stiffness, both patches at the block's centre
/// so translation and rotation decouple). Its fixing normal points away from the block, so the
/// block's weight pulls the fixing and its shear capacity is its cohesion alone. Returns the
/// structure and its fixing's area.
fn oblique(fixing: [[f64; 2]; 2], cohesion: impl Fn(f64, f32) -> f32) -> (Structure, f32) {
    let (side, rest_area, fixing_area) = (0.4_f32, 0.04_f32, 0.01_f32);
    let mut block = cube(Vec3::ZERO, side, false);
    block.material.friction = 0.5;
    let k = rest_stiffness(&block, rest_area);
    let weight = f64::from(block.mass) * g();
    // DERIVED eigen-decomposition of the symmetric 2×2 stiffness.
    let [[a, b], [_, c]] = fixing;
    let mean = (a + c) / 2.0;
    let spread = (((a - c) / 2.0).powi(2) + b * b).sqrt();
    let (stiff, soft) = (mean + spread, mean - spread);
    let normal = DVec3::new(b, stiff - a, 0.0).normalize();
    let seam = Seam {
        fixing: Some(super::super::Fixing {
            normal: (stiff * k / f64::from(fixing_area)) as f32,
            shear: (soft * k / f64::from(fixing_area)) as f32,
        }),
        ..seam(1e12, cohesion(weight, fixing_area))
    };
    let structure = structure(
        vec![block, cube(Vec3::new(1.0, 0.0, 0.0), side, true)],
        vec![0, 1],
        vec![
            joint(
                0,
                Side::Ground,
                Vec3::ZERO,
                Vec3::Y,
                rest_area,
                Bond::Resting,
            ),
            joint(
                0,
                Side::Chunk(1),
                Vec3::ZERO,
                -normal.as_vec3(),
                fixing_area,
                Bond::Bonded(seam),
            ),
        ],
    );
    (structure, fixing_area)
}

/// DERIVED slipped state of [`oblique`] under its weight W (units W and the rest stiffness k):
/// with the rest slipping toward +x at friction μN, `[[a, b], [b, c + 1]] u = (−μN, −W)` and
/// `N = −u_y`, so `u_y = −W / ((c + 1) − b (b − μ) / a)` and `u_x = −(b − μ) u_y / a`. Returns
/// the rest's compression and the fixing's force on the block's side, in W.
fn oblique_slipped(fixing: [[f64; 2]; 2], mu: f64) -> (f64, DVec3) {
    let [[a, b], [_, c]] = fixing;
    let y = -1.0 / ((c + 1.0) - b * (b - mu) / a);
    let x = -(b - mu) * y / a;
    (-y, DVec3::new(a * x + b * y, b * x + c * y, 0.0))
}

/// SATURATED FRICTION CONVERGES BY CONSTRUCTION. The reviewer's fixture: a block of weight W on
/// a rest of normal stiffness k at μ = ½, with an oblique fixing of stiffness k [[1, 3], [3, 9.5]].
/// A fixed point on the friction force alternates there (`f ← W/3 − f`); friction solved as a
/// term of the system settles in the tick it slips, at the DERIVED N = W/3 and μN = W/6, and the
/// next tick costs nothing.
#[test]
fn friction_in_the_system_settles_where_a_fixed_point_cycles() {
    let fixing = [[1.0, 3.0], [3.0, 9.5]];
    let (structure, _) = oblique(fixing, |_, _| 1e12);
    let weight = f64::from(structure.chunks[0].mass) * g();
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    let failures = tick(&structure, &mut state, &mut assessment, &[]);
    assert!(failures.is_empty(), "{failures:?}");
    assert!(assessment.slipping.contains_key(&0), "the rest slips");
    let (pressing, _) = oblique_slipped(fixing, 0.5);
    let graph = &assessment.graph;
    let n = graph.parts.len();
    let e = graph
        .joints
        .iter()
        .position(|&j| j == 0)
        .expect("the rest is an edge");
    let rest = graph.edges[e].resultant(&graph.nodes, &assessment.answer[n..]);
    let normal = graph.edges[e].normal;
    close(-rest.linear.dot(normal) / weight, pressing, "N / W");
    close(
        (rest.linear - normal * rest.linear.dot(normal)).length() / weight,
        0.5 * pressing,
        "μN / W",
    );
    let (failures, solves) =
        assessment.advance(&structure, &mut state, &[], Vec3::ZERO, STEP, false);
    assert!(failures.is_empty());
    assert_eq!(
        (solves, assessment.work),
        (0, 0),
        "a settled slip costs nothing"
    );
    assert!(!assessment.needs_tick());
}

/// STRENGTH IS JUDGED ON THE SETTLED CONTACT FIELD ONLY. The reviewer's fixture with fixing
/// stiffness k [[1, 2], [2, 5]]: DERIVED, the settled fixing carries a shear of ≈ 0.101 W / A
/// (from [`oblique_slipped`]), while a first slipped iterate of a fixed point reads ≈ 0.146 W / A.
/// A cohesion above the settled demand holds; one below it cracks.
#[test]
fn a_fixing_cracks_only_past_its_settled_demand() {
    let fixing = [[1.0, 2.0], [2.0, 5.0]];
    let (_, force) = oblique_slipped(fixing, 0.5);
    let (probe, area) = oblique(fixing, |_, _| 1e12);
    let normal = probe.joints[1].normal.as_dvec3();
    let shear = (force - normal * force.dot(normal)).length();
    assert!(
        (shear - 0.101).abs() < 1e-3,
        "DERIVED settled shear {shear} W / A"
    );
    for (factor, cracks) in [(1.15, false), (0.9, true)] {
        let (structure, _) = oblique(fixing, |weight, area| {
            (factor * shear * weight / f64::from(area)) as f32
        });
        let mut state = State::new(&structure);
        let mut assessment = Assessment::default();
        let failures = tick(&structure, &mut state, &mut assessment, &[]);
        assert_eq!(
            failures
                .iter()
                .any(|f| f.joint == 1 && f.cause == Cause::Shear),
            cracks,
            "cohesion {factor} × the settled demand ({area} m²): {failures:?}"
        );
    }
}

/// A REST BEARS ON ITS OWN, WHATEVER BOND SHARES ITS FACE. A block rests on a buried pier and is
/// fixed to it by a fixture fixing 1/1000 as stiff as the rest, both centred under the block.
/// An uplift P past its weight W lifts the rest, so the fixing carries all of P − W (DERIVED); at
/// P − W = 1.5 f_t A it cracks in tension. A bilateral rest would shield it to (P − W) / 1001.
#[test]
fn an_uplift_opens_the_rest_and_the_fixing_carries_it_all() {
    let (side, rest_area, fixing_area, tensile) = (0.4_f32, 0.04_f32, 0.01_f32, 1.0e5_f32);
    let block = cube(Vec3::new(0.0, side / 2.0, 0.0), side, false);
    let k = rest_stiffness(&block, rest_area);
    let weight = f64::from(block.mass) * g();
    let slope = (k / 1000.0 / f64::from(fixing_area)) as f32;
    let fixing = Bond::Bonded(Seam {
        fixing: Some(super::super::Fixing {
            normal: slope,
            shear: slope,
        }),
        ..seam(tensile, 1e12)
    });
    let structure = structure(
        vec![block, cube(Vec3::new(0.0, -side / 2.0, 0.0), side, true)],
        vec![0, 1],
        vec![
            joint(
                0,
                Side::Chunk(1),
                Vec3::ZERO,
                Vec3::Y,
                rest_area,
                Bond::Resting,
            ),
            joint(0, Side::Chunk(1), Vec3::ZERO, Vec3::Y, fixing_area, fixing),
        ],
    );
    let uplift = weight + 1.5 * f64::from(tensile * fixing_area);
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    let failures = tick(
        &structure,
        &mut state,
        &mut assessment,
        &[press(
            0,
            Vec3::new(0.0, side / 2.0, 0.0),
            Vec3::Y * uplift as f32,
        )],
    );
    assert!(
        failures
            .iter()
            .any(|f| f.joint == 0 && f.cause == Cause::LiftForce),
        "the rest lifts: {failures:?}"
    );
    let crack = failures
        .iter()
        .find(|f| f.joint == 1)
        .expect("the fixing fails");
    assert_eq!(crack.cause, Cause::Tension);
    close(
        f64::from(crack.demand) * f64::from(fixing_area),
        uplift - weight,
        "the fixing's tension times its area",
    );
}

/// THE COULOMB CAPACITY BOUNDS THE TWIST. A slab rests on one fixture disk of radius R under its
/// centre; equal and opposite pushes make a pure yaw couple T. DERIVED: a uniform-pressure patch
/// holds T up to μ W ρ, ρ = √(trace Q) = R / √2 its polar radius of gyration. Below it the slab
/// holds; past it the rest slips, nothing else holds the twist, and the slab is released.
#[test]
fn a_yaw_couple_past_the_twist_capacity_slips_the_rest() {
    let (side, area, arm) = (0.4_f32, 0.04_f32, 0.15_f32);
    let structure = structure(
        vec![
            cube(Vec3::new(0.0, -side / 2.0, 0.0), side, true),
            cube(Vec3::new(0.0, side / 2.0, 0.0), side, false),
        ],
        vec![0, 1],
        vec![joint(
            1,
            Side::Chunk(0),
            Vec3::ZERO,
            Vec3::Y,
            area,
            Bond::Resting,
        )],
    );
    let weight = f64::from(structure.chunks[1].mass) * g();
    let mu = bondsolve::pair_friction(&structure, &structure.joints[0]);
    let radius = (f64::from(area) / std::f64::consts::PI).sqrt();
    let limit = mu * weight * radius / 2.0_f64.sqrt();
    let couple = |factor: f64| {
        let push = (factor * limit / (2.0 * f64::from(arm))) as f32;
        [
            press(1, Vec3::new(0.0, side / 2.0, arm), Vec3::X * push),
            press(1, Vec3::new(0.0, side / 2.0, -arm), Vec3::NEG_X * push),
        ]
    };
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &couple(0.98)).is_empty());
    assert!(assessment.slipping.is_empty(), "below μ W ρ the rest holds");
    let failures = tick(&structure, &mut state, &mut assessment, &couple(1.02));
    assert!(
        !failures.is_empty() && failures.iter().all(|f| f.cause == Cause::Slide),
        "{failures:?}"
    );
    assert_eq!(assessment.support, vec![vec![1]]);
}

/// A FACE'S FRICTION IS THE SUM OF ITS PATCHES' μᵢNᵢ. A slab rests off-centre on one buried part
/// through two equal patches of different pair friction. DERIVED: the patches bear
/// `Nᵢ = W/2 ∓ W x s / (2 s² + r²/2)` (the lintel's lever), and a push in the face's plane slips
/// the face past `μ₁N₁ + μ₂N₂`, not past the area-mean μ times W.
#[test]
fn a_mixed_friction_face_holds_the_sum_of_its_patches() {
    let (s, side, area, x) = (0.5_f32, 0.4_f32, 0.04_f32, 0.3_f32);
    let mut chunks = vec![
        cube(Vec3::new(-s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(s, -side / 2.0, 0.0), side, true),
        cube(Vec3::new(x, side / 2.0, 0.0), side, false),
    ];
    chunks[0].material.friction = 0.1;
    chunks[1].material.friction = 0.9;
    chunks[2].material.friction = 0.5;
    let bond = Bond::Bonded(seam(1e12, 1e12));
    let structure = structure(
        chunks,
        vec![0, 0, 1],
        vec![
            joint(
                0,
                Side::Chunk(1),
                Vec3::new(0.0, -side / 2.0, 0.0),
                Vec3::X,
                area,
                bond,
            ),
            joint(
                2,
                Side::Chunk(0),
                Vec3::new(-s, 0.0, 0.0),
                Vec3::Y,
                area,
                Bond::Resting,
            ),
            joint(
                2,
                Side::Chunk(1),
                Vec3::new(s, 0.0, 0.0),
                Vec3::Y,
                area,
                Bond::Resting,
            ),
        ],
    );
    let weight = f64::from(structure.chunks[2].mass) * g();
    let (s, x) = (f64::from(s), f64::from(x));
    let lever = weight * x * s / (2.0 * s * s + f64::from(area) / std::f64::consts::PI / 2.0);
    let mu = |j: usize| bondsolve::pair_friction(&structure, &structure.joints[j]);
    let limit = mu(1) * (weight / 2.0 - lever) + mu(2) * (weight / 2.0 + lever);
    let mean = (mu(1) + mu(2)) / 2.0 * weight;
    assert!(
        (limit - mean).abs() > 0.05 * limit,
        "the fixture separates the two laws"
    );
    let push = |factor: f64| {
        [press(
            2,
            Vec3::new(x as f32, 0.0, 0.0),
            Vec3::X * (factor * limit) as f32,
        )]
    };
    let mut state = State::new(&structure);
    let mut assessment = Assessment::default();
    assert!(tick(&structure, &mut state, &mut assessment, &push(0.98)).is_empty());
    assert!(
        assessment.slipping.is_empty(),
        "below Σ μᵢNᵢ the face holds"
    );
    let failures = tick(&structure, &mut state, &mut assessment, &push(1.02));
    assert!(
        !failures.is_empty() && failures.iter().all(|f| f.cause == Cause::Slide),
        "{failures:?}"
    );
    assert_eq!(assessment.support, vec![vec![2]]);
}

/// A fixture ligament of the game's environment destruction model, one element: chunk 0 bonded
/// to the ground, chunk 1 beside it on a vertical joint of tensile strength `tensile`, chunk 2
/// under chunk 1 on the ground. Removing chunk 2 leaves chunk 1 a cantilever off chunk 0.
fn ligament(tensile: f32) -> Structure {
    let side = 0.4_f32;
    let strong = Bond::Bonded(seam(1e12, 1e12));
    structure(
        vec![
            cube(Vec3::new(-side / 2.0, 0.0, 0.0), side, false),
            cube(Vec3::new(side / 2.0, 0.0, 0.0), side, false),
            cube(Vec3::new(side / 2.0, -side, 0.0), side, false),
        ],
        vec![0, 0, 0],
        vec![
            joint(
                0,
                Side::Ground,
                Vec3::new(-side / 2.0, -side / 2.0, 0.0),
                Vec3::Y,
                0.16,
                strong,
            ),
            joint(
                1,
                Side::Chunk(0),
                Vec3::ZERO,
                Vec3::X,
                0.04,
                Bond::Bonded(seam(tensile, 1e12)),
            ),
            joint(
                1,
                Side::Chunk(2),
                Vec3::new(side / 2.0, -side / 2.0, 0.0),
                Vec3::Y,
                0.16,
                strong,
            ),
            joint(
                2,
                Side::Ground,
                Vec3::new(side / 2.0, -1.5 * side, 0.0),
                Vec3::Y,
                0.16,
                strong,
            ),
        ],
    )
}

/// The DERIVED Navier tension at the ligament's disk (area 0.04 m², radius r) carrying the moment
/// M: 4 M / (r A).
fn navier(moment: f64) -> f64 {
    let area = 0.04;
    let radius = (area / std::f64::consts::PI).sqrt();
    4.0 * moment / (radius * area)
}

/// Remove chunk 2 of [`ligament`] in the tick `loads` act, after an intact first tick.
fn open_hole(structure: &Structure, loads: &[Load]) -> (Vec<Failure>, State, Assessment) {
    let mut state = State::new(structure);
    let mut assessment = Assessment::default();
    assert!(tick(structure, &mut state, &mut assessment, &[]).is_empty());
    let volume = structure.chunks[2].volume;
    assert!(super::super::wear(structure, &mut state, 2, volume));
    let failures = tick(structure, &mut state, &mut assessment, loads);
    (failures, state, assessment)
}

/// A HOLE LEAVES A LIGAMENT ITS OWN WEIGHT CRACKS. Removing the chunk under chunk 1 leaves it
/// hanging off chunk 0 inside one rigid part, so only the chunk-scale solve sees the ligament.
/// DERIVED: it carries the moment W d, d the lever of chunk 1's centre, so it holds while
/// f_t > 4 W d / (r A); below that it cracks, and the next tick the part law tips chunk 1 off
/// its cracked face and releases it.
#[test]
fn a_hole_leaves_a_ligament_its_own_weight_cracks() {
    let probe = ligament(1e12);
    let weight = f64::from(probe.chunks[1].mass) * g();
    let needed = navier(weight * 0.2);
    for (factor, cracks) in [(1.25, false), (0.8, true)] {
        let structure = ligament((factor * needed) as f32);
        let (failures, mut state, mut assessment) = open_hole(&structure, &[]);
        assert_eq!(
            failures
                .iter()
                .any(|f| f.joint == 1 && f.cause == Cause::Tension),
            cracks,
            "tensile strength {factor} × 4 W d / (r A): {failures:?}"
        );
        tick(&structure, &mut state, &mut assessment, &[]);
        let released: Vec<Vec<usize>> = if cracks { vec![vec![1]] } else { Vec::new() };
        assert_eq!(assessment.support, released);
    }
}

/// A PRESS BESIDE A HOLE LOADS THE LIGAMENT NEXT TO IT. A press P on the free end of the ligament
/// in the tick the hole opens: DERIVED, the ligament carries W d + P L, so a tensile strength
/// holding its weight cracks once P > P* = (f_t r A / 4 − W d) / L.
#[test]
fn a_press_beside_a_hole_loads_the_ligament_next_to_it() {
    let probe = ligament(1e12);
    let weight = f64::from(probe.chunks[1].mass) * g();
    let tensile = 2.0 * navier(weight * 0.2);
    let structure = ligament(tensile as f32);
    let limit = (tensile / navier(1.0) - weight * 0.2) / 0.4;
    for (factor, cracks) in [(0.8, false), (1.2, true)] {
        let push = press(
            1,
            Vec3::new(0.4, 0.0, 0.0),
            Vec3::NEG_Y * (factor * limit) as f32,
        );
        let (failures, _, _) = open_hole(&structure, &[push]);
        assert_eq!(
            failures
                .iter()
                .any(|f| f.joint == 1 && f.cause == Cause::Tension),
            cracks,
            "a press {factor} × P*: {failures:?}"
        );
    }
}

/// A fixture block pair hung on a sliver: chunk 0 buried, chunk 1 on it by a disk joint of
/// `sliver` m², chunk 2 beside chunk 1 on a 0.04 m² joint, each chunk its own element. Both
/// joints bond at a fixture strength nothing reaches.
fn hung(sliver: f32) -> Structure {
    let side = 0.4_f32;
    let bond = Bond::Bonded(seam(1e12, 1e12));
    structure(
        vec![
            cube(Vec3::new(0.0, -side / 2.0, 0.0), side, true),
            cube(Vec3::new(0.0, side / 2.0, 0.0), side, false),
            cube(Vec3::new(side, side / 2.0, 0.0), side, false),
        ],
        vec![0, 1, 2],
        vec![
            joint(1, Side::Chunk(0), Vec3::ZERO, Vec3::Y, sliver, bond),
            joint(
                2,
                Side::Chunk(1),
                Vec3::new(side / 2.0, side / 2.0, 0.0),
                Vec3::X,
                0.04,
                bond,
            ),
        ],
    )
}

/// A disk's rocking stiffness is `K_n A / 4π`, `K_n ∝ A` (bondsolve's joint law), so it scales
/// as `A²`. DERIVED: a 1e-8 m² sliver rocks 6e-14 as stiffly as the 0.04 m² joint beside it,
/// below the factor's 1e-9 mechanism resolution; a 1e-3 m² one rocks 6e-4 as stiffly, above it.
const SLIVERS: [(f32, bool); 2] = [(1e-8, true), (1e-3, false)];

/// A CHUNK SET HUNG ON A SLIVER IS A MECHANISM, AND THE SLIVER PARTS. The kernel of chunks 1 and 2
/// rocks freely on the sliver joint (DERIVED, [`SLIVERS`]): the joint its free motion moves, the
/// sliver, parts (`Slide`) in every event and the rest factors without it, the stiff joint
/// between the two chunks kept. Where the sliver holds, nothing parts. The crumble and the hole
/// solve both read this kernel; a mechanism is a game state, never a refusal.
#[test]
fn a_kernel_hung_on_a_sliver_parts_the_sliver() {
    for (sliver, loose) in SLIVERS {
        let structure = hung(sliver);
        let state = State::new(&structure);
        let kernel = Kernel::of(&structure, &state, &[1, 2]);
        let hold = kernel.hold(&structure, &[Wrench::default(); 2]);
        let parted: Vec<_> = hold
            .failures
            .iter()
            .filter(|f| f.cause == Cause::Slide)
            .map(|f| f.joint)
            .collect();
        let expected = if loose { vec![0] } else { vec![] };
        assert_eq!(
            parted, expected,
            "a {sliver} m² sliver: {:?}",
            hold.failures
        );
        assert_eq!(hold.separated.contains(&0), loose, "a {sliver} m² sliver");
        assert!(
            hold.carried.iter().any(|&(j, _)| j == 1),
            "the stiff joint stays an edge"
        );
    }
}

/// A PART HUNG ON A SLIVER BY NO SLIPPING FACE IS A MECHANISM, AND THE SLIVER PARTS. The part law
/// on the same fixture: parts 1 and 2 rock freely on the sliver, which no slipping rest made, so
/// the joint the free motion moves separates (`Slide`) and connectivity releases both parts.
#[test]
fn a_part_hung_on_a_sliver_parts_the_sliver_and_is_released() {
    for (sliver, loose) in SLIVERS {
        let structure = hung(sliver);
        let mut state = State::new(&structure);
        let mut assessment = Assessment::default();
        let failures = tick(&structure, &mut state, &mut assessment, &[]);
        let parted: Vec<_> = failures
            .iter()
            .filter(|f| f.cause == Cause::Slide)
            .map(|f| f.joint)
            .collect();
        let expected = if loose { vec![0] } else { vec![] };
        assert_eq!(parted, expected, "a {sliver} m² sliver: {failures:?}");
        assert_eq!(state.separated[0], loose, "a {sliver} m² sliver");
        assert!(!state.separated[1], "the stiff joint holds");
        let released = if loose { vec![vec![1, 2]] } else { vec![] };
        assert_eq!(assessment.support, released, "a {sliver} m² sliver");
    }
}

/// A SLIPPING REST WHOSE FRICTION TURNS A DIAGONAL NEGATIVE IS STILL A MECHANISM. A block bears on
/// the ground by two slipping rests: one on a steep face `n = (0.96, 0.28, 0)` slipping up it,
/// along `t = (−0.28, 0.96, 0)`, one on a vertical face across `z`. DERIVED: the steep rest's
/// translation stiffness is `K_n (n − μ t) nᵀ`, rank one, so the block is free along `t`; its
/// `y` diagonal `K_n n_y (n_y − μ t_y)` is negative for the pair friction `μ > 0.29`, so a pivot
/// judged against it would pass the vanishing `y` pivot. Judged against its elastic diagonal
/// `K_n n_y²`, the factor finds the mechanism, and its free motion runs along `t`.
#[test]
fn a_friction_lowered_diagonal_still_finds_the_mechanism() {
    let side = 0.4_f32;
    let steep = Vec3::new(0.96, 0.28, 0.0);
    let chunks = vec![cube(Vec3::new(0.0, side / 2.0, 0.0), side, false)];
    let joints = vec![
        joint(0, Side::Ground, Vec3::ZERO, steep, 0.04, Bond::Resting),
        joint(
            0,
            Side::Ground,
            Vec3::new(0.0, side / 2.0, -side / 2.0),
            Vec3::Z,
            0.04,
            Bond::Resting,
        ),
    ];
    let structure = structure(chunks, vec![0], joints);
    let mu = bondsolve::pair_friction(&structure, &structure.joints[0]);
    let up = DVec3::new(-0.28, 0.96, 0.0);
    assert!(mu * up.y > 0.28, "the fixture friction turns the diagonal");
    let slip = |force: DVec3| Slip {
        force,
        twist: DVec3::ZERO,
    };
    let slipping = BTreeMap::from([(0, slip(up)), (1, slip(DVec3::X))]);
    let graph = Graph::of(&structure, &State::new(&structure), &slipping);
    let mechanism = Factor::new(&graph.nodes, &graph.edges).expect_err("the block is free along t");
    let motion = mechanism.mode[0].linear;
    let normal = graph.edges[0].normal;
    assert!(
        motion.dot(normal).abs() <= 1e-9 * motion.length(),
        "the free motion runs along the steep rest: {motion} against {normal}"
    );
}

fn shipped(id: &str) -> crate::construction::ConstructionLibrary {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    crate::construction::ConstructionLibrary::load(&root, [id])
}

/// Run the game's law pass on `live` with `loads` each tick until it rests: a tick with no
/// failure, no release, no solve and no work, after which the owner asks for no tick. Asserts a
/// finite field; returns the ticks taken, the failures by cause and the released groups.
fn settle(
    element_of: &[u16],
    live: &mut crate::construction::breakage::Live,
    loads: impl Fn(&crate::construction::breakage::Live) -> Vec<Load>,
) -> (usize, BTreeMap<String, usize>, Vec<usize>) {
    let (mut causes, mut released) = (BTreeMap::new(), Vec::new());
    for t in 0..256 {
        let changes = crate::construction::breakage::run_laws(
            element_of,
            live,
            &loads(live),
            Vec::new(),
            Vec3::ZERO,
            STEP,
        );
        assert!(
            live.assessment
                .answer
                .iter()
                .all(|w| w.linear.is_finite() && w.angular.is_finite()),
            "tick {t}: a finite field"
        );
        for failure in &changes.failures {
            *causes.entry(format!("{:?}", failure.cause)).or_default() += 1;
        }
        released.extend(changes.groups.iter().map(|(group, _)| group.len()));
        if changes.failures.is_empty()
            && changes.groups.is_empty()
            && changes.solves == 0
            && live.assessment.work == 0
            && !live.assessment.needs_tick()
        {
            return (t, causes, released);
        }
    }
    panic!("the owner did not come to rest in 256 ticks: {causes:?}, groups {released:?}");
}

fn live_of(kind: &crate::construction::ConstructionKind) -> crate::construction::breakage::Live {
    let mut live = crate::construction::breakage::Live::from_structure(
        (*kind.structure).clone(),
        kind.layout.elements.len(),
    );
    live.assessment = kind.equilibrium.clone();
    live
}

/// THE UNIT HOUSE UNDER GRAVITY SETTLES AND THEN RESTS AT ZERO WORK. Its design's part graph is
/// factored once at load. Under gravity alone it settles in finitely many ticks with a finite
/// field, and once settled it solves nothing. Its census, its failures by cause and the groups
/// it releases are printed for the lane report: observations, never pins.
#[test]
fn the_unit_house_settles_under_gravity_and_rests_at_zero_work() {
    let library = shipped("house_1");
    let kind = library.get("house_1").expect("the library holds the house");
    let at_rest = &kind.equilibrium;
    let graph = &at_rest.graph;
    let bonded: usize = graph.interfaces.iter().map(|i| i.bonded.len()).sum();
    let push: usize = graph.interfaces.iter().map(|i| i.push.len()).sum();
    println!(
        "HOUSE parts={} fixed={} interfaces={} push_only={} ground={} bonded_joints={bonded} push_joints={push} unknowns={} factor_entries={} factor_ops={} solve_ops={} initial_failures={:?}",
        graph.parts.len(),
        graph.parts.iter().filter(|p| p.node.fixed).count(),
        graph.interfaces.len(),
        graph
            .interfaces
            .iter()
            .filter(|i| i.bonded.is_empty())
            .count(),
        graph.interfaces.iter().filter(|i| i.b.is_none()).count(),
        at_rest.factor.unknowns(),
        at_rest.factor.entries(),
        at_rest.factor.ops,
        at_rest.factor.solve_ops(),
        at_rest.initial_verdict,
    );
    let mut live = live_of(kind);
    let (ticks, causes, released) = settle(&kind.layout.element_of, &mut live, |_| Vec::new());
    println!("HOUSE GRAVITY ticks={ticks} failures={causes:?} released_group_sizes={released:?}");
    let assessment = &live.assessment;
    let graph = &assessment.graph;
    let n = graph.parts.len();
    // Each bonded joint's demand over its capacity in the settled sustained field.
    let (mut tension, mut shear) = ((0.0_f64, 0), (0.0_f64, 0));
    for interface in &graph.interfaces {
        for &e in &interface.bonded {
            let edge = &graph.edges[e];
            let joint = &kind.structure.joints[graph.joints[e]];
            let seam = joint.seam().expect("a bonded edge holds by a seam");
            let carried = edge.resultant(&graph.nodes, &assessment.answer[..n]);
            let (t, s) = edge.stress(carried, edge.normal);
            let compression = (-carried.linear.dot(edge.normal) / edge.area).max(0.0);
            let holds = f64::from(seam.shear.cohesion)
                + bondsolve::pair_friction(&kind.structure, joint) * compression;
            for (worst, ratio) in [
                (&mut tension, t / f64::from(seam.tensile)),
                (&mut shear, s / holds),
            ] {
                if ratio > worst.0 {
                    *worst = (ratio, graph.joints[e]);
                }
            }
        }
    }
    // Each rest by the lean of its normal: (count, slipping).
    let mut lean = BTreeMap::<&str, (usize, usize)>::new();
    for interface in &graph.interfaces {
        for &e in &interface.push {
            let y = graph.edges[e].normal.y.abs();
            let class = if y > 0.999 {
                "level"
            } else if y > 0.5 {
                "pitched"
            } else {
                "upright"
            };
            let entry = lean.entry(class).or_default();
            entry.0 += 1;
            entry.1 += usize::from(assessment.slipping.contains_key(&graph.joints[e]));
        }
    }
    println!("HOUSE RESTS by_lean(count, slipping)={lean:?}");
    let kernels = &kind.equilibrium.kernels;
    let largest = kernels
        .iter()
        .max_by_key(|k| k.factor.unknowns())
        .expect("the house has elements");
    println!(
        "HOUSE MARGINS slipping_joints={} worst_tension_utilisation={tension:?} worst_shear_utilisation={shear:?} kernels={} kernel_factor_ops_total={} largest_kernel chunks={} unknowns={} entries={} factor_ops={} solve_ops={}",
        assessment.slipping.len(),
        kernels.len(),
        kernels.iter().map(|k| k.factor.ops).sum::<u64>(),
        largest.chunks.len(),
        largest.factor.unknowns(),
        largest.factor.entries(),
        largest.factor.ops,
        largest.factor.solve_ops(),
    );
    for _ in 0..3 {
        let changes = crate::construction::breakage::run_laws(
            &kind.layout.element_of,
            &mut live,
            &[],
            Vec::new(),
            Vec3::ZERO,
            STEP,
        );
        assert!(changes.failures.is_empty() && changes.groups.is_empty());
        assert_eq!(
            (changes.solves, live.assessment.work),
            (0, 0),
            "an owner at rest does no work"
        );
        assert!(!live.assessment.needs_tick());
    }
}

/// A FIXTURE PRESS ON THE UNIT HOUSE ENDS ITS CASCADE WITH A FINITE FIELD. A 1 MN press pushes
/// the highest chunk of the first part standing on the ground horizontally, through the game's own
/// law pass (`run_laws`), which releases each unsupported group; [`settle`] asserts that it comes
/// to rest with a finite field. What fails is printed for the report: an observation, since every
/// house part is held by several interfaces, so its capacity has no closed form to bracket.
#[test]
fn a_fixture_press_on_the_house_comes_to_rest_with_a_finite_field() {
    let library = shipped("house_1");
    let kind = library.get("house_1").expect("the library holds the house");
    let graph = &kind.equilibrium.graph;
    let free = |p: usize| !graph.parts[p].node.fixed;
    let base = graph
        .interfaces
        .iter()
        .find_map(|i| match i.b {
            None => free(i.a).then_some(i.a),
            Some(b) => (free(i.a) != free(b)).then(|| if free(i.a) { i.a } else { b }),
        })
        .expect("the house stands parts on the ground");
    let height = |c: usize| kind.structure.chunks[c].centroid.y;
    let chunk = *graph.parts[base]
        .chunks
        .iter()
        .max_by(|&&a, &&b| height(a).total_cmp(&height(b)))
        .expect("a part holds chunks");
    let mut live = live_of(kind);
    let at = kind.structure.chunks[chunk].centroid;
    let (ticks, causes, released) = settle(&kind.layout.element_of, &mut live, |live| {
        [press(chunk, at, Vec3::X * 1.0e6)]
            .into_iter()
            .filter(|l| live.state.standing(l.chunk))
            .collect()
    });
    println!("HOUSE PRESS ticks={ticks} failures={causes:?} released_group_sizes={released:?}");
}

/// THE PLACED WORLD AT SPAWN: every construction the shipped map places, counted by design, with
/// each design's part graph and its compiled gravity verdict (observations for the lane report).
/// Invariant: each design's parts partition its chunks, and each free part carries six unknowns.
#[test]
fn the_shipped_map_census_at_spawn() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let manifest = crate::map::load(&root);
    let mut placed = BTreeMap::<&str, usize>::new();
    for instance in &manifest.instances {
        if let crate::map::Prototype::Construction { asset } =
            &manifest.prototypes[&instance.prototype]
        {
            *placed.entry(asset.as_str()).or_default() += 1;
        } // A block prototype is no construction.
    }
    let mut failing = 0;
    for (id, count) in &placed {
        let kind = manifest
            .constructions
            .get(id)
            .expect("the map's library holds every placed design");
        let at_rest = &kind.equilibrium;
        println!(
            "CENSUS {id} placed={count} parts={} interfaces={} unknowns={} factor_entries={} initial_failures={} released={}",
            at_rest.graph.parts.len(),
            at_rest.graph.interfaces.len(),
            at_rest.factor.unknowns(),
            at_rest.factor.entries(),
            at_rest.initial_verdict.len(),
            at_rest.support.len(),
        );
        failing += count * (at_rest.initial_verdict.len() + at_rest.support.len());
        let mut chunks: Vec<_> = at_rest
            .graph
            .parts
            .iter()
            .flat_map(|p| p.chunks.iter().copied())
            .collect();
        chunks.sort_unstable();
        assert!(chunks.iter().copied().eq(0..kind.structure.chunks.len()));
        assert_eq!(
            at_rest.factor.unknowns(),
            6 * at_rest.graph.nodes.iter().filter(|n| !n.fixed).count()
        );
    }
    println!("CENSUS placements={placed:?} joint_failures_at_spawn={failing}");
}
