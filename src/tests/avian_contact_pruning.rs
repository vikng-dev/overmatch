//! The game's solver relies on bearing support surviving pruning; its speculative stops and
//! contact reducer need separated candidates in free slots. Generated manifolds exercise Avian's
//! public API without a world or historical physics targets.

use avian3d::prelude::*;
use bevy::prelude::*;

fn prune(points: &[ContactPoint]) -> Vec<ContactPoint> {
    let mut manifold = ContactManifold::new(points.iter().copied(), Vec3::Y);
    if manifold.points.len() > 4 {
        manifold.prune_points();
    }
    manifold.points
}

/// Published Avian 0.8.0's independent distance/area oracle, with first-wins ties.
/// Selection order exposes its picks over a subset; otherwise it returns polygon order.
fn upstream(points: &[ContactPoint], indices: &[usize], selection_order: bool) -> Vec<usize> {
    if !selection_order && indices.len() <= 4 {
        return indices.to_vec();
    }
    assert!(indices.len() >= 2, "a selection needs a segment");
    let projected: Vec<_> = points
        .iter()
        .map(|p| p.anchor1.reject_from_normalized(Vec3::Y))
        .collect();
    // Resolution: upstream's squared 1 mm floor prevents either zero factor erasing the other.
    let floor = 1e-6;
    let weight = |i: usize| (points[i].penetration * points[i].penetration).max(floor);
    let first_max = |score: &dyn Fn(usize) -> f32, excluded: Option<usize>| {
        indices
            .iter()
            .copied()
            .filter(|i| Some(*i) != excluded)
            .fold((usize::MAX, f32::MIN), |best, i| {
                let value = score(i);
                if value > best.1 { (i, value) } else { best }
            })
            .0
    };
    let a = first_max(
        &|i| projected[i].length_squared().max(floor) * weight(i),
        None,
    );
    let b = first_max(
        &|i| projected[i].distance_squared(projected[a]).max(floor) * weight(i),
        Some(a),
    );
    let perp = (projected[b] - projected[a]).cross(Vec3::Y);
    let (mut low, mut high) = (None, None);
    let (mut min, mut max) = (0.0, 0.0);
    for &i in indices {
        if i == a || i == b {
            continue;
        }
        let area = perp.dot(projected[i] - projected[a]);
        if area < min {
            (low, min) = (Some(i), area);
        } else if area > max {
            (high, max) = (Some(i), area);
        }
    }
    if selection_order {
        [Some(a), Some(b), low, high]
    } else {
        [Some(a), low, Some(b), high]
    }
    .into_iter()
    .flatten()
    .collect()
}

fn zero_depth(points: &[ContactPoint]) -> Vec<ContactPoint> {
    points
        .iter()
        .copied()
        .map(|mut p| {
            p.penetration = 0.0;
            p
        })
        .collect()
}

fn anchors(points: &[ContactPoint]) -> Vec<Vec3> {
    points.iter().map(|p| p.anchor1).collect()
}

