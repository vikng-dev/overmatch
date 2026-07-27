//! The player-facing settings page: a titled card of rows — `LABEL  < VALUE >` steppers for the
//! enum ladders, `LABEL  [====----] VALUE` sliders for the scalar ones.
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
//! * `Up`/`Down` — move the selection, SKIPPING disabled rows (the frame cap while vsync is not
//!   OFF). `W`/`S` are deliberately NOT bound: they drive the tank, and a settings page that also
//!   reads the drive keys teaches the wrong reflex.
//! * `Left`/`Right` — change the selected row's value: one ladder rung on a stepper, one stop on a
//!   slider.
//! * Mouse — hover selects; the `<` `>` glyphs are REAL affordances (visible, hover-highlighted,
//!   clickable — the invisible click-the-half-of-the-row scheme they replace was pure mystery
//!   meat); a slider's track is click-to-set and drag-to-scrub. Every hit test is
//!   `ComputedNode::contains_point`, i.e. bevy's own laid-out rect, so it cannot drift from what is
//!   drawn. That rect is in PHYSICAL pixels and the cursor has to be converted to match —
//!   [`mouse_input`]'s doc records the bug class, because getting it wrong makes the mouse dead in
//!   total silence.
//!
//! Every change saves immediately — there is no Apply button to forget, and an atomic write makes
//! that safe (`settings::save`). The one refinement: a slider DRAG saves once on release, not once
//! per dragged frame. A drag also FREEZES its track's rect at the grab and maps the cursor through
//! that for its whole life ([`ActiveDrag`]) — the UI SCALE row re-lays out the page it is being
//! dragged on, so a live rect would be an output of the value it computes.

use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::{
    FrameCap, MsaaLevel, PresentCaps, RenderScaleLevel, SaveSettings, Settings, ShadowCascades,
    ShadowDistance, ShadowResolution, UiScalePercent, VsyncMode, WindowModeSetting,
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
/// **The extension point**: add a variant, add it to [`Row::ORDER`], give it arms in
/// [`Row::label`] / [`Row::hint`] / [`Row::value`] / [`Row::step`] (and, for a scalar,
/// [`Row::kind`] + the two slider-fraction arms). Nothing else in this file, and nothing outside
/// it, needs to change — the card lays out from `ORDER`, and `Settings` already accepts new fields
/// without a migration (see the `settings` module doc).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    RenderScale,
    /// Windowed / borderless fullscreen. Reflects OS-side toggles too — see
    /// `settings::observe_window_mode`.
    WindowMode,
    /// How far shadows reach, `OFF` included — see [`ShadowDistance`] for why this is its own row.
    ShadowDistance,
    /// How crisp they are. Adjacent to the distance row so the pair reads as one subject, but it
    /// steps independently.
    ShadowResolution,
    /// How many cascades split that distance. Live only by grace of the vendored bevy_light
    /// backport — see `settings::SHADOW_CASCADES`. Inert (not disabled) while shadows are off:
    /// the change applies on the next casting frame, same as the resolution row's pattern of
    /// staying interactive whatever the distance says.
    ShadowCascades,
    Msaa,
    /// Capability-gated: only the rungs the surface's probe reported are offered ([`PresentCaps`]).
    VSync,
    /// The one row that can be DISABLED: it renders greyed and is skipped by every input path
    /// unless vsync is OFF (see [`Row::enabled`]).
    FrameCap,
    UiScale,
}

/// How a row's control renders and responds: a `< VALUE >` stepper or a draggable slider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowKind {
    Stepper,
    Slider,
}

impl Row {
    const ORDER: [Row; 9] = [
        Row::RenderScale,
        Row::WindowMode,
        Row::ShadowDistance,
        Row::ShadowResolution,
        Row::ShadowCascades,
        Row::Msaa,
        Row::VSync,
        Row::FrameCap,
        Row::UiScale,
    ];

    /// The row's name. ASCII only — it reaches `Text`.
    const fn label(self) -> &'static str {
        match self {
            Row::RenderScale => "RENDER SCALE",
            Row::WindowMode => "WINDOW MODE",
            Row::ShadowDistance => "SHADOW DISTANCE",
            // "QUALITY", not "RESOLUTION": the value beside it is already a texel count, so the
            // label's job is to say what the number buys, not to name its unit twice.
            Row::ShadowResolution => "SHADOW QUALITY",
            Row::ShadowCascades => "SHADOW CASCADES",
            Row::Msaa => "ANTI-ALIASING",
            Row::VSync => "VSYNC",
            Row::FrameCap => "FRAME CAP",
            Row::UiScale => "UI SCALE",
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
            Row::WindowMode => {
                "Borderless fullscreen on the current display. \
                 The OS's own fullscreen button is reflected here too."
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
            // Honest about the trade: each cascade is a shadow render pass, and fewer of them
            // means the near ones cover more ground per texel.
            Row::ShadowCascades => {
                "How many slices the shadow distance is split into. \
                 Fewer is cheaper but blockier up close."
            }
            Row::Msaa => "Smooths jagged edges. The cheapest setting to drop for more frames.",
            Row::VSync => {
                "ON locks frames to the display. FAST is uncapped without tearing. \
                 OFF can tear, and enables the frame cap. Only supported modes are offered."
            }
            Row::FrameCap => {
                "Caps the frame rate to limit heat and power draw. Active only while VSYNC is OFF."
            }
            Row::UiScale => "Scales the HUD and menus. The 3D world is unaffected.",
        }
    }

    /// Stepper or slider — the scalar rows are sliders (the owner's explicit ask: a scalar deserves
    /// a scrubbable control, not a disguised ladder).
    const fn kind(self) -> RowKind {
        match self {
            Row::RenderScale | Row::FrameCap | Row::UiScale => RowKind::Slider,
            _ => RowKind::Stepper,
        }
    }

