//! The Battle's match UI, game state only: a clock line always on screen (the wait for players,
//! the time until combat, the time left in it, the results' remaining time), and the scoreboard
//! (rank, name, knockouts, losses), held on Tab during combat and shown through results with the
//! authority's result marked.
//!
//! Every view reads the replicated [`BattleState`] and [`Scoreboard`] (one entity) and the
//! client's network tick ([`ShotClock`], the clock the death screen's respawn countdown reads); it
//! never advances a phase, breaks a tie or chooses a winner. The board orders standings by the
//! server's own [`rank`]. Before the first snapshot there is no Battle to show: both views hide.
//!
//! No player name rides the wire yet, so a row names its combatant by its replicated
//! [`CombatantId`]. Rendered strings stay within the bundled font's coverage
//! (`src/tests/ui_ascii.rs`). The phase rules live in `.agents/docs/design/battle-cycle.md`.

use std::fmt::Write as _;
use std::time::Duration;

use bevy::ecs::query::QuerySingleError;
use bevy::ecs::schedule::ScheduleConfigs;
use bevy::ecs::system::ScheduleSystem;
use bevy::prelude::*;

use super::battle::{BattleOutcome, BattlePhase, BattleState};
use super::score::{Scoreboard, Standing, rank};
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::overlay::{self, Overlay, Overlays};
use crate::ui_font::{PANEL_BG, TEXT, UiFonts, set_text};
use crate::{CombatantId, ShotClock};

/// Held to show the scoreboard during combat.
const BOARD_KEY: KeyCode = KeyCode::Tab;

/// What the match UI shows, from the replicated phase alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// Fill-up before its first player: the server has set no deadline.
    Waiting,
    /// Fill-up, `seconds` until combat.
    StartsIn { seconds: u32 },
    /// Combat, `seconds` of it left.
    Combat { seconds: u32 },
    /// Results with the authority's outcome, `seconds` until the next fill-up.
    Results {
        seconds: u32,
        outcome: BattleOutcome,
    },
}

impl View {
    fn of(phase: BattlePhase, now: u32, tick: Duration) -> Self {
        match phase {
            BattlePhase::FillUp { ends_at: None } => Self::Waiting,
            BattlePhase::FillUp { ends_at: Some(end) } => Self::StartsIn {
                seconds: seconds_left(end, now, tick),
            },
            BattlePhase::Combat { ends_at, .. } => Self::Combat {
                seconds: seconds_left(ends_at, now, tick),
            },
            BattlePhase::Results { ends_at, outcome } => Self::Results {
                seconds: seconds_left(ends_at, now, tick),
                outcome,
            },
        }
    }

    /// Whether the scoreboard shows: through results, and during combat while `held`.
    fn shows_board(self, held: bool) -> bool {
        match self {
            Self::Results { .. } => true,
            Self::Combat { .. } => held,
            Self::Waiting | Self::StartsIn { .. } => false,
        }
    }

    /// The combatant the authority named the winner; `None` outside results, and for a draw or no
    /// contest.
    fn winner(self) -> Option<CombatantId> {
        match self {
            Self::Results {
                outcome: BattleOutcome::Winner(combatant),
                ..
            } => Some(combatant),
            Self::Results { .. } | Self::Combat { .. } | Self::Waiting | Self::StartsIn { .. } => {
                None
            }
        }
    }

    fn clock_line(self) -> String {
        match self {
            Self::Waiting => "WAITING FOR PLAYERS".to_owned(),
            Self::StartsIn { seconds } => format!("BATTLE STARTS IN {}", clock(seconds)),
            Self::Combat { seconds } => clock(seconds),
            Self::Results { seconds, .. } => format!("RESULTS {}", clock(seconds)),
        }
    }
}

