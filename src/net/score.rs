//! The game's damage model: the match score, game state only — per combatant, knockouts, losses
//! and when its
//! knocked-out life may respawn. The server holds it on one replicated entity ([`Scoreboard`]),
//! opens a combatant's standing when it first enters combat, and changes it only at the authority's knockout
//! transition ([`TankKnockout`]): one loss to the knocked-out tank's combatant, one knockout to the
//! combatant whose round decided it ([`Credit`]), and the respawn's opening the mode's wait
//! ([`RespawnWait`]) later. Clients read the board and derive nothing; [`rank`] orders it.

use std::cmp::Ordering;
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use super::battle::{BattleOutcome, BattleState};
use crate::damage::{KnockoutCause, KnockoutReason, LaunchedTurret, TankKnockedOut, TankKnockout};
use crate::sim_tick::{SimTick, TickSpan};
use crate::tank::TankRoot;
use crate::{CombatantId, CombatantLife, ShotClock};

/// Every combat participant's standing, retained through disconnect and respawn.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Scoreboard(pub(crate) Vec<Standing>);

/// One combatant's standing.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Standing {
    pub(crate) combatant: CombatantId,
    /// Knockouts its rounds decided.
    pub(crate) knockouts: u32,
    /// Its tanks knocked out, whether or not anyone was credited.
    pub(crate) losses: u32,
    pub(crate) respawn: Respawn,
}

/// When a combatant's knocked-out life may respawn.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Respawn {
    /// None of its lives has been knocked out.
    NeverKnockedOut,
    /// Its life `life` was the last knocked out; its respawn opens at authority tick `tick`
    /// ([`ShotClock`] units, wrapping).
    Opens { life: CombatantLife, tick: u32 },
}

/// Whether a knocked-out life's respawn is open at a tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Opening {
    Open,
    /// It opens this many ticks later.
    In {
        ticks: u32,
    },
}

impl Scoreboard {
    /// Enter the contest once; both the combat boundary and a combat spawn use this opening.
    pub(super) fn enter_combat(&mut self, combatant: CombatantId) {
        if self.standing_mut(combatant).is_none() {
            self.0.push(Standing {
                combatant,
                knockouts: 0,
                losses: 0,
                respawn: Respawn::NeverKnockedOut,
            });
        }
    }

    /// The game's damage model: resolve the Battle from the scored standings, including players
    /// who disconnected. Empty boards name no contest; tied best standings name a draw.
    pub(super) fn outcome(&self) -> BattleOutcome {
        let Some(best) = self.0.iter().min_by(|a, b| rank(a, b)) else {
            return BattleOutcome::NoContest;
        };
        if self
            .0
            .iter()
            .filter(|standing| rank(standing, best).is_eq())
            .count()
            > 1
        {
            BattleOutcome::Draw
        } else {
            BattleOutcome::Winner(best.combatant)
        }
    }

    fn standing_mut(&mut self, combatant: CombatantId) -> Option<&mut Standing> {
        self.0
            .iter_mut()
            .find(|standing| standing.combatant == combatant)
    }

    /// Whether `combatant`'s knocked-out life `life` may respawn at authority tick `now`; `None`
    /// when the board names no opening for that life (no knockout of it scored).
    pub(crate) fn opening(
        &self,
        combatant: CombatantId,
        life: CombatantLife,
        now: SimTick,
    ) -> Option<Opening> {
        let standing = self
            .0
            .iter()
            .find(|standing| standing.combatant == combatant)?;
        match standing.respawn {
            Respawn::Opens {
                life: knocked,
                tick,
            } if knocked == life => {
                let opens = SimTick::new(tick);
                Some(if now.has_reached(opens) {
                    Opening::Open
                } else {
                    Opening::In {
                        ticks: now.remaining_until(opens),
                    }
                })
            }
            Respawn::Opens { .. } | Respawn::NeverKnockedOut => None,
        }
    }
}

/// The ranking: more knockouts first, then fewer losses; `Equal` is a draw. `Less` ranks `a` above
/// `b`, so sorting a board by it lists the winner first.
pub(crate) fn rank(a: &Standing, b: &Standing) -> Ordering {
    b.knockouts.cmp(&a.knockouts).then(a.losses.cmp(&b.losses))
}