    /// Whether the row currently responds at all. A disabled row renders dim and is skipped by the
    /// keyboard walk, the hover, and every click path. The frame cap is the only conditional row:
    /// its gate is the SAME fact `Settings::frame_limit_period` limits by, so the grey row and the
    /// idle limiter cannot disagree.
    ///
    /// `caps` is what makes that literally true rather than nearly true — the gate is the EFFECTIVE
    /// rung (`Settings::effective_vsync`), so a stored OFF on a surface that REPORTED it cannot
    /// present it greys the row exactly as the limiter declines to arm.
    ///
    /// Under an unknown capability state — pre-probe, or a probe that could not ask — a stored OFF
    /// keeps the row LIT. That is the deliberate choice: nothing was learned, so the player's own
    /// rung is the best statement of what they are on, and greying a row on a failure the player
    /// cannot see or fix would take their frame cap away for no evidence at all. It costs nothing to
    /// be wrong here — the limiter reads the same fact, so the row and the cap agree either way.
    fn enabled(self, settings: &Settings, caps: PresentCaps) -> bool {
        match self {
            Row::FrameCap => settings.effective_vsync(caps) == VsyncMode::Off,
            _ => true,
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

    /// The row's current value, as rendered. ASCII only — it reaches `Text`. A `String` since the
    /// scalar rows format live numbers.
    fn value(self, settings: &Settings) -> String {
        match self {
            Row::RenderScale => settings.render_scale.label().to_string(),
            Row::WindowMode => settings.window_mode.label().to_string(),
            Row::ShadowDistance => settings.shadow_distance.label().to_string(),
            Row::ShadowResolution => settings.shadow_resolution.label().to_string(),
            Row::ShadowCascades => settings.shadow_cascades.label().to_string(),
            Row::Msaa => settings.msaa.label().to_string(),
            Row::VSync => settings.vsync.label().to_string(),
            Row::FrameCap => settings.frame_cap.label(),
            Row::UiScale => settings.ui_scale.label(),
        }
    }

    /// Step this row by `delta` (`+1` / `-1`), saturating at the ends rather than wrapping.
    ///
    /// Saturating is the deliberate choice for a page a player reads: a wrapping list turns "press
    /// right until it stops" into an accidental jump from the highest setting to the lowest — and
    /// with `OFF` the floor of the shadow-distance ladder, a wrap would put "no shadows at all"
    /// one keypress past the most expensive setting.
    ///
    /// `caps` gates the vsync ladder: the walk happens INSIDE the offered rungs, so a Metal surface
    /// steps ON <-> OFF without ever visiting the unoffered FAST.
    fn step(self, settings: &mut Settings, delta: i32, caps: PresentCaps) {
        match self {
            Row::RenderScale => {
                settings.render_scale =
                    step_in(&RenderScaleLevel::ORDER, settings.render_scale, delta);
            }
            Row::WindowMode => {
                settings.window_mode =
                    step_in(&WindowModeSetting::ORDER, settings.window_mode, delta);
            }
            Row::ShadowDistance => {
                settings.shadow_distance =
                    step_in(&ShadowDistance::ORDER, settings.shadow_distance, delta);
            }
            Row::ShadowResolution => {
                settings.shadow_resolution =
                    step_in(&ShadowResolution::ORDER, settings.shadow_resolution, delta);
            }
            Row::ShadowCascades => {
                settings.shadow_cascades =
                    step_in(&ShadowCascades::ORDER, settings.shadow_cascades, delta);
            }
            Row::Msaa => settings.msaa = step_in(&MsaaLevel::ORDER, settings.msaa, delta),
            Row::VSync => {
                let offered: Vec<VsyncMode> = VsyncMode::ORDER
                    .into_iter()
                    .filter(|mode| caps.offers(*mode))
                    .collect();
                settings.vsync = step_in(&offered, settings.vsync, delta);
            }
            Row::FrameCap => settings.frame_cap = settings.frame_cap.step(delta),
            Row::UiScale => settings.ui_scale = settings.ui_scale.step(delta),
        }
    }

    /// Where this row's slider handle sits, `0.0..=1.0` — `None` for the steppers. Kept beside
    /// [`Row::set_from_fraction`] so the two directions of the mapping cannot drift.
    fn slider_fraction(self, settings: &Settings) -> Option<f32> {
        match self {
            Row::RenderScale => Some(ladder_fraction(
                &RenderScaleLevel::ORDER,
                settings.render_scale,
            )),
            Row::FrameCap => Some(settings.frame_cap.fraction()),
            Row::UiScale => Some(settings.ui_scale.fraction()),
            _ => None,
        }
    }

    /// Set a slider row from a drag position. Snaps to the row's own ladder stops — every value a
    /// drag can produce is a value the keyboard could also have reached.
    fn set_from_fraction(self, settings: &mut Settings, fraction: f32) {
        match self {
            Row::RenderScale => {
                settings.render_scale = ladder_at_fraction(&RenderScaleLevel::ORDER, fraction);
            }
            Row::FrameCap => settings.frame_cap = FrameCap::from_fraction(fraction),
            Row::UiScale => settings.ui_scale = UiScalePercent::from_fraction(fraction),
            _ => {}
        }
    }
}

/// Move `current` by `delta` places inside `order`, clamped to the ends. `current` missing from
/// `order` cannot happen through the UI for the static ladders (a capability-gated vsync value CAN
/// be missing — a config carried from another machine), and resolves to the first entry rather than
/// panicking.
fn step_in<T: Copy + PartialEq>(order: &[T], current: T, delta: i32) -> T {
    let index = order
        .iter()
        .position(|entry| *entry == current)
        .unwrap_or(0);
    let next = (index as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
    order[next]
}

/// Where `current` sits in a discrete ladder, as a `0.0..=1.0` slider position.
fn ladder_fraction<T: Copy + PartialEq>(order: &[T], current: T) -> f32 {
    let index = order
        .iter()
        .position(|entry| *entry == current)
        .unwrap_or(0);
    index as f32 / (order.len().saturating_sub(1)).max(1) as f32
}

/// The ladder entry a slider position lands on — the inverse of [`ladder_fraction`], rounded to the
/// nearest stop.
fn ladder_at_fraction<T: Copy>(order: &[T], fraction: f32) -> T {
    let last = order.len() - 1;
    let index = (fraction.clamp(0.0, 1.0) * last as f32).round() as usize;
    order[index.min(last)]
}

/// The selected row index into [`Row::ORDER`]. Only ever moved through [`Selection::step`] /
/// [`Selection::select`] and read through [`Selection::row`], so the clamp lives in one place
/// instead of at each of the five sites that used to hand-roll it.
#[derive(Resource, Default)]
struct Selection(usize);

impl Selection {
    /// The last valid index. `Row::ORDER` is a non-empty array literal, so this cannot underflow.
    const LAST: usize = Row::ORDER.len() - 1;

    /// Move the selection one row at a time in `delta`'s direction, SKIPPING rows `enabled`
    /// refuses, saturating at the ends — "hold down" stops at the last enabled row rather than
    /// jumping back to the top, and a disabled row is never landed on.
    fn step(&mut self, delta: i32, enabled: impl Fn(Row) -> bool) {
        let direction = delta.signum();
        if direction == 0 {
            return;
        }
        let mut index = self.0 as i32;
        loop {
            index += direction;
            if index < 0 || index > Self::LAST as i32 {
                return; // Ran off the end without finding an enabled row: stay put.
            }
            if enabled(Row::ORDER[index as usize]) {
                self.0 = index as usize;
                return;
            }
        }
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

/// The `LABEL` text of a row — carried so a disabled row can dim its label.
#[derive(Component)]
struct RowLabelText(Row);

/// The `VALUE` text of a row, rebuilt from [`Settings`].
#[derive(Component)]
struct RowValueText(Row);

/// The row's whole clickable line — carries the [`Row`] and is what hover-selection hit-tests
/// against.
#[derive(Component)]
struct RowLine(Row);

/// One of a stepper row's `<` / `>` glyphs: a visible, hover-highlighted, clickable affordance.
/// `delta` is what a click applies (`-1` left, `+1` right).
#[derive(Component, Clone, Copy, PartialEq)]
struct ArrowGlyph {
    row: Row,
    delta: i32,
}

/// A slider row's clickable track (the full-height hit area, wider than the visible bar so it is
/// actually clickable).
#[derive(Component)]
struct SliderTrack(Row);

/// The filled portion of a slider's bar — its width IS the value.
#[derive(Component)]
struct SliderFill(Row);

/// The footer hint line — the text itself, which [`refresh_page`] rewrites as the selection moves.
#[derive(Component)]
struct HintText;

/// The fixed-height box the hint is drawn into: always exactly two lines tall, whether the selected
/// row's hint fills one line or two, so walking the selection never moves the rows above it. See
/// [`HINT_SLOT_HEIGHT_PX`].
#[derive(Component)]
struct HintSlot;

/// Which arrow glyph the cursor is over this frame — written by [`mouse_input`], read by
/// [`refresh_page`] for the hover highlight.
#[derive(Resource, Default, PartialEq)]
struct HoveredArrow(Option<(Row, i32)>);

/// The slider drag in progress, if any. While `Some`, the mouse belongs to that slider: the value
/// scrubs with the cursor every frame (even outside the track's rect — standard slider behaviour),
/// and the SAVE happens once, on release, instead of once per dragged frame.
#[derive(Resource, Default)]
struct SliderDrag(Option<ActiveDrag>);

/// A drag in flight: which row is being scrubbed, and the track geometry FROZEN at the grab — both
/// in PHYSICAL pixels, straight off the `UiGlobalTransform`/`ComputedNode` the hit test used to
/// decide the grab landed on this track.
///
/// # Why the geometry is frozen rather than re-read each frame
///
/// The mapping cursor→value has to divide by a track rect. Reading the LIVE rect every frame makes
/// that rect an output of the very value it is used to compute, whenever a row's effect resizes the
/// page: [`Row::UiScale`] writes bevy's `UiScale` through `settings::apply_settings` the same frame
/// the drag moves it, `ui_layout_system` then re-lays the whole card — INCLUDING the track under the
/// cursor — and the next frame maps the same physical cursor position through a different, wider or
/// narrower rect. That is a closed loop: value → layout → rect → value, with the player's hand held
/// still. MEASURED as jitter/run-away while scrubbing the UI SCALE row.
///
/// Freezing at the grab cuts the loop without costing the live preview: the page still visibly
/// rescales as you scrub (that feedback is the point of the row), but the number it lands on is a
/// function of the cursor and the grab-time rect ALONE.
///
/// It is applied to EVERY slider, not just the one row with the loop today. The frozen rect is
/// strictly more correct for all of them — a slider whose track moves mid-drag for any reason (a
/// value-well text width, a future row that resizes the card, a window resize under the cursor)
/// otherwise shifts the value under a stationary hand — and one mechanism is a mechanism nobody can
/// forget to extend to the next resizing setting.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ActiveDrag {
    row: Row,
    /// The track's centre on x at grab time, PHYSICAL px (`UiGlobalTransform::translation.x`).
    track_center_x: f32,
    /// The track's width at grab time, PHYSICAL px (`ComputedNode::size().x`).
    track_width: f32,
}

impl ActiveDrag {
    /// Grab this track: freeze what the hit test just measured. Physical px in, physical px stored —
    /// see [`track_fraction`] for why mixing spaces here is a silent 2x error on Retina.
    fn grab(row: Row, node: &ComputedNode, transform: &UiGlobalTransform) -> Self {
        Self {
            row,
            track_center_x: transform.translation.x,
            track_width: node.size().x,
        }
    }

    /// Where `cursor_x` (PHYSICAL px) sits along the FROZEN track — the whole point of the type.
    fn fraction(self, cursor_x: f32) -> f32 {
        track_fraction(cursor_x, self.track_center_x, self.track_width)
    }
}

/// The selection highlight, and its unselected counterpart. Kept next to each other so the contrast
/// between them is a decision rather than two literals that drifted. The card slab and both text
/// colours are the shared family constants in `ui_font`, not local copies.
const ROW_BG_SELECTED: Color = Color::srgba(0.16, 0.28, 0.36, 0.95);
const ROW_BG: Color = Color::NONE;

/// The slider family: the empty track, the filled bar, and the filled bar of a DISABLED row. The
/// fill leans on the same cold blue-white family as `ui_font::TEXT`.
const SLIDER_TRACK_BG: Color = Color::srgba(0.10, 0.14, 0.18, 0.9);
const SLIDER_FILL_COLOR: Color = Color::srgb(0.45, 0.72, 0.92);
const SLIDER_FILL_DISABLED: Color = Color::srgb(0.28, 0.34, 0.40);

/// Layout constants shared by [`spawn_page`]: the slider track's clickable width/height, and the
/// fixed value-text wells that keep the controls from shifting as the value's text width changes.
const TRACK_WIDTH_PX: f32 = 150.0;
const TRACK_HIT_HEIGHT_PX: f32 = 18.0;
const TRACK_BAR_HEIGHT_PX: f32 = 6.0;
const STEPPER_VALUE_WELL_PX: f32 = 110.0;
const SLIDER_VALUE_WELL_PX: f32 = 64.0;

/// The footer hint's type block, and the FIXED slot it is drawn into.
///
/// A hint is one or two lines depending on the row ([`Row::hint`]), so a slot sized to its text
/// grows and shrinks as the SELECTION moves: every row above it shifts by a line, and the whole card
/// changes height, while the player is only walking the list. The slot is therefore always exactly
/// two lines tall and a one-line hint simply leaves the second line empty.
///
/// Two constants, because the reservation and the drawing must agree:
///
/// * [`HINT_LINE_HEIGHT_PX`] is written onto the hint as an explicit `LineHeight` rather than left
///   to bevy's default (`RelativeToFont(1.2)`, i.e. the same 16.8 px today). Same pixels, but now
///   the number the layout reserves is the number the text is laid out with, instead of two
///   independent statements that a future bevy default could quietly separate.
/// * [`HINT_SLOT_HEIGHT_PX`] is that line height twice, applied as `min_height`. `min_height`, not
///   `height`: the reservation is a floor, so copy that outgrew it would still be READ (it would
///   shift the layout, and that is the honest failure) rather than silently clipped.
///   `the_hint_slot_is_two_lines_and_every_hint_fits_it` is what stops it coming to that — it
///   measures every hint against the card's content width in the shipped font.
///
/// **The `min_height` goes on a WRAPPER node ([`HintSlot`]), never on the text itself.** Bevy 0.19's
/// text measure resolves a node's effective height as `known.or(preferred.or(min))` — with no
/// `height` set, a `min_height` IS the answer, and the text's own measured height is never consulted
/// (`bevy_ui::measurement::resolve_axis`). A `min_height` on the `Text` node is therefore a FIXED
/// height in disguise: three lines of copy would be clipped to two, silently, which is the one
/// outcome the floor above is written to avoid. MEASURED — the control leg of the test caught this
/// (an overlong string laid out in a node that never grew).
///
/// Neither node carries padding, so the slot is the two lines alone; the top margin sits outside the
/// box and is unaffected either way.
const HINT_FONT_PX: f32 = 14.0;
const HINT_LINE_HEIGHT_PX: f32 = HINT_FONT_PX * 1.2;
const HINT_SLOT_HEIGHT_PX: f32 = HINT_LINE_HEIGHT_PX * 2.0;

/// The card's outer width and its padding. Named rather than inline because their DIFFERENCE — 512
/// logical px — is the width every hint has to wrap inside, i.e. the whole budget behind the
/// two-line slot above.
const CARD_WIDTH_PX: f32 = 560.0;
const CARD_PADDING_PX: f32 = 24.0;

/// The shared page: spawn, input, refresh. Mounted by `settings::plugin`, which also adds exactly
/// one of the ENTRY declarers ([`declare_from_overlay_menu`] / [`declare_from_pause_state`]) — they
/// are the one thing the two windowed roots do differently, which is why they are that plugin's
/// single parameter rather than a system each root must remember.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<SettingsPageVisible>()
        .init_resource::<Selection>()
        .init_resource::<HoveredArrow>()
        .init_resource::<SliderDrag>()
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
                        width: Val::Px(CARD_WIDTH_PX),
                        padding: UiRect::all(Val::Px(CARD_PADDING_PX)),
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
                        spawn_row(card, &fonts, &settings, row);
                    }

                    // The hint's fixed two-line SLOT, with the hint text inside it. See
                    // [`HINT_SLOT_HEIGHT_PX`] for why the reservation is a node of its own rather
                    // than a `min_height` on the text.
                    card.spawn((
                        HintSlot,
                        Node {
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(14.0)),
                            min_height: Val::Px(HINT_SLOT_HEIGHT_PX),
                            // The hint sits on the FIRST line and leaves the second empty.
                            // Stretching (the default) would instead hand the text node the
                            // slot's whole height, leaving the text's own height nowhere to be
                            // read — which is how the pinning below hides.
                            align_items: AlignItems::FlexStart,
                            ..default()
                        },
                    ))
                    .with_children(|slot| {
                        slot.spawn((
                            HintText,
                            Text::new(""),
                            TextFont {
                                // Regular: the quiet explanatory footer.
                                font: fonts.body.clone().into(),
                                font_size: FontSize::Px(HINT_FONT_PX),
                                ..default()
                            },
                            LineHeight::Px(HINT_LINE_HEIGHT_PX),
                            TextColor(TEXT_DIM),
                        ));
                    });
                    card.spawn((
                        Text::new(
                            "Arrows or drag to change  \u{2014}  Esc to close  \u{2014}  saved automatically",
                        ),
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

/// One row: `LABEL` on the left; on the right either `< VALUE >` (stepper) or `[track] VALUE`
/// (slider). The value sits in a fixed-width well so the arrows and track do not shuffle as the
/// value text's width changes.
fn spawn_row(card: &mut ChildSpawnerCommands, fonts: &UiFonts, settings: &Settings, row: Row) {
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
            RowLabelText(row),
            Text::new(row.label()),
            TextFont {
                // Regular: a dense settings row.
                font: fonts.body.clone().into(),
                font_size: FontSize::Px(18.0),
                ..default()
            },
            TextColor(TEXT),
        ));
        line.spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(6.0),
            ..default()
        })
        .with_children(|controls| match row.kind() {
            RowKind::Stepper => {
                spawn_arrow(controls, fonts, row, -1, "<");
                controls
                    .spawn(Node {
                        width: Val::Px(STEPPER_VALUE_WELL_PX),
                        justify_content: JustifyContent::Center,
                        ..default()
                    })
                    .with_children(|well| {
                        spawn_value_text(well, fonts, settings, row);
                    });
                spawn_arrow(controls, fonts, row, 1, ">");
            }
            RowKind::Slider => {
                controls
                    .spawn((
                        SliderTrack(row),
                        Node {
                            width: Val::Px(TRACK_WIDTH_PX),
                            height: Val::Px(TRACK_HIT_HEIGHT_PX),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                    ))
                    .with_children(|track| {
                        track
                            .spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(TRACK_BAR_HEIGHT_PX),
                                    border_radius: BorderRadius::all(Val::Px(3.0)),
                                    ..default()
                                },
                                BackgroundColor(SLIDER_TRACK_BG),
                            ))
                            .with_children(|bar| {
                                let fraction = row.slider_fraction(settings).unwrap_or(0.0);
                                bar.spawn((
                                    SliderFill(row),
                                    Node {
                                        width: Val::Percent(fraction * 100.0),
                                        height: Val::Percent(100.0),
                                        border_radius: BorderRadius::all(Val::Px(3.0)),
                                        ..default()
                                    },
                                    BackgroundColor(SLIDER_FILL_COLOR),
                                ));
                            });
                    });
                controls
                    .spawn(Node {
                        width: Val::Px(SLIDER_VALUE_WELL_PX),
                        justify_content: JustifyContent::FlexEnd,
                        ..default()
                    })
                    .with_children(|well| {
                        spawn_value_text(well, fonts, settings, row);
                    });
            }
        });
    });
}

