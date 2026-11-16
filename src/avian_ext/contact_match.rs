//! Warm-start matching: which of a pair's previous contact points gives each new point its
//! warm-start impulses. Avian calls [`match_pair`] through its `ContactMatcher` hook, under its own
//! gate (`NarrowPhaseConfig::match_contacts` on, at most four new manifolds).
//!
//! The law: a new point takes the impulses of the closest admissible point among all of the pair's
//! previous manifolds, or starts cold when none is admissible. A point whose two feature ids are
//! known admits a previous point with the same two ids, in either order, at any distance. A point
//! with an UNKNOWN id has no identity and admits a previous point whose two anchors each lie within
//! `distance_threshold` of its own, in either orientation. Closest is the smaller sum of the two
//! anchors' squared distances over the admissible orientations; a tie goes to the earlier previous
//! point.
//!
//! Upstream's matcher and its defects: `OVERMATCH_PATCH.md`, "Warm-start matching". Who relies on
//! the law: the server's resting stacks and construction contacts, whose warm starts must stay with
//! the contact point they were solved for.

use avian3d::collision::contact_types::PackedFeatureId;
use avian3d::prelude::*;

/// `distance_threshold` is in metres; see the module's matching law.
pub fn match_pair(new: &mut [ContactManifold], old: &[ContactManifold], distance_threshold: f32) {
    let admissible_squared = distance_threshold * distance_threshold;
    for point in new
        .iter_mut()
        .flat_map(|manifold| manifold.points.iter_mut())
    {
        let known = point.feature_id1 != PackedFeatureId::UNKNOWN
            && point.feature_id2 != PackedFeatureId::UNKNOWN;
        let nearest = old
            .iter()
            .flat_map(|manifold| manifold.points.iter())
            .filter_map(|previous| {
                let same_features = known
                    && ((point.feature_id1 == previous.feature_id1
                        && point.feature_id2 == previous.feature_id2)
                        || (point.feature_id1 == previous.feature_id2
                            && point.feature_id2 == previous.feature_id1));
                [
                    (previous.anchor1, previous.anchor2),
                    (previous.anchor2, previous.anchor1),
                ]
                .into_iter()
                .map(|(anchor1, anchor2)| {
                    (
                        point.anchor1.distance_squared(anchor1),
                        point.anchor2.distance_squared(anchor2),
                    )
                })
                .filter(|&(near1, near2)| {
                    same_features
                        || (!known && near1 < admissible_squared && near2 < admissible_squared)
                })
                .map(|(near1, near2)| near1 + near2)
                .min_by(f32::total_cmp)
                .map(|distance| (distance, previous))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0));
        // No admissible predecessor: a new contact point, which starts cold.
        if let Some((_, previous)) = nearest {
            point.warm_start_normal_impulse = previous.warm_start_normal_impulse;
            point.warm_start_tangent_impulse = previous.warm_start_tangent_impulse;
        }
    }
}