/// Who a knockout credits with one knockout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Credit {
    /// Another combatant's round decided it.
    Shooter(CombatantId),
    /// No keyed round decided it: a loss with no credit.
    NoShooter,
    /// The knocked-out tank's own combatant's round decided it: a loss with no credit.
    OwnRound,
}

fn credit(victim: CombatantId, cause: KnockoutCause) -> Credit {
    match cause {
        KnockoutCause::Round(shot) if shot.combatant == victim => Credit::OwnRound,
        KnockoutCause::Round(shot) => Credit::Shooter(shot.combatant),
        KnockoutCause::NoKeyedRound => Credit::NoShooter,
    }
}

/// The mode's respawn wait ([`super::battle_mode::BattleMode`]) in authority ticks.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RespawnWait {
    ticks: TickSpan,
}

impl RespawnWait {
    /// `wait` in whole ticks of `tick`, rounded up so the respawn never opens early. Built while
    /// the server is: a wait longer than an opening can count ahead (`i32::MAX` ticks, the wrapped
    /// signed difference [`Scoreboard::opening`] reads) stops startup.
    pub(crate) fn new(wait: Duration, tick: Duration) -> Self {
        let ticks = wait.as_nanos().div_ceil(tick.as_nanos());
        let ticks = u32::try_from(ticks)
            .ok()
            .and_then(|ticks| TickSpan::try_from(ticks).ok())
            .unwrap_or_else(|| {
                panic!(
                    "assets/modes/battle.ron: respawn_wait_s is {ticks} ticks, more than an \
                     opening counts ahead ({}) — fix the mode data",
                    i32::MAX
                )
            });
        Self { ticks }
    }

    #[cfg(test)]
    pub(crate) fn ticks(self) -> u32 {
        self.ticks.raw()
    }
}

/// The server's respawn gate: whether a knocked-out life's respawn is open now.
#[derive(SystemParam)]
pub(super) struct RespawnGate<'w, 's> {
    boards: Query<'w, 's, &'static Scoreboard>,
    clock: Res<'w, ShotClock>,
}

impl RespawnGate<'_, '_> {
    /// Whether the knocked-out `life` of `combatant` may respawn this tick.
    pub(super) fn opening(&self, combatant: CombatantId, life: CombatantLife) -> Opening {
        self.boards
            .single()
            .expect("server startup spawns exactly one scoreboard before any combatant spawns")
            .opening(combatant, life, SimTick::new(self.clock.0))
            .expect(
                "the knockout transition (`TankKnockout`) scored this life's opening the tick it \
                 was knocked out",
            )
    }
}

/// The server's half: the replicated board, the standings it opens and the knockouts it scores.
pub(super) fn install_server(app: &mut App, wait: RespawnWait) {
    mount(app, wait);
    app.add_systems(Startup, spawn_scoreboard);
}

/// The scoring observers and the wait, for a world that spawns its own board.
pub(crate) fn mount(app: &mut App, wait: RespawnWait) {
    app.insert_resource(wait)
        .add_observer(open_standing)
        .add_observer(score_knockout);
}

fn spawn_scoreboard(mut commands: Commands) {
    commands.spawn((
        Name::new("Scoreboard"),
        Scoreboard::default(),
        BattleState::default(),
        Replicate::to_clients(NetworkTarget::All),
    ));
}

/// A first combat spawn opens its standing; a fill-up spawn waits for the combat boundary.
fn open_standing(
    add: On<Add, CombatantId>,
    combatants: Query<&CombatantId>,
    mut boards: Query<(&mut Scoreboard, &BattleState)>,
) {
    let combatant = *combatants
        .get(add.entity)
        .expect("an added CombatantId is on its entity");
    let (mut board, battle) = boards
        .single_mut()
        .expect("server startup spawns exactly one scoreboard before any combatant spawns");
    if battle.is_combat() {
        board.enter_combat(combatant);
    }
}

/// Whether a knocked-out tank's turret left it. A separate fact from the knockout's kind: today
/// every cook-off launches the turret, and the launch may become a chance on a cook-off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurretFate {
    Mounted,
    Launched,
}

/// The server's one knockout line: the life, the knockout's kind, the turret's fate and the credit,
/// read from the state the authority holds at the transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KnockoutLine {
    victim: CombatantId,
    life: CombatantLife,
    kind: KnockoutReason,
    turret: TurretFate,
    credit: Credit,
}

