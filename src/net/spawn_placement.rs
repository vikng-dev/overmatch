//! Automatic server spawn selection. Map data owns candidates; mode data owns distances.

use std::collections::hash_map::RandomState;
use std::hash::BuildHasher;

use bevy::prelude::*;

use crate::map::SpawnPoints;

/// The mode's spawn distances, in horizontal metres, as [`super::battle_mode::BattleMode`] checked
/// them: `minimum_distance_m` positive and finite, `target_distance_m` finite and at least it.
#[derive(Debug)]
pub(super) struct SpawnRule {
    pub(super) target_distance_m: f32,
    pub(super) minimum_distance_m: f32,
}

/// A server-owned seeded SplitMix64 stream. Only the authority draws from it: placement is sent
/// as authoritative state rather than derived by clients. The seed comes from OS-seeded hashing.
#[derive(Resource)]
pub(super) struct SpawnPlacement {
    rule: SpawnRule,
    seed: u64,
}

impl SpawnPlacement {
    pub(super) fn new(rule: SpawnRule) -> Self {
        Self {
            rule,
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

    pub(super) fn choose(
        &mut self,
        points: &SpawnPoints,
        live: &[Vec2],
        clearance: f32,
    ) -> Option<Vec2> {
        let alone_index = if live.is_empty() {
            self.random_index(points.as_slice().len())
        } else {
            0 // No random draw is consumed when another tank is live.
        };
        choose_spawn(points, live, &self.rule, alone_index, clearance)
    }
}

/// Choose the authored point whose nearest live tank is closest to the mode's target distance,
/// excluding points inside either its minimum or the body construction clearance. Distances are horizontal metres; ties use
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
    clearance: f32,
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
            (nearest >= rule.minimum_distance_m.max(clearance))
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
        SpawnRule {
            target_distance_m: 40.0,
            minimum_distance_m: 20.0,
        }
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
            choose_spawn(
                &points,
                &[Vec2::ZERO, Vec2::new(45.0, 0.0)],
                &rule(),
                0,
                0.0
            ),
            Some(Vec2::new(80.0, 0.0)),
        );
    }

    /// The minimum protects automatic placement even when a prohibited point is closer to the
    /// target; equality at the minimum is allowed. Mutant: scoring before exclusion picks inside.
    #[test]
    fn an_inside_minimum_point_never_beats_an_available_point() {
        let candidates = points(&[[19.0, 0.0], [100.0, 0.0]]);
        assert_eq!(
            choose_spawn(&candidates, &[Vec2::ZERO], &rule(), 0, 0.0),
            Some(Vec2::new(100.0, 0.0))
        );
        let boundary = points(&[[20.0, 0.0]]);
        assert_eq!(
            choose_spawn(&boundary, &[Vec2::ZERO], &rule(), 0, 0.0),
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
                0,
                0.0
            ),
            None
        );
    }

    /// Construction clearance can exceed the mode's minimum. A larger body must defer instead
    /// of overlapping another tank just because the mode's preferred spacing allowed its centre.
    #[test]
    fn physical_clearance_can_exceed_the_mode_minimum() {
        let candidates = points(&[[21.0, 0.0], [50.0, 0.0]]);
        assert_eq!(
            choose_spawn(&candidates, &[Vec2::ZERO], &rule(), 0, 30.0),
            Some(Vec2::new(50.0, 0.0))
        );
        assert_eq!(
            choose_spawn(&points(&[[21.0, 0.0]]), &[Vec2::ZERO], &rule(), 0, 30.0),
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
                choose_spawn(&points, &[], &rule(), index, 0.0),
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
                .choose(&points, &[], 0.0)
                .expect("nonempty points with no live tanks");
            assert_eq!(Some(selected), b.choose(&points, &[], 0.0));
            if !seen.contains(&selected) {
                seen.push(selected);
            }
        }
        assert_eq!(seen.len(), points.as_slice().len());
    }
    /// A full stationary lobby must spawn without requiring an earlier player to move. Each
    /// possible first random point is exercised; subsequent spawns use the production rule.
    /// This is a map/mode/capacity contract, independent of the tank's identity or geometry.
    #[test]
    fn authored_points_admit_a_full_stationary_lobby_from_every_first_point() {
        let manifest = crate::map::tests::shipped_manifest();
        let mode = super::super::battle_mode::BattleMode::shipped();
        // Fixture lobby size: the owner's early-access capacity decision (#220).
        let capacity = 30;
        for first in 0..manifest.spawn_points.as_slice().len() {
            let mut live = Vec::new();
            for _ in 0..capacity {
                let point = choose_spawn(&manifest.spawn_points, &live, &mode.spawn, first, 0.0)
                    .expect("author map points far enough apart to admit the configured capacity");
                live.push(point);
            }
        }
    }
}
