//! The player-facing settings page: a titled card of `LABEL  < VALUE >` rows.
//!
//! Player-facing, NOT `dev_tools`-gated. View + input only — it writes [`Settings`] and asks for a
//! save; the reconcile is `settings::apply_settings`'s job.
//!
//! # Entry
//!
//! The page IS the content of the existing pause surface — no new key, no new overlay layer:
//!
//! * **Net client** — `Esc` opens `overlay::Overlay::Menu`, which already blocks input, releases the
//!   cursor, owns the scrim and is priority-gated (`may_open`) against the connect/death screens.
//!   Those are precisely a settings page's requirements, so the page rides the menu rather than
//!   inventing a parallel layer with its own precedence to keep in sync. `Esc` closes it. The menu's
//!   old "MENU / Esc to close" placeholder banner is gone — this is what it was a placeholder for.
//! * **Single-player / `--offline`** — the same page under `AppState::Paused`, whose `Esc` toggle and
//!   cursor release already exist in `state::client_plugin`.
//!
//! Both roots feed exactly one bool ([`SettingsPageVisible`]); everything below is shared, so the two
//! pause surfaces cannot drift into different settings pages.
//!
//! # Input
//!
//! Keyboard and mouse, because the cursor is released under either surface and both are things a
//! player will reach for:
//!
//! * `Up`/`Down` — move the selection. `W`/`S` are deliberately NOT bound: they drive the tank, and
//!   a settings page that also reads the drive keys teaches the wrong reflex.
//! * `Left`/`Right` — change the selected row's value.
//! * Mouse — hover selects; a click on the row's left or right half steps its value down or up. The
//!   hit test is `ComputedNode::contains_point`, i.e. bevy's own laid-out rect, so it cannot drift
//!   from what is drawn (the failure mode `net::spawn_map`'s doc records from hand-computed rects).
//!   That rect is in PHYSICAL pixels and the cursor has to be converted to match — [`mouse_input`]'s
//!   doc records the bug class, because getting it wrong makes the mouse dead in total silence.
//!
//! Every change saves immediately — there is no Apply button to forget, and an atomic write makes
//! that safe (`settings::save`).

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::{
    MsaaLevel, RenderScaleLevel, SaveSettings, Settings, ShadowDistance, ShadowResolution,
    VsyncMode,
};
use crate::ui_font::{MODAL_BG, TEXT, TEXT_DIM, UiFonts};

/// Whether the page is on screen this frame. Written by whichever pause surface the composition root
/// mounts ([`declare_from_overlay_menu`] on the net client, [`declare_from_pause_state`] in SP), read
/// by everything else here. One bool is the entire coupling between the two entry paths.
#[derive(Resource, Default, PartialEq)]
pub(super) struct SettingsPageVisible(bool);

/// Ordering handle for the visibility DECLARERS. Whichever declarer a root registers must land in
/// this set, and the page's input/refresh chain runs `.after` it — otherwise bevy is free to run the
/// readers first, and the page consumes LAST frame's visibility on every open/close transition: the
/// first keypress after opening is dropped, and the card lingers one frame after its owning surface
/// closes (Codex review finding, 2026-07-27).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclareSettingsPage;

/// Which row of the page a line is. The order here IS the on-screen order.
///
/// **This is the extension point for the research-pending entries** (window mode, render scale, UI
/// scale, frame cap): add a variant, add it to [`Row::ORDER`], and give it arms in [`Row::label`] /
/// [`Row::value`] / [`Row::step`]. Nothing else in this file, and nothing outside it, needs to
/// change — the card lays out from `ORDER`, and `Settings` already accepts new fields without a
/// migration (see the `settings` module doc).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    RenderScale,
    /// How far shadows reach, `OFF` included — see [`ShadowDistance`] for why this is its own row.
    ShadowDistance,
    /// How crisp they are. Adjacent to the distance row so the pair reads as one subject, but it
    /// steps independently.
    ShadowResolution,
    Msaa,
    VSync,
}

