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
    /// An empty server has no clock to advance until a player connects.
    WaitingForPlayer,
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
            phase: BattlePhase::WaitingForPlayer,
        }
    }
}

/// Checked mode spans in authority ticks. Each span fits a wrapped signed deadline comparison.
#[derive(Resource, Debug)]
pub(super) struct CycleRules {
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
            combat: ticks("combat_s", mode.combat),
            late_join: ticks("late_join_s", mode.late_join),
            results: ticks("results_s", mode.results),
        }
    }

    fn combat_at(&self, now: SimTick) -> BattlePhase {
        BattlePhase::Combat {
            ends_at: now.after(self.combat),
            late_join: if self.late_join.raw() == 0 {
                LateJoin::Closed
            } else {
                LateJoin::Open {
                    closes_at: now.after(self.late_join),
                }
            },
        }
    }
}

/// Boundary work the server must perform in the same tick as the phase change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Boundary {
    None,
    Combat,
    Results,
    NextBattle,
}

impl BattleState {
    pub(super) fn is_combat(self) -> bool {
        matches!(self.phase, BattlePhase::Combat { .. })
    }

    pub(super) fn accepts_joins(self) -> bool {
        matches!(
            self.phase,
            BattlePhase::WaitingForPlayer
                | BattlePhase::Combat {
                    late_join: LateJoin::Open { .. },
                    ..
                }
        )
    }

    /// The first connected player starts combat on this tick. Occupancy never ends combat early;
    /// `Boundary::NextBattle` resets the world and score, with combat already live if occupied.
    pub(super) fn advance(
        &mut self,
        now: SimTick,
        occupied: bool,
        rules: &CycleRules,
        score: &Scoreboard,
    ) -> Boundary {
        match self.phase {
            BattlePhase::WaitingForPlayer if occupied => {
                self.phase = rules.combat_at(now);
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
                self.phase = if occupied {
                    rules.combat_at(now)
                } else {
                    BattlePhase::WaitingForPlayer
                };
                return Boundary::NextBattle;
            }
            _ => {} // An empty server or a phase whose deadline has not arrived.
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
    let combat = match battles.single() {
        Ok(battle) => battle.is_combat(),
        // The local tank may precede its Battle snapshot. It commands nothing until the phase
        // arrives, and the published state must agree with the raw and parsed commands.
        Err(bevy::ecs::query::QuerySingleError::NoEntities(_)) => false,
        Err(bevy::ecs::query::QuerySingleError::MultipleEntities(_)) => {
            panic!("a peer holds at most one Battle: the server spawns one, a client replicates it")
        }
    };
    if combat {
        published.set_if_neq(MatchCommands::Live);
        return;
    }
    published.set_if_neq(MatchCommands::Held);
    for (mut raw, mut parsed) in &mut tanks {
        *parsed = ParsedTankCommand::hold();
        raw.set_if_neq(parsed.command());
    }
}

/// Results has swept the tanks; every other phase permits placement.
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

    /// A local tank can arrive before its Battle snapshot. Gameplay and instruments both rely
    /// on that interval holding raw and parsed commands, rather than publishing a hold alone.
    #[test]
    fn a_tank_before_its_battle_snapshot_commands_nothing() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(MatchCommands::Live);
        // Fixture intent: nonzero drive and a fire edge expose an unheld pre-snapshot command.
        let raw = TankCommand {
            throttle: 1.0,
            fire_primary: true,
            ..default()
        };
        let tank = world
            .spawn((raw, ParsedTankCommand::try_from(raw).unwrap()))
            .id();
        world.run_system_once(hold_commands).unwrap();
        assert!(world.get::<ParsedTankCommand>(tank).unwrap().held());
        assert_eq!(
            *world.get::<TankCommand>(tank).unwrap(),
            TankCommand::default()
        );
        assert_eq!(*world.resource::<MatchCommands>(), MatchCommands::Held);
    }

