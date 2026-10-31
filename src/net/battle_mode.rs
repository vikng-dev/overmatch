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
    fill_up_s: f32,
    combat_s: f32,
    late_join_s: f32,
    results_s: f32,
}

/// The checked mode.
#[derive(Debug)]
pub(super) struct BattleMode {
    pub(super) spawn: SpawnRule,
    /// From a tank's knockout until its player's respawn opens.
    pub(super) respawn_wait: Duration,
    pub(super) fill_up: Duration,
    pub(super) combat: Duration,
    pub(super) late_join: Duration,
    pub(super) results: Duration,
}

impl BattleMode {
    /// The shipped mode.
    pub(super) fn shipped() -> Self {
        Self::parse(include_str!("../../assets/modes/battle.ron"))
    }

    pub(super) fn parse(text: &str) -> Self {
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
        let seconds = |field, value, allow_zero| {
            let duration = Duration::try_from_secs_f32(value).unwrap_or_else(|err| {
                panic!("assets/modes/battle.ron: {field} must be finite, nonnegative seconds ({err}) — fix the mode data")
            });
            assert!(
                allow_zero || !duration.is_zero(),
                "assets/modes/battle.ron: {field} must be positive — fix the mode data"
            );
            duration
        };
        let respawn_wait = seconds("respawn_wait_s", written.respawn_wait_s, true);
        let fill_up = seconds("fill_up_s", written.fill_up_s, false);
        let combat = seconds("combat_s", written.combat_s, false);
        let late_join = seconds("late_join_s", written.late_join_s, true);
        let results = seconds("results_s", written.results_s, false);
        assert!(
            late_join <= combat,
            "assets/modes/battle.ron: late_join_s must not exceed combat_s — fix the mode data"
        );
        Self {
            spawn: SpawnRule {
                target_distance_m: written.target_distance_m,
                minimum_distance_m: written.minimum_distance_m,
            },
            respawn_wait,
            fill_up,
            combat,
            late_join,
            results,
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
        let valid = include_str!("fixtures/battle-short.ron");
        BattleMode::parse(valid);
        for (field, written, invalid) in [
            ("minimum_distance_m", "20.0", "0.0"),
            ("minimum_distance_m", "20.0", "NaN"),
            ("target_distance_m", "40.0", "10.0"),
            ("target_distance_m", "40.0", "inf"),
            ("respawn_wait_s", "0.125", "-1.0"),
            ("respawn_wait_s", "0.125", "NaN"),
            ("fill_up_s", "0.25", "0.0"),
            ("combat_s", "1.0", "0.0"),
            ("combat_s", "1.0", "inf"),
            ("late_join_s", "0.5", "2.0"),
            ("late_join_s", "0.5", "-1.0"),
            ("results_s", "0.25", "0.0"),
        ] {
            let text = valid.replace(
                &format!("{field}: {written}"),
                &format!("{field}: {invalid}"),
            );
            let failure = std::panic::catch_unwind(|| BattleMode::parse(&text))
                .expect_err("invalid mode data refuses startup");
            let message = match failure.downcast_ref::<String>() {
                Some(message) => message.as_str(),
                None => failure.downcast_ref::<&str>().expect("string data refusal"),
            };
            assert!(
                message.contains("assets/modes/battle.ron") && message.contains(field),
                "{message}"
            );
        }
        for text in [
            "()".to_owned(),
            valid.replace("    results_s: 0.25,", ""),
            valid.replace("    results_s: 0.25,", "    results_s: 0.25, extra: 1,"),
        ] {
            assert!(std::panic::catch_unwind(|| BattleMode::parse(&text)).is_err());
        }
        BattleMode::shipped();
    }
}
