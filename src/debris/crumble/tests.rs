//! The crumble's invariants, in the game's environment destruction model (game state only), on
//! the shipped house's data. Which chunks a test reads is chosen by a class rule (a breakable
//! element's chunks, or every chunk ordered by mass), never by number.

use super::*;
use crate::construction::tests::shipped_house;

/// The shipped house's law state, and the chunks of its breakable element with the most internal
/// seams: a released part's fixture (its element's own chunks, class rule).
fn released_part() -> (Structure, State, Vec<usize>) {
    let breakage = Breakage::new(shipped_house(), 1);
    let structure = (*breakage.live.structure).clone();
    let layout = &breakage.kind.layout;
    let element = (0..layout.elements.len())
        .max_by_key(|&e| {
            structure
                .joints
                .iter()
                .filter(|joint| {
                    matches!(joint.b, Side::Chunk(b)
                        if layout.element_of[joint.a] as usize == e
                            && layout.element_of[b] as usize == e)
                })
                .count()
        })
        .expect("the house has elements");
    let group: Vec<usize> = (0..structure.chunks.len())
        .filter(|&c| layout.element_of[c] as usize == element && !structure.chunks[c].buried)
        .collect();
    let mut state = breakage.live.state.clone();
    state.retain_group(&group);
    (structure, state, group)
}

/// Every internal seam of `group` with its opening work, cheapest first.
fn seams(structure: &Structure, state: &State, group: &[usize]) -> Vec<(f32, usize)> {
    let mut seams: Vec<_> = (0..structure.joints.len())
        .filter(|&j| match structure.joints[j].b {
            Side::Chunk(b) => group.contains(&structure.joints[j].a) && group.contains(&b),
            Side::Ground => false,
        })
        .map(|j| (opening_work(structure, state, j), j))
        .collect();
    seams.sort_by(|a, b| a.0.total_cmp(&b.0));
    seams
}

/// THE ENERGY ACCOUNT: an impact spends at most its energy, on exactly the seams it opens, the
/// cheapest first, and stops only at a seam the rest cannot pay; its fragments partition the
/// group. A mutant that overspends, skips the order, or keeps going past an unaffordable seam
/// breaks it; the size split and every player's decoration rely on these fragments.
#[test]
fn an_impact_spends_at_most_its_energy_on_the_cheapest_seams() {
    let (structure, state, group) = released_part();
    let seams = seams(&structure, &state, &group);
    let total: f32 = seams.iter().map(|(work, _)| work).sum();
    assert!(total > 0.0, "the fixture part has seams to open");
    for share in [0.0, 1e-3, 0.1, 0.5, 0.999, 2.0] {
        let energy = total * share;
        let result = crumble(&structure, &state, &group, energy);
        assert!(
            result.spent <= energy,
            "{share}: spent {} of {energy}",
            result.spent
        );
        let opened: f32 = result
            .broken
            .iter()
            .map(|&j| opening_work(&structure, &state, j))
            .sum();
        assert!((opened - result.spent).abs() <= 1e-4 * total.max(1.0));
        let dearest_opened = result
            .broken
            .iter()
            .map(|&j| opening_work(&structure, &state, j))
            .fold(0.0, f32::max);
        for &(work, j) in &seams {
            if !result.broken.contains(&j) {
                assert!(
                    work >= dearest_opened,
                    "{share}: a cheaper seam stayed shut"
                );
                assert!(
                    result.spent + work > energy,
                    "{share}: an affordable seam stayed shut"
                );
            }
        }
        let mut chunks: Vec<usize> = result.fragments.concat();
        chunks.sort_unstable();
        assert_eq!(chunks, group, "{share}: the fragments partition the group");
    }
}

/// A HIGHER DROP NEVER MAKES FEWER FRAGMENTS, AND A HIGH ENOUGH ONE MAKES MORE: the energy of a
/// drop of height `h` is `m g h` (DERIVED), and the crumble opens a superset of seams for more of
/// it. Heights are fixture data chosen as shares of the part's total seam work.
#[test]
fn a_higher_drop_makes_at_least_as_many_fragments() {
    let (structure, state, group) = released_part();
    let total: f32 = seams(&structure, &state, &group)
        .iter()
        .map(|(work, _)| work)
        .sum();
    let mass: f32 = group.iter().map(|&c| structure.chunks[c].mass).sum();
    let weight = mass * crate::physical_laws::gravity_mps2();
    let mut last = (0, Vec::new());
    for share in [0.0, 0.05, 0.3, 1.0] {
        let height = total * share / weight;
        let result = crumble(&structure, &state, &group, weight * height);
        assert!(result.fragments.len() >= last.0, "drop {height} m");
        assert!(last.1.iter().all(|j| result.broken.contains(j)));
        last = (result.fragments.len(), result.broken);
    }
    assert_eq!(
        last.0,
        group.len(),
        "a drop paying every seam frees every chunk"
    );
    assert!(crumble(&structure, &state, &group, 0.0).fragments.len() < group.len());
}

/// A rest and an opened seam hold by no cohesion: they part for nothing, so any impact separates
/// what only rests together. A mutant charging a rest its neighbour's seam work keeps a stack
/// whole.
#[test]
fn rests_and_opened_seams_part_for_nothing() {
    let (structure, mut state, group) = released_part();
    let (_, j) = seams(&structure, &state, &group)[0];
    state.cracked[j] = true;
    assert_eq!(opening_work(&structure, &state, j), 0.0);
    let result = crumble(&structure, &state, &group, 0.0);
    assert!(result.broken.contains(&j));
    assert_eq!(result.spent, 0.0);
}

/// ROUTING: a fragment stays a server body exactly when its mass reaches the cut-off, read from its
/// owner ([`PhysicalLaws::server_fragment_kg`]); single chunks and growing runs of them, by mass,
/// straddle it. A mutant that compares volume, counts chunks or flips the comparison breaks it.
#[test]
fn fragments_at_the_cut_off_stay_server_bodies_and_lighter_ones_shed() {
    let (structure, _, _) = released_part();
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
    let (structure, _, group) = released_part();
    let frame = Transform::from_xyz(3.0, 2.0, -1.0).with_rotation(Quat::from_rotation_y(0.4));
    let (velocity, spin, centre) = (
        Vec3::new(1.0, -2.0, 0.5),
        Vec3::new(0.0, 3.0, 1.0),
        Vec3::ZERO,
    );
    let fragment = &group[..1];
    let shed = Shed::of(
        &structure,
        (5, frame),
        fragment,
        (velocity, spin, centre),
        9,
    );
    let at = frame.transform_point(structure.chunks[fragment[0]].centroid);
    let expected = velocity + spin.cross(at - centre);
    assert!((Vec3::from_array(shed.velocity) - expected).length() < 1e-4);
    assert_eq!(shed.spin, spin.to_array());
    assert_eq!((shed.construction, shed.tick), (5, 9));
    assert_eq!(shed.chunks, vec![fragment[0] as u16]);
}