/// Whole seconds, rounded up, from the client's tick `now` to the authority's `deadline`; zero
/// once it is due, while the phase change is still in flight. Rounded up so the line reads zero
/// only once the deadline has come. DERIVED: the wrapped signed difference is exact because the
/// server's phase spans never exceed `i32::MAX` ticks (`battle::CycleRules`).
fn seconds_left(deadline: u32, now: u32, tick: Duration) -> u32 {
    let ahead = deadline.wrapping_sub(now) as i32;
    if ahead <= 0 {
        return 0;
    }
    let nanos = u128::from(ahead.unsigned_abs()) * tick.as_nanos();
    u32::try_from(nanos.div_ceil(1_000_000_000))
        .expect("an i32 tick span of a sub-second authority tick fits u32 seconds")
}

/// `m:ss`.
fn clock(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// One scoreboard row: a standing and its place. Standings [`rank`] holds equal share a place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Row {
    place: usize,
    standing: Standing,
}

/// The board's rows in [`rank`] order; equal standings keep the board's first-spawn order and
/// share the place of the first of them (1, 1, 3).
fn rows(board: &Scoreboard) -> Vec<Row> {
    let mut sorted = board.0.clone();
    sorted.sort_by(rank);
    let mut rows: Vec<Row> = Vec::with_capacity(sorted.len());
    for (index, standing) in sorted.into_iter().enumerate() {
        let place = match rows.last() {
            Some(above) if rank(&above.standing, &standing).is_eq() => above.place,
            _ => index + 1,
        };
        rows.push(Row { place, standing });
    }
    rows
}

fn name(combatant: CombatantId) -> String {
    format!("PLAYER {}", combatant.0)
}

/// One column of the scoreboard card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Column {
    Place,
    Name,
    Knockouts,
    Losses,
    /// The winner's line, in results.
    Mark,
}

impl Column {
    const ALL: [Self; 5] = [
        Self::Place,
        Self::Name,
        Self::Knockouts,
        Self::Losses,
        Self::Mark,
    ];

    /// The column's text: its header, then one line per row.
    fn text(self, rows: &[Row], winner: Option<CombatantId>) -> String {
        let mut text = String::from(match self {
            Self::Place => "RANK",
            Self::Name => "PLAYER",
            Self::Knockouts => "KNOCKOUTS",
            Self::Losses => "LOSSES",
            Self::Mark => "",
        });
        for row in rows {
            text.push('\n');
            let standing = row.standing;
            match self {
                Self::Place => write!(text, "{}", row.place).expect("a String write succeeds"),
                Self::Name => text.push_str(&name(standing.combatant)),
                Self::Knockouts => {
                    write!(text, "{}", standing.knockouts).expect("a String write succeeds");
                }
                Self::Losses => {
                    write!(text, "{}", standing.losses).expect("a String write succeeds");
                }
                Self::Mark => {
                    if winner == Some(standing.combatant) {
                        text.push_str("WINNER");
                    }
                }
            }
        }
        text
    }

    fn justify(self) -> Justify {
        match self {
            Self::Place | Self::Knockouts | Self::Losses => Justify::Right,
            Self::Name | Self::Mark => Justify::Left,
        }
    }
}

/// The board's title: the Battle, and in results the authority's outcome.
fn title(battle: BattleState) -> String {
    let number = battle.number;
    match battle.phase {
        BattlePhase::Results { outcome, .. } => match outcome {
            BattleOutcome::Winner(combatant) => {
                format!("BATTLE {number} — WINNER: {}", name(combatant))
            }
            BattleOutcome::Draw => format!("BATTLE {number} — DRAW"),
            BattleOutcome::NoContest => format!("BATTLE {number} — NO CONTEST"),
        },
        BattlePhase::FillUp { .. } | BattlePhase::Combat { .. } => format!("BATTLE {number}"),
    }
}

/// The match UI's two roots.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum MatchUi {
    /// The clock line, shown whenever a Battle has arrived.
    Clock,
    /// The scoreboard card.
    Scoreboard,
}

/// A rendered piece of the match UI.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    ClockLine,
    Title,
    Column(Column),
}

