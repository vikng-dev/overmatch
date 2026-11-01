//! The server's Battle state, replicated whole with authority-tick deadlines. Clients read it;
//! only [`super::server::cycle_battles`] advances it. Mode choices and boundary rules live in
//! `.agents/docs/design/battle-cycle.md`.

use std::time::Duration;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::battle_mode::BattleMode;
use super::protocol::InputBridge;
use super::score::Scoreboard;
use crate::CombatantId;
use crate::command::{ParsedTankCommand, TankCommand};
use crate::sim_tick::{SimTick, TickSpan};
use crate::state::{GameplaySet, MatchCommands};

/// One server-owned Battle, on the scoreboard's entity. Deadlines use `ShotClock`'s wrapping ticks.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BattleState {
    pub(crate) number: u32,
    pub(crate) phase: BattlePhase,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BattlePhase {
    /// No deadline until the first player joins. Once started, fill-up runs to its deadline.
    FillUp { ends_at: Option<SimTick> },
    Combat {
        ends_at: SimTick,
        late_join: LateJoin,
    },
    Results {
        ends_at: SimTick,
        outcome: BattleOutcome,
    },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LateJoin {
    Open { closes_at: SimTick },
    Closed,
}

/// The authority's result; the client never breaks a draw or chooses a winner.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BattleOutcome {
    NoContest,
    Winner(CombatantId),
    Draw,
}

impl Default for BattleState {
    fn default() -> Self {
        Self {
            number: 1,
            phase: BattlePhase::FillUp { ends_at: None },
        }
    }
}

/// Checked mode spans in authority ticks. Each span fits a wrapped signed deadline comparison.
#[derive(Resource, Debug)]
pub(super) struct CycleRules {
    fill_up: TickSpan,
    combat: TickSpan,
    late_join: TickSpan,
    results: TickSpan,
}

impl CycleRules {
    pub(super) fn new(mode: &BattleMode, tick: Duration) -> Self {
        let ticks = |field, span: Duration| {
            TickSpan::from_duration(span, tick).unwrap_or_else(|err| {
                panic!("assets/modes/battle.ron: {field}: {err} — reduce the duration or fix the timestep")
            })
        };
        Self {
            fill_up: ticks("fill_up_s", mode.fill_up),
            combat: ticks("combat_s", mode.combat),
            late_join: ticks("late_join_s", mode.late_join),
            results: ticks("results_s", mode.results),
        }
    }
}

/// Boundary work the server must perform in the same tick as the phase change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    None,
    Combat,
    Results,
    FillUp,
}

impl BattleState {
    pub(super) fn is_combat(self) -> bool {
        matches!(self.phase, BattlePhase::Combat { .. })
    }

    pub(super) fn accepts_joins(self) -> bool {
        matches!(
            self.phase,
            BattlePhase::FillUp { .. }
                | BattlePhase::Combat {
                    late_join: LateJoin::Open { .. },
                    ..
                }
        )
    }

    /// Advance one authority tick. Occupancy starts an idle fill-up and the next fill-up's timer;
    /// it never ends combat early. `Boundary::FillUp` asks the caller to clear the frozen score.
    pub(super) fn advance(
        &mut self,
        now: SimTick,
        occupied: bool,
        rules: &CycleRules,
        score: &Scoreboard,
    ) -> Boundary {
        match self.phase {
            BattlePhase::FillUp { ends_at: None } if occupied => {
                self.phase = BattlePhase::FillUp {
                    ends_at: Some(now.after(rules.fill_up)),
                };
            }
            BattlePhase::FillUp { ends_at: Some(end) } if now.has_reached(end) => {
                self.phase = BattlePhase::Combat {
                    ends_at: now.after(rules.combat),
                    late_join: if rules.late_join.raw() == 0 {
                        LateJoin::Closed
                    } else {
                        LateJoin::Open {
                            closes_at: now.after(rules.late_join),
                        }
                    },
                };
                return Boundary::Combat;
            }
            BattlePhase::Combat { ends_at, .. } if now.has_reached(ends_at) => {
                self.phase = BattlePhase::Results {
                    ends_at: now.after(rules.results),
                    outcome: score.outcome(),
                };
                return Boundary::Results;
            }
            BattlePhase::Combat {
                ends_at,
                late_join: LateJoin::Open { closes_at },
            } if now.has_reached(closes_at) => {
                self.phase = BattlePhase::Combat {
                    ends_at,
                    late_join: LateJoin::Closed,
                };
            }
            BattlePhase::Results { ends_at, .. } if now.has_reached(ends_at) => {
                self.number = self
                    .number
                    .checked_add(1)
                    .expect("Battle counter exhausted u32");
                self.phase = BattlePhase::FillUp {
                    ends_at: occupied.then(|| now.after(rules.fill_up)),
                };
                return Boundary::FillUp;
            }
            _ => {} // An idle fill-up or a phase whose deadline has not arrived.
        }
        Boundary::None
    }

    #[cfg(test)]
    pub(crate) fn combat_fixture() -> Self {
        Self {
            number: 1,
            phase: BattlePhase::Combat {
                ends_at: SimTick::new(100_000),
                late_join: LateJoin::Closed,
            },
        }
    }
}

/// The ordering handle of [`hold_commands`]: after every command writer, before every consumer.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct HoldCommands;