impl Row {
    const ORDER: [Row; 5] = [
        Row::RenderScale,
        Row::ShadowDistance,
        Row::ShadowResolution,
        Row::Msaa,
        Row::VSync,
    ];

    /// The row's name. ASCII only — it reaches `Text`.
    const fn label(self) -> &'static str {
        match self {
            Row::RenderScale => "RENDER SCALE",
            Row::ShadowDistance => "SHADOW DISTANCE",
            // "QUALITY", not "RESOLUTION": the value beside it is already a texel count, so the
            // label's job is to say what the number buys, not to name its unit twice.
            Row::ShadowResolution => "SHADOW QUALITY",
            Row::Msaa => "ANTI-ALIASING",
            Row::VSync => "VSYNC",
        }
    }

    /// A one-line explanation of what the selected row costs or buys, shown at the foot of the card.
    /// ASCII plus the verified typographic set only — it reaches `Text`.
    const fn hint(self) -> &'static str {
        match self {
            // Says what it does NOT touch, because that is the surprising half: the crosshair, the
            // range dial and every label stay pin-sharp while the world softens. The MSAA advice is
            // the one honest limit a player can act on (the multisample resolve is full-res
            // whatever this row says, so 4X below 100% is paying twice for the same edges).
            Row::RenderScale => {
                "How much of the window the 3D world is drawn at, before upscaling. \
                 The HUD stays sharp. Below 100%, prefer 2X or OFF anti-aliasing."
            }
            // Honest about the cut-off, because that is what a player actually sees when they lower
            // it: shadows simply stop at a radius. The OFF caveat is stated at the point of contact
            // rather than in a patch note — the rung exists pending perf data and may be withdrawn
            // (see `ShadowDistance`), and a player deserves to know that before building a habit
            // around it.
            Row::ShadowDistance => {
                "How far shadows are drawn before they cut off. \
                 OFF disables them entirely, and may be removed later."
            }
            // Says what it does NOT change, because a player who lowers this expecting more range
            // gets neither.
            Row::ShadowResolution => {
                "How crisp shadows are, at any distance. Higher costs more memory and fill rate."
            }
            Row::Msaa => "Smooths jagged edges. The cheapest setting to drop for more frames.",
            Row::VSync => {
                "Locks frames to the display refresh. Off can tear, but lowers input lag."
            }
        }
    }

    /// Where this row sits in [`Row::ORDER`] — the one place the array is searched, so the selection
    /// and the mouse hit test cannot disagree about what "row 2" means. A row missing from `ORDER`
    /// is unreachable (the page lays out FROM `ORDER`), and resolves to the first rather than
    /// panicking.
    fn index(self) -> usize {
        Row::ORDER
            .iter()
            .position(|row| *row == self)
            .unwrap_or_default()
    }

    /// The row's current value, as rendered. ASCII only — it reaches `Text`.
    fn value(self, settings: &Settings) -> &'static str {
        match self {
            Row::RenderScale => settings.render_scale.label(),
            Row::ShadowDistance => settings.shadow_distance.label(),
            Row::ShadowResolution => settings.shadow_resolution.label(),
            Row::Msaa => settings.msaa.label(),
            Row::VSync => settings.vsync.label(),
        }
    }

    /// Step this row by `delta` (`+1` / `-1`), saturating at the ends rather than wrapping.
    ///
    /// Saturating is the deliberate choice for a page a player reads: a wrapping list turns "press
    /// right until it stops" into an accidental jump from the highest setting to the lowest — and
    /// with `OFF` now the floor of the shadow-distance ladder, a wrap would put "no shadows at all"
    /// one keypress past the most expensive setting.
    fn step(self, settings: &mut Settings, delta: i32) {
        match self {
            Row::RenderScale => {
                settings.render_scale =
                    step_in(&RenderScaleLevel::ORDER, settings.render_scale, delta);
            }
            Row::ShadowDistance => {
                settings.shadow_distance =
                    step_in(&ShadowDistance::ORDER, settings.shadow_distance, delta);
            }
            Row::ShadowResolution => {
                settings.shadow_resolution =
                    step_in(&ShadowResolution::ORDER, settings.shadow_resolution, delta);
            }
            Row::Msaa => settings.msaa = step_in(&MsaaLevel::ORDER, settings.msaa, delta),
            Row::VSync => settings.vsync = step_in(&VsyncMode::ORDER, settings.vsync, delta),
        }
    }
}

