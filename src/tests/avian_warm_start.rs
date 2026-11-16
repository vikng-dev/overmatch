//! Guards for the in-engine warm-start fixes (`vendor/avian3d-0.8.0-overmatch`,
//! `OVERMATCH_PATCH.md`, "Warm-start matching"). Game
//! state only: the server's contacts between bodies. The server relies on warm starts following the
//! closest admissible contact, within a manifold and across the pair. Drafts:
//! `upstream/avian-unknown-contact-feature-identity.md` and
//! `upstream/avian-nearest-warm-start-contact.md`.

use avian3d::collision::contact_types::PackedFeatureId;
use avian3d::prelude::*;
use bevy::prelude::*;

/// UNKNOWN feature ids carry no identity; only nearby anchors may match them, and the closest wins.
/// Upstream's matcher reads two UNKNOWN ids as one identity and takes the first previous point.
#[test]
fn unknown_contact_features_match_by_position() {
    // Fixture: two distinct points, both outside and inside a 0.1 m matching radius.
    for separation in [1.0, 0.05] {
        let at = Vec3::X * separation;
        let mut previous = [Vec3::ZERO, at].map(|at| ContactPoint::new(at, at, at, 0.0));
        previous[0].warm_start_normal_impulse = 5.0;
        previous[1].warm_start_normal_impulse = 11.0;
        let mut next = ContactManifold::new([ContactPoint::new(at, at, at, 0.0)], Vec3::Y);
        next.match_contacts(&previous, 0.1);
        assert_eq!(
            next.points[0].warm_start_normal_impulse, 11.0,
            "UNKNOWN IDs must select the closest admissible anchors, separation {separation}"
        );
    }
}

/// A point with no admissible predecessor starts cold: an UNKNOWN point whose only predecessor lies
/// beyond the matching radius, and a known point whose only predecessor holds other feature ids,
/// its anchors coincident in either orientation. Fails if UNKNOWN ids admit by identity (upstream's
/// first point at any distance), or known ids admit by distance in either orientation.
#[test]
fn a_point_without_an_admissible_predecessor_starts_cold() {
    // Fixture: two distinct anchors, 1 m apart, against a 0.1 m matching radius.
    let (near, far) = (Vec3::ZERO, Vec3::Y);
    let point = |anchor1: Vec3, anchor2: Vec3, ids: Option<[u32; 2]>| {
        let mut point = ContactPoint::new(anchor1, anchor2, anchor1, 0.0);
        if let Some([id1, id2]) = ids {
            point.feature_id1 = PackedFeatureId::face(id1);
            point.feature_id2 = PackedFeatureId::face(id2);
        }
        point
    };
    // Each case: the new point, then its predecessor, as (anchor1, anchor2, feature ids).
    let cases = [
        (
            "UNKNOWN, 1 m away",
            point(near + Vec3::X, far + Vec3::X, None),
            point(near, far, None),
        ),
        (
            "other ids, coincident",
            point(near, far, Some([1, 2])),
            point(near, far, Some([3, 4])),
        ),
        (
            "other ids, swapped",
            point(near, far, Some([1, 2])),
            point(far, near, Some([3, 4])),
        ),
    ];
    for (case, new, mut previous) in cases {
        // Fixture: a cached impulse, N s.
        previous.warm_start_normal_impulse = 11.0;
        previous.warm_start_tangent_impulse = Vec2::new(7.0, 13.0);
        let mut next = ContactManifold::new([new], Vec3::Y);
        next.match_contacts(&[previous], 0.1);
        let started = next.points[0];
        assert_eq!(
            (
                started.warm_start_normal_impulse,
                started.warm_start_tangent_impulse
            ),
            (0.0, Vec2::ZERO),
            "{case}: a point without an admissible predecessor took its impulses"
        );
    }
}

/// The shipped narrow phase must rank predecessors across the pair. Upstream's matcher, run once
/// per old manifold, lets a later offset point overwrite a
/// coincident point; either order must keep both impulse fields from the coincident point. Sensors
/// preserve the cache without solver writes.
#[test]
fn nearest_warm_start_is_selected_across_old_manifolds() {
    for reversed in [false, true] {
        let mut app = super::avian_child_pose_churn::physics_app();
        app.insert_resource(Gravity(Vec3::ZERO));
        // Fixture: overlapping stationary spheres produce one stable contact.
        let a = app
            .world_mut()
            .spawn((
                RigidBody::Static,
                Position(Vec3::ZERO),
                Collider::sphere(1.0),
            ))
            .id();
        let b = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Position(Vec3::Y * 1.5),
                Collider::sphere(1.0),
                Sensor,
            ))
            .id();
        app.update();
        app.update();
        let expected;
        {
            let mut graph = app.world_mut().resource_mut::<ContactGraph>();
            let (_, pair) = graph.get_mut(a, b).expect("fixture spheres overlap");
            assert_eq!(pair.manifolds.len(), 1);
            let manifold = &pair.manifolds[0];
            assert_eq!(manifold.points.len(), 1);
            let mut coincident = manifold.points[0];
            // Fixture: distinct cached normal and tangent impulses, N s.
            coincident.warm_start_normal_impulse = 11.0;
            coincident.warm_start_tangent_impulse = Vec2::new(7.0, 13.0);
            expected = coincident;
            let mut offset = coincident;
            // Fixture: distinct anchors retain the generated contact's feature identity.
            offset.anchor1 += Vec3::X * 0.05;
            offset.anchor2 += Vec3::X * 0.05;
            offset.warm_start_normal_impulse = 5.0;
            offset.warm_start_tangent_impulse = Vec2::new(2.0, 3.0);
            let normal = manifold.normal;
            pair.manifolds = vec![
                ContactManifold::new([coincident], normal),
                ContactManifold::new([offset], normal),
            ];
            if reversed {
                pair.manifolds.reverse();
            }
        }
        app.update();
        let graph = app.world().resource::<ContactGraph>();
        let (_, pair) = graph.get(a, b).expect("stationary pair persists");
        assert_eq!(pair.manifolds.len(), 1);
        assert_eq!(pair.manifolds[0].points.len(), 1);
        let selected = pair.manifolds[0].points[0];
        assert_eq!(
            selected.warm_start_normal_impulse, expected.warm_start_normal_impulse,
            "manifold order reversed: {reversed}"
        );
        assert_eq!(
            selected.warm_start_tangent_impulse, expected.warm_start_tangent_impulse,
            "manifold order reversed: {reversed}"
        );
    }
}
