//! Automatic server spawn selection. Map data owns candidates; mode data owns distances.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use bevy::prelude::*;

use crate::map::SpawnPoints;

/// Checked mode distances, in horizontal metres. No code defaults stand in for mode data.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnRule {
    target_distance_m: f32,
    minimum_distance_m: f32,
}

impl SpawnRule {
    fn parse(text: &str) -> Self {
        let rule: Self = ron::from_str(text)
            .unwrap_or_else(|err| panic!("assets/modes/battle.ron spawn rule: {err}"));
        assert!(
            rule.minimum_distance_m.is_finite()
                && rule.minimum_distance_m > 0.0
                && rule.target_distance_m.is_finite()
                && rule.target_distance_m >= rule.minimum_distance_m,
            "assets/modes/battle.ron: minimum_distance_m must be positive and finite; \
             target_distance_m must be finite and at least minimum_distance_m — fix the mode data"
        );
        rule
    }
}

/// A server-owned seeded SplitMix64 stream. Only the authority draws from it: placement is sent
/// as authoritative state rather than derived by clients. The seed comes from OS-seeded hashing.
#[derive(Resource)]
pub(super) struct SpawnPlacement {
    rule: SpawnRule,
    seed: u64,
}

impl SpawnPlacement {
    pub(super) fn battle() -> Self {
        Self {
            rule: SpawnRule::parse(include_str!("../../assets/modes/battle.ron")),
            seed: RandomState::new().hash_one("server spawn placement"),
        }
    }

    fn random_index(&mut self, count: usize) -> usize {
        // DERIVED: rejection removes the incomplete modulo bucket, so every candidate has the
        // same probability. SplitMix64's constants define the generator, not mode parameters.
        let bound = count as u64;
        let threshold = bound.wrapping_neg() % bound;
        loop {
            self.seed = self.seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut mixed = self.seed;
            mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            mixed ^= mixed >> 31;
            if mixed >= threshold {
                return (mixed % bound) as usize;
            }
        }
    }

    pub(super) fn choose(&mut self, points: &SpawnPoints, live: &[Vec2]) -> Option<Vec2> {
        let alone_index = if live.is_empty() {
            self.random_index(points.as_slice().len())
        } else {
            0 // No random draw is consumed when another tank is live.
        };
        choose_spawn(points, live, &self.rule, alone_index)
    }
}

/// Choose the authored point whose nearest live tank is closest to the mode's target distance,
/// excluding every point strictly inside its minimum. Distances are horizontal metres; ties use
/// authored order. With no live tanks, `alone_index` is the server's random draw into the checked
/// nonempty points. This function has no RNG or world side effects.
///
/// When every point is inside the minimum, returns `None`: the caller defers the spawn and retries
/// against the next live positions. It never relaxes the minimum or invents another point.
fn choose_spawn(
    points: &SpawnPoints,
    live: &[Vec2],
    rule: &SpawnRule,
    alone_index: usize,
) -> Option<Vec2> {
    if live.is_empty() {
        return Some(points.as_slice()[alone_index]);
    }
    points
        .as_slice()
        .iter()
        .filter_map(|&point| {
            let nearest = live
                .iter()
                .map(|&tank| point.distance(tank))
                .fold(f32::INFINITY, f32::min);
            (nearest >= rule.minimum_distance_m)
                .then_some((point, (nearest - rule.target_distance_m).abs()))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(point, _)| point)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    // ASSUMED fixture distances for an arbitrary mode, deliberately unlike the shipped tuning.
    fn rule() -> SpawnRule {
        SpawnRule::parse("(target_distance_m: 40.0, minimum_distance_m: 20.0)")
    }

    fn points(xz: &[[f32; 2]]) -> SpawnPoints {
        SpawnPoints::checked(xz.to_vec(), 1000.0, Path::new("fixture/level.json"))
    }

    /// Joining and respawning players rely on distance to the NEAREST live tank, not the first
    /// tank, the furthest one, or distance from the origin. A closer-to-target candidate wins.
    #[test]
    fn the_nearest_tank_distance_closest_to_the_target_wins() {
        let points = points(&[[40.0, 0.0], [0.0, 70.0], [80.0, 0.0], [0.0, 25.0]]);
        assert_eq!(
            choose_spawn(&points, &[Vec2::ZERO, Vec2::new(45.0, 0.0)], &rule(), 0),
            Some(Vec2::new(80.0, 0.0)),
        );
    }

    /// The minimum protects automatic placement even when a prohibited point is closer to the
    /// target; equality at the minimum is allowed. Mutant: scoring before exclusion picks inside.
    #[test]
    fn an_inside_minimum_point_never_beats_an_available_point() {
        let candidates = points(&[[19.0, 0.0], [100.0, 0.0]]);
        assert_eq!(
            choose_spawn(&candidates, &[Vec2::ZERO], &rule(), 0),
            Some(Vec2::new(100.0, 0.0))
        );
        let boundary = points(&[[20.0, 0.0]]);
        assert_eq!(
            choose_spawn(&boundary, &[Vec2::ZERO], &rule(), 0),
            Some(Vec2::new(20.0, 0.0))
        );
    }

    /// A crowded map defers the spawn; it must never weaken the mode's minimum for a player.
    #[test]
    fn all_points_inside_the_minimum_defer_the_spawn() {
        assert_eq!(
            choose_spawn(
                &points(&[[0.0, 0.0], [19.0, 0.0]]),
                &[Vec2::ZERO],
                &rule(),
                0
            ),
            None
        );
    }

    /// Alone players can draw every authored point, and the same seed reproduces their sequence.
    /// Mutant: always choosing the first point loses coverage; client-local entropy loses replay.
    #[test]
    fn alone_any_authored_point_can_be_chosen_from_the_server_seed() {
        let points = points(&[[0.0, 0.0], [10.0, 20.0], [-30.0, 0.0]]);
        for index in 0..points.as_slice().len() {
            assert_eq!(
                choose_spawn(&points, &[], &rule(), index),
                Some(points.as_slice()[index])
            );
        }
        let mut a = SpawnPlacement {
            rule: rule(),
            seed: 42,
        };
        let mut b = SpawnPlacement {
            rule: rule(),
            seed: 42,
        };
        let mut seen = Vec::new();
        for _ in 0..100 {
            let selected = a
                .choose(&points, &[])
                .expect("nonempty points with no live tanks");
            assert_eq!(Some(selected), b.choose(&points, &[]));
            if !seen.contains(&selected) {
                seen.push(selected);
            }
        }
        assert_eq!(seen.len(), points.as_slice().len());
    }

    /// Invalid mode data must stop the server before players join, rather than invent tuning.
    #[test]
    fn mode_distances_are_required_finite_positive_and_ordered() {
        for text in [
            "()",
            "(target_distance_m: 40.0, minimum_distance_m: 0.0)",
            "(target_distance_m: 10.0, minimum_distance_m: 20.0)",
            "(target_distance_m: inf, minimum_distance_m: 20.0)",
            "(target_distance_m: 40.0, minimum_distance_m: NaN)",
        ] {
            assert!(
                std::panic::catch_unwind(|| SpawnRule::parse(text)).is_err(),
                "{text}"
            );
        }
        SpawnPlacement::battle();
    }
}
