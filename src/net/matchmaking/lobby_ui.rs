//! The Join menu's view: a modal card on the lobby scrim — title and version, the region picker,
//! a status line, and the Join / Cancel buttons. The card is the `Flow::Lobby` screen's.
//!
//! The picker list is rebuilt only when what it shows changes ([`ListView`] compared against the
//! last one drawn into the same list node, so a respawned card draws at once); the footer is reconciled every frame with change-guarded writes. Clicks are
//! one `Pointer<Click>` observer over [`Target`]; hover is picking's own [`Hovered`].
//!
//! Strings from the network (city names, failure reasons) pass through [`printable`] before they
//! reach `Text`: the bundled font has no fallback glyphs.

use std::collections::BTreeSet;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;

use super::picker::{self, GroupView, Tick};
use super::ticket::{self, Phase};
use super::worker::WorkerLink;
use super::{Lobby, Locations};
use crate::flow::{Flow, FlowScoped};
use crate::ui_font::{MODAL_BG, TEXT, TEXT_DIM, UiFonts};

const CARD_WIDTH_PX: f32 = 620.0;
const TITLE_PX: f32 = 34.0;
const ROW_PX: f32 = 19.0;
const SMALL_PX: f32 = 15.0;
/// The lobby scrim: the world behind it loads but is not yet anyone's view.
const SCRIM: Color = Color::srgba(0.01, 0.02, 0.03, 0.94);
const ROW_HOVER: Color = Color::srgba(0.85, 0.95, 1.0, 0.08);
const BUTTON: Color = Color::srgb(0.16, 0.30, 0.38);
const BUTTON_HOVER: Color = Color::srgb(0.22, 0.40, 0.50);
const BUTTON_DISABLED: Color = Color::srgb(0.12, 0.15, 0.18);
/// The HUD family's amber (`net::debug_hud`'s warn literal).
const WARN: Color = Color::srgb(1.0, 0.8, 0.3);

/// What a click on this node does.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub(super) enum Target {
    /// The tick box of a continent: select or clear all its locations.
    GroupTick(String),
    /// The rest of a continent's row: list or hide its locations.
    GroupExpand(String),
    Location(String),
    Join,
    Cancel,
    Retry,
}

#[derive(Component)]
pub(super) struct LobbyList;

#[derive(Component)]
pub(super) struct StatusLine;

/// Everything the list draws; the list is respawned only when this changes.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum ListView {
    Loading,
    Failed(String),
    Groups {
        groups: Vec<GroupView>,
        selection: BTreeSet<String>,
        expanded: BTreeSet<String>,
        editable: bool,
    },
}

/// Replace every character outside printable ASCII: common Latin-1 letters by their base letter,
/// anything else by `?`.
pub(super) fn printable(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' '..='~' => c,
            'À'..='Å' => 'A',
            'à'..='å' => 'a',
            'Ç' => 'C',
            'ç' => 'c',
            'È'..='Ë' => 'E',
            'è'..='ë' => 'e',
            'Ì'..='Ï' => 'I',
            'ì'..='ï' => 'i',
            'Ñ' => 'N',
            'ñ' => 'n',
            'Ò'..='Ö' | 'Ø' => 'O',
            'ò'..='ö' | 'ø' => 'o',
            'Ù'..='Ü' => 'U',
            'ù'..='ü' => 'u',
            'Ý' => 'Y',
            'ý' | 'ÿ' => 'y',
            _ => '?',
        })
        .collect()
}

fn font(handle: &Handle<Font>, px: f32) -> TextFont {
    TextFont {
        font: handle.clone().into(),
        font_size: FontSize::Px(px),
        ..default()
    }
}