/// Move `current` by `delta` places inside `order`, clamped to the ends. `current` missing from
/// `order` cannot happen through the UI, and resolves to the first entry rather than panicking.
fn step_in<T: Copy + PartialEq>(order: &[T], current: T, delta: i32) -> T {
    let index = order
        .iter()
        .position(|entry| *entry == current)
        .unwrap_or(0);
    let next = (index as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
    order[next]
}

/// The selected row index into [`Row::ORDER`]. Only ever moved through [`Selection::step`] /
/// [`Selection::select`] and read through [`Selection::row`], so the clamp lives in one place
/// instead of at each of the five sites that used to hand-roll it.
#[derive(Resource, Default)]
struct Selection(usize);

impl Selection {
    /// The last valid index. `Row::ORDER` is a non-empty array literal, so this cannot underflow.
    const LAST: usize = Row::ORDER.len() - 1;

    /// Move the selection by `delta` rows, clamped to the ends — the same saturating choice
    /// [`Row::step`] makes, and for the same reason: "hold down" must stop at the bottom row rather
    /// than jump back to the top.
    fn step(&mut self, delta: i32) {
        self.0 = (self.0 as i32 + delta).clamp(0, Self::LAST as i32) as usize;
    }

    /// Point the selection at a specific row — what a mouse hover does.
    fn select(&mut self, row: Row) {
        self.0 = row.index();
    }

    /// The selected row. Clamped on READ as well, so a stale index (a row removed from `ORDER`
    /// between frames) resolves to the last row instead of panicking on an out-of-bounds index.
    fn row(&self) -> Row {
        Row::ORDER[self.0.min(Self::LAST)]
    }
}

/// The card node, so visibility is one write.
#[derive(Component)]
struct SettingsCard;

/// The `VALUE` text of a row, and the row's `< >` affordances, rebuilt from [`Settings`].
#[derive(Component)]
struct RowValueText(Row);

/// The row's whole clickable line — carries the [`Row`] and is what the mouse hit-tests against.
#[derive(Component)]
struct RowLine(Row);

/// The footer hint line.
#[derive(Component)]
struct HintText;

/// The selection highlight, and its unselected counterpart. Kept next to each other so the contrast
/// between them is a decision rather than two literals that drifted. The card slab and both text
/// colours are the shared family constants in `ui_font`, not local copies.
const ROW_BG_SELECTED: Color = Color::srgba(0.16, 0.28, 0.36, 0.95);
const ROW_BG: Color = Color::NONE;

/// The shared page: spawn, input, refresh. Mounted by `settings::plugin`, which also adds exactly
/// one of the ENTRY declarers ([`declare_from_overlay_menu`] / [`declare_from_pause_state`]) — they
/// are the one thing the two windowed roots do differently, which is why they are that plugin's
/// single parameter rather than a system each root must remember.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<SettingsPageVisible>()
        .init_resource::<Selection>()
        .add_systems(Startup, spawn_page)
        .add_systems(
            Update,
            (
                // Input first, then the refresh reads the same frame's result — so a keypress
                // repaints immediately instead of one frame late.
                (keyboard_input, mouse_input),
                refresh_page,
            )
                .chain()
                // After the declarer so open/close is THIS frame's truth (see
                // [`DeclareSettingsPage`]), and before the reconcile so a change lands in the same
                // frame it was made.
                .after(DeclareSettingsPage)
                .before(super::ApplySettings),
        );
}