pub fn plugin(app: &mut App) {
    crate::flow::require(app, "net::battle_hud");
    app.add_systems(OnEnter(Flow::InMatch), spawn_battle_hud)
        .add_systems(Update, systems().in_set(InMatchSystems));
}

/// After `OverlaySet::Toggle`, so the board reads the frame's reconciled overlay set.
fn systems() -> ScheduleConfigs<ScheduleSystem> {
    update_battle_hud
        .after(overlay::OverlaySet::Toggle)
        .into_configs()
}

/// The match UI's spawn and systems without the match flow, for a composed schedule under test.
#[cfg(test)]
fn mount_bare(app: &mut App) {
    app.add_systems(Startup, spawn_battle_hud)
        .add_systems(Update, systems());
}

fn spawn_battle_hud(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            MatchUi::Clock,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Part::ClockLine,
                Text::default(),
                TextFont {
                    // SemiBold: a terse all-caps status line.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(TEXT),
            ));
        });
    commands
        .spawn((
            FlowScoped(Flow::InMatch),
            MatchUi::Scoreboard,
            Visibility::Hidden,
            GlobalZIndex(Overlay::SCOREBOARD_Z),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Percent(14.0),
                width: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(16.0)),
                    row_gap: Val::Px(10.0),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                BackgroundColor(PANEL_BG),
            ))
            .with_children(|card| {
                card.spawn((
                    Part::Title,
                    Text::default(),
                    TextFont {
                        // SemiBold: the card's all-caps heading.
                        font: fonts.hud.clone().into(),
                        font_size: FontSize::Px(26.0),
                        ..default()
                    },
                    TextColor(TEXT),
                ));
                card.spawn(Node {
                    column_gap: Val::Px(28.0),
                    ..default()
                })
                .with_children(|columns| {
                    for column in Column::ALL {
                        columns.spawn((
                            Part::Column(column),
                            Text::default(),
                            TextFont {
                                // Regular: dense table rows.
                                font: fonts.body.clone().into(),
                                font_size: FontSize::Px(20.0),
                                ..default()
                            },
                            TextLayout::justify(column.justify()),
                            TextColor(TEXT),
                        ));
                    }
                });
            });
        });
}

