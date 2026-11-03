//! The Battle's match UI, game state only: a clock line in combat and results, and the scoreboard
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
use bevy::ecs::system::{ScheduleSystem, SystemParam};
use bevy::prelude::*;

use super::battle::{BattleOutcome, BattlePhase, BattleState};
use super::score::{Scoreboard, Standing, rank};
use crate::CombatantId;
use crate::flow::{Flow, FlowScoped, InMatchSystems};
use crate::hud::ModalExtent;
use crate::overlay::{self, Overlays};
use crate::ui_font::{MODAL_CARD_BG, TEXT, UiFonts, set_text};
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::NetworkTimeline;

/// Held to show the scoreboard during combat.
const BOARD_KEY: KeyCode = KeyCode::Tab;

/// One table line, logical px: the columns' line height, the unit pages count whole standings
/// in. ASSUMED typography: 14 px Barlow Condensed body on a 16 px line.
const ROW_PX: f32 = 16.0;

/// What the match UI shows, from the replicated phase alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    /// Combat, `seconds` of it left.
    Combat { seconds: u32 },
    /// Results with the authority's outcome, `seconds` until the next Battle.
    Results {
        seconds: u32,
        outcome: BattleOutcome,
    },
}

impl View {
    fn of(phase: BattlePhase, now: u32, tick: Duration) -> Option<Self> {
        Some(match phase {
            BattlePhase::WaitingForPlayer => return None,
            BattlePhase::Combat { ends_at, .. } => Self::Combat {
                seconds: seconds_left(ends_at, now, tick),
            },
            BattlePhase::Results { ends_at, outcome } => Self::Results {
                seconds: seconds_left(ends_at, now, tick),
                outcome,
            },
        })
    }

    /// Whether the scoreboard shows: through results, and during combat while `held`.
    fn shows_board(self, held: bool) -> bool {
        match self {
            Self::Results { .. } => true,
            Self::Combat { .. } => held,
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
            Self::Results { .. } | Self::Combat { .. } => None,
        }
    }