/// Net client: the page is visible exactly when the Esc menu owns the scrim. Reading `draws_scrim`
/// (not "is Menu latched") is what keeps the page from showing THROUGH a death or connect screen
/// that outranks the menu — the one-scrim rule, applied to the menu's content as well as its
/// backdrop. Runs after `OverlaySet::Toggle`, like every other consumer of the reconciled set.
///
/// `set_if_neq`, like every other per-frame resource write in this tree: a menu that has been open
/// for a second is not a change, and marking the resource dirty every frame would defeat any future
/// `resource_changed` reader of it.
pub(super) fn declare_from_overlay_menu(
    overlays: Res<crate::overlay::Overlays>,
    mut visible: ResMut<SettingsPageVisible>,
) {
    visible.set_if_neq(SettingsPageVisible(crate::overlay::draws_scrim(
        &overlays,
        crate::overlay::Overlay::Menu,
    )));
}

/// Single-player: the page is visible while paused. SP has no overlay authority — `state` owns the
/// Esc toggle, the cursor release and the physics freeze — so the predicate is simply the state.
pub(super) fn declare_from_pause_state(
    state: Res<State<crate::state::AppState>>,
    mut visible: ResMut<SettingsPageVisible>,
) {
    visible.set_if_neq(SettingsPageVisible(
        *state.get() == crate::state::AppState::Paused,
    ));
}