pub(super) fn spawn(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((
            FlowScoped(Flow::Lobby),
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(SCRIM),
        ))
        .with_children(|scrim| {
            scrim
                .spawn((
                    Node {
                        width: Val::Px(CARD_WIDTH_PX),
                        max_height: Val::Percent(90.0),
                        padding: UiRect::all(Val::Px(22.0)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(12.0),
                        overflow: Overflow::clip_y(),
                        ..default()
                    },
                    BackgroundColor(MODAL_BG),
                ))
                .with_children(|card| {
                    card.spawn(Node {
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Baseline,
                        ..default()
                    })
                    .with_children(|header| {
                        header.spawn((
                            Text::new("OVERMATCH"),
                            font(&fonts.hud, TITLE_PX),
                            TextColor(TEXT),
                        ));
                        header.spawn((
                            Text::new(format!("VERSION {}", super::super::CLIENT_VERSION)),
                            font(&fonts.body, SMALL_PX),
                            TextColor(TEXT_DIM),
                        ));
                    });
                    card.spawn((
                        LobbyList,
                        Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            row_gap: Val::Px(2.0),
                            overflow: Overflow::clip_y(),
                            ..default()
                        },
                    ));
                    card.spawn((
                        StatusLine,
                        Text::new(""),
                        font(&fonts.body, ROW_PX),
                        TextColor(TEXT),
                    ));
                    card.spawn(Node {
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::FlexEnd,
                        column_gap: Val::Px(10.0),
                        ..default()
                    })
                    .with_children(|buttons| {
                        button(buttons, &fonts, Target::Cancel, "CANCEL");
                        button(buttons, &fonts, Target::Join, "JOIN");
                    });
                });
        });
}