    fn clock_line(self) -> String {
        match self {
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
            Self::Knockouts => "KILLS",
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
        BattlePhase::WaitingForPlayer | BattlePhase::Combat { .. } => format!("BATTLE {number}"),
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

/// The board's card: its title, table and hint, sized to its content within the placed region.
#[derive(Component)]
struct BoardCard;

/// The table: its columns scroll sideways as one unit, so every standing stays aligned, and list
/// whole standings from `first` under their headers, so no page cuts a row or loses a label.
#[derive(Component, Default)]
struct BoardViewport {
    first: usize,
}

/// The board's laid-out nodes: its placed region, its card and its table.
#[derive(SystemParam)]
struct BoardNodes<'w, 's> {
    region: Query<'w, 's, (&'static ComputedNode, &'static mut ModalExtent), With<MatchUi>>,
    card: Query<'w, 's, &'static ComputedNode, With<BoardCard>>,
    table: Query<
        'w,
        's,
        (
            &'static ComputedNode,
            &'static mut ScrollPosition,
            &'static mut BoardViewport,
        ),
    >,
}

/// Whole standings that fit under the header line in `height` logical px of table.
fn standings_that_fit(height: f32) -> usize {
    ((height / ROW_PX).floor().max(0.0) as usize).saturating_sub(1)
}

/// The first standing listed after `step` pages (Page Down positive) through `total` standings,
/// `capacity` to a page: whole pages, and a last page as full as the roster allows.
fn page(first: usize, capacity: usize, total: usize, step: i8) -> usize {
    let moved = match step.signum() {
        1 => first.saturating_add(capacity),
        -1 => first.saturating_sub(capacity),
        _ => first,
    };
    moved.min(total.saturating_sub(capacity))
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
            crate::hud::HudDock::Fixed,
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
            ModalExtent::default(),
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
                BoardCard,
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
                    BoardViewport::default(),
                    ScrollPosition::default(),
                    Node {
                        column_gap: Val::Px(12.0),
                        align_items: AlignItems::Start,
                        max_height: Val::Percent(100.0),
                        min_height: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        min_width: Val::Px(0.0),
                        // Rows page as whole lines into the height the region leaves.
                        overflow: Overflow {
                            x: OverflowAxis::Scroll,
                            y: OverflowAxis::Clip,
                        },
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
                            bevy::text::LineHeight::Px(ROW_PX),
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
/// the previous link's paged roster.
fn hide_views(roots: &mut Query<(&MatchUi, &mut Visibility)>, nodes: &mut BoardNodes) {
    for (_, mut visibility) in roots.iter_mut() {
        visibility.set_if_neq(Visibility::Hidden);
    }
    for (_, mut scroll, mut table) in &mut nodes.table {
        rewind(&mut scroll, &mut table);
    }
}

/// Rewinds through `Mut`, so an already rewound table is not marked changed.
fn rewind(scroll: &mut Mut<ScrollPosition>, table: &mut Mut<BoardViewport>) {
    if scroll.0 != Vec2::ZERO {
        scroll.0 = Vec2::ZERO;
    }
    if table.first != 0 {
        table.first = 0;
    }
}

/// Rewrite and show the clock line and the board from the replicated Battle and score. The board
/// shows only while nothing captures input ([`overlay::input_blocked`]). It lists the whole
/// standings that fit the region its placement left on the last layout, and states its least and
/// natural sizes ([`ModalExtent`]) for the next placement.
#[expect(
    clippy::too_many_arguments,
    reason = "the replicated view also owns its table's paging and its natural extent"
)]
fn update_battle_hud(
    battles: Query<(&BattleState, &Scoreboard)>,
    clock: Option<Res<InterpolationTimeline>>,
    time: Res<Time<Fixed>>,
    keys: Res<ButtonInput<KeyCode>>,
    overlays: Res<Overlays>,
    mut roots: Query<(&MatchUi, &mut Visibility)>,
    mut parts: Query<(&Part, &mut Text)>,
    mut nodes: BoardNodes,
) {
    let (battle, board) = match battles.single() {
        Ok(replicated) => replicated,
        // Before the first snapshot, or between links: no Battle to show.
        Err(QuerySingleError::NoEntities(_)) => {
            hide_views(&mut roots, &mut nodes);
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
        hide_views(&mut roots, &mut nodes);
        return;
    }
    let now = clock.tick().0;
    let Some(view) = View::of(battle.phase, now, time.timestep()) else {
        // A stale empty-server snapshot can arrive before combat; loading owns that interval.
        hide_views(&mut roots, &mut nodes);
        return;
    };
    let board_shown =
        view.shows_board(keys.pressed(BOARD_KEY)) && !overlay::input_blocked(&overlays, true);
    let (table_node, mut scroll, mut table) = nodes
        .table
        .single_mut()
        .expect("the match UI spawns one board table");
    let board_rows = if board_shown {
        let (region, mut extent) = nodes
            .region
            .single_mut()
            .expect("the match UI spawns one board root");
        let card = nodes
            .card
            .single()
            .expect("the match UI spawns one board card");
        let logical = |node: &ComputedNode| node.size * node.inverse_scale_factor;
        // The card's title, hint, padding and gaps: everything but the table.
        let furniture = logical(card) - logical(table_node);
        let capacity = standings_that_fit(logical(region).y - furniture.y);
        let rows = rows(board);
        let step = i8::from(keys.just_pressed(KeyCode::PageDown))
            - i8::from(keys.just_pressed(KeyCode::PageUp));
        let first = page(table.first, capacity, rows.len(), step);
        if table.first != first {
            table.first = first;
        }
        let across = f32::from(u8::from(keys.just_pressed(KeyCode::ArrowRight)))
            - f32::from(u8::from(keys.just_pressed(KeyCode::ArrowLeft)));
        if across != 0.0 {
            let view = logical(table_node).x;
            let limit =
                (table_node.content_size.x * table_node.inverse_scale_factor - view).max(0.0);
            let next = (scroll.0.x + across * view).clamp(0.0, limit);
            if scroll.0.x != next {
                scroll.0.x = next;
            }
        }
        let table_width = table_node.content_size.x * table_node.inverse_scale_factor;
        let width = logical(card).x.max(furniture.x + table_width);
        let lines = |standings: usize| furniture.y + ROW_PX * (standings + 1) as f32;
        extent.set_if_neq(ModalExtent {
            // Every column, its header and a standing: a region narrower cuts the table's columns.
            least: Vec2::new(width, lines(rows.len().min(1))),
            natural: Vec2::new(width, lines(rows.len())),
        });
        Some(rows[first..first.saturating_add(capacity).min(rows.len())].to_vec())
    } else {
        rewind(&mut scroll, &mut table);
        None
    };
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

    /// FIXTURE DATA: viewport sizes/scales, thirty tied standings in combat with Tab held, and the
    /// fixed readouts' text as the shipped seed tank renders them in the view-5 stills (its vitals
    /// card with the repair hint and six module rows, its seat row and its drive row). Uses the
    /// shipped spawners, fonts, Bevy layout and this module's own system; no simulated tank or link
    /// is needed.
    fn hud_layout_app(width: u32, height: u32, scale: f32) -> App {
        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
            resolution: bevy::window::WindowResolution::new(width, height),
            visible: false,
            ..default()
        })))
        .add_plugins(crate::ui_font::plugin)
        .insert_resource(UiScale(scale))
        .init_resource::<InterpolationTimeline>()
        .init_resource::<Overlays>()
        .add_systems(Update, systems())
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
        let world = app.world_mut();
        world
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);
        world.resource_mut::<Overlays>().declare_in_match(true);
        world.spawn((
            BattleState::combat_fixture(),
            Scoreboard((1..=30).map(|i| standing(i, 0, 0)).collect()),
        ));
        world
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BOARD_KEY);
        let status = world
            .query_filtered::<Entity, With<crate::crew_ui::StatusPanelText>>()
            .single(world)
            .unwrap();
        world.get_mut::<Text>(status).unwrap().0 = [
            "ALIVE",
            "Crew 5/5",
            "Drive 100%",
            "Weapons: Coax 150 rds   HullMG 150 rds   MainGun READY",
            "MainGun round: 8.8 cm Pzgr. 39 ring",
            "Breech: READY",
            "Coax_MG_Barrel: READY",
            "Engine: READY",
            "Hull_MG_Barrel: READY",
            "Main_Gun_Barrel: READY",
            "Transmission: READY",
        ]
        .join("\n");
        for (seats, mut text) in world
            .query_filtered::<(Has<crate::crew_ui::CrewBarText>, &mut Text), (
                With<crate::hud::HudDock>,
                Without<crate::crew_ui::StatusPanelText>,
            )>()
            .iter_mut(world)
        {
            text.0 = if seats {
                "1: Commander   2: Gunner   3: Loader   4: Driver   5: Bow gunner"
            } else {
                "Gear F1P   RPM 0.6k   Speed   0 km/h"
            }
            .to_owned();
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

    /// The standings the board lists: the rank column's lines under its header.
    fn listed(world: &mut World) -> usize {
        let place = world
            .query::<(&Part, &Text)>()
            .iter(world)
            .find(|(part, _)| **part == Part::Column(Column::Place))
            .expect("the rank column")
            .1
            .0
            .clone();
        assert!(
            place.starts_with("RANK"),
            "every page keeps its header: {place:?}"
        );
        place.lines().count() - 1
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
            *app.world_mut().get_mut::<Visibility>(status).unwrap() = Visibility::Hidden;
            settle_hud(&mut app);
            assert!(
                {
                    let size = app.world().get::<ComputedNode>(root).unwrap().size();
                    size.x * size.y > reserved.x * reserved.y
                },
                "hiding a panel increases the clear region"
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
    /// clipped columns. FIXTURE DATA: a window narrower than every column, diagnostics shown.
    #[test]
    fn a_narrow_board_keeps_every_column_in_its_scroll_content() {
        let mut app = hud_layout_app(640, 1080, 2.0);
        // FIXTURE DATA: full-width ids and a results winner keep every rendered column wide,
        // independently of compact typography. An empty combat winner column has no ink.
        {
            let world = app.world_mut();
            let (mut battle, mut board) = world
                .query::<(&mut BattleState, &mut Scoreboard)>()
                .single_mut(world)
                .unwrap();
            board.0 = (0..30).map(|i| standing(u64::MAX - i, 0, 0)).collect();
            battle.phase = BattlePhase::Results {
                ends_at: crate::sim_tick::SimTick::new(100_000),
                outcome: BattleOutcome::Winner(CombatantId(u64::MAX)),
            };
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

    /// The compact board lists one line per standing, whole lines only, under headers every page
    /// keeps. The shipped-size fixtures pin the capacity: a large window lists the full Battle; the
    /// small high-scale target pages, its last page ending on the last standing. There the board
    /// takes the region that holds every column over a narrower one beside the vitals card that
    /// would list more standings with their columns cut. Players rely on reading every listed row
    /// with its column's label. FIXTURE DATA: thirty standings and the two viewport/scale
    /// combinations below.
    #[test]
    fn compact_board_names_the_rows_that_fit_at_both_owner_fixtures() {
        for (width, height, scale, expected_rows) in [(1280, 720, 2.0, 6), (1920, 1080, 1.0, 30)] {
            let mut app = hud_layout_app(width, height, scale);
            let rows = listed(app.world_mut());
            eprintln!(
                "MEASURED compact board {width}x{height} scale {scale}: {rows} standings fit"
            );
            assert_eq!(
                rows, expected_rows,
                "MEASURED fixture capacity must remain readable"
            );
            let world = app.world_mut();
            for (part, text, layout) in world
                .query::<(&Part, &Text, &bevy::text::TextLayoutInfo)>()
                .iter(world)
            {
                if matches!(part, Part::Column(_)) && !text.0.trim().is_empty() {
                    assert!(
                        (layout.size.y - (rows + 1) as f32 * ROW_PX).abs() < 1.0,
                        "{part:?}: one header and {rows} rendered lines, got {} logical px",
                        layout.size.y
                    );
                    assert_eq!(
                        layout.glyphs.last().unwrap().line_index,
                        rows,
                        "soft wrapping must not add standing lines"
                    );
                }
            }
            let viewport = world
                .query_filtered::<&ComputedNode, With<BoardViewport>>()
                .single(world)
                .unwrap();
            assert!(
                viewport.content_size.y <= viewport.size.y + 1.0,
                "the table clips no listed line: content {} px in {} px",
                viewport.content_size.y,
                viewport.size.y
            );
            // The last page: the rank header, then whole rows ending on the last standing.
            world
                .query::<&mut BoardViewport>()
                .single_mut(world)
                .unwrap()
                .first = usize::MAX;
            settle_hud(&mut app);
            assert_eq!(listed(app.world_mut()), rows);
            let names = text(&mut app, Part::Column(Column::Name));
            assert_eq!(names.lines().last(), Some("PLAYER 30"), "{names:?}");
        }
    }

    /// The game's damage-model view: the live inset is a passing panel. Held Tab keeps clear of it
    /// while the board fits beside it, and covers it only to list more: at the small high-scale
    /// target the real vitals card, clock and diagnostics leave no room beside it, so the board
    /// lists as many standings with the inset up as without; at 1280x720 / scale 1 it lists all
    /// thirty beside it. Its public rig marker is the integration seam; world rig entities never
    /// dock. FIXTURE DATA: the corner's real spawn, which at both targets is its laid-out width
    /// (DERIVED: 30 % of 640 or 1280 logical px clamps to its 384 px floor).
    #[test]
    fn the_live_corner_yields_to_the_board_only_when_it_must() {
        for (scale, beside) in [(2.0, false), (1.0, true)] {
            let mut app = hud_layout_app(1280, 720, scale);
            let alone = listed(app.world_mut());
            let corner = crate::damage_cam::pen_cam::spawn_corner_for_layout_test(app.world_mut());
            *app.world_mut().get_mut::<Visibility>(corner).unwrap() = Visibility::Visible;
            settle_hud(&mut app);
            let (_, card) = board_entities(app.world_mut());
            assert_eq!(
                app.world().get::<crate::hud::HudDock>(corner),
                Some(&crate::hud::HudDock::Transient)
            );
            assert_eq!(
                app.world().get::<GlobalZIndex>(corner).unwrap().0,
                Overlay::PEN_CAM_CORNER_Z
            );
            assert_modal_clear(app.world_mut(), card);
            let with_inset = listed(app.world_mut());
            let covers = overlaps(hud_rect(app.world(), card), hud_rect(app.world(), corner));
            eprintln!(
                "MEASURED inset at scale {scale}: {alone} standings alone, {with_inset} with the inset, covers {covers}"
            );
            assert_eq!(
                with_inset, alone,
                "the passing inset costs the board no standing"
            );
            assert_eq!(covers, !beside, "scale {scale}");
            if beside {
                assert_eq!(with_inset, 30);
            }
        }
    }

    /// The board overlaps no visible fixed readout and lists at least one standing.
    fn assert_modal_clear(world: &mut World, card: Entity) {
        let board = hud_rect(world, card);
        assert!(board.width() > 0.0 && board.height() > 0.0);
        for (entity, dock, node, visibility) in world
            .query::<(Entity, &crate::hud::HudDock, &ComputedNode, &Visibility)>()
            .iter(world)
        {
            if *dock == crate::hud::HudDock::Fixed
                && *visibility != Visibility::Hidden
                && node.size().min_element() > 0.0
            {
                assert!(
                    !overlaps(board, hud_rect(world, entity)),
                    "MEASURED board {board:?} overlaps fixed panel {:?}",
                    hud_rect(world, entity)
                );
            }
        }
        assert!(listed(world) > 0, "the populated roster lists a standing");
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

    /// Lay out the board by hand for the layout-free harness: a placed region `region_h` logical px
    /// tall, a card whose title, hint, padding and gaps take 40 logical px of height and 20 of
    /// width, and a table viewport `table` wide over `content` of columns. FIXTURE DATA, physical
    /// px at a scale of 2.
    fn lay_out_board(app: &mut App, region_h: f32, table: f32, content: f32) {
        let world = app.world_mut();
        let sized = |size: Vec2, content_size: Vec2| ComputedNode {
            size: size * 2.0,
            content_size: content_size * 2.0,
            inverse_scale_factor: 0.5,
            ..default()
        };
        let (root, card) = board_entities(world);
        world
            .entity_mut(root)
            .insert(sized(Vec2::new(table + 40.0, region_h), Vec2::ZERO));
        world
            .entity_mut(card)
            .insert(sized(Vec2::new(table + 20.0, 100.0), Vec2::ZERO));
        let viewport = world
            .query_filtered::<Entity, With<BoardViewport>>()
            .single(world)
            .unwrap();
        world
            .entity_mut(viewport)
            .insert(sized(Vec2::new(table, 60.0), Vec2::new(content, 60.0)));
    }

    /// Players reach every standing and column while the board is shown, a page of whole rows at
    /// a time under headers every page keeps; the last page is full and ends on the last
    /// standing. Columns move sideways one view at a time to the end. A menu suppresses paging
    /// and rewinds the table rather than paging an obscured board. FIXTURE DATA: twelve tied
    /// standings and a region with room for five under the header.
    #[test]
    fn the_board_pages_all_columns_together_only_while_shown() {
        let mut app = app(Duration::from_millis(500));
        app.world_mut().spawn((
            BattleState::combat_fixture(),
            Scoreboard((1..=12).map(|i| standing(i, 0, 0)).collect()),
        ));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(BOARD_KEY);
        app.update();
        // 40 px of card furniture, a header line and five rows (DERIVED: 40 + 6 × 16).
        lay_out_board(&mut app, 136.0, 400.0, 1000.0);
        let tap = |app: &mut App, key| {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(key);
            keys.clear();
            keys.press(key);
            app.update();
            app.world_mut()
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(key);
        };
        let names = |app: &mut App| text(app, Part::Column(Column::Name));
        app.update();
        assert_eq!(
            names(&mut app),
            "PLAYER\nPLAYER 1\nPLAYER 2\nPLAYER 3\nPLAYER 4\nPLAYER 5"
        );
        for expected in [
            "PLAYER\nPLAYER 6\nPLAYER 7\nPLAYER 8\nPLAYER 9\nPLAYER 10",
            "PLAYER\nPLAYER 8\nPLAYER 9\nPLAYER 10\nPLAYER 11\nPLAYER 12",
            "PLAYER\nPLAYER 8\nPLAYER 9\nPLAYER 10\nPLAYER 11\nPLAYER 12",
        ] {
            tap(&mut app, KeyCode::PageDown);
            assert_eq!(names(&mut app), expected);
            assert!(text(&mut app, Part::Column(Column::Place)).starts_with("RANK\n"));
        }
        tap(&mut app, KeyCode::PageUp);
        assert_eq!(
            names(&mut app),
            "PLAYER\nPLAYER 3\nPLAYER 4\nPLAYER 5\nPLAYER 6\nPLAYER 7"
        );
        let viewport = {
            let world = app.world_mut();
            world
                .query_filtered::<Entity, With<BoardViewport>>()
                .single(world)
                .unwrap()
        };
        let across = |app: &mut App| app.world().get::<ScrollPosition>(viewport).unwrap().0;
        for expected in [400.0, 600.0, 600.0] {
            tap(&mut app, KeyCode::ArrowRight);
            assert_eq!(across(&mut app), Vec2::new(expected, 0.0));
        }
        tap(&mut app, KeyCode::ArrowLeft);
        assert_eq!(across(&mut app), Vec2::new(200.0, 0.0));
        app.world_mut()
            .resource_mut::<Overlays>()
            .declare(Overlay::Menu, true);
        app.update();
        assert_eq!(across(&mut app), Vec2::ZERO);
        assert_eq!(app.world().get::<BoardViewport>(viewport).unwrap().first, 0);
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
        let page_on = |app: &mut App| {
            app.world_mut()
                .get_mut::<ScrollPosition>(viewport)
                .unwrap()
                .0
                .x = 250.0;
            app.world_mut()
                .get_mut::<BoardViewport>(viewport)
                .unwrap()
                .first = 3;
        };
        let rewound = |app: &mut App| {
            app.world().get::<ScrollPosition>(viewport).unwrap().0 == Vec2::ZERO
                && app.world().get::<BoardViewport>(viewport).unwrap().first == 0
        };
        page_on(&mut app);
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(false);
        app.update();
        assert!(rewound(&mut app));
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);
        page_on(&mut app);
        app.world_mut().entity_mut(replica).despawn();
        app.update();
        assert!(rewound(&mut app));
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
        app.update();
        lay_out_board(&mut app, 400.0, 400.0, 400.0);
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
    /// before the first snapshot; a stale empty-server phase shows no match UI
    /// even on Tab; the time left in combat, the board only while Tab is held and nothing
    /// captures input; the same board through results without Tab, the authority's winner on its
    /// own line, or the draw in the title. Mutants: showing the board while waiting, needing Tab in
    /// results, drawing over the menu, or marking the board's top row instead of the authority's
    /// winner all fail. Fixture ticks of half a second.
    #[test]
    fn the_view_follows_the_replicated_phase() {
        let tick = Duration::from_millis(500); // Fixture presentation timestep.
        let mode = super::super::battle_mode::BattleMode::shipped();
        let end = |duration| {
            crate::sim_tick::SimTick::new(40)
                .after(crate::sim_tick::TickSpan::from_duration(duration, tick).unwrap())
        };
        let display = |duration: Duration| {
            let seconds = duration.as_secs_f64().ceil() as u64;
            format!("{}:{:02}", seconds / 60, seconds % 60)
        };
        let mut app = app(tick);
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
        assert_eq!(clock_line(&mut app), None);
        assert!(!shown(&mut app, MatchUi::Scoreboard));

        // Fixture input time leads presentation time. The match display must follow the latter.
        app.world_mut().resource_mut::<crate::ShotClock>().0 = crate::sim_tick::SimTick::new(400);
        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_now(lightyear::core::time::TickInstant::from(
                lightyear::core::tick::Tick(40),
            ));
        set_phase(
            &mut app,
            BattlePhase::Combat {
                ends_at: end(mode.combat),
                late_join: LateJoin::Closed,
            },
        );
        app.update();
        assert_eq!(clock_line(&mut app), Some(display(mode.combat)));
        assert!(
            shown(&mut app, MatchUi::Scoreboard),
            "Tab held in combat shows the board"
        );
        assert_eq!(text(&mut app, Part::Title), "BATTLE 1");
        let column = |app: &mut App, which| text(app, Part::Column(which));
        assert_eq!(column(&mut app, Column::Place), "RANK\n1\n2");
        assert_eq!(column(&mut app, Column::Name), "PLAYER\nPLAYER 9\nPLAYER 7");
        assert_eq!(column(&mut app, Column::Knockouts), "KILLS\n3\n1");
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
                ends_at: end(mode.results),
                outcome: BattleOutcome::Winner(CombatantId(7)),
            },
        );
        app.update();
        assert_eq!(
            clock_line(&mut app),
            Some(format!("RESULTS {}", display(mode.results)))
        );
        assert!(
            shown(&mut app, MatchUi::Scoreboard),
            "results show the board without Tab"
        );
        assert_eq!(text(&mut app, Part::Title), "BATTLE 1 — WINNER: PLAYER 7");
        assert_eq!(column(&mut app, Column::Mark), "\n\nWINNER");

        set_phase(
            &mut app,
            BattlePhase::Results {
                ends_at: end(mode.results),
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
