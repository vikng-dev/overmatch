//! The game's damage model: where a mounted round is born, the first open point of its bore.

use avian3d::prelude::Collider;
use bevy::prelude::*;

/// The game's damage model: the first point of the bore, walked forward from its mount, that lies
/// outside every shape of the source's physics envelope. `mount` is the authored bore start,
/// measured along the bore from the muzzle: zero or negative. Firing supplies the negative of
/// the load-validated bore length, so recoil cannot move the muzzle behind that start. A bore that
/// leaves the envelope behind its muzzle (a barrel through a turret
/// face) is born at that face, one that the envelope still holds at the muzzle is born at the
/// envelope's forward boundary, and one whose mount stands outside every shape is born at the
/// mount. The ordinary march owns everything from this point on, including any barrel length still
/// ahead of it.
///
/// Envelope shapes beyond the first open point (a hull front below a depressed gun, a second proxy
/// past a gap) do not move the birth: a foreign wall in that gap is the march's to meet.
///
/// Invariant: every shape is convex, so the bore line meets each in one interval. The envelope is
/// the tank's collision proxies (`tank::spawn::spawn_proxy_parts`, convex hulls of the bake's loose
/// parts) and track bump stops (`track::bump_stop`, convex prisms).
pub(crate) fn outside_source(
    muzzle: Vec3,
    bore: Dir3,
    mount: f32,
    shapes: &[(Vec3, Quat, &Collider)],
) -> Vec3 {
    assert!(
        mount <= 0.0,
        "a weapon's mount lies on or behind its muzzle along the bore: {mount}"
    );
    let spans: Vec<_> = shapes
        .iter()
        .filter_map(|&(at, turn, shape)| {
            // The game's damage model reads both ends on one local ray. World-space rays formed
            // independently ahead and behind the muzzle can straddle a grazing face by rounding.
            let inverse = turn.inverse();
            crate::ballistics::convex_interval(shape, inverse * (muzzle - at), inverse * *bore)
                .expect("source physics proxies are cuboids or closed polyhedral convex hulls")
        })
        .collect();
    let mut open = f64::from(mount);
    while let Some(exit) = spans
        .iter()
        .filter(|&&(enter, exit)| enter <= open && open < exit)
        .map(|&(_, exit)| exit)
        .max_by(f64::total_cmp)
    {
        open = exit;
    }
    if open == f64::from(mount) {
        muzzle + bore * mount
    } else {
        muzzle + bore * (open as f32 + crate::ballistics::MARCH_EPS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f32 = crate::ballistics::MARCH_EPS;

    /// Unit boxes along `-Z` (the bore), centred at `z`, half-depth `half`. Fixture geometry: the
    /// rule is a class invariant of convex envelopes, not of one tank.
    fn boxes(slabs: &[(f32, f32)]) -> Vec<Collider> {
        slabs
            .iter()
            .map(|&(_, half)| Collider::cuboid(2.0, 2.0, half * 2.0))
            .collect()
    }

    fn born(muzzle: Vec3, mount: f32, slabs: &[(f32, f32)]) -> Vec3 {
        let colliders = boxes(slabs);
        let shapes: Vec<_> = slabs
            .iter()
            .zip(&colliders)
            .map(|(&(z, _), collider)| (Vec3::Z * z, Quat::IDENTITY, collider))
            .collect();
        outside_source(muzzle, Dir3::NEG_Z, mount, &shapes)
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1.0e-5
    }

    /// A barrel through a turret face is born at that face, behind its muzzle: the march must meet
    /// a wall the barrel pokes through. A muzzle the envelope still holds is born at the
    /// envelope's forward boundary. Both read the same contiguous walk from the mount.
    #[test]
    fn a_bore_is_born_where_it_leaves_the_envelope_it_starts_in() {
        let muzzle = Vec3::NEG_Z * 5.0;
        let face = born(muzzle, -5.0, &[(0.0, 1.0)]);
        assert!(near(face, Vec3::NEG_Z * (1.0 + EPS)), "{face:?}");
        let held = born(Vec3::NEG_Z * 0.5, -0.5, &[(0.0, 1.0)]);
        assert!(near(held, Vec3::NEG_Z * (1.0 + EPS)), "{held:?}");
    }

    /// Envelope pieces that overlap are one run; a piece past a gap ahead of the first open point
    /// is not. A wall in that gap would otherwise be skipped by a birth beyond the far piece.
    #[test]
    fn a_gap_in_the_envelope_ends_the_walk_at_its_first_open_point() {
        let muzzle = Vec3::NEG_Z * 0.5;
        let joined = born(muzzle, -0.5, &[(0.0, 1.0), (-1.5, 1.0)]);
        assert!(near(joined, Vec3::NEG_Z * (2.5 + EPS)), "{joined:?}");
        let gapped = born(muzzle, -0.5, &[(0.0, 1.0), (-4.0, 1.0)]);
        assert!(near(gapped, Vec3::NEG_Z * (1.0 + EPS)), "{gapped:?}");
    }

    /// The game's damage model: finite bores grazing a convex proxy cannot panic while finding
    /// their birth. Fixture geometry sweeps hull edges within world-coordinate rounding.
    #[test]
    fn grazing_a_source_proxy_never_loses_one_end_of_its_interval() {
        let points = [-1.0, 1.0]
            .into_iter()
            .flat_map(|x| {
                [-1.0, 1.0]
                    .into_iter()
                    .flat_map(move |y| [-1.0, 1.0].into_iter().map(move |z| Vec3::new(x, y, z)))
            })
            .collect();
        let hull = Collider::convex_hull(points).expect("fixture cube has volume");
        for at in [Vec3::ZERO, Vec3::new(1600.0, 20.0, -1200.0)] {
            for step in 0..64 {
                let angle = step as f32 * 0.097;
                let turn = Quat::from_euler(EulerRot::YXZ, angle, angle * 0.37, angle * 0.73);
                let bore = Dir3::new(turn * Vec3::NEG_Z).unwrap();
                for ulp in -32..=32 {
                    let edge = f32::from_bits(1.0_f32.to_bits().wrapping_add_signed(ulp));
                    for local in [Vec3::new(edge, 0.0, -3.0), Vec3::new(edge, 1.0, -3.0)] {
                        outside_source(at + turn * local, bore, -6.0, &[(at, turn, &hull)]);
                    }
                }
            }
        }
    }

    /// A mount outside every shape is born at the mount: a bore line that meets no shape, one that
    /// meets a shape only behind the mount, and an empty envelope all have the same answer, and
    /// none of them panics.
    #[test]
    fn a_mount_outside_the_envelope_is_born_at_the_mount() {
        let muzzle = Vec3::new(0.0, 3.0, -2.0);
        let mount = muzzle - Vec3::NEG_Z * 0.4;
        assert_eq!(
            born(muzzle, -0.4, &[(0.0, 1.0)]),
            mount,
            "clear above the box"
        );
        let behind = Vec3::NEG_Z * 4.0;
        assert_eq!(born(behind, -0.4, &[(0.0, 1.0)]), behind + Vec3::Z * 0.4);
        assert_eq!(born(muzzle, -0.4, &[]), mount, "no envelope at all");
    }
}
