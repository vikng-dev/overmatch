//! The Battle mode's data, `assets/modes/battle.ron`: the one home of the mode's tunables, parsed
//! and checked once ([`BattleMode::shipped`]) while the server is built, so bad data stops startup.
//! Spawn placement holds its distances ([`SpawnRule`]); the score holds its respawn wait
//! ([`super::score::RespawnWait`]).

use std::time::Duration;

use super::spawn_placement::SpawnRule;

/// The file as written. Every field is required: no code default stands in for mode data.
#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    target_distance_m: f32,
    minimum_distance_m: f32,
    respawn_wait_s: f32,
}

/// The checked mode.
#[derive(Debug)]
pub(super) struct BattleMode {
    pub(super) spawn: SpawnRule,
    /// From a tank's knockout until its player's respawn opens.
    pub(super) respawn_wait: Duration,
}

impl BattleMode {
    /// The shipped mode.
    pub(super) fn shipped() -> Self {
        Self::parse(include_str!("../../assets/modes/battle.ron"))
    }

    fn parse(text: &str) -> Self {
        let written: Written =
            ron::from_str(text).unwrap_or_else(|err| panic!("assets/modes/battle.ron: {err}"));
        assert!(
            written.minimum_distance_m.is_finite()
                && written.minimum_distance_m > 0.0
                && written.target_distance_m.is_finite()
                && written.target_distance_m >= written.minimum_distance_m,
            "assets/modes/battle.ron: minimum_distance_m must be positive and finite; \
             target_distance_m must be finite and at least minimum_distance_m — fix the mode data"
        );
        // Refuses a negative, NaN or infinite value.
        let respawn_wait =
            Duration::try_from_secs_f32(written.respawn_wait_s).unwrap_or_else(|err| {
                panic!(
                    "assets/modes/battle.ron: respawn_wait_s must be a finite number of seconds, \
                     zero or more (it is {}: {err}) — fix the mode data",
                    written.respawn_wait_s
                )
            });
        Self {
            spawn: SpawnRule {
                target_distance_m: written.target_distance_m,
                minimum_distance_m: written.minimum_distance_m,
            },
            respawn_wait,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Invalid mode data must stop the server before players join, rather than invent tuning: a
    /// missing field, an unknown one, distances out of order or not finite, and a respawn wait that
    /// is negative or not finite are all refused. The shipped file parses.
    #[test]
    fn mode_data_is_required_finite_and_ordered() {
        for text in [
            "()",
            "(target_distance_m: 40.0, minimum_distance_m: 20.0)",
            "(target_distance_m: 40.0, minimum_distance_m: 20.0, respawn_wait_s: 1.0, lives: 2)",
            "(target_distance_m: 40.0, minimum_distance_m: 0.0, respawn_wait_s: 1.0)",
            "(target_distance_m: 10.0, minimum_distance_m: 20.0, respawn_wait_s: 1.0)",
            "(target_distance_m: inf, minimum_distance_m: 20.0, respawn_wait_s: 1.0)",
            "(target_distance_m: 40.0, minimum_distance_m: NaN, respawn_wait_s: 1.0)",
            "(target_distance_m: 40.0, minimum_distance_m: 20.0, respawn_wait_s: -1.0)",
            "(target_distance_m: 40.0, minimum_distance_m: 20.0, respawn_wait_s: inf)",
            "(target_distance_m: 40.0, minimum_distance_m: 20.0, respawn_wait_s: NaN)",
        ] {
            assert!(
                std::panic::catch_unwind(|| BattleMode::parse(text)).is_err(),
                "{text}"
            );
        }
        BattleMode::shipped();
    }
}