fn spawn_page(mut commands: Commands, fonts: Res<UiFonts>, settings: Res<Settings>) {
    // A full-screen, centring wrapper holding the card — the same shape `ui_font::spawn_overlay`
    // uses, so the page centres itself without depending on a parent container that neither pause
    // surface provides. The wrapper draws nothing (no `BackgroundColor`): the menu's scrim is the
    // backdrop on the net client, and SP deliberately has none.
    commands
        .spawn((
            SettingsCard,
            // The page's z comes from `overlay`'s ladder, not from a literal here: it is the menu's
            // CONTENT rung, and `overlay` is this project's z authority (see `Overlay::zindex`).
            GlobalZIndex(crate::overlay::Overlay::MENU_CONTENT_Z),
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            Visibility::Hidden,
        ))
        .with_children(|wrapper| {
            wrapper
                .spawn((
                    Node {
                        width: Val::Px(560.0),
                        padding: UiRect::all(Val::Px(24.0)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(6.0),
                        ..default()
                    },
                    BackgroundColor(MODAL_BG),
                ))
                .with_children(|card| {
                    card.spawn((
                Text::new("SETTINGS"),
                TextFont {
                    // SemiBold: the page title.
                    font: fonts.hud.clone().into(),
                    font_size: FontSize::Px(30.0),
                    ..default()
                },
                TextColor(TEXT),
                Node {
                    margin: UiRect::bottom(Val::Px(10.0)),
                    ..default()
                },
            ));

            for row in Row::ORDER {
                card.spawn((
                    RowLine(row),
                    Node {
                        width: Val::Percent(100.0),
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(7.0)),
                        border_radius: BorderRadius::all(Val::Px(4.0)),
                        ..default()
                    },
                    BackgroundColor(ROW_BG),
                ))
                .with_children(|line| {
                    line.spawn((
                        Text::new(row.label()),
                        TextFont {
                            // Regular: a dense settings row.
                            font: fonts.body.clone().into(),
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                    line.spawn((
                        RowValueText(row),
                        Text::new(row.value(&settings)),
                        TextFont {
                            // SemiBold: the value is what the eye goes to.
                            font: fonts.hud.clone().into(),
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(TEXT),
                    ));
                });
            }

            card.spawn((
                HintText,
                Text::new(""),
                TextFont {
                    // Regular: the quiet explanatory footer.
                    font: fonts.body.clone().into(),
                    font_size: FontSize::Px(14.0),
                    ..default()
                },
                TextColor(TEXT_DIM),
                Node {
                    margin: UiRect::top(Val::Px(14.0)),
                    ..default()
                },
            ));
            card.spawn((
                Text::new("Arrows to change  \u{2014}  Esc to close  \u{2014}  saved automatically"),
                TextFont {
                    // Regular: the control legend.
                    font: fonts.body.clone().into(),
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(TEXT_DIM),
                Node {
                    margin: UiRect::top(Val::Px(6.0)),
                    ..default()
                },
            ));
                });
        });
}

/// Arrow keys only — see the module doc on why `W`/`S` are not bound.
fn keyboard_input(
    visible: Res<SettingsPageVisible>,
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<Selection>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    if !visible.0 {
        return;
    }
    // Guarded rather than stepped by zero every frame: `Selection` is a `ResMut`, and a write is a
    // change-tick bump even when the value lands where it already was.
    let move_by = i32::from(keys.just_pressed(KeyCode::ArrowDown))
        - i32::from(keys.just_pressed(KeyCode::ArrowUp));
    if move_by != 0 {
        selection.step(move_by);
    }
    let delta = i32::from(keys.just_pressed(KeyCode::ArrowRight))
        - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
    if delta != 0 {
        change_row(selection.row(), delta, &mut settings, &mut save);
    }
}

/// Hover selects; a click on a row's left/right half steps its value. The hit test uses bevy's own
/// laid-out rect (`ComputedNode::contains_point`), and the half is decided by the click's position
/// WITHIN that rect, so the affordance matches the pixels at any window size or UI scale.
///
/// # The cursor must be in PHYSICAL pixels, and this whole system was dead until it was
///
/// `Window::cursor_position` returns LOGICAL pixels (bevy_window 0.19 `window.rs:614`: "The cursor
/// position in this window in logical pixels", and its body is literally
/// `physical_cursor_position() / scale_factor()`). Everything on the UI side of the test is
/// PHYSICAL: `ComputedNode::size` is documented "in physical pixels" (bevy_ui 0.19 `ui_node.rs:30`,
/// and every other resolved field on that type says the same), and `UiGlobalTransform` is built
/// alongside those values in `ui_layout_system`, so `contains_point` (`ui_node.rs:223`) compares a
/// point against a physical-pixel rect. Bevy's own picking backend does the conversion explicitly
/// before calling the same function — `pointer_pos = pointer_location.position *
/// camera.target_scaling_factor()` (`picking_backend.rs:133`).
///
/// MEASURED consequence on this project's target hardware: on a Retina 2x display every row's rect
/// was twice as far out and twice as large as the cursor value being tested, so no row ever
/// contained the cursor and the page's mouse support silently did nothing — no hover, no click.
/// It is a silent class of bug precisely because 1x machines work fine.
///
/// [`Window::physical_cursor_position`] is used rather than a hand-rolled
/// `cursor_position() * scale_factor()` because it is the value the logical accessor divides — same
/// conversion, no round trip through a float divide and multiply. The picking backend also subtracts
/// `Camera::physical_viewport_rect().min`; nothing here does, because this project never sets
/// `Camera::viewport` (see [`crate::render_scale`], which scales the main pass through a resolution
/// override precisely so the camera viewport stays the whole window).
///
/// [`click_delta`] then compares the cursor's `x` against `UiGlobalTransform::translation.x`, which
/// is in that same physical space — the two MUST be converted together or the split lands off-centre.
fn mouse_input(
    visible: Res<SettingsPageVisible>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    lines: Query<(&RowLine, &ComputedNode, &UiGlobalTransform)>,
    mut selection: ResMut<Selection>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    if !visible.0 {
        return;
    }
    let Some(window) = window else {
        return;
    };
    let Some(cursor) = window.physical_cursor_position() else {
        return;
    };
    let clicked = buttons.just_pressed(MouseButton::Left);
    for (line, node, transform) in &lines {
        if !node.contains_point(*transform, cursor) {
            continue;
        }
        // Hover alone moves the selection, so the keyboard and the mouse never disagree about
        // which row the footer hint is describing.
        selection.select(line.0);
        if clicked {
            let delta = click_delta(cursor.x, transform.translation.x);
            change_row(line.0, delta, &mut settings, &mut save);
        }
        return;
    }
}

/// Which way a click steps the row: right half increases, left half decreases. Pure, so the
/// left/right split is testable without a laid-out UI.
///
/// **Both arguments are PHYSICAL pixels.** `row_center_x` comes from `UiGlobalTransform`, which is
/// physical; a logical cursor compared against it is the bug [`mouse_input`]'s doc records, and here
/// it would not fail loudly — it would just split the row in the wrong place.
fn click_delta(cursor_x: f32, row_center_x: f32) -> i32 {
    if cursor_x >= row_center_x { 1 } else { -1 }
}

/// Apply one step and — only if the value actually moved — request a save. The guard is what keeps a
/// player leaning on the right arrow at the top of a ladder from rewriting the file every frame.
fn change_row(
    row: Row,
    delta: i32,
    settings: &mut ResMut<Settings>,
    save: &mut MessageWriter<SaveSettings>,
) {
    let before = **settings;
    let mut next = before;
    row.step(&mut next, delta);
    if next == before {
        return;
    }
    // Through `ResMut`'s `DerefMut` so change detection fires and `apply_settings` runs.
    **settings = next;
    save.write(SaveSettings);
    info!("settings: {} -> {}", row.label(), row.value(&next));
}

/// Rebuild every value, the selection highlight and the footer hint. Runs only while the page is up;
/// the visibility write itself is `set_if_neq`, so a closed page costs one comparison per frame.
fn refresh_page(
    visible: Res<SettingsPageVisible>,
    settings: Res<Settings>,
    selection: Res<Selection>,
    mut card: Query<&mut Visibility, With<SettingsCard>>,
    mut values: Query<(&RowValueText, &mut Text), Without<HintText>>,
    mut lines: Query<(&RowLine, &mut BackgroundColor)>,
    mut hint: Query<&mut Text, With<HintText>>,
) {
    let Ok(mut card) = card.single_mut() else {
        return;
    };
    card.set_if_neq(if visible.0 {
        Visibility::Visible
    } else {
        Visibility::Hidden
    });
    if !visible.0 {
        return;
    }

    let selected = selection.row();
    for (value, mut text) in &mut values {
        let rendered = value.0.value(&settings);
        if text.0 != rendered {
            text.0 = rendered.to_string();
        }
    }
    for (line, mut background) in &mut lines {
        background.set_if_neq(BackgroundColor(if line.0 == selected {
            ROW_BG_SELECTED
        } else {
            ROW_BG
        }));
    }
    if let Ok(mut hint) = hint.single_mut() {
        let rendered = selected.hint();
        if hint.0 != rendered {
            hint.0 = rendered.to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stepping saturates rather than wrapping — the property that keeps "hold right" from dropping
    /// a player from the highest setting to the lowest.
    #[test]
    fn stepping_saturates_at_both_ends() {
        let top = *ShadowDistance::ORDER.last().expect("the ladder has rungs");
        let bottom = *ShadowDistance::ORDER.first().expect("the ladder has rungs");
        let mut settings = Settings {
            shadow_distance: top,
            ..default()
        };
        Row::ShadowDistance.step(&mut settings, 1);
        assert_eq!(settings.shadow_distance, top, "top stays put");
        for _ in 0..ShadowDistance::ORDER.len() {
            Row::ShadowDistance.step(&mut settings, -1);
        }
        assert_eq!(settings.shadow_distance, bottom, "bottom stays put");
    }

    /// Every row responds to both directions, and lands on a value its own `value()` can render —
    /// the cheap guard against a row added to `ORDER` without arms in all three matches.
    #[test]
    fn every_row_steps_both_ways_and_renders() {
        for row in Row::ORDER {
            let mut settings = Settings::default();
            for delta in [1, -1, 1, 1, -1, -1] {
                row.step(&mut settings, delta);
                assert!(
                    !row.value(&settings).is_empty(),
                    "{row:?} produced an unrenderable value"
                );
            }
            assert!(!row.label().is_empty());
            assert!(!row.hint().is_empty(), "{row:?} needs a footer hint");
        }
    }

    /// MSAA reaches every rung, including `Off` — unlike shadows, it is not gameplay information.
    #[test]
    fn msaa_can_reach_off_and_the_top() {
        let mut settings = Settings::default();
        for _ in 0..5 {
            Row::Msaa.step(&mut settings, -1);
        }
        assert_eq!(settings.msaa, MsaaLevel::Off);
        for _ in 0..5 {
            Row::Msaa.step(&mut settings, 1);
        }
        assert_eq!(settings.msaa, MsaaLevel::X4);
    }

    /// The two shadow rows are INDEPENDENT, and the distance row reaches `OFF` — the UI half of the
    /// contract `settings::off_is_the_only_rung_that_stops_casting` pins in the model. Walking one
    /// row to each end must leave the other exactly where it was; before the split, one ladder moved
    /// both.
    #[test]
    fn the_shadow_rows_step_independently_and_distance_reaches_off() {
        let mut settings = Settings::default();
        let resolution_before = settings.shadow_resolution;
        for _ in 0..ShadowDistance::ORDER.len() {
            Row::ShadowDistance.step(&mut settings, -1);
        }
        assert!(
            !settings.shadow_distance.casts(),
            "holding left on the distance row must reach OFF"
        );
        assert_eq!(
            settings.shadow_resolution, resolution_before,
            "the distance row must not move the resolution row"
        );
        assert_eq!(Row::ShadowDistance.value(&settings), "OFF");

        let distance_before = settings.shadow_distance;
        for _ in 0..ShadowResolution::ORDER.len() {
            Row::ShadowResolution.step(&mut settings, 1);
        }
        assert_eq!(
            settings.shadow_resolution,
            *ShadowResolution::ORDER
                .last()
                .expect("the ladder has rungs"),
        );
        assert_eq!(
            settings.shadow_distance, distance_before,
            "the resolution row must not move the distance row — including out of OFF"
        );

        // And OFF is not a one-way door from the page either.
        Row::ShadowDistance.step(&mut settings, 1);
        assert!(settings.shadow_distance.casts());
    }

    /// The render-scale row steps the whole ladder in both directions and saturates at native — a
    /// player leaning on the right arrow must land on 100%, not wrap round to 50%.
    #[test]
    fn the_render_scale_row_walks_the_ladder_and_saturates_at_native() {
        let mut settings = Settings::default();
        assert_eq!(Row::RenderScale.value(&settings), "100%");
        for expected in [
            RenderScaleLevel::Percent85,
            RenderScaleLevel::Percent75,
            RenderScaleLevel::Percent67,
            RenderScaleLevel::Percent50,
            RenderScaleLevel::Percent50,
        ] {
            Row::RenderScale.step(&mut settings, -1);
            assert_eq!(settings.render_scale, expected);
        }
        for _ in 0..8 {
            Row::RenderScale.step(&mut settings, 1);
        }
        assert_eq!(
            settings.render_scale,
            RenderScaleLevel::Percent100,
            "holding right must stop at native, never wrap to the bottom rung"
        );
    }

    /// The page's FIRST row is render scale: it is the one setting with a large, immediate frame
    /// effect, so it is what a player looking for frames should meet first.
    #[test]
    fn render_scale_leads_the_page() {
        assert_eq!(Row::ORDER.first(), Some(&Row::RenderScale));
    }

    /// VSync is a two-rung LADDER like every other row — not a special-cased toggle. It saturates
    /// at both ends (the arm used to be `settings.vsync = delta > 0`, which could not), and both
    /// rungs are reachable and render.
    #[test]
    fn vsync_row_is_a_two_rung_ladder_that_saturates() {
        let mut settings = Settings::default();
        for _ in 0..3 {
            Row::VSync.step(&mut settings, -1);
        }
        assert_eq!(settings.vsync, VsyncMode::Off);
        assert_eq!(Row::VSync.value(&settings), "OFF");
        for _ in 0..3 {
            Row::VSync.step(&mut settings, 1);
        }
        assert_eq!(settings.vsync, VsyncMode::On);
        assert_eq!(Row::VSync.value(&settings), "ON");
    }

    /// The selection walks with the arrows and saturates at both ends, and a hover lands on exactly
    /// the row it names — the three sites that used to hand-roll a `min`/`saturating_sub`/`position`
    /// each, now one type.
    #[test]
    fn the_selection_saturates_and_a_hover_lands_on_its_row() {
        let mut selection = Selection::default();
        assert_eq!(selection.row(), Row::ORDER[0]);
        selection.step(-1);
        assert_eq!(selection.row(), Row::ORDER[0], "the top row stays put");
        for _ in 0..Row::ORDER.len() * 2 {
            selection.step(1);
        }
        assert_eq!(
            selection.row(),
            *Row::ORDER.last().expect("the page has rows"),
            "holding down must stop at the last row, never wrap to the first",
        );
        for row in Row::ORDER {
            selection.select(row);
            assert_eq!(selection.row(), row, "{row:?} did not survive a hover");
        }
        // A stale index (a row removed from ORDER between frames) resolves rather than panicking.
        assert_eq!(
            Selection(usize::MAX).row(),
            *Row::ORDER.last().expect("the page has rows"),
        );
    }

    /// The click split is the row's own centre, so the affordance follows the laid-out rect rather
    /// than a hard-coded window fraction.
    #[test]
    fn clicks_step_by_which_half_of_the_row_was_hit() {
        assert_eq!(click_delta(700.0, 640.0), 1, "right half increases");
        assert_eq!(click_delta(500.0, 640.0), -1, "left half decreases");
        assert_eq!(click_delta(640.0, 640.0), 1, "the centre resolves upward");
    }

    /// **The Retina regression, as a pure arithmetic statement.** Both arguments to [`click_delta`]
    /// are PHYSICAL pixels; feeding it the LOGICAL cursor (what `Window::cursor_position` returns)
    /// against a physical row centre silently inverts the split — see [`mouse_input`]'s doc for the
    /// full evidence, including the same mistake making `contains_point` miss every row entirely.
    ///
    /// The scenario is a 2x display and a click just right of a row's centre.
    #[test]
    fn the_click_split_needs_both_values_in_the_same_pixel_space() {
        const SCALE_FACTOR: f32 = 2.0;
        let row_center_physical = 1280.0;
        let cursor_physical = 1400.0;
        let cursor_logical = cursor_physical / SCALE_FACTOR;

        assert_eq!(
            click_delta(cursor_physical, row_center_physical),
            1,
            "a click right of centre steps up once both values are physical"
        );
        assert_eq!(
            click_delta(cursor_logical, row_center_physical),
            -1,
            "the un-converted cursor would step the row the WRONG way — the bug this pins",
        );
    }

    /// `step_in` clamps and tolerates a value that is not in the order (which the UI cannot produce,
    /// but a hand-edited config could hand us via serde).
    #[test]
    fn step_in_clamps_and_survives_an_unknown_current() {
        assert_eq!(step_in(&[1, 2, 3], 1, -5), 1);
        assert_eq!(step_in(&[1, 2, 3], 3, 5), 3);
        assert_eq!(step_in(&[1, 2, 3], 2, 1), 3);
        assert_eq!(step_in(&[1, 2, 3], 99, 1), 2, "unknown current starts at 0");
    }
}