fn generated(mut check: impl FnMut(&[ContactPoint])) {
    // Fixture: a square with edge midpoints and a convex pentagon. Rotations and
    // translations vary first-wins ties and distance from the center of mass; every sign mask
    // covers touching, sub-floor and above-floor depths, with varied weights within each tier.
    let square = [
        (-1.0, -1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (-1.0, 0.0),
        (1.0, 0.0),
    ];
    let pentagon = [
        (-1.0, -1.0),
        (1.0, -1.0),
        (2.0, 0.0),
        (1.0, 1.0),
        (-1.0, 1.0),
    ];
    for shape in [&square[..], &pentagon[..]] {
        for n in 1..=shape.len() {
            for rotation in 0..n {
                for mask in 0_u32..1 << n {
                    for depth in [0.0, 0.0005, 0.003] {
                        let points: Vec<_> = (0..n)
                            .map(|i| {
                                let (x, z) = shape[(i + rotation) % shape.len()];
                                let at = Vec3::new(x + rotation as f32 * 0.25, 0.0, z);
                                let penetration = if mask & (1 << i) != 0 {
                                    depth * (i + 1) as f32
                                } else {
                                    -0.001 * (i + 1) as f32
                                };
                                ContactPoint::new(at, at, at, penetration)
                            })
                            .collect();
                        check(&points);
                    }
                }
            }
        }
    }
}

/// R1: the solver retains upstream's complete all-bearing selection, including polygon order.
#[test]
fn r1_all_bearing_equals_upstream_in_order() {
    generated(|points| {
        if points.iter().all(|p| p.penetration >= 0.0) {
            let expected: Vec<_> = upstream(points, &(0..points.len()).collect::<Vec<_>>(), false)
                .into_iter()
                .map(|i| points[i])
                .collect();
            assert_eq!(
                prune(points),
                expected,
                "R1: all-bearing upstream compatibility"
            );
        }
    });
}

/// R2: speculative stops use the touching manifold's anchors and order; the solver must not
/// prefer a point because it is separated farther.
#[test]
fn r2_all_separated_equals_zero_depth_anchors_in_order() {
    generated(|points| {
        if points.iter().all(|p| p.penetration < 0.0) {
            assert_eq!(
                anchors(&prune(points)),
                anchors(&prune(&zero_depth(points))),
                "R2: separation must have zero depth weight"
            );
        }
    });
}

/// R3: the solver must never trade available bearing support for speculative candidates.
#[test]
fn r3_mixed_keeps_bearing_count_up_to_capacity() {
    generated(|points| {
        let bearing = points.iter().filter(|p| p.penetration >= 0.0).count();
        if bearing > 0 && bearing < points.len() {
            assert_eq!(
                prune(points)
                    .iter()
                    .filter(|p| p.penetration >= 0.0)
                    .count(),
                bearing.min(4),
                "R3: bearing support takes priority"
            );
        }
    });
}

/// R4: mixed manifolds fill four slots, so the solver and contact reducer retain speculative
/// stops (and their normal_speed) wherever bearing support leaves room.
#[test]
fn r4_mixed_over_capacity_fills_four_slots() {
    generated(|points| {
        if points.len() > 4
            && points.iter().any(|p| p.penetration >= 0.0)
            && points.iter().any(|p| p.penetration < 0.0)
        {
            assert_eq!(prune(points).len(), 4, "R4: mixed capacity");
        }
    });
}

/// R5: excess bearing support excludes speculation and retains every upstream subset pick;
/// the solver relies on the same distance/area support even when the oracle leaves empty sides.
#[test]
fn r5_mixed_excess_bearing_keeps_every_bearing_oracle_pick() {
    generated(|points| {
        let bearing: Vec<_> = (0..points.len())
            .filter(|&i| points[i].penetration >= 0.0)
            .collect();
        if bearing.len() > 4 && bearing.len() < points.len() {
            let kept = prune(points);
            assert!(
                kept.iter().all(|p| p.penetration >= 0.0),
                "R5: no speculative displacement"
            );
            assert!(
                upstream(points, &bearing, true)
                    .into_iter()
                    .all(|i| kept.contains(&points[i])),
                "R5: every bearing oracle pick survives"
            );
        }
    });
}

/// R6: competing speculative stops use upstream's selection order in the free slots;
/// the solver needs distance/area coverage of future support, independent of input order.
#[test]
fn r6_mixed_speculation_uses_selection_order_in_free_slots() {
    generated(|points| {
        let bearing = points.iter().filter(|p| p.penetration >= 0.0).count();
        let separated: Vec<_> = (0..points.len())
            .filter(|&i| points[i].penetration < 0.0)
            .collect();
        if bearing > 0 && bearing < 4 && separated.len() > 4 - bearing {
            let expected: Vec<_> = upstream(&zero_depth(points), &separated, true)
                .into_iter()
                .take(4 - bearing)
                .map(|i| points[i])
                .collect();
            let kept: Vec<_> = prune(points)
                .into_iter()
                .filter(|p| p.penetration < 0.0)
                .collect();
            assert_eq!(kept, expected, "R6: speculative selection order");
        }
    });
}

/// R7: the solver must not reward separation: moving a separated point farther away cannot
/// promote it from dropped to kept. This is independent of the reducer's use of normal_speed.
#[test]
fn r7_more_separation_never_promotes_a_dropped_point() {
    generated(|points| {
        let kept = anchors(&prune(points));
        for (i, p) in points
            .iter()
            .enumerate()
            .filter(|(_, p)| p.penetration < 0.0)
        {
            // Fixture: changes above the depth floor, each applied to this point alone.
            for extra in [0.012, 0.04, 0.1] {
                let mut farther = points.to_vec();
                farther[i].penetration -= extra;
                assert!(
                    kept.contains(&p.anchor1) || !anchors(&prune(&farther)).contains(&p.anchor1),
                    "R7: separation rewarded point {i} in {points:?}"
                );
            }
        }
    });
}

/// Upstream's all-bearing selection need not fill four slots: collinear points have zero signed
/// areas, and a depth-weighted first segment on a convex polygon's edge has points on one side
/// only. Exact upstream compatibility therefore conflicts with unconditional capacity.
#[test]
fn upstream_can_keep_fewer_than_four_bearing_points() {
    // Fixture: five collinear anchors, and a convex pentagon with depth linear in z.
    let collinear: Vec<_> = (-2..=2)
        .map(|x| {
            let at = Vec3::X * x as f32;
            ContactPoint::new(at, at, at, 0.001)
        })
        .collect();
    let pentagon: Vec<_> = [
        (-1.0, -1.0),
        (1.0, -1.0),
        (2.0, 0.0),
        (1.0, 1.0),
        (-1.0, 1.0),
    ]
    .into_iter()
    .map(|(x, z)| {
        let at = Vec3::new(x, 0.0, z);
        ContactPoint::new(at, at, at, 0.001 + 0.1 * (1.0 - z))
    })
    .collect();
    for points in [collinear, pentagon] {
        let reference: Vec<_> = upstream(&points, &(0..points.len()).collect::<Vec<_>>(), false)
            .into_iter()
            .map(|i| points[i])
            .collect();
        assert!(
            reference.len() < 4,
            "upstream has fewer than two nonzero area extremes"
        );
        assert_eq!(
            prune(&points),
            reference,
            "all-bearing upstream compatibility"
        );
    }
}