/// A `<` / `>` affordance: dim at rest, bright on hover ([`refresh_page`] paints it from
/// [`HoveredArrow`]), padded so its clickable rect is meaningfully larger than the glyph.
fn spawn_arrow(
    controls: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    row: Row,
    delta: i32,
    glyph: &str,
) {
    controls.spawn((
        ArrowGlyph { row, delta },
        Text::new(glyph),
        TextFont {
            // SemiBold: an affordance, not prose.
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(TEXT_DIM),
        Node {
            padding: UiRect::axes(Val::Px(8.0), Val::Px(2.0)),
            ..default()
        },
    ));
}

fn spawn_value_text(
    well: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    settings: &Settings,
    row: Row,
) {
    well.spawn((
        RowValueText(row),
        Text::new(row.value(settings)),
        TextFont {
            // SemiBold: the value is what the eye goes to.
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(TEXT),
    ));
}

/// Arrow keys only — see the module doc on why `W`/`S` are not bound.
fn keyboard_input(
    visible: Res<SettingsPageVisible>,
    keys: Res<ButtonInput<KeyCode>>,
    caps: Res<PresentCaps>,
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
        let current = *settings;
        selection.step(move_by, |row| row.enabled(&current, *caps));
    }
    let delta = i32::from(keys.just_pressed(KeyCode::ArrowRight))
        - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
    if delta != 0 && selection.row().enabled(&settings, *caps) {
        change_row(selection.row(), delta, &mut settings, &mut save, *caps);
    }
}

/// Hover selects; the `<` `>` glyphs step on click; a slider's track sets on click and scrubs on
/// drag. All hit tests use bevy's own laid-out rect (`ComputedNode::contains_point`), so the
/// affordance matches the pixels at any window size or UI scale.
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
/// [`track_fraction`] then works in that same physical space — cursor and track rect MUST be
/// converted together or a drag lands the handle away from the cursor. The rect it works against
/// during a drag is the one [`ActiveDrag`] froze at the grab, in that same physical space, for the
/// feedback-loop reason that type's doc records.
fn mouse_input(
    visible: Res<SettingsPageVisible>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    lines: Query<(&RowLine, &ComputedNode, &UiGlobalTransform)>,
    arrows: Query<(&ArrowGlyph, &ComputedNode, &UiGlobalTransform)>,
    tracks: Query<(&SliderTrack, &ComputedNode, &UiGlobalTransform)>,
    caps: Res<PresentCaps>,
    mut selection: ResMut<Selection>,
    mut hovered_arrow: ResMut<HoveredArrow>,
    mut drag: ResMut<SliderDrag>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    let cursor = window.and_then(|window| window.physical_cursor_position());

    // A drag in progress owns the mouse outright: scrub while held, save ONCE on release (or on
    // the page closing under the drag), and never hover/click anything else meanwhile.
    //
    // The mapping deliberately does NOT consult `tracks` here — the geometry frozen at the grab is
    // the whole rect this drag will ever use. See [`ActiveDrag`] for the layout feedback loop that
    // re-reading the live rect closes.
    if let Some(active) = drag.0 {
        if !visible.0 || !buttons.pressed(MouseButton::Left) {
            drag.0 = None;
            save.write(SaveSettings);
            info!(
                "settings: {} -> {}",
                active.row.label(),
                active.row.value(&settings)
            );
        } else if let Some(cursor) = cursor {
            let mut next = *settings;
            active
                .row
                .set_from_fraction(&mut next, active.fraction(cursor.x));
            if next != *settings {
                *settings = next;
            }
        }
        hovered_arrow.set_if_neq(HoveredArrow(None));
        return;
    }

    if !visible.0 {
        hovered_arrow.set_if_neq(HoveredArrow(None));
        return;
    }
    let Some(cursor) = cursor else {
        hovered_arrow.set_if_neq(HoveredArrow(None));
        return;
    };
    let clicked = buttons.just_pressed(MouseButton::Left);

    // Arrows first: they sit INSIDE a row line, and the more specific target wins.
    let mut arrow_hit = None;
    for (arrow, node, transform) in &arrows {
        if node.contains_point(*transform, cursor) && arrow.row.enabled(&settings, *caps) {
            arrow_hit = Some(*arrow);
            break;
        }
    }
    hovered_arrow.set_if_neq(HoveredArrow(
        arrow_hit.map(|arrow| (arrow.row, arrow.delta)),
    ));

    // Hover alone moves the selection (enabled rows only — a disabled row cannot be "described" by
    // the footer hint it is excluded from acting on), so keyboard and mouse never disagree about
    // which row the hint is describing.
    for (line, node, transform) in &lines {
        if node.contains_point(*transform, cursor) && line.0.enabled(&settings, *caps) {
            selection.select(line.0);
            break;
        }
    }

    if !clicked {
        return;
    }
    if let Some(arrow) = arrow_hit {
        change_row(arrow.row, arrow.delta, &mut settings, &mut save, *caps);
        return;
    }
    for (track, node, transform) in &tracks {
        if track.0.enabled(&settings, *caps) && node.contains_point(*transform, cursor) {
            selection.select(track.0);
            // Freeze the rect the hit test just used — this click and every frame of the drag it
            // starts map through THIS geometry, whatever the layout does meanwhile.
            let active = ActiveDrag::grab(track.0, node, transform);
            drag.0 = Some(active);
            let mut next = *settings;
            active
                .row
                .set_from_fraction(&mut next, active.fraction(cursor.x));
            if next != *settings {
                // No save here — the release path above writes the file once per drag.
                *settings = next;
            }
            return;
        }
    }
}

/// Where along a slider track a cursor sits, `0.0..=1.0`. Pure, so the mapping is testable without
/// a laid-out UI.
///
/// **All three arguments are PHYSICAL pixels** (`track_center_x`/`track_width` come from
/// `UiGlobalTransform`/`ComputedNode`, which are physical). A logical cursor fed in here would not
/// fail loudly — it would scrub the handle to the wrong place, at exactly 2x the error on the
/// Retina displays this project targets. Same bug class as [`mouse_input`]'s doc records.
fn track_fraction(cursor_x: f32, track_center_x: f32, track_width: f32) -> f32 {
    if track_width <= 0.0 {
        return 0.0;
    }
    ((cursor_x - (track_center_x - track_width / 2.0)) / track_width).clamp(0.0, 1.0)
}

/// Apply one step and — only if the value actually moved — request a save. The guard is what keeps a
/// player leaning on the right arrow at the top of a ladder from rewriting the file every frame.
fn change_row(
    row: Row,
    delta: i32,
    settings: &mut ResMut<Settings>,
    save: &mut MessageWriter<SaveSettings>,
    caps: PresentCaps,
) {
    let before = **settings;
    let mut next = before;
    row.step(&mut next, delta, caps);
    if next == before {
        return;
    }
    // Through `ResMut`'s `DerefMut` so change detection fires and `apply_settings` runs.
    **settings = next;
    save.write(SaveSettings);
    info!("settings: {} -> {}", row.label(), row.value(&next));
}

/// Rebuild every value, the selection highlight, the slider fills, the enabled/disabled colours and
/// the footer hint. Runs only while the page is up; the visibility write itself is `set_if_neq`, so
/// a closed page costs one comparison per frame.
fn refresh_page(
    visible: Res<SettingsPageVisible>,
    settings: Res<Settings>,
    // The greying reads the EFFECTIVE vsync rung, not the stored one — see [`Row::enabled`].
    caps: Res<PresentCaps>,
    selection: Res<Selection>,
    hovered_arrow: Res<HoveredArrow>,
    mut card: Query<&mut Visibility, With<SettingsCard>>,
    mut values: Query<
        (&RowValueText, &mut Text, &mut TextColor),
        (
            Without<HintText>,
            Without<RowLabelText>,
            Without<ArrowGlyph>,
        ),
    >,
    mut labels: Query<
        (&RowLabelText, &mut TextColor),
        (
            Without<HintText>,
            Without<RowValueText>,
            Without<ArrowGlyph>,
        ),
    >,
    mut arrow_glyphs: Query<
        (&ArrowGlyph, &mut TextColor),
        (
            Without<HintText>,
            Without<RowValueText>,
            Without<RowLabelText>,
        ),
    >,
    mut fills: Query<(&SliderFill, &mut Node, &mut BackgroundColor), Without<RowLine>>,
    mut lines: Query<(&RowLine, &mut BackgroundColor), Without<SliderFill>>,
    mut hint: Query<&mut Text, (With<HintText>, Without<RowValueText>)>,
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
    for (value, mut text, mut color) in &mut values {
        let rendered = value.0.value(&settings);
        if text.0 != rendered {
            text.0 = rendered;
        }
        color.set_if_neq(TextColor(if value.0.enabled(&settings, *caps) {
            TEXT
        } else {
            TEXT_DIM
        }));
    }
    for (label, mut color) in &mut labels {
        color.set_if_neq(TextColor(if label.0.enabled(&settings, *caps) {
            TEXT
        } else {
            TEXT_DIM
        }));
    }
    for (arrow, mut color) in &mut arrow_glyphs {
        let hovered = hovered_arrow.0 == Some((arrow.row, arrow.delta));
        color.set_if_neq(TextColor(if hovered { TEXT } else { TEXT_DIM }));
    }
    for (fill, mut node, mut color) in &mut fills {
        let width = Val::Percent(fill.0.slider_fraction(&settings).unwrap_or(0.0) * 100.0);
        if node.width != width {
            node.width = width;
        }
        color.set_if_neq(BackgroundColor(if fill.0.enabled(&settings, *caps) {
            SLIDER_FILL_COLOR
        } else {
            SLIDER_FILL_DISABLED
        }));
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

    /// The unprobed capability state every pure test steps under: all rungs offered.
    const ALL_RUNGS: PresentCaps = PresentCaps::Unprobed;

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
        Row::ShadowDistance.step(&mut settings, 1, ALL_RUNGS);
        assert_eq!(settings.shadow_distance, top, "top stays put");
        for _ in 0..ShadowDistance::ORDER.len() {
            Row::ShadowDistance.step(&mut settings, -1, ALL_RUNGS);
        }
        assert_eq!(settings.shadow_distance, bottom, "bottom stays put");
    }

    /// Every row responds to both directions, and lands on a value its own `value()` can render —
    /// the cheap guard against a row added to `ORDER` without arms in all the matches.
    #[test]
    fn every_row_steps_both_ways_and_renders() {
        for row in Row::ORDER {
            let mut settings = Settings::default();
            for delta in [1, -1, 1, 1, -1, -1] {
                row.step(&mut settings, delta, ALL_RUNGS);
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
            Row::Msaa.step(&mut settings, -1, ALL_RUNGS);
        }
        assert_eq!(settings.msaa, MsaaLevel::Off);
        for _ in 0..5 {
            Row::Msaa.step(&mut settings, 1, ALL_RUNGS);
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
            Row::ShadowDistance.step(&mut settings, -1, ALL_RUNGS);
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
            Row::ShadowResolution.step(&mut settings, 1, ALL_RUNGS);
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
        Row::ShadowDistance.step(&mut settings, 1, ALL_RUNGS);
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
            Row::RenderScale.step(&mut settings, -1, ALL_RUNGS);
            assert_eq!(settings.render_scale, expected);
        }
        for _ in 0..8 {
            Row::RenderScale.step(&mut settings, 1, ALL_RUNGS);
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

    /// The vsync row walks only the rungs the probe offered: on a Metal-shaped surface (no
    /// Mailbox) the walk goes ON <-> OFF and FAST is never visited; unprobed walks all three; a
    /// current value the caps no longer offer (a config from another machine) still steps out into
    /// the offered set instead of panicking.
    #[test]
    fn the_vsync_row_walks_only_the_offered_rungs() {
        let metal = PresentCaps::Reported {
            immediate: true,
            mailbox: false,
        };
        let mut settings = Settings::default();
        assert_eq!(settings.vsync, VsyncMode::On);
        Row::VSync.step(&mut settings, -1, metal);
        assert_eq!(
            settings.vsync,
            VsyncMode::Off,
            "one step down on Metal must skip the unoffered FAST"
        );
        Row::VSync.step(&mut settings, 1, metal);
        assert_eq!(settings.vsync, VsyncMode::On);

        let mut settings = Settings::default();
        Row::VSync.step(&mut settings, -1, ALL_RUNGS);
        assert_eq!(settings.vsync, VsyncMode::Fast, "unprobed offers all rungs");
        Row::VSync.step(&mut settings, -1, ALL_RUNGS);
        assert_eq!(settings.vsync, VsyncMode::Off);
        Row::VSync.step(&mut settings, -1, ALL_RUNGS);
        assert_eq!(settings.vsync, VsyncMode::Off, "the floor saturates");

        // A stored FAST on a Metal surface: not offered, so a step resolves into the offered set.
        let mut settings = Settings {
            vsync: VsyncMode::Fast,
            ..default()
        };
        Row::VSync.step(&mut settings, 1, metal);
        assert!(
            metal.offers(settings.vsync),
            "stepping an unoffered value must land inside the offered set, got {:?}",
            settings.vsync,
        );
    }

    /// The frame-cap row is enabled exactly under vsync OFF — the same fact the limiter arms on —
    /// and every other row is unconditional.
    #[test]
    fn only_the_frame_cap_row_can_be_disabled() {
        let off = Settings {
            vsync: VsyncMode::Off,
            ..default()
        };
        for row in Row::ORDER {
            assert!(
                row.enabled(&off, ALL_RUNGS),
                "{row:?} must be enabled under vsync OFF"
            );
        }
        for vsync in [VsyncMode::Fast, VsyncMode::On] {
            let settings = Settings { vsync, ..default() };
            for row in Row::ORDER {
                assert_eq!(
                    row.enabled(&settings, ALL_RUNGS),
                    row != Row::FrameCap,
                    "{row:?} under {vsync:?}"
                );
            }
        }
    }

    /// **Codex review finding, 2026-07-27.** A stored OFF the surface cannot present (Wayland
    /// refuses `Immediate`) must grey the frame-cap row, because that is precisely what the limiter
    /// does with it — the row used to stay lit and armed over a compositor-paced surface. The gate
    /// is the EFFECTIVE rung, so this holds in the frames before `settings::normalize_vsync` has
    /// written the correction back.
    #[test]
    fn an_unpresentable_off_greys_the_frame_cap_row() {
        let wayland = PresentCaps::Reported {
            immediate: false,
            mailbox: true,
        };
        let settings = Settings {
            vsync: VsyncMode::Off,
            frame_cap: FrameCap(120),
            ..default()
        };
        assert!(
            !Row::FrameCap.enabled(&settings, wayland),
            "a stored OFF this surface cannot present must not arm the cap row",
        );
        assert_eq!(
            settings.frame_limit_period(wayland),
            None,
            "the grey row and the idle limiter must be the same fact",
        );
        // The same file on a surface that DOES offer Immediate: lit and limiting.
        let metal = PresentCaps::Reported {
            immediate: true,
            mailbox: false,
        };
        assert!(Row::FrameCap.enabled(&settings, metal));
        assert!(settings.frame_limit_period(metal).is_some());

        // And a probe that could not ASK is not a surface that said no: the row stays lit on the
        // player's own rung, matching the limiter (see `Row::enabled`'s doc for the choice).
        for unknown in [PresentCaps::Unprobed, PresentCaps::Unavailable] {
            assert!(
                Row::FrameCap.enabled(&settings, unknown),
                "{unknown:?} must not grey the row on evidence nobody produced",
            );
            assert!(settings.frame_limit_period(unknown).is_some());
        }
    }

    /// The selection walk skips disabled rows in BOTH directions and still saturates. With the
    /// default settings (vsync ON) the frame-cap row is a hole in the ladder: walking down from
    /// VSYNC lands on UI SCALE, walking back up from UI SCALE lands on VSYNC.
    #[test]
    fn the_selection_skips_disabled_rows() {
        let settings = Settings::default();
        assert_eq!(settings.vsync, VsyncMode::On, "the premise: cap disabled");
        let enabled = |row: Row| row.enabled(&settings, ALL_RUNGS);

        let mut selection = Selection(Row::VSync.index());
        selection.step(1, enabled);
        assert_eq!(
            selection.row(),
            Row::UiScale,
            "down from VSYNC must skip the disabled FRAME CAP"
        );
        selection.step(-1, enabled);
        assert_eq!(
            selection.row(),
            Row::VSync,
            "up from UI SCALE must skip it too"
        );

        // With vsync OFF the row is an ordinary stop on the walk.
        let armed = Settings {
            vsync: VsyncMode::Off,
            ..default()
        };
        let mut selection = Selection(Row::VSync.index());
        selection.step(1, |row| row.enabled(&armed, ALL_RUNGS));
        assert_eq!(selection.row(), Row::FrameCap);
    }

    /// The selection walks with the arrows and saturates at both ends, and a hover lands on exactly
    /// the row it names.
    #[test]
    fn the_selection_saturates_and_a_hover_lands_on_its_row() {
        let all = |_: Row| true;
        let mut selection = Selection::default();
        assert_eq!(selection.row(), Row::ORDER[0]);
        selection.step(-1, all);
        assert_eq!(selection.row(), Row::ORDER[0], "the top row stays put");
        for _ in 0..Row::ORDER.len() * 2 {
            selection.step(1, all);
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

    /// The slider mapping round-trips through the Row API for every ladder stop of every slider
    /// row — a drag to where the handle already sits is always a no-op.
    #[test]
    fn slider_fractions_round_trip_for_every_slider_row() {
        for row in Row::ORDER {
            if row.kind() != RowKind::Slider {
                assert_eq!(row.slider_fraction(&Settings::default()), None);
                continue;
            }
            // Walk the row's whole ladder via step, checking the fraction inverts at each stop.
            let mut settings = Settings {
                vsync: VsyncMode::Off, // arm the frame cap so its row participates honestly
                ..default()
            };
            for _ in 0..64 {
                row.step(&mut settings, -1, ALL_RUNGS);
            }
            loop {
                let fraction = row
                    .slider_fraction(&settings)
                    .expect("slider rows have fractions");
                assert!((0.0..=1.0).contains(&fraction), "{row:?} at {fraction}");
                let mut reconstructed = settings;
                row.set_from_fraction(&mut reconstructed, fraction);
                assert_eq!(
                    reconstructed, settings,
                    "{row:?}: a drag to the current handle position must be a no-op",
                );
                let mut next = settings;
                row.step(&mut next, 1, ALL_RUNGS);
                if next == settings {
                    break;
                }
                settings = next;
            }
        }
    }

    /// The discrete-ladder slider helpers: endpoints, rounding to the nearest stop, and tolerance
    /// of a fraction outside `0..=1` (a drag released past the track's edge).
    #[test]
    fn ladder_fraction_helpers_round_and_clamp() {
        let order = [10, 20, 30, 40, 50];
        assert_eq!(ladder_fraction(&order, 10), 0.0);
        assert_eq!(ladder_fraction(&order, 50), 1.0);
        assert_eq!(ladder_fraction(&order, 30), 0.5);
        assert_eq!(ladder_at_fraction(&order, 0.0), 10);
        assert_eq!(ladder_at_fraction(&order, 1.0), 50);
        assert_eq!(ladder_at_fraction(&order, 0.55), 30, "rounds to nearest");
        assert_eq!(ladder_at_fraction(&order, -3.0), 10, "clamps below");
        assert_eq!(ladder_at_fraction(&order, 7.0), 50, "clamps above");
        assert_eq!(
            ladder_fraction(&order, 99),
            0.0,
            "an unknown current resolves to the first stop, like step_in"
        );
    }

    /// The track mapping: a cursor at the track's left edge is 0, right edge is 1, centre is 0.5,
    /// and positions beyond either end clamp.
    #[test]
    fn track_fraction_maps_the_physical_rect() {
        // A 200px-wide track centred at x=500 spans 400..600.
        assert_eq!(track_fraction(400.0, 500.0, 200.0), 0.0);
        assert_eq!(track_fraction(600.0, 500.0, 200.0), 1.0);
        assert_eq!(track_fraction(500.0, 500.0, 200.0), 0.5);
        assert_eq!(track_fraction(0.0, 500.0, 200.0), 0.0, "clamps left");
        assert_eq!(track_fraction(9999.0, 500.0, 200.0), 1.0, "clamps right");
        assert_eq!(
            track_fraction(500.0, 500.0, 0.0),
            0.0,
            "a zero-width track (first frame before layout) is inert, not NaN"
        );
    }

    /// **The Retina regression, restated for the slider.** All of [`track_fraction`]'s arguments
    /// are PHYSICAL pixels; feeding it the LOGICAL cursor (what `Window::cursor_position` returns)
    /// against a physical track rect scrubs the handle to the wrong place — silently, because 1x
    /// displays work fine. The scenario is a 2x display and a cursor at the track's true centre.
    #[test]
    fn the_track_mapping_needs_both_values_in_the_same_pixel_space() {
        const SCALE_FACTOR: f32 = 2.0;
        let track_center_physical = 1000.0;
        let track_width_physical = 300.0;
        let cursor_physical = 1000.0;
        let cursor_logical = cursor_physical / SCALE_FACTOR;

        assert_eq!(
            track_fraction(cursor_physical, track_center_physical, track_width_physical),
            0.5,
            "converted together, the centre reads as the centre"
        );
        assert_eq!(
            track_fraction(cursor_logical, track_center_physical, track_width_physical),
            0.0,
            "the un-converted cursor lands the handle at the far LEFT — the bug this pins",
        );
    }

    /// **The UI-SCALE feedback loop, pinned.** The bug: `apply_settings` writes `UiScale` live
    /// during the drag, `ui_layout_system` re-lays the settings card out — including the track being
    /// dragged — so a live rect re-read next frame maps the SAME physical cursor to a DIFFERENT
    /// value. The scenario below is one grab followed by a 1.25x page rescale under a stationary
    /// hand: with the frozen rect the value cannot move, with the live rect it does.
    #[test]
    fn a_drag_maps_through_the_grab_time_rect_even_when_the_page_rescales() {
        // A 150px track centred at x=600 (spans 525..675), cursor grabbed a quarter in.
        let grabbed = ActiveDrag {
            row: Row::UiScale,
            track_center_x: 600.0,
            track_width: 150.0,
        };
        let cursor_x = 562.5;
        let at_grab = grabbed.fraction(cursor_x);
        assert_eq!(at_grab, 0.25);

        // What applying the new UI scale does to the layout: same card, every `Val::Px` multiplied,
        // so the track is wider AND its centre has moved. (The exact numbers do not matter — only
        // that the live rect is a DIFFERENT rect.)
        let rescaled_center_x = 640.0;
        let rescaled_width = 187.5;
        assert_ne!(
            track_fraction(cursor_x, rescaled_center_x, rescaled_width),
            at_grab,
            "the premise: a live rect re-read after the rescale maps this cursor somewhere else",
        );

        // The fix: the drag's own mapping is unchanged by any of that.
        assert_eq!(
            grabbed.fraction(cursor_x),
            at_grab,
            "the value must depend on the cursor and the GRAB-time rect alone",
        );

        // And the setting it lands on is likewise identical across the rescale.
        let mut before = Settings::default();
        Row::UiScale.set_from_fraction(&mut before, at_grab);
        let mut after = Settings::default();
        Row::UiScale.set_from_fraction(&mut after, grabbed.fraction(cursor_x));
        assert_eq!(before.ui_scale, after.ui_scale);

        // A cursor that genuinely MOVES still scrubs — freezing the rect must not freeze the value.
        assert!(
            grabbed.fraction(cursor_x + 40.0) > at_grab,
            "the frozen rect maps a moved cursor to a moved value",
        );
    }

    /// The Retina lesson, restated for the FROZEN rect: [`ActiveDrag`] stores what
    /// `UiGlobalTransform`/`ComputedNode` reported, i.e. PHYSICAL pixels, and
    /// [`ActiveDrag::fraction`] must be fed the PHYSICAL cursor to match. Freezing the geometry
    /// changes the lifetime of those numbers, never their pixel space — a logical cursor is the same
    /// silent 2x error it always was, just held for the length of a drag.
    #[test]
    fn the_frozen_rect_is_physical_pixels_like_everything_else_it_is_compared_against() {
        const SCALE_FACTOR: f32 = 2.0;
        let drag = ActiveDrag {
            row: Row::UiScale,
            track_center_x: 1000.0,
            track_width: 300.0,
        };
        let cursor_physical = 1000.0;
        assert_eq!(
            drag.fraction(cursor_physical),
            0.5,
            "same space on both sides: the centre reads as the centre"
        );
        assert_eq!(
            drag.fraction(cursor_physical / SCALE_FACTOR),
            0.0,
            "a logical cursor against the physical frozen rect still lands at the far LEFT",
        );
        assert_eq!(
            drag.fraction(cursor_physical),
            track_fraction(cursor_physical, drag.track_center_x, drag.track_width),
            "the frozen path is the same mapping, only over a captured rect",
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

    // --- The fixed hint slot ------------------------------------------------------------------
    //
    // Measured on the REAL page, headless. `DefaultPlugins` with the GPU backends and winit off (the
    // `headless_test` recipe) still runs the whole UI stack — taffy layout and parley text shaping
    // against the SHIPPED Barlow Condensed — so what these tests read out of `ComputedNode` is
    // bevy's own answer about the card, not an arithmetic re-derivation of it.

    /// How long to wait for the font asset (real, async, wall-clock file IO) before calling it a
    /// failure. Generous: the loop exits the moment the card has laid out.
    const LAYOUT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

    /// A headless app with the real card spawned and `UiScale` set to `ui_scale`.
    ///
    /// The window and camera are load-bearing rather than ceremony: the UI's scale factor and
    /// viewport come from the camera's target (`propagate_ui_target_cameras`), and with no target at
    /// all the root node is zero-sized, the fixed-width card flex-shrinks into it, and every hint
    /// wraps into nonsense. No winit and no GPU are needed for that — a `Window` COMPONENT carries
    /// its own resolution, which is all `Camera::physical_viewport_size` reads.
    fn headless_card_app(ui_scale: f32) -> App {
        let mut app = App::new();
        app.add_plugins(
            DefaultPlugins
                .set(bevy::render::RenderPlugin {
                    render_creation: bevy::render::settings::WgpuSettings {
                        backends: None,
                        ..default()
                    }
                    .into(),
                    ..default()
                })
                .set(WindowPlugin {
                    // Wide enough that the card never flex-shrinks at any rung of the ladder (the
                    // top rung asks for CARD_WIDTH_PX * 1.5 = 840 physical px).
                    primary_window: Some(Window {
                        resolution: bevy::window::WindowResolution::new(1920, 1080),
                        ..default()
                    }),
                    exit_condition: bevy::window::ExitCondition::DontExit,
                    ..default()
                })
                .disable::<bevy::winit::WinitPlugin>(),
        )
        .add_plugins(crate::ui_font::plugin)
        .init_resource::<Settings>()
        .insert_resource(UiScale(ui_scale))
        .add_systems(
            Startup,
            (
                |mut commands: Commands| {
                    commands.spawn(Camera2d);
                },
                spawn_page,
            ),
        );

        // `App::run` normally drives plugin finish/cleanup; a bare `update()` loop must do it.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// Pump the app until the hint's font has actually LOADED. Nothing may be measured before this:
    /// the reserved slot is a `min_height`, so an unshaped (or unfindable) font reports exactly the
    /// height the test is looking for and every assertion below would pass vacuously.
    fn settle_with_font(app: &mut App) {
        let started = std::time::Instant::now();
        loop {
            app.update();
            let body = app
                .world()
                .resource::<crate::ui_font::UiFonts>()
                .body
                .clone();
            let state = app.world().resource::<AssetServer>().load_state(&body);
            if state.is_loaded() {
                // One more pass so the loaded font reaches the text pipeline and the card relays out.
                app.update();
                app.update();
                return;
            }
            assert!(
                started.elapsed() < LAYOUT_DEADLINE && !state.is_failed(),
                "the hint font never loaded headless after {:?} ({state:?}) — assets/fonts/\
                 BarlowCondensed-Regular.ttf is what the whole measurement rests on",
                started.elapsed(),
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// The hint SLOT laid out with `text` in it: its height and the scale factor it was laid out
    /// under, in the PHYSICAL pixels `ComputedNode` reports. The slot, not the text node, because
    /// the slot is the thing whose height must not move.
    ///
    /// Physical, not converted back to logical, because bevy ROUNDS a node's edges to whole physical
    /// pixels: at 75% a 33.6-logical-px slot lands on 26 physical px, and dividing that back out
    /// reads 34.67 "logical" px — a phantom 1.07 px of drift that is really sub-pixel rounding.
    /// Comparing where the rounding happens keeps the tolerance a flat pixel instead of a
    /// scale-dependent fudge.
    fn hint_layout(app: &mut App, text: &str) -> (f32, f32) {
        {
            let world = app.world_mut();
            let mut hints = world.query_filtered::<&mut Text, With<HintText>>();
            let mut hint = hints
                .single_mut(world)
                .expect("the card carries exactly one hint node");
            hint.0 = text.to_string();
        }
        // The write lands in `Update`; the text measure and the layout that consumes it are both in
        // `PostUpdate`. Rather than guess how many passes a new measure takes to reach the node,
        // run until the node stops moving — a static text settles in two or three.
        let mut last = f32::NAN;
        for _ in 0..32 {
            app.update();
            let world = app.world_mut();
            let mut nodes = world.query_filtered::<&ComputedNode, With<HintSlot>>();
            let node = nodes
                .single(world)
                .expect("the card carries exactly one hint slot");
            let (height, scale) = (node.size().y, node.inverse_scale_factor().recip());
            if height == last {
                return (height, scale);
            }
            last = height;
        }
        panic!("the hint node never settled on a height for {text:?} (last {last})");
    }

    /// Assert the hint node is exactly the two-line slot tall. `physical` and `scale` are what
    /// [`hint_layout`] measured; `what` names the case for the failure message.
    ///
    /// The tolerance is one physical pixel of edge rounding (see [`hint_layout`]) — nowhere near the
    /// line of type (16.8 logical px) that either failure direction would move the node by, so it
    /// discriminates cleanly while absorbing the only noise there is.
    fn assert_two_line_slot(physical: f32, scale: f32, what: &str) {
        let want = HINT_SLOT_HEIGHT_PX * scale;
        let lines = physical / (HINT_LINE_HEIGHT_PX * scale);
        assert!(
            (physical - want).abs() <= 1.01,
            "{what}: the hint node lays out {physical:.2} physical px at scale {scale} — \
             {lines:.2} lines, where the slot is fixed at two ({want:.2} px). MORE means this copy \
             needs a third line at this scale, so selecting the row grows the whole card: trim the \
             hint. LESS means the reserved slot stopped being applied and the layout shifts between \
             one- and two-line hints again.",
        );
    }

    /// **The hint slot is exactly two lines, for every row, at every UI-scale rung.**
    ///
    /// One equality carries both halves of the fix:
    ///
    /// * a ONE-line hint must still occupy two lines, or the rows above the footer jump by a line
    ///   every time the selection crosses between a short hint and a long one — the shift this
    ///   reserved slot exists to remove;
    /// * no hint may need a THIRD line, or the slot's `min_height` floor is exceeded and the page
    ///   grows anyway. That is the copy check, done in the real font at the real wrapping width
    ///   rather than by eye.
    ///
    /// The rungs matter because the copy question was asked as "at any UI scale". `UiScale`
    /// multiplies `Val::Px` and `FontSize::Px` by the SAME factor (it is one scale factor, applied
    /// to the whole UI — `propagate_ui_target_cameras`), so the card and the type zoom together and
    /// the wrap is scale-invariant BY CONSTRUCTION; the ends of the ladder are here to pin that
    /// reasoning to a measurement instead of an argument.
    #[test]
    fn the_hint_slot_is_two_lines_and_every_hint_fits_it() {
        for ui in [
            UiScalePercent(UiScalePercent::MIN),
            UiScalePercent::default(),
            UiScalePercent(UiScalePercent::MAX),
        ] {
            let mut app = headless_card_app(ui.factor());
            settle_with_font(&mut app);
            // The empty string the card is born with: the slot is reserved before any row is ever
            // selected, so an unopened page is already the height it will keep.
            let (physical, scale) = hint_layout(&mut app, "");
            assert_two_line_slot(
                physical,
                scale,
                &format!("at {} with an EMPTY hint", ui.label()),
            );
            for row in Row::ORDER {
                let (physical, scale) = hint_layout(&mut app, row.hint());
                assert_two_line_slot(physical, scale, &format!("at {} on {row:?}", ui.label()));
            }

            // CONTROL — the slot is a floor, not a clamp, so the measurement above is only
            // meaningful if overlong copy actually MOVES it. Two hints glued together is roughly
            // four lines; if this reads two, the node has stopped tracking its text and every
            // assertion above is passing vacuously.
            let overlong = format!("{} {}", Row::VSync.hint(), Row::RenderScale.hint());
            let (physical, scale) = hint_layout(&mut app, &overlong);
            assert!(
                physical > HINT_SLOT_HEIGHT_PX * scale + 1.01,
                "CONTROL at {}: deliberately overlong copy laid out {physical:.2} physical px, \
                 i.e. still the two-line slot — the instrument is not measuring the text",
                ui.label(),
            );
        }
    }
}
