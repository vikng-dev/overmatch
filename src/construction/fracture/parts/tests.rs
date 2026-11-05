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

/// A PART RESTING ON TWO OTHERS BEARS HALF ITS WEIGHT ON EACH, AND SLIDES PAST FRICTION. A slab
/// rests (no bond) on two buried piers at x = ±s. DERIVED: each rest bears W / 2; a push P in the
/// rests' plane loads each with P / 2 of shear, so both slide once P > μ W, μ the pair friction.
#[test]
fn a_part_resting_on_two_others_bears_and_slides_at_friction() {
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

fn shipped(id: &str) -> crate::construction::ConstructionLibrary {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    crate::construction::ConstructionLibrary::load(&root, [id])
}

/// Run the game's law pass on `live` with `loads` each tick until eight quiet ticks, asserting
/// a finite field; returns the ticks taken, the failures by cause and the released groups.
fn settle(
    kind: &crate::construction::ConstructionKind,
    live: &mut crate::construction::breakage::Live,
    loads: impl Fn(&crate::construction::breakage::Live) -> Vec<Load>,
) -> (usize, BTreeMap<String, usize>, Vec<usize>) {
    let (mut causes, mut released, mut quiet) = (BTreeMap::new(), Vec::new(), 0);
    for t in 0..256 {
        let changes = crate::construction::breakage::run_laws(
            &kind.layout.element_of,
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
        quiet = if changes.failures.is_empty() && changes.groups.is_empty() {
            quiet + 1
        } else {
            0
        };
        if quiet == 8 {
            return (t, causes, released);
        }
    }
    panic!("the cascade did not end in 256 ticks: {causes:?}, groups {released:?}");
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
    let (ticks, causes, released) = settle(kind, &mut live, |_| Vec::new());
    println!("HOUSE GRAVITY ticks={ticks} failures={causes:?} released_group_sizes={released:?}");
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

/// A PRESS PAST CAPACITY ON THE UNIT HOUSE: parts fail as the data says, every value stays
/// finite, and the cascade ends. A fixture 1 MN press pushes the highest chunk of the first part
/// standing on the ground horizontally, through the game's own law pass (`run_laws`), which
/// releases each unsupported group.
#[test]
fn a_press_past_capacity_fails_house_parts_without_a_stall() {
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
    let (ticks, causes, released) = settle(kind, &mut live, |live| {
        [press(chunk, at, Vec3::X * 1.0e6)]
            .into_iter()
            .filter(|l| live.state.standing(l.chunk))
            .collect()
    });
    println!("HOUSE PRESS ticks={ticks} failures={causes:?} released_group_sizes={released:?}");
    assert!(
        !causes.is_empty(),
        "a fixture 1 MN press exceeds a part's capacity"
    );
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
