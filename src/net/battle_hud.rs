//! The Battle's match UI, game state only: a clock line always on screen (the wait for players,
//! the time until combat, the time left in it, the results' remaining time), and the scoreboard
//! (rank, name, knockouts, losses), held on Tab during combat and shown through results with the
//! authority's result marked.
//!
//! Every view reads the replicated [`BattleState`] and [`Scoreboard`] (one entity) and the
//! client's shared presentation tick ([`InterpolationTimeline`]); it
//! never advances a phase, breaks a tie or chooses a winner. The board orders standings by the
//! server's own [`rank`]. Before the first snapshot, or while the shared presentation clock is
//! synchronizing, there is no timed Battle view to show: both views hide.
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
use crate::CombatantId;
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::overlay::{self, Overlays};
use crate::ui_font::{MODAL_CARD_BG, TEXT, UiFonts, set_text};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::NetworkTimeline;

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

/// Whole seconds, rounded up, from the presentation tick `now` to the authority's `deadline`; zero
/// once it is due, while the phase change is still in flight. Rounded up so the line reads zero
/// only once the deadline has come. DERIVED: the wrapped signed difference is exact because the
/// server's phase spans never exceed `i32::MAX` ticks (`battle::CycleRules`).
fn seconds_left(deadline: crate::sim_tick::SimTick, now: u32, tick: Duration) -> u32 {
    let ahead = crate::sim_tick::SimTick::new(now).remaining_until(deadline);
    let nanos = u128::from(ahead) * tick.as_nanos();
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
            Self::Knockouts => "Kills",
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
    ScrollHint,
    Column(Column),
}

/// The table scrolls as one unit so every standing's columns stay aligned.
#[derive(Component)]
struct BoardViewport;

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
            crate::hud::HudDock::Top,
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
            crate::hud::HudLayer::Modal,
            Node {
                position_type: PositionType::Absolute,
                // The shared HUD layout fills the region left clear by visible docked panels.
                overflow: Overflow::clip(),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                align_items: AlignItems::Start,
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    max_height: Val::Percent(100.0),
                    max_width: Val::Percent(100.0),
                    min_height: Val::Px(0.0),
                    min_width: Val::Px(0.0),
                    padding: UiRect::all(Val::Px(8.0)),
                    row_gap: Val::Px(4.0),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    ..default()
                },
                // Modal HUD text needs opaque backing above world-anchored labels.
                BackgroundColor(MODAL_CARD_BG),
            ))
            .with_children(|card| {
                card.spawn((
                    Part::Title,
                    Text::default(),
                    TextFont {
                        // SemiBold: the card's all-caps heading.
                        font: fonts.hud.clone().into(),
                        font_size: FontSize::Px(18.0),
                        ..default()
                    },
                    TextColor(TEXT),
                ));
                card.spawn((
                    BoardViewport,
                    ScrollPosition::default(),
                    Node {
                        column_gap: Val::Px(12.0),
                        align_items: AlignItems::Start,
                        max_height: Val::Percent(100.0),
                        min_height: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        min_width: Val::Px(0.0),
                        overflow: Overflow::scroll(),
                        ..default()
                    },
                ))
                .with_children(|columns| {
                    for column in Column::ALL {
                        columns.spawn((
                            Part::Column(column),
                            Text::default(),
                            TextFont {
                                // Regular: dense table rows.
                                font: fonts.body.clone().into(),
                                font_size: FontSize::Px(14.0),
                                ..default()
                            },
                            bevy::text::LineHeight::Px(16.0),
                            TextLayout::justify(column.justify()).with_no_wrap(),
                            Node {
                                flex_shrink: 0.0,
                                ..default()
                            },
                            TextColor(TEXT),
                        ));
                    }
                });
                card.spawn((
                    Part::ScrollHint,
                    Text::new("PAGE UP/DOWN: ROWS  LEFT/RIGHT: COLUMNS"),
                    TextFont {
                        font: fonts.body.clone().into(),
                        font_size: FontSize::Px(12.0),
                        ..default()
                    },
                    TextColor(TEXT),
                ));
            });
        });
}

/// No usable Battle view also clears its table position, so a replacement link cannot inherit
/// the previous link's scrolled roster.
fn hide_views(
    roots: &mut Query<(&MatchUi, &mut Visibility)>,
    viewports: &mut Query<(&ComputedNode, &mut ScrollPosition), With<BoardViewport>>,
) {
    for (_, mut visibility) in roots.iter_mut() {
        visibility.set_if_neq(Visibility::Hidden);
    }
    for (_, mut scroll) in viewports.iter_mut() {
        if scroll.0 != Vec2::ZERO {
            scroll.0 = Vec2::ZERO;
        }
    }
}