    /// Instruments read the same phase hold as gameplay: waiting and results stop counting,
    /// and combat publishes Live immediately. Fixture deadlines do not choose mode balance.
    #[test]
    fn phase_publication_matches_the_command_hold() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        world.insert_resource(MatchCommands::Live);
        let battle = world.spawn(BattleState::default()).id();
        let combat = BattlePhase::Combat {
            ends_at: SimTick::new(100),
            late_join: LateJoin::Closed,
        };
        let results = BattlePhase::Results {
            ends_at: SimTick::new(200),
            outcome: BattleOutcome::NoContest,
        };
        for (phase, expected) in [
            (BattlePhase::WaitingForPlayer, MatchCommands::Held),
            (combat, MatchCommands::Live),
            (results, MatchCommands::Held),
            (combat, MatchCommands::Live),
        ] {
            world.get_mut::<BattleState>(battle).unwrap().phase = phase;
            world.run_system_once(hold_commands).unwrap();
            assert_eq!(*world.resource::<MatchCommands>(), expected, "{phase:?}");
        }
    }

    /// Every authored phase span is refused at the load gate when the shared signed deadline
    /// relation would be ambiguous. Operators receive the mode file, field and repair direction.
    #[test]
    fn mode_spans_outside_the_signed_horizon_are_refused_by_field() {
        for field in ["combat_s", "late_join_s", "results_s"] {
            let mut mode = BattleMode::parse(include_str!("fixtures/battle-short.ron"));
            // DERIVED fixture: at the authority's default fixed step this exceeds half u32.
            let too_long = Duration::from_secs(67_108_860);
            match field {
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

    /// Combat keeps its full authored span across tick wrap even after everyone leaves.
    /// The next empty Battle waits indefinitely; a connected player starts either Battle at once.
    #[test]
    fn empty_servers_wait_and_occupied_battles_start_on_that_tick() {
        let mode = BattleMode::shipped();
        let rules = CycleRules::new(&mode, Time::<Fixed>::default().timestep());
        let score = Scoreboard::default();
        for occupied_next in [false, true] {
            let mut battle = BattleState::default();
            // Fixture epoch: combat crosses the authority tick's wrap.
            let start = SimTick::new(u32::MAX - 2);
            for now in [SimTick::new(0), start] {
                assert_eq!(battle.advance(now, false, &rules, &score), Boundary::None);
                assert_eq!(battle.phase, BattlePhase::WaitingForPlayer);
            }
            assert_eq!(
                battle.advance(start, true, &rules, &score),
                Boundary::Combat
            );
            let expected_combat = |now: SimTick| BattlePhase::Combat {
                ends_at: now.after(
                    TickSpan::from_duration(mode.combat, Time::<Fixed>::default().timestep())
                        .unwrap(),
                ),
                late_join: if mode.late_join.is_zero() {
                    LateJoin::Closed
                } else {
                    LateJoin::Open {
                        closes_at: now.after(
                            TickSpan::from_duration(
                                mode.late_join,
                                Time::<Fixed>::default().timestep(),
                            )
                            .unwrap(),
                        ),
                    }
                },
            };
            assert_eq!(battle.phase, expected_combat(start));
            assert_eq!(battle.accepts_joins(), !mode.late_join.is_zero());
            let close = start.after(rules.late_join);
            battle.advance(close, false, &rules, &score);
            assert!(!battle.accepts_joins());
            let end = start.after(rules.combat);
            assert_eq!(
                battle.advance(end, false, &rules, &score),
                Boundary::Results
            );
            let next = end.after(rules.results);
            assert_eq!(
                battle.advance(next, occupied_next, &rules, &score),
                Boundary::NextBattle
            );
            assert_eq!(battle.number, 2);
            assert_eq!(
                battle.phase,
                if occupied_next {
                    expected_combat(next)
                } else {
                    BattlePhase::WaitingForPlayer
                }
            );
            if !occupied_next {
                assert_eq!(
                    battle.advance(next.after(rules.combat), false, &rules, &score),
                    Boundary::None
                );
                assert_eq!(battle.phase, BattlePhase::WaitingForPlayer);
            }
        }
    }
}