/// Both endpoints: the hold runs on the fixed tick after the input bridge.
pub(super) fn install_hold(app: &mut App) {
    app.insert_resource(MatchCommands::Held)
        .configure_sets(
            FixedUpdate,
            HoldCommands.after(InputBridge).before(GameplaySet),
        )
        .add_systems(FixedUpdate, hold_commands.in_set(HoldCommands));
}

/// Until combat starts a player controls only the camera. Every tank's command is held
/// ([`ParsedTankCommand::hold`]) while the replicated phase is not combat: drive, turret, gun, fire
/// and every edge by this one rule, and the tank still rests on its suspension. The authority
/// releases them on combat's first tick, because `server::cycle_battles` advances the phase before
/// this set. A client holds its own turret and fire presentation by the same rule from its
/// replicated phase, and the sim-level [`MatchCommands`] states the same rule for instruments.
/// Before the Battle snapshot (a client's first frames) every command is held.
fn hold_commands(
    battles: Query<&BattleState>,
    mut published: ResMut<MatchCommands>,
    mut tanks: Query<(&mut TankCommand, &mut ParsedTankCommand)>,
) {
    let battle = match battles.single() {
        Ok(battle) => battle,
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => {
            published.set_if_neq(MatchCommands::Held);
            return;
        }
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a peer holds at most one Battle: the server spawns one, a client replicates it")
        }
    };
    if battle.is_combat() {
        published.set_if_neq(MatchCommands::Live);
        return;
    }
    published.set_if_neq(MatchCommands::Held);
    for (mut raw, mut parsed) in &mut tanks {
        *parsed = ParsedTankCommand::hold();
        raw.set_if_neq(parsed.command());
    }
}

/// Whether the Battle places tanks: through fill-up and combat. Results has swept them, and the
/// next fill-up places every connected player.
pub(super) fn places_tanks(battles: Query<&BattleState>) -> bool {
    !matches!(
        battles
            .single()
            .expect("server startup spawns exactly one Battle with its scoreboard")
            .phase,
        BattlePhase::Results { .. }
    )
}

pub(super) fn in_combat(battles: Query<&BattleState>) -> bool {
    battles
        .single()
        .expect("server startup spawns exactly one Battle with its scoreboard")
        .is_combat()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every authored phase span is refused at the load gate when the shared signed deadline
    /// relation would be ambiguous. Operators receive the mode file, field and repair direction.
    #[test]
    fn mode_spans_outside_the_signed_horizon_are_refused_by_field() {
        for field in ["fill_up_s", "combat_s", "late_join_s", "results_s"] {
            let mut mode = BattleMode::parse(include_str!("fixtures/battle-short.ron"));
            // DERIVED fixture: at the authority's default fixed step this exceeds half u32.
            let too_long = Duration::from_secs(67_108_860);
            match field {
                "fill_up_s" => mode.fill_up = too_long,
                "combat_s" => mode.combat = too_long,
                "late_join_s" => mode.late_join = too_long,
                "results_s" => mode.results = too_long,
                _ => unreachable!(),
            }
            let failure = std::panic::catch_unwind(|| {
                CycleRules::new(&mode, Time::<Fixed>::default().timestep())
            })
            .expect_err("a mode span must fit the signed horizon");
            let message = failure.downcast_ref::<String>().unwrap();
            for word in ["assets/modes/battle.ron", field, "reduce"] {
                assert!(message.contains(word), "{message}");
            }
        }
    }

    /// The cycle's deadlines and full combat span survive the authority tick's wrap. Losing the
    /// last player cannot shorten combat; an empty next fill-up waits for another first join.
    #[test]
    fn the_cycle_runs_in_full_across_wrap_and_waits_when_empty() {
        // Fixture spans in ticks, unrelated to shipped tuning.
        let rules = CycleRules {
            fill_up: TickSpan::of(2),
            combat: TickSpan::of(4),
            late_join: TickSpan::ONE,
            results: TickSpan::of(2),
        };
        let mut battle = BattleState::default();
        let score = Scoreboard::default();
        let start = u32::MAX - 2;
        battle.advance(SimTick::new(start), false, &rules, &score);
        assert_eq!(battle.phase, BattlePhase::FillUp { ends_at: None });
        battle.advance(SimTick::new(start), true, &rules, &score);
        assert_eq!(
            battle.advance(SimTick::new(start + 1), false, &rules, &score),
            Boundary::None
        );
        assert_eq!(
            battle.advance(SimTick::new(u32::MAX), false, &rules, &score),
            Boundary::Combat
        );
        assert!(battle.accepts_joins());
        battle.advance(SimTick::new(0), false, &rules, &score);
        assert!(!battle.accepts_joins());
        assert_eq!(
            battle.advance(SimTick::new(2), false, &rules, &score),
            Boundary::None
        );
        assert_eq!(
            battle.advance(SimTick::new(3), false, &rules, &score),
            Boundary::Results
        );
        assert_eq!(
            battle.advance(SimTick::new(5), false, &rules, &score),
            Boundary::FillUp
        );
        assert_eq!(battle.number, 2);
        assert_eq!(battle.phase, BattlePhase::FillUp { ends_at: None });
        assert!(battle.accepts_joins());
    }
}