impl KnockoutLine {
    /// `tank`'s line: its knockout kind from its [`TankKnockedOut`], its turret launched when a
    /// [`LaunchedTurret`] names it as root.
    fn read(
        tank: Entity,
        cause: KnockoutCause,
        tanks: &Query<(&CombatantId, &CombatantLife, &TankKnockedOut)>,
        launched: &Query<&TankRoot, With<LaunchedTurret>>,
    ) -> Self {
        let (&victim, &life, knocked_out) = tanks.get(tank).expect(
            "every server tank root carries its spawn identity, and its knockout transition \
             follows its TankKnockedOut",
        );
        Self {
            victim,
            life,
            kind: knocked_out.reason,
            turret: if launched.iter().any(|root| root.0 == tank) {
                TurretFate::Launched
            } else {
                TurretFate::Mounted
            },
            credit: credit(victim, cause),
        }
    }
}

impl std::fmt::Display for KnockoutLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let turret = match self.turret {
            TurretFate::Mounted => "mounted",
            TurretFate::Launched => "launched",
        };
        write!(
            f,
            "server: combatant {} life {} knocked out by {}, turret {turret} ({:?})",
            self.victim.0,
            self.life.0,
            self.kind.label(),
            self.credit
        )
    }
}

/// The knockout transition, scored: a loss and the respawn's opening for the knocked-out tank's
/// combatant, a knockout for the combatant its [`Credit`] names.
fn score_knockout(
    knockout: On<TankKnockout>,
    tanks: Query<(&CombatantId, &CombatantLife, &TankKnockedOut)>,
    launched: Query<&TankRoot, With<LaunchedTurret>>,
    clock: Res<ShotClock>,
    wait: Res<RespawnWait>,
    mut boards: Query<(&mut Scoreboard, &BattleState)>,
) {
    let line = KnockoutLine::read(knockout.tank, knockout.cause, &tanks, &launched);
    let (victim, life) = (line.victim, line.life);
    let (mut board, battle) = boards.single_mut().expect(
        "server startup spawns exactly one scoreboard and Battle before any combatant spawns",
    );
    if !battle.is_combat() {
        // The phase boundary owns scoring; fixtures and pending consequences cannot score outside combat.
        return;
    }
    let standing = board
        .standing_mut(victim)
        .expect("a combatant's standing opens when it enters combat");
    standing.losses += 1;
    standing.respawn = Respawn::Opens {
        life,
        tick: SimTick::new(clock.0).after(wait.ticks).raw(),
    };
    match line.credit {
        Credit::Shooter(shooter) => {
            board
                .standing_mut(shooter)
                .expect("a round's combatant entered combat before it fired, and a standing outlives its tanks")
                .knockouts += 1;
        }
        Credit::NoShooter | Credit::OwnRound => {}
    }
    info!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(combatant: u64, knockouts: u32, losses: u32) -> Standing {
        Standing {
            combatant: CombatantId(combatant),
            knockouts,
            losses,
            respawn: Respawn::NeverKnockedOut,
        }
    }

    /// The winner rule the results and the scoreboard rely on (#220): most knockouts wins
    /// whatever the losses; equal knockouts go to fewer losses; equal in both is a draw. Mutants:
    /// ranking losses first, preferring more losses, or breaking a draw by combatant all fail.
    #[test]
    fn the_ranking_orders_knockouts_then_fewer_losses_then_draws() {
        assert_eq!(rank(&standing(1, 3, 9), &standing(2, 2, 0)), Ordering::Less);
        assert_eq!(rank(&standing(1, 2, 1), &standing(2, 2, 4)), Ordering::Less);
        assert_eq!(
            rank(&standing(1, 2, 4), &standing(2, 2, 1)),
            Ordering::Greater
        );
        assert_eq!(
            rank(&standing(1, 2, 3), &standing(2, 2, 3)),
            Ordering::Equal
        );
        let mut board = [
            standing(1, 1, 0),
            standing(2, 2, 5),
            standing(3, 2, 1),
            standing(4, 0, 0),
        ];
        board.sort_by(rank);
        let order: Vec<u64> = board.iter().map(|standing| standing.combatant.0).collect();
        assert_eq!(order, [3, 2, 1, 4]);
    }

    /// Credit names who a knockout counts for: another combatant's round credits it, the tank's
    /// own round and no keyed round credit nobody. Mutant: crediting the victim for its own round,
    /// or any fallback combatant for no round, fails.
    #[test]
    fn a_knockout_credits_only_another_combatants_round() {
        let shot = |combatant| crate::ShotId {
            combatant: CombatantId(combatant),
            weapon: 0,
            fire_tick: 7,
        };
        let victim = CombatantId(2);
        assert_eq!(
            credit(victim, KnockoutCause::Round(shot(1))),
            Credit::Shooter(CombatantId(1))
        );
        assert_eq!(
            credit(victim, KnockoutCause::Round(shot(2))),
            Credit::OwnRound
        );
        assert_eq!(
            credit(victim, KnockoutCause::NoKeyedRound),
            Credit::NoShooter
        );
    }

    /// The game's damage model: result consumers rely on the authority choosing the unique best
    /// standing, preserving a tie, and naming an empty contest without inventing a winner.
    #[test]
    fn the_authority_reports_a_winner_a_draw_or_no_contest() {
        assert_eq!(Scoreboard::default().outcome(), BattleOutcome::NoContest);
        assert_eq!(
            Scoreboard(vec![standing(1, 3, 5), standing(2, 3, 1)]).outcome(),
            BattleOutcome::Winner(CombatantId(2))
        );
        assert_eq!(
            Scoreboard(vec![standing(1, 3, 1), standing(2, 3, 1)]).outcome(),
            BattleOutcome::Draw
        );
    }

    /// The game's damage model: results must exclude a player who only waited in fill-up,
    /// while a combat participant's standing survives disconnect and respawn. ASSUMED fixture
    /// identities and respawn wait; phase membership is the invariant.
    #[test]
    fn standings_start_at_combat_participation_and_outlive_its_tanks() {
        let mut app = App::new();
        mount(
            &mut app,
            RespawnWait::new(Duration::from_secs(1), Duration::from_millis(10)),
        );
        let board = app
            .world_mut()
            .spawn((Scoreboard::default(), BattleState::default()))
            .id();
        let waiting = app.world_mut().spawn(CombatantId(1)).id();
        assert!(
            app.world().get::<Scoreboard>(board).unwrap().0.is_empty(),
            "waiting alone is not combat participation"
        );
        app.world_mut().despawn(waiting);
        assert_eq!(
            app.world().get::<Scoreboard>(board).unwrap().outcome(),
            BattleOutcome::NoContest
        );
        *app.world_mut().get_mut::<BattleState>(board).unwrap() = BattleState::combat_fixture();
        let fighter = app.world_mut().spawn(CombatantId(2)).id();
        assert_eq!(
            app.world().get::<Scoreboard>(board).unwrap().outcome(),
            BattleOutcome::Winner(CombatantId(2))
        );
        app.world_mut().despawn(fighter);
        app.world_mut().spawn(CombatantId(2));
        let standings = &app.world().get::<Scoreboard>(board).unwrap().0;
        assert_eq!(
            standings.len(),
            1,
            "a combat disconnect or respawn cannot erase or duplicate a standing"
        );
        assert_eq!(standings[0].combatant, CombatantId(2));
    }

    /// The game's damage model: only combat can change score. Fill-up and results ignore a
    /// knockout fact; the result and the retained table stay consistent until the next fill-up.
    #[test]
    fn nothing_scores_outside_combat() {
        use super::super::battle::{BattlePhase, LateJoin};
        let mut app = App::new();
        app.init_resource::<ShotClock>();
        mount(
            &mut app,
            RespawnWait::new(Duration::from_secs(1), Duration::from_millis(10)),
        );
        let board = app
            .world_mut()
            .spawn((Scoreboard::default(), BattleState::combat_fixture()))
            .id();
        let tank = app
            .world_mut()
            .spawn((
                CombatantId(1),
                CombatantLife(0),
                TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                },
            ))
            .id();
        let knockout = TankKnockout {
            tank,
            cause: KnockoutCause::NoKeyedRound,
        };
        app.world_mut().get_mut::<BattleState>(board).unwrap().phase = BattleState::default().phase;
        app.world_mut().trigger(knockout);
        assert_eq!(
            app.world().get::<Scoreboard>(board).expect("board").0[0].losses,
            0
        );
        app.world_mut()
            .get_mut::<BattleState>(board)
            .expect("Battle")
            .phase = BattlePhase::Combat {
            ends_at: crate::sim_tick::SimTick::new(100),
            late_join: LateJoin::Closed,
        };
        app.world_mut().trigger(knockout);
        assert_eq!(
            app.world().get::<Scoreboard>(board).expect("board").0[0].losses,
            1
        );
        app.world_mut()
            .get_mut::<BattleState>(board)
            .expect("Battle")
            .phase = BattlePhase::Results {
            ends_at: crate::sim_tick::SimTick::new(200),
            outcome: BattleOutcome::Winner(CombatantId(1)),
        };
        app.world_mut().trigger(knockout);
        assert_eq!(
            app.world().get::<Scoreboard>(board).expect("board").0[0].losses,
            1
        );
    }

    /// The game's damage model's server log: the one knockout line names the knockout's kind and,
    /// as its own fact, whether the turret launched, both read from the authority's state at the
    /// transition. A cook-off that launched the turret and a crew loss with it mounted; the
    /// identities are fixture data.
    #[test]
    fn the_knockout_line_names_its_kind_and_the_turrets_fate() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let cooked = world
            .spawn((
                CombatantId(1),
                CombatantLife(0),
                TankKnockedOut {
                    reason: KnockoutReason::Cookoff,
                },
            ))
            .id();
        world.spawn((LaunchedTurret, TankRoot(cooked)));
        let crew = world
            .spawn((
                CombatantId(2),
                CombatantLife(3),
                TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                },
            ))
            .id();
        let lines = world
            .run_system_once(
                move |tanks: Query<(&CombatantId, &CombatantLife, &TankKnockedOut)>,
                      launched: Query<&TankRoot, With<LaunchedTurret>>| {
                    [cooked, crew].map(|tank| {
                        KnockoutLine::read(tank, KnockoutCause::NoKeyedRound, &tanks, &launched)
                    })
                },
            )
            .unwrap();
        assert_eq!(
            (lines[0].kind, lines[0].turret),
            (KnockoutReason::Cookoff, TurretFate::Launched)
        );
        assert_eq!(
            (lines[1].kind, lines[1].turret),
            (KnockoutReason::CrewLoss, TurretFate::Mounted)
        );
        assert_eq!(
            lines[0].to_string(),
            "server: combatant 1 life 0 knocked out by ammo cookoff, turret launched (NoShooter)"
        );
        assert_eq!(
            lines[1].to_string(),
            "server: combatant 2 life 3 knocked out by crew loss, turret mounted (NoShooter)"
        );
    }

    /// The server's respawn gate and the client's countdown read one opening: a life is open from
    /// its tick on, the ticks before it count down, and an opening across the tick counter's wrap
    /// still reads as ahead. Another life's opening, or none, names nothing for this life.
    /// Fixture ticks.
    #[test]
    fn an_opening_counts_down_to_its_tick_across_the_wrap() {
        let at = |tick| {
            Scoreboard(vec![Standing {
                respawn: Respawn::Opens {
                    life: CombatantLife(1),
                    tick,
                },
                ..standing(5, 0, 1)
            }])
        };
        let (who, life) = (CombatantId(5), CombatantLife(1));
        assert_eq!(
            at(100).opening(who, life, SimTick::new(40)),
            Some(Opening::In { ticks: 60 })
        );
        assert_eq!(
            at(100).opening(who, life, SimTick::new(100)),
            Some(Opening::Open)
        );
        assert_eq!(
            at(100).opening(who, life, SimTick::new(101)),
            Some(Opening::Open)
        );
        assert_eq!(
            at(5).opening(who, life, SimTick::new(u32::MAX - 4)),
            Some(Opening::In { ticks: 10 })
        );
        assert_eq!(
            at(100).opening(who, CombatantLife(0), SimTick::new(40)),
            None
        );
        assert_eq!(
            at(100).opening(CombatantId(6), life, SimTick::new(40)),
            None
        );
        assert_eq!(
            Scoreboard(vec![standing(5, 0, 0)]).opening(who, life, SimTick::new(40)),
            None
        );
    }

    /// The wait is whole ticks rounded up, so the respawn never opens before the mode's wait has
    /// passed. Fixture waits on the game's 64 Hz tick.
    #[test]
    fn the_wait_rounds_up_to_whole_ticks() {
        let tick = Duration::from_nanos(15_625_000);
        assert_eq!(RespawnWait::new(Duration::from_secs(10), tick).ticks(), 640);
        assert_eq!(RespawnWait::new(Duration::from_millis(10), tick).ticks(), 1);
        assert_eq!(RespawnWait::new(Duration::ZERO, tick).ticks(), 0);
    }
}