fn button(parent: &mut ChildSpawnerCommands, fonts: &UiFonts, target: Target, label: &str) {
    parent
        .spawn((
            target,
            Hovered::default(),
            Node {
                padding: UiRect::axes(Val::Px(26.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(BUTTON),
        ))
        .with_child((Text::new(label), font(&fonts.hud, 22.0), TextColor(TEXT)));
}

/// A fixed-width cell, right-aligned unless `width` is `None` (then it takes the free space).
fn cell(
    parent: &mut ChildSpawnerCommands,
    width: Option<f32>,
    text: impl Into<String>,
    font: TextFont,
    color: Color,
) {
    let node = match width {
        Some(px) => Node {
            width: Val::Px(px),
            justify_content: JustifyContent::FlexEnd,
            ..default()
        },
        None => Node {
            flex_grow: 1.0,
            ..default()
        },
    };
    parent
        .spawn(node)
        .with_child((Text::new(text), font, TextColor(color)));
}

const TICK_W: f32 = 30.0;
const PING_W: f32 = 110.0;
const SPREAD_W: f32 = 70.0;
const MATCHES_W: f32 = 90.0;
const CHEVRON_W: f32 = 22.0;

fn row_node(indent: f32) -> Node {
    Node {
        width: Val::Percent(100.0),
        align_items: AlignItems::Center,
        column_gap: Val::Px(8.0),
        padding: UiRect {
            left: Val::Px(8.0 + indent),
            right: Val::Px(8.0),
            top: Val::Px(5.0),
            bottom: Val::Px(5.0),
        },
        border_radius: BorderRadius::all(Val::Px(4.0)),
        ..default()
    }
}

fn spawn_group(
    list: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    group: &GroupView,
    selection: &BTreeSet<String>,
    expanded: bool,
    editable: bool,
) {
    let tick_color = if editable { TEXT } else { TEXT_DIM };
    list.spawn((
        Target::GroupExpand(group.continent.clone()),
        Hovered::default(),
        row_node(0.0),
        BackgroundColor(Color::NONE),
    ))
    .with_children(|row| {
        row.spawn((
            Target::GroupTick(group.continent.clone()),
            Hovered::default(),
            Node {
                width: Val::Px(TICK_W),
                ..default()
            },
        ))
        .with_child((
            Text::new(picker::group_tick(group, selection).glyph()),
            font(&fonts.hud, ROW_PX),
            TextColor(tick_color),
        ));
        cell(
            row,
            None,
            printable(&group.continent).to_uppercase(),
            font(&fonts.hud, ROW_PX),
            TEXT,
        );
        cell(
            row,
            Some(PING_W),
            picker::group_ping_label(group),
            font(&fonts.body, ROW_PX),
            TEXT,
        );
        cell(
            row,
            Some(SPREAD_W),
            "",
            font(&fonts.body, SMALL_PX),
            TEXT_DIM,
        );
        cell(
            row,
            Some(MATCHES_W),
            picker::matches_label(group.matches),
            font(&fonts.body, ROW_PX),
            TEXT_DIM,
        );
        cell(
            row,
            Some(CHEVRON_W),
            if expanded { "–" } else { "+" },
            font(&fonts.hud, ROW_PX),
            TEXT_DIM,
        );
    });
    if !expanded {
        return;
    }
    for location in &group.rows {
        let ticked = if selection.contains(&location.id) {
            Tick::All
        } else {
            Tick::None
        };
        list.spawn((
            Target::Location(location.id.clone()),
            Hovered::default(),
            row_node(TICK_W),
            BackgroundColor(Color::NONE),
        ))
        .with_children(|row| {
            row.spawn(Node {
                width: Val::Px(TICK_W),
                ..default()
            })
            .with_child((
                Text::new(ticked.glyph()),
                font(&fonts.hud, ROW_PX),
                TextColor(tick_color),
            ));
            cell(
                row,
                None,
                printable(&location.label),
                font(&fonts.body, ROW_PX),
                TEXT,
            );
            cell(
                row,
                Some(PING_W),
                picker::estimate_label(&location.estimate),
                font(&fonts.body, ROW_PX),
                TEXT,
            );
            cell(
                row,
                Some(SPREAD_W),
                picker::spread_label(&location.estimate),
                font(&fonts.body, SMALL_PX),
                TEXT_DIM,
            );
            cell(
                row,
                Some(MATCHES_W),
                picker::matches_label(location.matches),
                font(&fonts.body, ROW_PX),
                TEXT_DIM,
            );
            cell(row, Some(CHEVRON_W), "", font(&fonts.hud, ROW_PX), TEXT_DIM);
        });
    }
}

fn list_view(lobby: &Lobby) -> ListView {
    match &lobby.locations {
        Locations::Loading => ListView::Loading,
        Locations::Failed(reason) => ListView::Failed(reason.clone()),
        Locations::Loaded(_) => ListView::Groups {
            groups: lobby.groups().unwrap_or_default(),
            selection: lobby.selection.clone(),
            expanded: lobby.expanded.clone(),
            editable: !lobby.flow.busy(),
        },
    }
}

pub(super) fn refresh_list(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    lobby: Res<Lobby>,
    list: Single<Entity, With<LobbyList>>,
    mut shown: Local<Option<(Entity, ListView)>>,
) {
    let view = list_view(&lobby);
    if shown
        .as_ref()
        .is_some_and(|(drawn, last)| *drawn == *list && *last == view)
    {
        return;
    }
    let list_entity = *list;
    let mut list = commands.entity(list_entity);
    list.despawn_children();
    list.with_children(|list| match &view {
        ListView::Loading => {
            list.spawn((
                Text::new("Contacting the matchmaker…"),
                font(&fonts.body, ROW_PX),
                TextColor(TEXT_DIM),
            ));
        }
        ListView::Failed(reason) => {
            list.spawn((
                Text::new(printable(reason)),
                font(&fonts.body, ROW_PX),
                TextColor(WARN),
            ));
            list.spawn(Node {
                margin: UiRect::top(Val::Px(8.0)),
                ..default()
            })
            .with_children(|row| button(row, &fonts, Target::Retry, "RETRY"));
        }
        ListView::Groups {
            groups,
            selection,
            expanded,
            editable,
        } => {
            if groups.is_empty() {
                list.spawn((
                    Text::new("No regions are available for this version."),
                    font(&fonts.body, ROW_PX),
                    TextColor(TEXT_DIM),
                ));
            }
            for group in groups {
                spawn_group(
                    list,
                    &fonts,
                    group,
                    selection,
                    expanded.contains(&group.continent),
                    *editable,
                );
            }
        }
    });
    *shown = Some((list_entity, view));
}

/// The status line's text and colour for the current lobby state.
fn status(lobby: &Lobby, now: f64) -> (String, Color) {
    let flow = &lobby.flow;
    if let Phase::Connecting { .. } = flow.phase() {
        return ("Connecting to the match server…".into(), TEXT);
    }
    if let (Some(label), Some(elapsed)) = (flow.progress_label(), flow.elapsed(now)) {
        return (format!("{label}  {}", ticket::clock_label(elapsed)), TEXT);
    }
    if let Some(reason) = flow.failure() {
        return (format!("Couldn't join: {}", printable(reason)), WARN);
    }
    if let Some(reason) = flow.ended() {
        return (format!("The match ended: {}", printable(reason)), TEXT);
    }
    match &lobby.locations {
        Locations::Loaded(_) if lobby.selection.is_empty() => {
            ("Tick at least one region to join.".into(), TEXT_DIM)
        }
        _ => (String::new(), TEXT),
    }
}

pub(super) fn refresh_footer(
    lobby: Res<Lobby>,
    time: Res<Time<Real>>,
    mut line: Single<(&mut Text, &mut TextColor), With<StatusLine>>,
    mut buttons: Query<(&Target, &Hovered, &mut Node, &mut BackgroundColor)>,
) {
    let (text, color) = status(&lobby, time.elapsed_secs_f64());
    let (line_text, line_color) = &mut *line;
    if line_text.0 != text {
        line_text.0 = text;
    }
    line_color.set_if_neq(TextColor(color));

    let idle = !lobby.flow.busy();
    let waiting = matches!(
        lobby.flow.phase(),
        Phase::Requesting { .. } | Phase::Waiting { .. }
    );
    let joinable =
        idle && matches!(lobby.locations, Locations::Loaded(_)) && !lobby.selection.is_empty();
    for (target, hovered, mut node, mut background) in &mut buttons {
        let (shown, enabled, row) = match target {
            Target::Join => (idle, joinable, false),
            Target::Cancel => (waiting, true, false),
            Target::Retry => (true, true, false),
            Target::GroupExpand(_) | Target::Location(_) => (true, true, true),
            Target::GroupTick(_) => continue,
        };
        let display = if shown { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        let fill = match (row, enabled, hovered.get()) {
            (true, _, true) => ROW_HOVER,
            (true, _, false) => Color::NONE,
            (false, false, _) => BUTTON_DISABLED,
            (false, true, true) => BUTTON_HOVER,
            (false, true, false) => BUTTON,
        };
        background.set_if_neq(BackgroundColor(fill));
    }
}

/// The one click handler for the lobby. Selection changes only while no join is in progress;
/// expanding a continent is always allowed.
pub(super) fn clicked(
    mut click: On<Pointer<Click>>,
    targets: Query<&Target>,
    link: Option<Res<WorkerLink>>,
    time: Res<Time<Real>>,
    mut lobby: ResMut<Lobby>,
) {
    let Ok(target) = targets.get(click.entity) else {
        return;
    };
    click.propagate(false);
    let Some(link) = link else { return };
    if click.button != PointerButton::Primary {
        return;
    }
    let editable = !lobby.flow.busy();
    match target {
        Target::GroupExpand(continent) => {
            if !lobby.expanded.remove(continent) {
                lobby.expanded.insert(continent.clone());
            }
        }
        Target::GroupTick(continent) if editable => {
            let Some(groups) = lobby.groups() else { return };
            if let Some(group) = groups.iter().find(|g| &g.continent == continent) {
                picker::toggle_group(group, &mut lobby.selection);
                lobby.touched = true;
            }
        }
        Target::Location(id) if editable => {
            picker::toggle_location(id, &mut lobby.selection);
            lobby.touched = true;
        }
        Target::Join => lobby.join(&link, time.elapsed_secs_f64()),
        Target::Cancel => lobby.flow.cancel(),
        Target::Retry => lobby.reload(&link),
        Target::GroupTick(_) | Target::Location(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_keeps_ascii_and_folds_latin_letters() {
        assert_eq!(printable("São Paulo"), "Sao Paulo");
        assert_eq!(printable("Zürich, Österreich"), "Zurich, Osterreich");
        assert_eq!(printable("東京"), "??");
        assert_eq!(printable("plain ASCII ~"), "plain ASCII ~");
    }
}