/// Rewrite and show the clock line and the board from the replicated Battle and score. The board
/// shows only while nothing captures input ([`overlay::input_blocked`]).
#[expect(
    clippy::too_many_arguments,
    reason = "the replicated view also owns its table's scroll position"
)]
fn update_battle_hud(
    battles: Query<(&BattleState, &Scoreboard)>,
    clock: Option<Res<InterpolationTimeline>>,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    mut roots: Query<(&MatchUi, &mut Visibility)>,
    mut parts: Query<(&Part, &mut Text)>,
    mut viewports: Query<(&ComputedNode, &mut ScrollPosition), With<BoardViewport>>,
) {
    let (battle, board) = match battles.single() {
        Ok(replicated) => replicated,
        // Before the first snapshot, or between links: no Battle to show.
        Err(QuerySingleError::NoEntities(_)) => {
            hide_views(&mut roots, &mut viewports);
            return;
        }
        Err(QuerySingleError::MultipleEntities(_)) => panic!(
            "a replica has one Battle; disconnect cleanup removes the old link's replicated \
             entities before a replacement arrives"
        ),
    };
    let clock = clock.expect(
        "a replicated Battle arrives only where the shared presentation clock is installed",
    );
    if !clock.is_synced() {
        // The first snapshot, or a reconnect, can precede a usable presentation-time estimate.
        // Do not invent a deadline offset while its timeline is still synchronizing.
        hide_views(&mut roots, &mut viewports);
        return;
    }
    let now = clock.tick().0;
    let view = View::of(battle.phase, now, time.timestep());
    let board_shown =
        view.shows_board(keys.pressed(BOARD_KEY)) && !overlay::input_blocked(&overlays, true);
    for (node, mut scroll) in &mut viewports {
        if !board_shown {
            if scroll.0 != Vec2::ZERO {
                scroll.0 = Vec2::ZERO;
            }
            continue;
        }
        let direction = Vec2::new(
            u8::from(keys.just_pressed(KeyCode::ArrowRight)) as f32
                - u8::from(keys.just_pressed(KeyCode::ArrowLeft)) as f32,
            u8::from(keys.just_pressed(KeyCode::PageDown)) as f32
                - u8::from(keys.just_pressed(KeyCode::PageUp)) as f32,
        );
        if direction != Vec2::ZERO {
            let page = node.size * node.inverse_scale_factor;
            let limit =
                ((node.content_size - node.size) * node.inverse_scale_factor).max(Vec2::ZERO);
            let next = (scroll.0 + direction * page).clamp(Vec2::ZERO, limit);
            if scroll.0 != next {
                scroll.0 = next;
            }
        }
    }
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
            (Part::ScrollHint, _) => continue,
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
    use crate::overlay::Overlay;
    use lightyear::prelude::TimelineSync;

    /// FIXTURE DATA: viewport sizes/scales and a synthetic populated status panel/roster.
    /// Uses the shipped spawners, fonts and Bevy layout; no simulated tank or link is needed.
    fn hud_layout_app(width: u32, height: u32, scale: f32) -> App {
        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
            resolution: bevy::window::WindowResolution::new(width, height),
            visible: false,
            ..default()
        })))
        .add_plugins(crate::ui_font::plugin)
        .insert_resource(UiScale(scale))
        .add_systems(
            Startup,
            (
                spawn_battle_hud,
                crate::crew_ui::spawn_status_panel,
                crate::crew_ui::spawn_crew_bar,
                |mut commands: Commands| {
                    commands.spawn(Camera2d);
                },
            ),
        );
        app.set_error_handler(bevy::ecs::error::panic);
        crate::hud::mount_layout_test(&mut app);
        crate::drive_hud::mount_layout_test(&mut app);
        super::super::debug_hud::mount_layout_test(&mut app);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app.update();
        let roster: Vec<Row> = (1..=30)
            .map(|i| Row {
                place: i as usize,
                standing: standing(i, 0, 0),
            })
            .collect();
        let world = app.world_mut();
        for (part, mut text) in world.query::<(&Part, &mut Text)>().iter_mut(world) {
            text.0 = match part {
                Part::ClockLine => "14:55".to_owned(),
                Part::Title => "BATTLE 1".to_owned(),
                Part::Column(column) => column.text(&roster, None),
                Part::ScrollHint => "PAGE UP/DOWN: ROWS  LEFT/RIGHT: COLUMNS".to_owned(),
            };
        }
        let status = world
            .query_filtered::<Entity, With<crate::crew_ui::StatusPanelText>>()
            .single(world)
            .unwrap();
        world.get_mut::<Text>(status).unwrap().0 =
            "ALIVE\nCrew 5/5\nDrive 100%   Gunner 100%\nWeapons: Primary READY Secondary READY\nPrimary ammunition: fixture\nF - repair modules\nModule A READY\nModule B READY\nModule C READY\nModule D READY\nModule E READY\nModule F READY".to_owned();
        for (_, mut text) in world.query_filtered::<
            (&crate::hud::HudDock, &mut Text), Without<crate::crew_ui::StatusPanelText>
        >().iter_mut(world) {
            text.0 = "FIXTURE FIXED HUD".to_owned();
        }
        for label in crate::hud::labels_for_test(world) {
            world.get_mut::<Text>(label).unwrap().0 = "FIXTURE WORLD LABEL".to_owned();
        }
        for mut visibility in world.query::<&mut Visibility>().iter_mut(world) {
            *visibility = Visibility::Visible;
        }
        settle_hud(&mut app);
        app
    }

    /// Wait for both real fonts and stable geometry; font IO can finish on different frames.
    fn settle_hud(app: &mut App) {
        let start = std::time::Instant::now();
        let mut previous = Vec::new();
        let mut stable = 0;
        loop {
            app.update();
            let world = app.world_mut();
            let fonts = world.resource::<UiFonts>();
            let server = world.resource::<AssetServer>();
            let loaded = server.load_state(&fonts.hud).is_loaded()
                && server.load_state(&fonts.body).is_loaded();
            let geometry: Vec<_> = world
                .query::<(Entity, &ComputedNode, &UiGlobalTransform)>()
                .iter(world)
                .map(|(entity, node, transform)| (entity, node.size(), transform.translation))
                .collect();
            if loaded && geometry == previous {
                stable += 1;
            } else {
                stable = 0;
            }
            if stable >= 4 {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(30),
                "HUD fonts/layout did not settle"
            );
            previous = geometry;
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn board_entities(world: &mut World) -> (Entity, Entity) {
        let (root, children) = world
            .query::<(Entity, &MatchUi, &Children)>()
            .iter(world)
            .find_map(|(entity, kind, children)| {
                (*kind == MatchUi::Scoreboard).then_some((entity, children))
            })
            .unwrap();
        (root, children[0])
    }

    fn hud_rect(world: &World, entity: Entity) -> Rect {
        Rect::from_center_size(
            world.get::<UiGlobalTransform>(entity).unwrap().translation,
            world.get::<ComputedNode>(entity).unwrap().size(),
        )
    }

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.min.x < b.max.x && a.max.x > b.min.x && a.min.y < b.max.y && a.max.y > b.min.y
    }

    /// The Tab board leaves every visible dock readable at different UI scales and after text
    /// growth. Hiding a dock releases its space. Players rely on layout, not a fixed-panel size
    /// guessed from one rig's readout; the table must retain a nonempty scrollable viewport.
    #[test]
    fn modal_board_is_placed_clear_of_the_fixed_status_panel() {
        for (width, height, scale) in [(1280, 720, 2.0), (1280, 720, 1.0), (1920, 1080, 1.5)] {
            let mut app = hud_layout_app(width, height, scale);
            let (root, card) = board_entities(app.world_mut());
            let status = {
                let world = app.world_mut();
                world
                    .query_filtered::<Entity, With<crate::crew_ui::StatusPanelText>>()
                    .single(world)
                    .unwrap()
            };
            let before = hud_rect(app.world(), card);
            assert_modal_clear(app.world_mut(), card);
            app.world_mut()
                .get_mut::<Text>(status)
                .unwrap()
                .0
                .push_str("\nFixture reserve A\nFixture reserve B");
            settle_hud(&mut app);
            let after = hud_rect(app.world(), card);
            assert_modal_clear(app.world_mut(), card);
            let reserved = app.world().get::<ComputedNode>(root).unwrap().size();
            {
                let world = app.world_mut();
                for (dock, mut visibility) in world
                    .query::<(&crate::hud::HudDock, &mut Visibility)>()
                    .iter_mut(world)
                {
                    if matches!(dock, crate::hud::HudDock::Right) {
                        *visibility = Visibility::Hidden;
                    }
                }
            }
            settle_hud(&mut app);
            assert!(
                {
                    let size = app.world().get::<ComputedNode>(root).unwrap().size();
                    size.x * size.y > reserved.x * reserved.y
                },
                "hiding diagnostics increases the clear region"
            );
            assert_modal_clear(app.world_mut(), card);
            eprintln!(
                "MEASURED {width}x{height} scale {scale}: status growth board top {} -> {} px; final board {:?}",
                before.min.y,
                after.min.y,
                hud_rect(app.world(), card)
            );
        }
    }

    /// The narrow-window roster must keep every score column reachable without wrapping any
    /// standing onto extra lines. Players on tall windows rely on horizontal paging rather than
    /// clipped columns. FIXTURE DATA: a narrow window, diagnostics shown.
    #[test]
    fn a_narrow_board_keeps_every_column_in_its_scroll_content() {
        let mut app = hud_layout_app(960, 1080, 2.0);
        // FIXTURE DATA: full-width ids and a results winner keep every rendered column wide,
        // independently of compact typography. An empty combat winner column has no ink.
        {
            let roster: Vec<Row> = (0..30)
                .map(|i| Row {
                    place: 1,
                    standing: standing(u64::MAX - i, 0, 0),
                })
                .collect();
            let world = app.world_mut();
            for (part, mut text) in world.query::<(&Part, &mut Text)>().iter_mut(world) {
                if let Part::Column(column) = part {
                    text.0 = column.text(&roster, Some(CombatantId(u64::MAX)));
                }
            }
        }
        settle_hud(&mut app);
        let world = app.world_mut();
        let (entity, viewport) = world
            .query_filtered::<(Entity, &ComputedNode), With<BoardViewport>>()
            .single(world)
            .unwrap();
        let bounds = hud_rect(world, entity);
        assert!(
            viewport.content_size.x > viewport.size.x,
            "fixture must need horizontal scrolling"
        );
        let right = bounds.min.x + viewport.content_size.x;
        for (entity, part, layout) in world.query::<(Entity, &Part, &TextLayout)>().iter(world) {
            if matches!(part, Part::Column(_)) {
                assert_eq!(layout.linebreak, bevy::text::LineBreak::NoWrap);
                let column = hud_rect(world, entity);
                assert!(
                    column.max.x <= right + 1.0,
                    "column {part:?} {column:?} lies outside reachable right edge {right} (viewport {bounds:?})"
                );
            }
        }
    }

    /// The compact board preserves one logical row per standing. The shipped-size fixtures pin
    /// the capacity: a large window fits the full Battle; the small high-scale target retains
    /// scrolling. FIXTURE DATA: thirty standings and the two viewport/scale combinations below.
    #[test]
    fn compact_board_names_the_rows_that_fit_at_both_owner_fixtures() {
        for (width, height, scale, expected_rows) in [(1280, 720, 2.0, 6), (1920, 1080, 1.0, 30)] {
            let mut app = hud_layout_app(width, height, scale);
            let world = app.world_mut();
            let viewport = world
                .query_filtered::<&ComputedNode, With<BoardViewport>>()
                .single(world)
                .unwrap();
            let viewport_height = viewport.size.y * viewport.inverse_scale_factor;
            let rows = ((viewport_height / 16.0).floor() as usize)
                .saturating_sub(1)
                .min(30);
            eprintln!(
                "MEASURED compact board {width}x{height} scale {scale}: {rows} standings fit, viewport {viewport_height} logical px"
            );
            assert_eq!(
                rows, expected_rows,
                "MEASURED fixture capacity must remain readable"
            );
            assert!(
                viewport.content_size.y * viewport.inverse_scale_factor >= 31.0 * 16.0,
                "paging must reach every line of the real laid-out roster"
            );
            // The combat board's unused winner column is all blank lines and has no ink box.
            for (part, text, layout) in world
                .query::<(&Part, &Text, &bevy::text::TextLayoutInfo)>()
                .iter(world)
            {
                if matches!(part, Part::Column(_)) && !text.0.trim().is_empty() {
                    assert!(
                        (layout.size.y - 31.0 * 16.0).abs() < 1.0,
                        "{part:?}: one header and thirty rendered lines, got {} logical px",
                        layout.size.y
                    );
                    assert_eq!(
                        layout.glyphs.last().unwrap().line_index,
                        30,
                        "soft wrapping must not add standing lines"
                    );
                }
            }
        }
    }

    /// The game's damage-model view: the live inset is a fixed right panel, so Tab leaves it
    /// readable. Its public rig marker is the integration seam; world rig entities never dock.
    /// FIXTURE DATA: the corner's initial dimensions in the small high-scale target.
    #[test]
    fn the_live_corner_reserves_its_laid_out_panel_for_the_board() {
        let mut app = hud_layout_app(1280, 720, 2.0);
        // FIXTURE DATA: the inset coexists with the short vitals state in this clearance case.
        {
            let world = app.world_mut();
            for mut text in world
                .query_filtered::<&mut Text, With<crate::crew_ui::StatusPanelText>>()
                .iter_mut(world)
            {
                text.0 =
                    "ALIVE\nCrew 5/5\nDrive 100%\nWeapons READY\nAmmunition fixture".to_owned();
            }
        }
        settle_hud(&mut app);
        let corner = app
            .world_mut()
            .spawn((
                crate::damage_cam::pen_cam::CornerRig,
                Node {
                    position_type: PositionType::Absolute,
                    top: Val::Px(16.0),
                    right: Val::Px(16.0),
                    width: Val::Px(384.0),
                    height: Val::Px(240.0),
                    ..default()
                },
                GlobalZIndex(Overlay::PEN_CAM_CORNER_Z),
                Visibility::Visible,
            ))
            .id();
        settle_hud(&mut app);
        let (_, card) = board_entities(app.world_mut());
        assert!(matches!(
            app.world().get::<crate::hud::HudDock>(corner),
            Some(crate::hud::HudDock::Right)
        ));
        assert_eq!(
            app.world().get::<GlobalZIndex>(corner).unwrap().0,
            Overlay::PEN_CAM_CORNER_Z
        );
        assert_modal_clear(app.world_mut(), card);
    }

    fn assert_modal_clear(world: &mut World, card: Entity) {
        let board = hud_rect(world, card);
        assert!(board.width() > 0.0 && board.height() > 0.0);
        for (entity, _, node, visibility) in world
            .query::<(Entity, &crate::hud::HudDock, &ComputedNode, &Visibility)>()
            .iter(world)
        {
            if *visibility != Visibility::Hidden && node.size().min_element() > 0.0 {
                assert!(
                    !overlaps(board, hud_rect(world, entity)),
                    "MEASURED board {board:?} overlaps fixed panel {:?}",
                    hud_rect(world, entity)
                );
            }
        }
        let viewport = world
            .query_filtered::<&ComputedNode, With<BoardViewport>>()
            .single(world)
            .unwrap();
        assert!(
            viewport.size().y > 0.0 && viewport.content_size.y > 0.0,
            "the populated roster retains a nonempty viewport"
        );
    }

    /// Players must read modal text without world labels bleeding through it. Ordering is a
    /// semantic HUD rule, independent of which plugin happened to spawn its roots first.
    #[test]
    fn world_labels_draw_below_fixed_hud_below_modal_boards() {
        let mut app = hud_layout_app(1280, 720, 2.0);
        let world = app.world_mut();
        let (board, card) = board_entities(world);
        let status = world
            .query_filtered::<Entity, With<crate::crew_ui::StatusPanelText>>()
            .single(world)
            .unwrap();
        let label = crate::hud::labels_for_test(world)[0];
        let z = |entity| world.get::<GlobalZIndex>(entity).map_or(0, |z| z.0);
        assert!(
            z(label) < z(status) && z(status) < z(board),
            "world label {}, fixed HUD {}, modal {}",
            z(label),
            z(status),
            z(board)
        );
        let stack = |entity| world.get::<bevy::ui::ComputedStackIndex>(entity).unwrap().0;
        assert!(stack(label) < stack(status) && stack(status) < stack(card));
        assert_eq!(
            world.get::<BackgroundColor>(card).unwrap().0.alpha(),
            1.0,
            "a modal backing must obscure lower HUD layers"
        );
    }

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
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(100), 40, tick),
            1
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(64 * 900), 0, tick),
            900
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(64 * 900), 1, tick),
            900
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(64 * 900), 64, tick),
            899
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(100), 100, tick),
            0
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(100), 101, tick),
            0
        );
        assert_eq!(
            seconds_left(crate::sim_tick::SimTick::new(63), u32::MAX - 64, tick),
            2
        );
        assert_eq!(clock(900), "15:00");
        assert_eq!(clock(59), "0:59");
        assert_eq!(clock(0), "0:00");
    }

    /// Players can reach standings beyond the viewport while the board is shown. Fixture layout
    /// is in physical pixels with a scaled UI; paging uses logical pixels and stops at the end.
    /// A menu suppresses paging and resets the viewport rather than scrolling an obscured board.
    #[test]
    fn the_board_pages_all_columns_together_only_while_shown() {
        let mut app = app(Duration::from_millis(500));
        app.world_mut()
            .spawn((BattleState::combat_fixture(), Scoreboard::default()));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BOARD_KEY);
        app.update();
        let viewport = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<BoardViewport>>()
                .single(world)
                .unwrap()
        };
        app.world_mut().entity_mut(viewport).insert(ComputedNode {
            size: Vec2::new(800.0, 600.0),
            content_size: Vec2::new(2000.0, 2000.0),
            inverse_scale_factor: 0.5,
            ..default()
        });
        for expected in [400.0, 600.0, 600.0] {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::ArrowRight);
            keys.press(KeyCode::ArrowRight);
            app.update();
            assert_eq!(
                app.world().get::<ScrollPosition>(viewport).unwrap().0.x,
                expected
            );
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowRight);
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::ArrowLeft);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().0.x,
            200.0
        );
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .release(KeyCode::ArrowLeft);
        for expected in [300.0, 600.0, 700.0, 700.0] {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::PageDown);
            keys.press(KeyCode::PageDown);
            app.update();
            assert_eq!(
                app.world().get::<ScrollPosition>(viewport).unwrap().0.y,
                expected
            );
        }
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::PageUp);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().0.y,
            400.0
        );
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::Menu, true);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().0.y,
            0.0
        );
    }

    /// A replicated deadline is not meaningful against an unsynchronized cursor. Players must
    /// see neither a fabricated countdown nor an early results board while the clock settles.
    #[test]
    fn the_view_waits_for_its_shared_presentation_clock() {
        let mut app = app(Duration::from_millis(500));
        let replica = app
            .world_mut()
            .spawn((
                BattleState {
                    number: 1,
                    phase: BattlePhase::Results {
                        ends_at: crate::sim_tick::SimTick::new(100),
                        outcome: BattleOutcome::Draw,
                    },
                },
                Scoreboard::default(),
            ))
            .id();
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(false);
        app.update();
        assert_eq!(clock_line(&mut app), None);
        assert!(!shown(&mut app, MatchUi::Scoreboard));
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);
        app.update();
        assert_eq!(clock_line(&mut app).as_deref(), Some("RESULTS 0:50"));
        assert!(shown(&mut app, MatchUi::Scoreboard));
        let viewport = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<BoardViewport>>()
                .single(world)
                .unwrap()
        };
        app.world_mut()
            .get_mut::<ScrollPosition>(viewport)
            .unwrap()
            .0
            .y = 250.0;
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(false);
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().0.y,
            0.0
        );
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);
        app.world_mut()
            .get_mut::<ScrollPosition>(viewport)
            .unwrap()
            .0
            .y = 250.0;
        app.world_mut().entity_mut(replica).despawn();
        app.update();
        assert_eq!(
            app.world().get::<ScrollPosition>(viewport).unwrap().0.y,
            0.0
        );
    }

    fn app(fixed: Duration) -> App {
        let mut app = App::new();
        app.insert_resource(UiFonts {
            hud: Handle::default(),
            body: Handle::default(),
        })
        .insert_resource(Time::<Fixed>::from_duration(fixed))
        .init_resource::<InterpolationTimeline>()
        .init_resource::<crate::ShotClock>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Overlays>();
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare_in_match(true);
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);
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

        // Fixture input time leads presentation time. The match display must follow the latter.
        app.world_mut().resource_mut::<crate::ShotClock>().0 = 400;
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_now(lightyear::core::time::TickInstant::from(
                lightyear::core::tick::Tick(40),
            ));
        set_phase(
            &mut app,
            BattlePhase::FillUp {
                ends_at: Some(crate::sim_tick::SimTick::new(100)),
            },
        );
        app.update();
        assert_eq!(
            clock_line(&mut app).as_deref(),
            Some("BATTLE STARTS IN 0:30")
        );
        assert!(!shown(&mut app, MatchUi::Scoreboard));

        set_phase(
            &mut app,
            BattlePhase::Combat {
                ends_at: crate::sim_tick::SimTick::new(40 + 2 * 900),
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
        assert_eq!(column(&mut app, Column::Knockouts), "Kills\n3\n1");
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
                ends_at: crate::sim_tick::SimTick::new(70),
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
                ends_at: crate::sim_tick::SimTick::new(70),
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