/// Rewrite and show the clock line and the board from the replicated Battle and score. The board
/// shows only while nothing captures input ([`overlay::input_blocked`]).
fn update_battle_hud(
    battles: Query<(&BattleState, &Scoreboard)>,
    clock: Option<Res<ShotClock>>,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    mut roots: Query<(&MatchUi, &mut Visibility)>,
    mut parts: Query<(&Part, &mut Text)>,
) {
    let (battle, board) = match battles.single() {
        Ok(replicated) => replicated,
        // Before the first snapshot, or between links: no Battle to show.
        Err(QuerySingleError::NoEntities(_)) => {
            for (_, mut visibility) in &mut roots {
                visibility.set_if_neq(Visibility::Hidden);
            }
            return;
        }
        Err(QuerySingleError::MultipleEntities(_)) => panic!(
            "a replica has one Battle; disconnect cleanup removes the old link's replicated \
             entities before a replacement arrives"
        ),
    };
    let now = clock
        .expect("a replicated Battle arrives only where the network clock is published")
        .0;
    let view = View::of(battle.phase, now, time.timestep());
    let board_shown =
        view.shows_board(keys.pressed(BOARD_KEY)) && !overlay::input_blocked(&overlays, true);
    for (root, mut visibility) in &mut roots {
        let shown = match root {
            MatchUi::Clock => true,
            MatchUi::Scoreboard => board_shown,
        };
        visibility.set_if_neq(if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    let board_rows = board_shown.then(|| rows(board));
    for (&part, mut text) in &mut parts {
        let value = match (part, &board_rows) {
            (Part::ClockLine, _) => view.clock_line(),
            (Part::Title, Some(_)) => title(*battle),
            (Part::Column(column), Some(rows)) => column.text(rows, view.winner()),
            // A hidden board keeps its last text; it is rewritten before it shows again.
            (Part::Title | Part::Column(_), None) => continue,
        };
        set_text(text.reborrow(), &value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::battle::LateJoin;
    use crate::net::score::Respawn;

    fn standing(combatant: u64, knockouts: u32, losses: u32) -> Standing {
        Standing {
            combatant: CombatantId(combatant),
            knockouts,
            losses,
            respawn: Respawn::NeverKnockedOut,
        }
    }

    /// The game's damage model, as the board presents it: rows follow the server's `rank` (more
    /// knockouts, then fewer losses), not the board's first-spawn order, and standings `rank`
    /// holds equal share a place in board order. Mutants: listing in board order, sorting by
    /// losses first, or numbering a tie 1, 2 all fail. Fixture standings.
    #[test]
    fn the_board_rows_follow_rank_and_ties_share_a_place() {
        let board = Scoreboard(vec![
            standing(1, 1, 0),
            standing(2, 2, 5),
            standing(3, 2, 1),
            standing(4, 0, 0),
            standing(5, 2, 1),
        ]);
        let listed: Vec<(usize, u64)> = rows(&board)
            .iter()
            .map(|row| (row.place, row.standing.combatant.0))
            .collect();
        assert_eq!(listed, [(1, 3), (1, 5), (3, 2), (4, 1), (5, 4)]);
        assert!(rows(&Scoreboard::default()).is_empty());
    }

    /// The time left counts from the replicated deadline on the client's tick: whole seconds
    /// rounded up, zero from the deadline on, and still counting across the tick counter's wrap.
    /// Mutants: rounding down (reading 0:00 a tick early), counting from the phase start, or an
    /// unsigned difference (a due deadline reading as ~2^32 ticks ahead) all fail. Fixture
    /// deadlines on the game's 64 Hz tick.
    #[test]
    fn the_time_left_counts_from_the_replicated_deadline() {
        let tick = Duration::from_nanos(15_625_000);
        assert_eq!(seconds_left(100, 40, tick), 1);
        assert_eq!(seconds_left(64 * 900, 0, tick), 900);
        assert_eq!(seconds_left(64 * 900, 1, tick), 900);
        assert_eq!(seconds_left(64 * 900, 64, tick), 899);
        assert_eq!(seconds_left(100, 100, tick), 0);
        assert_eq!(seconds_left(100, 101, tick), 0);
        assert_eq!(seconds_left(63, u32::MAX - 64, tick), 2);
        assert_eq!(clock(900), "15:00");
        assert_eq!(clock(59), "0:59");
        assert_eq!(clock(0), "0:00");
    }

    fn app(fixed: Duration) -> App {
        let mut app = App::new();
        app.insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        })
        .insert_resource(Time::<Fixed>::from_duration(fixed))
        .init_resource::<ShotClock>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Overlays>();
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare_in_match(true);
        mount_bare(&mut app);
        app
    }

    fn shown(app: &mut App, root: MatchUi) -> bool {
        let world = app.world_mut();
        let visibility = world
            .query::<(&MatchUi, &Visibility)>()
            .iter(world)
            .find(|(each, _)| **each == root)
            .expect("both roots spawned")
            .1;
        *visibility != Visibility::Hidden
    }

    fn text(app: &mut App, part: Part) -> String {
        let world = app.world_mut();
        world
            .query::<(&Part, &Text)>()
            .iter(world)
            .find(|(each, _)| **each == part)
            .expect("every part spawned")
            .1
            .0
            .clone()
    }

    /// The clock line, `None` while hidden.
    fn clock_line(app: &mut App) -> Option<String> {
        shown(app, MatchUi::Clock).then(|| text(app, Part::ClockLine))
    }

    /// Players rely on the view following the replicated phase alone: no clock and no board
    /// before the first snapshot; the wait, then the time until combat in fill-up, with no board
    /// even on Tab; the time left in combat, the board only while Tab is held and nothing
    /// captures input; the same board through results without Tab, the authority's winner on its
    /// own line, or the draw in the title. Mutants: showing the board in fill-up, needing Tab in
    /// results, drawing over the menu, or marking the board's top row instead of the authority's
    /// winner all fail. Fixture ticks of half a second.
    #[test]
    fn the_view_follows_the_replicated_phase() {
        let mut app = app(Duration::from_millis(500));
        app.update();
        assert_eq!(clock_line(&mut app), None);
        assert!(!shown(&mut app, MatchUi::Scoreboard));

        let replica = app
            .world_mut()
            .spawn((
                BattleState::default(),
                Scoreboard(vec![standing(7, 1, 0), standing(9, 3, 2)]),
            ))
            .id();
        let set_phase = |app: &mut App, phase| {
            app.world_mut()
                .get_mut::<BattleState>(replica)
                .expect("the replica")
                .phase = phase;
        };
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BOARD_KEY);
        app.update();
        assert_eq!(clock_line(&mut app).as_deref(), Some("WAITING FOR PLAYERS"));
        assert!(!shown(&mut app, MatchUi::Scoreboard));

        app.world_mut().resource_mut::<ShotClock>().0 = 40;
        set_phase(&mut app, BattlePhase::FillUp { ends_at: Some(100) });
        app.update();
        assert_eq!(
            clock_line(&mut app).as_deref(),
            Some("BATTLE STARTS IN 0:30")
        );
        assert!(!shown(&mut app, MatchUi::Scoreboard));

        set_phase(
            &mut app,
            BattlePhase::Combat {
                ends_at: 40 + 2 * 900,
                late_join: LateJoin::Closed,
            },
        );
        app.update();
        assert_eq!(clock_line(&mut app).as_deref(), Some("15:00"));
        assert!(
            shown(&mut app, MatchUi::Scoreboard),
            "Tab held in combat shows the board"
        );
        assert_eq!(text(&mut app, Part::Title), "BATTLE 1");
        let column = |app: &mut App, which| text(app, Part::Column(which));
        assert_eq!(column(&mut app, Column::Place), "RANK\n1\n2");
        assert_eq!(column(&mut app, Column::Name), "PLAYER\nPLAYER 9\nPLAYER 7");
        assert_eq!(column(&mut app, Column::Knockouts), "KNOCKOUTS\n3\n1");
        assert_eq!(column(&mut app, Column::Losses), "LOSSES\n2\n0");
        assert_eq!(column(&mut app, Column::Mark), "\n\n");
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::Menu, true);
        app.update();
        assert!(
            !shown(&mut app, MatchUi::Scoreboard),
            "the menu hides a held board"
        );
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::Menu, false);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(BOARD_KEY);
        app.update();
        assert!(
            !shown(&mut app, MatchUi::Scoreboard),
            "combat shows the board only on Tab"
        );

        // The authority's winner is the lower-ranked fixture combatant: the board marks the
        // result it was sent, never one it would derive.
        set_phase(
            &mut app,
            BattlePhase::Results {
                ends_at: 70,
                outcome: BattleOutcome::Winner(CombatantId(7)),
            },
        );
        app.update();
        assert_eq!(clock_line(&mut app).as_deref(), Some("RESULTS 0:15"));
        assert!(
            shown(&mut app, MatchUi::Scoreboard),
            "results show the board without Tab"
        );
        assert_eq!(text(&mut app, Part::Title), "BATTLE 1 — WINNER: PLAYER 7");
        assert_eq!(column(&mut app, Column::Mark), "\n\nWINNER");

        set_phase(
            &mut app,
            BattlePhase::Results {
                ends_at: 70,
                outcome: BattleOutcome::Draw,
            },
        );
        app.update();
        assert_eq!(text(&mut app, Part::Title), "BATTLE 1 — DRAW");
        assert_eq!(column(&mut app, Column::Mark), "\n\n");

        app.world_mut().entity_mut(replica).despawn();
        app.update();
        assert_eq!(clock_line(&mut app), None);
        assert!(!shown(&mut app, MatchUi::Scoreboard));
    }
}
