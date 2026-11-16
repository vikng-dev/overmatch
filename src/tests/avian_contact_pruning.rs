//! The game's solver relies on bearing support surviving pruning, while its speculative stops
//! and contact reducer need separated candidates in the remaining slots. No world is needed:
//! these guards exercise Avian's public manifold API against generated contact mixes.

use avian3d::prelude::*;
use bevy::prelude::*;

fn prune(points: &[ContactPoint]) -> Vec<ContactPoint> {
    let mut manifold = ContactManifold::new(points.iter().copied(), Vec3::Y);
    if manifold.points.len() > 4 {
        manifold.prune_points();
    }
    manifold.points
}

/// Published Avian 0.8.0, `collision/contact_types/mod.rs::prune_points`: an independent
/// distance/area oracle, including first-wins ties and polygon order, for all-bearing compatibility.
fn upstream(points: &[ContactPoint]) -> Vec<ContactPoint> {
    if points.len() <= 4 {
        return points.to_vec();
    }
    let projected: Vec<_> = points
        .iter()
        .map(|p| p.anchor1.reject_from_normalized(Vec3::Y))
        .collect();
    // Resolution: upstream's squared 1 mm floor prevents either zero factor erasing the other.
    let floor = 1e-6;
    let weight = |i: usize| (points[i].penetration * points[i].penetration).max(floor);
    let first_max = |score: &dyn Fn(usize) -> f32, excluded: Option<usize>| {
        (0..points.len())
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
    for (i, p) in projected.iter().enumerate() {
        if i == a || i == b {
            continue;
        }
        let area = perp.dot(*p - projected[a]);
        if area < min {
            (low, min) = (Some(i), area);
        } else if area > max {
            (high, max) = (Some(i), area);
        }
    }
    [Some(a), low, Some(b), high]
        .into_iter()
        .flatten()
        .map(|i| points[i])
        .collect()
}

/// The solver must never trade bearing support for speculation, and must retain speculative
/// stops in free slots. All-bearing callers retain upstream's exact selection. The contact
/// reducer reads normal_speed after pruning, so extra separation must never improve admission.
#[test]
fn bearing_priority_capacity_upstream_compatibility_and_separation_monotonicity() {
    // Fixture: a square with edge midpoints, plus rotated input orders; every bearing assignment
    // for 1..=8 points, at touching, sub-floor and above-floor depths, uniform and varied.
    let anchors = [
        (-1.0, -1.0),
        (1.0, -1.0),
        (1.0, 1.0),
        (-1.0, 1.0),
        (0.0, 1.0),
        (0.0, -1.0),
        (-1.0, 0.0),
        (1.0, 0.0),
    ];
    for n in 1..=anchors.len() {
        for rotation in 0..n {
            for mask in 0_u32..1 << n {
                for depth in [0.0, 0.0005, 0.003] {
                    let points: Vec<_> = (0..n)
                        .map(|i| {
                            let (x, z) = anchors[(i + rotation) % anchors.len()];
                            let at = Vec3::new(x, 0.0, z);
                            // Fixture: varying signed depths exercise both weighted selections.
                            let penetration = if mask & (1 << i) != 0 {
                                depth * (i + 1) as f32
                            } else {
                                -0.001 * (i + 1) as f32
                            };
                            ContactPoint::new(at, at, at, penetration)
                        })
                        .collect();
                    let kept = prune(&points);
                    let bearing = mask.count_ones() as usize;
                    if bearing == n {
                        assert_eq!(
                            kept,
                            upstream(&points),
                            "all-bearing upstream compatibility"
                        );
                    } else {
                        assert_eq!(
                            kept.iter().filter(|p| p.penetration >= 0.0).count(),
                            bearing.min(4),
                            "solver bearing support lost: n={n}, mask={mask}, rotation={rotation}"
                        );
                        assert_eq!(
                            kept.len(),
                            n.min(4),
                            "speculative stops lost from free slots"
                        );
                    }
                    for (i, point) in points.iter().enumerate() {
                        if point.penetration >= 0.0 {
                            continue;
                        }
                        let was_kept = kept.iter().any(|p| p.anchor1 == point.anchor1);
                        // Fixture: increase only this point's separation, across and above the floor.
                        for separation in [0.012, 0.04, 0.1] {
                            let mut farther = points.clone();
                            farther[i].penetration -= separation;
                            let now_kept =
                                prune(&farther).iter().any(|p| p.anchor1 == point.anchor1);
                            assert!(
                                was_kept || !now_kept,
                                "extra separation improved admission: n={n}, mask={mask}, point={i}"
                            );
                        }
                    }
                }
            }
        }
    }
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
        let reference = upstream(&points);
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
