//! Settings card with VIDEO and GAMEPLAY tabs, enum steppers (`LABEL < VALUE >`) and scalar
//! sliders (`LABEL [====----] VALUE`). Tabs exist only with rows. View/input writes Settings
//! and requests saves; `settings::apply_settings` reconciles the world.
//!
//! Both roots supply [`SettingsPageVisible`]: the net client uses Overlay::Menu's input-blocking,
//! cursor-releasing, priority-gated scrim; offline uses AppState::Paused and state's existing
//! Esc/cursor handling. Esc closes either; no extra key or overlay layer.
//!
//! * Up/Down selects enabled rows; Up from the first selects the tab strip. W/S stays unbound
//!   because it drives the tank.
//! * Left/Right steps a row or switches tabs on the strip. Tab remains the offline tank switch.
//! * Hover selects; visible, highlighted arrow glyphs click to step, tracks click/drag to scrub,
//!   tab names click to open. Visible arrows avoid invisible half-row click targets.
//!
//! Use bevy_picking (UiPickingPlugin comes with UiPlugin): Hovered selects/highlights, Press
//! observers click. Deepest-node bubbling gives arrows priority over rows; InheritedVisibility
//! excludes hidden controls. Rewalking UiStack rects would duplicate the backend's work.
//!
//! Atomic writes make immediate saves safe, with no Apply button to forget; drags save on release.
//! [`ActiveDrag`] freezes the physical track rect so UI-scale-driven layout cannot feed back into
//! its value.
//! [`track_fraction`] owns the remaining physical-vs-logical pixel arithmetic.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui::{ComputedNode, Overflow, ScrollPosition, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::displays::DisplayLadder;
use super::look::page as page_look;
use super::preset::Preset;
use super::{
    BloomLevel, DebugPanel, FrameCap, KillCam, MsaaLevel, PenCamCorner, PixelBudget, PresentCaps,
    ReduceMotion, RenderScaleLevel, SaveSettings, SaveStatus, Settings, ShadowCascades,
    ShadowDistance, ShadowResolution, TrackPrintRange, TrackPrintRelief, UiScalePercent, VsyncMode,
    WindowModeSetting,
};
use crate::ui_font::{MODAL_BG, TEXT, TEXT_DIM, UiFonts};

/// Whether the page is on screen this frame. Written by whichever pause surface the composition root
/// mounts ([`declare_from_overlay_menu`] on the net client, [`declare_from_pause_state`] in SP), read
/// by everything else here. One bool is the entire coupling between the two entry paths.
#[derive(Resource, Default, PartialEq)]
pub(super) struct SettingsPageVisible(bool);

/// Visibility declarers precede the page's input/refresh chain. Reading last frame's visibility
/// would drop the first opening keypress and leave the card up a frame after its surface closes.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct DeclareSettingsPage;

/// The page's tabs, in on-screen order. Page state, never saved: the page opens on the tab it was
/// last left on this session, VIDEO at launch.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Tab {
    /// Display and graphics: what the window shows and what a frame costs.
    #[default]
    Video,
    /// How the game shows the player's own hits and knockout, plus the online debug card.
    Gameplay,
}

impl Tab {
    const ORDER: [Tab; 2] = [Tab::Video, Tab::Gameplay];

    /// ASCII only — it reaches `Text`.
    const fn label(self) -> &'static str {
        match self {
            Tab::Video => "VIDEO",
            Tab::Gameplay => "GAMEPLAY",
        }
    }

    /// This tab's rows in on-screen order: [`Row::ORDER`] filtered by [`Row::tab`].
    fn rows(self) -> impl Iterator<Item = Row> {
        Row::ORDER.into_iter().filter(move |row| row.tab() == self)
    }
}

/// The footer hint while the tab strip is selected. ASCII only — it reaches `Text`.
const TAB_STRIP_HINT: &str = "Left and right switch tabs. VIDEO holds display and graphics; \
     GAMEPLAY holds the pen cam, the kill cam and the debug panel.";

/// Settings rows, ordered within tabs by [`Row::ORDER`]. To extend: add a variant to ORDER and
/// arms to tab/label/hint/value/step; scalars also need kind and both slider-fraction mappings.
/// The card lays out from ORDER; Settings accepts new fields without migration.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    /// LOW to ULTRA: writes every row directly below it at once, and reads CUSTOM whenever their
    /// values match no tier — see `settings::preset`. Heads VIDEO, its rows beneath it.
    Preset,
    RenderScale,
    /// The LOD screen-space error budget, in pixels. Beside render scale because they are the two
    /// rows that trade picture for frames by drawing LESS, rather than by drawing it differently.
    /// Consumed by `geometry_lod::compose_view_profile`; see `settings::PixelBudget`.
    LodPixelBudget,
    /// Windowed / borderless fullscreen. Reflects OS-side toggles too — see
    /// `settings::observe_window_placement`.
    WindowMode,
    /// OS-named offered ladder: `settings::displays::DisplayLadder`. Unplugged stored choices resolve
    /// read-side only; see `settings::DisplaySelection`.
    Display,
    /// How far shadows reach, `OFF` included — see [`ShadowDistance`] for why this is its own row.
    ShadowDistance,
    /// How crisp they are. Adjacent to the distance row so the pair reads as one subject, but it
    /// steps independently.
    ShadowResolution,
    /// Count changes while Off remain interactive and apply on the next casting frame.
    /// See `settings::SHADOW_CASCADES` for the runtime-growth requirement.
    ShadowCascades,
    TrackPrints,
    TrackPrintRange,
    Msaa,
    Bloom,
    /// Capability-gated: only the rungs the surface's probe reported are offered ([`PresentCaps`]).
    VSync,
    /// The one row that can be DISABLED: it renders greyed and is skipped by every input path
    /// unless vsync is OFF (see [`Row::enabled`]).
    FrameCap,
    UiScale,
    DebugPanel,
    /// Which of the player's hits open the pen cam's corner — see `settings::PenCamCorner`.
    PenCamCorner,
    /// See `settings::KillCam`.
    KillCam,
    /// See `settings::ReduceMotion`.
    ReduceMotion,
}

/// How a row's control renders and responds: a `< VALUE >` stepper or a draggable slider.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowKind {
    Stepper,
    Slider,
}

impl Row {
    const ORDER: [Row; 19] = [
        Row::Preset,
        // Preset-driven rows must sit directly below PRESET, as its hint promises.
        Row::RenderScale,
        Row::LodPixelBudget,
        Row::ShadowDistance,
        Row::ShadowResolution,
        Row::ShadowCascades,
        Row::Msaa,
        Row::Bloom,
        Row::TrackPrints,
        Row::TrackPrintRange,
        // Display rows: no preset touches them.
        Row::WindowMode,
        Row::Display,
        Row::VSync,
        Row::FrameCap,
        Row::UiScale,
        Row::PenCamCorner,
        Row::KillCam,
        Row::ReduceMotion,
        Row::DebugPanel,
    ];

    /// The tab the row is drawn on. Exhaustive rather than defaulted, so a new row cannot land on a
    /// tab by accident.
    const fn tab(self) -> Tab {
        match self {
            Row::Preset
            | Row::RenderScale
            | Row::LodPixelBudget
            | Row::WindowMode
            | Row::Display
            | Row::ShadowDistance
            | Row::ShadowResolution
            | Row::ShadowCascades
            | Row::TrackPrints
            | Row::TrackPrintRange
            | Row::Msaa
            | Row::Bloom
            | Row::VSync
            | Row::FrameCap
            | Row::UiScale => Tab::Video,
            Row::PenCamCorner | Row::KillCam | Row::ReduceMotion | Row::DebugPanel => Tab::Gameplay,
        }
    }

    /// The row's name. ASCII only — it reaches `Text`.
    const fn label(self) -> &'static str {
        match self {
            Row::Preset => "PRESET",
            Row::RenderScale => "RENDER SCALE",
            Row::LodPixelBudget => "DETAIL BUDGET",
            Row::WindowMode => "WINDOW MODE",
            Row::Display => "DISPLAY",
            Row::ShadowDistance => "SHADOW DISTANCE",
            // "QUALITY", not "RESOLUTION": the value beside it is already a texel count, so the
            // label's job is to say what the number buys, not to name its unit twice.
            Row::ShadowResolution => "SHADOW QUALITY",
            Row::ShadowCascades => "SHADOW CASCADES",
            Row::TrackPrints => "TRACK PRINTS",
            Row::TrackPrintRange => "TRACK PRINT RANGE",
            Row::Msaa => "ANTI-ALIASING",
            Row::Bloom => "BLOOM",
            Row::VSync => "VSYNC",
            Row::FrameCap => "FRAME CAP",
            Row::UiScale => "UI SCALE",
            Row::DebugPanel => "DEBUG PANEL",
            Row::PenCamCorner => "PEN CAM CORNER",
            Row::KillCam => "KILL CAM",
            Row::ReduceMotion => "REDUCE MOTION",
        }
    }

    /// A one-line explanation of what the selected row costs or buys, shown at the foot of the card.
    /// ASCII plus the verified typographic set only — it reaches `Text`.
    const fn hint(self) -> &'static str {
        match self {
            // Moving a window or frame cap via a graphics preset would surprise players.
            // The hint's "nine" is `preset::DRIVEN_ROWS`; its contiguous rows are pinned below.
            Row::Preset => {
                "Sets the nine rows below it at once; editing one reads CUSTOM. \
                 Window, display, vsync, frame cap and UI scale are never touched."
            }
            // Name the sharp HUD: crosshair, range dial and labels stay crisp as the world softens.
            // The multisample resolve stays full-res; 4X below 100% pays twice for the same edges.
            // See `render_scale` for this limit.
            Row::RenderScale => {
                "How much of the window the 3D world is drawn at, before upscaling. \
                 The HUD stays sharp. Below 100%, prefer 2X or OFF anti-aliasing."
            }
            // Error budget reverses the usual direction: larger is cheaper and less crisp.
            Row::LodPixelBudget => {
                "Largest on-screen error, in pixels, a simplified model may show. \
                 Lower is crisper and costs more."
            }
            Row::WindowMode => {
                "Borderless fullscreen on the current display. \
                 The OS's own fullscreen button is reflected here too."
            }
            // Explain the unplugged-display fallback; unexplained substitution looks broken.
            Row::Display => {
                "Which monitor the window opens on, * marking your main display. \
                 One that is not attached falls back to it."
            }
            // Name the visible cutoff and Off's provisional status; see `ShadowDistance`.
            Row::ShadowDistance => {
                "How far shadows are drawn before they cut off. \
                 OFF disables them entirely, and may be removed later."
            }
            // Shadow bias is in texels: a wider envelope detaches shadows unless resolution rises.
            // MEASURED: see `ShadowDistance`'s detachment table.
            Row::ShadowResolution => {
                "How crisp shadows are, and how tightly distant ones stay attached. \
                 Higher costs more memory."
            }
            Row::ShadowCascades => {
                "How many slices the shadow distance is split into. \
                 Fewer is cheaper but blockier up close."
            }
            // Reassure players lowering this: the trail survives; only depth gets cheaper.
            Row::TrackPrints => {
                "How track prints show their depth: lit only, with parallax, or with parallax \
                 and the sun's shadow in the rut. Every level keeps the trail."
            }
            Row::TrackPrintRange => {
                "How far from you track prints keep their parallax depth. Beyond it they are lit \
                 only. Farther costs more."
            }
            Row::Msaa => "Smooths jagged edges. The cheapest setting to drop for more frames.",
            // Reassure players lowering this: tracers and halos remain, drawn coarser.
            Row::Bloom => {
                "The glow around tracers, muzzle flashes and bright light. \
                 LOW draws it at half resolution: cheaper, with coarser halos."
            }
            Row::VSync => {
                "ON locks frames to the display. FAST is uncapped without tearing. \
                 OFF can tear, and enables the frame cap. Only supported modes are offered."
            }
            Row::FrameCap => {
                "Caps the frame rate to limit heat and power draw. Active only while VSYNC is OFF."
            }
            Row::UiScale => "Scales the HUD and menus. The 3D world is unaffected.",
            Row::DebugPanel => {
                "Ping, jitter, frame rate and version in the corner during an online match."
            }
            Row::PenCamCorner => {
                "Which of your hits open the inset that shows what the round did: \
                 all, penetrations only, or none."
            }
            Row::KillCam => {
                "When a recorded shot knocks you out, shows what it did before the death screen. \
                 OFF goes straight to the death screen."
            }
            Row::ReduceMotion => "Holds the pen cam's camera still and shortens its animations.",
        }
    }

    /// Scalar rows use scrubbable sliders rather than disguised ladders (owner requirement).
    const fn kind(self) -> RowKind {
        match self {
            Row::RenderScale | Row::LodPixelBudget | Row::FrameCap | Row::UiScale => {
                RowKind::Slider
            }
            _ => RowKind::Stepper,
        }
    }

    /// How wide the row's value well is, in logical px. Wider for the display row alone: its values
    /// are display NAMES, and every other stepper's are two to six characters.
    fn value_well_px(self) -> f32 {
        match (self, self.kind()) {
            (Row::Display, _) => page_look().display_value_well_px.0,
            (_, RowKind::Stepper) => page_look().stepper_value_well_px.0,
            (_, RowKind::Slider) => page_look().slider_value_well_px.0,
        }
    }

    /// Disabled rows dim and are excluded from keyboard, hover and clicks. Only frame cap is
    /// conditional, gated on the same effective vsync as `Settings::frame_limit_period`, even
    /// before normalization. Unknown caps preserve the player's OFF: no evidence justifies taking
    /// the cap away, and limiter/page still agree.
    fn enabled(self, settings: &Settings, caps: PresentCaps) -> bool {
        match self {
            Row::FrameCap => settings.effective_vsync(caps) == VsyncMode::Off,
            _ => true,
        }
    }

    /// ORDER owns the indices shared by selection and controls; every constructed row belongs to it.
    fn index(self) -> usize {
        Row::ORDER
            .iter()
            .position(|row| *row == self)
            .expect("every constructed Row belongs to Row::ORDER")
    }

    /// Rendered value: ASCII plus the verified typographic set; scalar rows format numbers.
    /// The display ladder supplies panel names rather than ordinal positions.
    fn value(self, settings: &Settings, ladder: &DisplayLadder) -> String {
        match self {
            Row::Preset => Preset::of(settings)
                .map_or("CUSTOM", Preset::label)
                .to_string(),
            Row::RenderScale => settings.render_scale.label().to_string(),
            Row::LodPixelBudget => settings.lod_pixel_budget.label(),
            Row::WindowMode => settings.window_mode.label().to_string(),
            Row::Display => ladder.label(settings.display).to_string(),
            Row::ShadowDistance => settings.shadow_distance.label().to_string(),
            Row::ShadowResolution => settings.shadow_resolution.label().to_string(),
            Row::ShadowCascades => settings.shadow_cascades.label().to_string(),
            Row::TrackPrints => settings.track_prints.label().to_string(),
            Row::TrackPrintRange => settings.track_print_range.label(),
            Row::Msaa => settings.msaa.label().to_string(),
            Row::Bloom => settings.bloom.label().to_string(),
            Row::VSync => settings.vsync.label().to_string(),
            Row::FrameCap => settings.frame_cap.label(),
            Row::UiScale => settings.ui_scale.label(),
            Row::DebugPanel => settings.debug_panel.label().to_string(),
            Row::PenCamCorner => settings.pen_cam_corner.label().to_string(),
            Row::KillCam => settings.kill_cam.label().to_string(),
            Row::ReduceMotion => settings.reduce_motion.label().to_string(),
        }
    }

    /// Step by delta, saturating: holding right must stop, not wrap from the highest setting to
    /// Off. Walk within caps/display-ladder offers; Metal skips FAST and two displays exclude a
    /// third display.
    fn step(self, settings: &mut Settings, delta: i32, caps: PresentCaps, ladder: &DisplayLadder) {
        match self {
            Row::Preset => Preset::step(settings, delta),
            Row::RenderScale => {
                settings.render_scale =
                    step_in(&RenderScaleLevel::ORDER, settings.render_scale, delta);
            }
            Row::LodPixelBudget => {
                settings.lod_pixel_budget = settings.lod_pixel_budget.step(delta);
            }
            Row::WindowMode => {
                settings.window_mode =
                    step_in(&WindowModeSetting::ORDER, settings.window_mode, delta);
            }
            Row::Display => settings.display = ladder.step(settings.display, delta),
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
            Row::TrackPrints => {
                settings.track_prints =
                    step_in(&TrackPrintRelief::ORDER, settings.track_prints, delta);
            }
            Row::TrackPrintRange => {
                settings.track_print_range =
                    step_in(&TrackPrintRange::ORDER, settings.track_print_range, delta);
            }
            Row::Msaa => settings.msaa = step_in(&MsaaLevel::ORDER, settings.msaa, delta),
            Row::Bloom => settings.bloom = step_in(&BloomLevel::ORDER, settings.bloom, delta),
            Row::VSync => {
                let offered: Vec<VsyncMode> = VsyncMode::ORDER
                    .into_iter()
                    .filter(|mode| caps.offers(*mode))
                    .collect();
                settings.vsync = step_in(&offered, settings.vsync, delta);
            }
            Row::FrameCap => settings.frame_cap = settings.frame_cap.step(delta),
            Row::UiScale => settings.ui_scale = settings.ui_scale.step(delta),
            Row::DebugPanel => {
                settings.debug_panel = step_in(&DebugPanel::ORDER, settings.debug_panel, delta);
            }
            Row::PenCamCorner => {
                settings.pen_cam_corner =
                    step_in(&PenCamCorner::ORDER, settings.pen_cam_corner, delta);
            }
            Row::KillCam => {
                settings.kill_cam = step_in(&KillCam::ORDER, settings.kill_cam, delta);
            }
            Row::ReduceMotion => {
                settings.reduce_motion =
                    step_in(&ReduceMotion::ORDER, settings.reduce_motion, delta);
            }
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
            Row::LodPixelBudget => Some(settings.lod_pixel_budget.fraction()),
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
            Row::LodPixelBudget => {
                settings.lod_pixel_budget = PixelBudget::from_fraction(fraction);
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
        .expect("a discrete slider ladder contains its current value");
    index as f32 / (order.len().saturating_sub(1)).max(1) as f32
}

/// The ladder entry a slider position lands on — the inverse of [`ladder_fraction`], rounded to the
/// nearest stop.
fn ladder_at_fraction<T: Copy>(order: &[T], fraction: f32) -> T {
    let last = order.len() - 1;
    let index = (fraction.clamp(0.0, 1.0) * last as f32).round() as usize;
    order[index.min(last)]
}

/// The selected line: the tab strip, or one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Line {
    Tabs,
    Row(Row),
}

/// Page state keeps the selected row on the open tab. Switch/open selects the strip;
/// step/select refuses other tabs, so callers cannot split the invariant.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
struct Page {
    tab: Tab,
    line: Line,
}

impl Default for Page {
    /// The first row of the default tab — `the_tabs_partition_the_rows` pins that `ORDER[0]` is on
    /// it.
    fn default() -> Self {
        Self {
            tab: Tab::default(),
            line: Line::Row(Row::ORDER[0]),
        }
    }
}

impl Page {
    /// The selected row, or `None` while the tab strip is selected.
    fn row(self) -> Option<Row> {
        match self.line {
            Line::Tabs => None,
            Line::Row(row) => Some(row),
        }
    }

    /// Step through enabled rows of the open tab, saturating at bottom; running off the top
    /// selects the tab strip. Holding down never wraps or lands on a disabled row.
    fn step(&mut self, delta: i32, enabled: impl Fn(Row) -> bool) {
        let direction = delta.signum();
        if direction == 0 {
            return;
        }
        let selectable = |row: Row| row.tab() == self.tab && enabled(row);
        let mut index = match self.line {
            Line::Tabs if direction < 0 => return,
            Line::Tabs => -1,
            Line::Row(row) => row.index() as i32,
        };
        loop {
            index += direction;
            if index < 0 {
                self.line = Line::Tabs;
                return;
            }
            let Some(&row) = Row::ORDER.get(index as usize) else {
                return; // Ran off the bottom without finding a selectable row: stay put.
            };
            if selectable(row) {
                self.line = Line::Row(row);
                return;
            }
        }
    }

    /// Point the selection at a specific row — what a mouse hover does. A row of another tab is
    /// hidden and cannot be hovered; refusing it here keeps the invariant independent of that.
    fn select(&mut self, row: Row) {
        if row.tab() == self.tab {
            self.line = Line::Row(row);
        }
    }

    /// Switch tabs by `delta` places, saturating at both ends like every row, with the strip
    /// selected — what `Left`/`Right` do on the strip.
    fn switch(&mut self, delta: i32) {
        self.open(step_in(&Tab::ORDER, self.tab, delta));
    }

    /// Open `tab` with the strip selected — what a click on a tab's name does.
    fn open(&mut self, tab: Tab) {
        self.tab = tab;
        self.line = Line::Tabs;
    }
}

/// The card node, so visibility is one write.
#[derive(Component)]
pub(crate) struct SettingsCard;

/// The clipped, scrollable box the rows live in — everything between the tab strip and the hint
/// slot. Carries the [`ScrollPosition`] that [`scroll_rows`] drives.
#[derive(Component)]
struct RowViewport;

/// The tab strip's whole line: selectable like a row ([`Line::Tabs`]), highlighted like one.
#[derive(Component)]
struct TabStrip;

/// A tab's clickable area on the strip: its name and the bar under it.
#[derive(Component, Clone, Copy)]
struct TabLabel(Tab);

/// A tab's name text, inside its [`TabLabel`]: lit while its tab is open or it is hovered.
#[derive(Component)]
struct TabName(Tab);

/// The bar under a tab's name, filled while that tab is open.
#[derive(Component)]
struct TabMarker(Tab);

/// One tab's rows, inside [`RowViewport`]. A closed tab's panel is `Display::None` (out of the
/// layout, so the viewport scrolls only the open tab) and `Visibility::Hidden` (so picking, which
/// skips anything not inherited-visible, cannot hover or click a row it does not show).
#[derive(Component)]
struct TabPanel(Tab);

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
#[derive(Component, Clone, Copy)]
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

/// The control legend, replaced by retained load or save failures.
#[derive(Component)]
struct SaveNotice;

const SAVE_LEGEND: &str = "Arrows or drag to change  —  Esc to close  —  saved automatically";

/// Static copy avoids rendering arbitrary OS paths/errors outside the bundled font's coverage.
fn save_failure_text(error: &super::store::SaveError) -> &'static str {
    use super::store::{SaveError, SaveOperation};
    match error {
        SaveError::NoLocation => "Settings not saved: your config folder is unavailable.",
        SaveError::InvalidPath(_) => "Settings not saved: the config file path is invalid.",
        SaveError::Io { source, .. } if source.kind() == std::io::ErrorKind::PermissionDenied => {
            "Settings not saved: permission denied. Check your config folder permissions."
        }
        SaveError::Io { source, .. } if source.kind() == std::io::ErrorKind::StorageFull => {
            "Settings not saved: storage is full. Free space, then change a setting to retry."
        }
        SaveError::Io {
            operation: SaveOperation::CreateDirectory,
            ..
        } => "Settings not saved: cannot create your config folder. Check the game log.",
        SaveError::Io {
            operation: SaveOperation::Write,
            ..
        } => "Settings not saved: cannot write your config file. Check the game log.",
        SaveError::Io {
            operation: SaveOperation::Replace,
            ..
        } => "Settings not saved: cannot replace your config file. Check the game log.",
    }
}

/// File paths and parse errors are outside input; escape unsupported glyphs without losing them.
fn notice_text(text: &str) -> String {
    text.chars().fold(String::new(), |mut rendered, character| {
        if (' '..='~').contains(&character) {
            rendered.push(character);
        } else {
            rendered.extend(character.escape_unicode());
        }
        rendered
    })
}

fn load_failure_text(note: &super::store::LoadNote) -> Option<String> {
    use super::store::LoadNote;
    let text = match note {
        LoadNote::Corrupt { error, kept } => format!(
            "Settings reset: invalid settings file ({error}). {}.",
            super::describe_kept(kept.as_deref(), "Old file"),
        ),
        LoadNote::FutureVersion { version, kept } => format!(
            "Settings reset: file version {version} needs a newer game build. {}.",
            super::describe_kept(kept.as_deref(), "Old file"),
        ),
        LoadNote::Unreadable(error) => format!(
            "Settings reset: cannot read the settings file ({error}). Original file was not moved."
        ),
        LoadNote::NoLocation => {
            "Settings reset: config folder unavailable. Changes cannot be saved.".to_string()
        }
        LoadNote::Loaded | LoadNote::FirstLaunch => return None,
    };
    Some(notice_text(&text))
}

fn refresh_save_notice(
    status: Res<SaveStatus>,
    report: Option<Res<super::StoreReport>>,
    mut notice: Query<&mut Text, With<SaveNotice>>,
) {
    if !status.is_changed() && report.as_ref().is_none_or(|report| !report.is_changed()) {
        return;
    }
    let mut notice = notice
        .single_mut()
        .expect("settings::ui::spawn_page creates exactly one SaveNotice");
    let load = report
        .as_ref()
        .and_then(|report| load_failure_text(&report.0));
    let rendered = match (load, status.0.as_ref()) {
        (Some(load), Some(error)) => format!("{load} {}", save_failure_text(error)),
        (Some(load), None) => load,
        (None, Some(error)) => save_failure_text(error).to_string(),
        (None, None) => SAVE_LEGEND.to_string(),
    };
    if notice.0 != rendered {
        notice.0 = rendered;
    }
}

/// The fixed-height box the hint is drawn into: always exactly two lines tall, whether the selected
/// row's hint fills one line or two, so walking the selection never moves the rows above it. See
/// [`super::look::PageLook::hint_slot_height_px`].
#[derive(Component)]
struct HintSlot;

/// An active drag owns the mouse, scrubs even outside the track, and saves once on release.
#[derive(Resource, Default)]
struct SliderDrag(Option<ActiveDrag>);

/// Drag row and grab-time track rect, in PHYSICAL px from UiGlobalTransform/ComputedNode.
///
/// A live rect would feed layout back into its own value: UI SCALE changes UiScale, layout
/// resizes the track, then a stationary cursor maps to a different value. MEASURED jitter/
/// run-away on UI SCALE. Freeze every slider to break the loop while preserving live preview;
/// text-width changes, future resizing rows and window resizing can move any slider's rect.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ActiveDrag {
    row: Row,
    /// The track's centre on x at grab time, PHYSICAL px (`UiGlobalTransform::translation.x`).
    track_center_x: f32,
    /// The track's width at grab time, PHYSICAL px (`ComputedNode::size().x`).
    track_width: f32,
}

impl ActiveDrag {
    /// Freeze the hit-test rect in physical px; [`track_fraction`] explains the Retina 2x trap.
    fn grab(row: Row, node: &ComputedNode, transform: &UiGlobalTransform) -> Self {
        Self {
            row,
            track_center_x: transform.translation.x,
            track_width: node.size().x,
        }
    }

    /// Map PHYSICAL cursor x through the frozen rect.
    fn fraction(self, cursor_x: f32) -> f32 {
        track_fraction(cursor_x, self.track_center_x, self.track_width)
    }
}

/// Shared page plugin; `settings::plugin` must also mount exactly one entry declarer. Making
/// entry its parameter prevents a mounted page that never becomes visible.
pub(super) fn plugin(app: &mut App) {
    // The display row's offered rungs. Mounted here rather than beside the other settings plugins
    // because this page is the only thing that reads them.
    app.add_plugins(super::displays::plugin)
        .init_resource::<SettingsPageVisible>()
        .init_resource::<Page>()
        .init_resource::<SliderDrag>()
        .init_resource::<SaveStatus>()
        .add_systems(Startup, spawn_page)
        .add_systems(Update, refresh_save_notice.after(super::save_on_request))
        // The click path is a `bevy_picking` observer, which runs in `PreUpdate` — ahead of
        // everything below, so a click and the repaint that shows it land in the same frame.
        .add_observer(page_pressed)
        .add_systems(
            Update,
            (
                // Input first, then the refresh reads the same frame's result — so a keypress
                // repaints immediately instead of one frame late.
                (keyboard_input, mouse_input),
                // After the input, so the selection this follows is the one the arrow key just
                // moved rather than last frame's.
                scroll_rows,
                (refresh_page, refresh_tabs),
            )
                .chain()
                // After declaration, before reconcile: input and picture use this frame's state.
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

fn spawn_page(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    settings: Res<Settings>,
    ladder: Res<DisplayLadder>,
    page: Res<Page>,
) {
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
                        width: Val::Px(page_look().card_width_px.0),
                        // Limit the card so overflow scrolls rows; see `super::look::PageLook::card_max_height_percent`.
                        max_height: Val::Percent(page_look().card_max_height_percent.0),
                        padding: UiRect::all(Val::Px(page_look().card_padding_px.0)),
                        border_radius: BorderRadius::all(Val::Px(page_look().card_radius_px.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(page_look().card_row_gap_px.0),
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
                            font_size: FontSize::Px(page_look().title_font_px.0),
                            ..default()
                        },
                        TextColor(TEXT),
                        Node {
                            margin: UiRect::bottom(Val::Px(page_look().title_bottom_margin_px.0)),
                            // Only rows may shrink; flexbox's default would squeeze title and footer first.
                            flex_shrink: 0.0,
                            ..default()
                        },
                    ));

                    spawn_tab_strip(card, &fonts, page.tab);

                    // The scrolling viewport. Every row lives inside it, in its tab's panel;
                    // nothing else does.
                    card.spawn((
                        RowViewport,
                        ScrollPosition::default(),
                        Node {
                            width: Val::Percent(100.0),
                            flex_direction: FlexDirection::Column,
                            overflow: Overflow::scroll_y(),
                            // Load-bearing: a flex item's default `min_height: auto` is its CONTENT
                            // height, so without this the viewport refuses to shrink, the card
                            // grows past its `max_height` anyway, and nothing ever scrolls.
                            min_height: Val::Px(0.0),
                            ..default()
                        },
                    ))
                    .with_children(|viewport| {
                        for tab in Tab::ORDER {
                            let open = tab == page.tab;
                            viewport
                                .spawn((
                                    TabPanel(tab),
                                    Node {
                                        width: Val::Percent(100.0),
                                        flex_direction: FlexDirection::Column,
                                        // Match the card gap to avoid a seam; see `super::look::PageLook::card_row_gap_px`.
                                        row_gap: Val::Px(page_look().card_row_gap_px.0),
                                        // The panel is the content the viewport scrolls, so it
                                        // must keep its full height rather than shrink into it.
                                        flex_shrink: 0.0,
                                        display: panel_display(open),
                                        ..default()
                                    },
                                    panel_visibility(open),
                                ))
                                .with_children(|panel| {
                                    for row in tab.rows() {
                                        spawn_row(panel, &fonts, &settings, &ladder, row);
                                    }
                                });
                        }
                    });

                    // Reserve on a wrapper, not Text; see `super::look::PageLook::hint_slot_height_px`.
                    card.spawn((
                        HintSlot,
                        Node {
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(page_look().hint_top_margin_px.0)),
                            min_height: Val::Px(page_look().hint_slot_height_px()),
                            flex_shrink: 0.0,
                            // Do not stretch text to slot height: its measured height must remain visible to layout.
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
                                font_size: FontSize::Px(page_look().hint_font_px.0),
                                ..default()
                            },
                            LineHeight::Px(page_look().hint_line_height_px()),
                            TextColor(TEXT_DIM),
                        ));
                    });
                    card.spawn((
                        SaveNotice,
                        Text::new(SAVE_LEGEND),
                        TextFont {
                            // Regular: the control legend.
                            font: fonts.body.clone().into(),
                            font_size: FontSize::Px(page_look().legend_font_px.0),
                            ..default()
                        },
                        TextColor(TEXT_DIM),
                        Node {
                            margin: UiRect::top(Val::Px(page_look().legend_top_margin_px.0)),
                            flex_shrink: 0.0,
                            ..default()
                        },
                    ));
                });
        });
}

/// The tab strip: every tab's name in one line, the open one lit and underlined. The strip is a
/// selectable line of its own ([`Line::Tabs`]), padded and highlighted like a row, so it reads as
/// one more stop on the arrow-key walk.
fn spawn_tab_strip(card: &mut ChildSpawnerCommands, fonts: &UiFonts, open: Tab) {
    card.spawn((
        TabStrip,
        // Opts the strip into picking's hover tracking, like a row line — see [`mouse_input`].
        Hovered::default(),
        Node {
            width: Val::Percent(100.0),
            align_items: AlignItems::Center,
            column_gap: Val::Px(page_look().tab_gap_px.0),
            padding: UiRect::axes(
                Val::Px(page_look().row_padding_x_px.0),
                Val::Px(page_look().row_padding_y_px.0),
            ),
            border_radius: BorderRadius::all(Val::Px(page_look().row_radius_px.0)),
            // Fixed furniture, like the title: see the title's `flex_shrink`.
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(page_look().row_bg()),
    ))
    .with_children(|strip| {
        for tab in Tab::ORDER {
            strip
                .spawn((
                    TabLabel(tab),
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(page_look().tab_row_gap_px.0),
                        ..default()
                    },
                ))
                .with_children(|label| {
                    label.spawn((
                        TabName(tab),
                        // Lights the name under the pointer, like an arrow glyph.
                        Hovered::default(),
                        Text::new(tab.label()),
                        TextFont {
                            // SemiBold: a tab name is an affordance, like the arrows.
                            font: fonts.hud.clone().into(),
                            font_size: FontSize::Px(page_look().control_font_px.0),
                            ..default()
                        },
                        TextColor(if tab == open { TEXT } else { TEXT_DIM }),
                    ));
                    label.spawn((
                        TabMarker(tab),
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Px(page_look().tab_marker_height_px.0),
                            ..default()
                        },
                        BackgroundColor(marker_color(tab == open)),
                    ));
                });
        }
    });
}

/// A tab panel's layout: in the flow while open, out of it while closed.
const fn panel_display(open: bool) -> Display {
    if open { Display::Flex } else { Display::None }
}

/// A tab panel's visibility — see [`TabPanel`] for why a closed panel is hidden as well as
/// removed from layout.
const fn panel_visibility(open: bool) -> Visibility {
    if open {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    }
}

/// The bar under a tab's name: the slider fill's colour under the open tab, nothing under the
/// others.
fn marker_color(open: bool) -> Color {
    if open {
        page_look().slider_fill_color()
    } else {
        Color::NONE
    }
}

/// One row: `LABEL` on the left; on the right either `< VALUE >` (stepper) or `[track] VALUE`
/// (slider). The value sits in a fixed-width well so the arrows and track do not shuffle as the
/// value text's width changes.
fn spawn_row(
    card: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    settings: &Settings,
    ladder: &DisplayLadder,
    row: Row,
) {
    card.spawn((
        RowLine(row),
        // Opts the line into picking's hover tracking — see [`mouse_input`].
        Hovered::default(),
        Node {
            width: Val::Percent(100.0),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            padding: UiRect::axes(
                Val::Px(page_look().row_padding_x_px.0),
                Val::Px(page_look().row_padding_y_px.0),
            ),
            border_radius: BorderRadius::all(Val::Px(page_look().row_radius_px.0)),
            ..default()
        },
        BackgroundColor(page_look().row_bg()),
    ))
    .with_children(|line| {
        line.spawn((
            RowLabelText(row),
            Text::new(row.label()),
            TextFont {
                // Regular: a dense settings row.
                font: fonts.body.clone().into(),
                font_size: FontSize::Px(page_look().control_font_px.0),
                ..default()
            },
            TextColor(TEXT),
        ));
        line.spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(page_look().control_gap_px.0),
            ..default()
        })
        .with_children(|controls| match row.kind() {
            RowKind::Stepper => {
                spawn_arrow(controls, fonts, row, -1, "<");
                spawn_value_well(
                    controls,
                    fonts,
                    settings,
                    ladder,
                    row,
                    JustifyContent::Center,
                );
                spawn_arrow(controls, fonts, row, 1, ">");
            }
            RowKind::Slider => {
                controls
                    .spawn((
                        SliderTrack(row),
                        Node {
                            width: Val::Px(page_look().track_width_px.0),
                            height: Val::Px(page_look().track_hit_height_px.0),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                    ))
                    .with_children(|track| {
                        track
                            .spawn((
                                Node {
                                    width: Val::Percent(100.0),
                                    height: Val::Px(page_look().track_bar_height_px.0),
                                    border_radius: BorderRadius::all(Val::Px(
                                        page_look().track_radius_px.0,
                                    )),
                                    ..default()
                                },
                                BackgroundColor(page_look().slider_track_bg()),
                            ))
                            .with_children(|bar| {
                                let fraction = row
                                    .slider_fraction(settings)
                                    .expect("RowKind::Slider declares a slider_fraction mapping");
                                bar.spawn((
                                    SliderFill(row),
                                    Node {
                                        width: Val::Percent(fraction * 100.0),
                                        height: Val::Percent(100.0),
                                        border_radius: BorderRadius::all(Val::Px(
                                            page_look().track_radius_px.0,
                                        )),
                                        ..default()
                                    },
                                    BackgroundColor(page_look().slider_fill_color()),
                                ));
                            });
                    });
                spawn_value_well(
                    controls,
                    fonts,
                    settings,
                    ladder,
                    row,
                    JustifyContent::FlexEnd,
                );
            }
        });
    });
}

/// A `<` / `>` affordance: dim at rest, bright on hover ([`refresh_page`] paints it from the glyph's
/// own [`Hovered`]), padded so its clickable rect is meaningfully larger than the glyph. Picking
/// honours that padding — the UI backend hit-tests the whole node rect and only narrows to a text
/// SECTION when the point is inside a shaped run, so a click in the pad still resolves here.
fn spawn_arrow(
    controls: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    row: Row,
    delta: i32,
    glyph: &str,
) {
    controls.spawn((
        ArrowGlyph { row, delta },
        Hovered::default(),
        Text::new(glyph),
        TextFont {
            // SemiBold: an affordance, not prose.
            font: fonts.hud.clone().into(),
            font_size: FontSize::Px(page_look().control_font_px.0),
            ..default()
        },
        TextColor(TEXT_DIM),
        Node {
            padding: UiRect::axes(
                Val::Px(page_look().arrow_padding_x_px.0),
                Val::Px(page_look().arrow_padding_y_px.0),
            ),
            ..default()
        },
    ));
}

/// The row's value, in a fixed-width well ([`Row::value_well_px`]) so the control beside it does not
/// shuffle as the value text's width changes. `justify` is the only other thing the two row kinds
/// disagree about.
fn spawn_value_well(
    controls: &mut ChildSpawnerCommands,
    fonts: &UiFonts,
    settings: &Settings,
    ladder: &DisplayLadder,
    row: Row,
    justify: JustifyContent,
) {
    controls
        .spawn(Node {
            width: Val::Px(row.value_well_px()),
            justify_content: justify,
            ..default()
        })
        .with_child((
            RowValueText(row),
            Text::new(row.value(settings, ladder)),
            TextFont {
                // SemiBold: the value is what the eye goes to.
                font: fonts.hud.clone().into(),
                font_size: FontSize::Px(page_look().control_font_px.0),
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
    ladder: Res<DisplayLadder>,
    mut page: ResMut<Page>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    if !visible.0 {
        return;
    }
    // Guarded rather than stepped by zero every frame: `Page` is a `ResMut`, and a write is a
    // change-tick bump even when the value lands where it already was.
    let move_by = i32::from(keys.just_pressed(KeyCode::ArrowDown))
        - i32::from(keys.just_pressed(KeyCode::ArrowUp));
    if move_by != 0 {
        let current = *settings;
        page.step(move_by, |row| row.enabled(&current, *caps));
    }
    let delta = i32::from(keys.just_pressed(KeyCode::ArrowRight))
        - i32::from(keys.just_pressed(KeyCode::ArrowLeft));
    if delta == 0 {
        return;
    }
    match page.line {
        Line::Tabs => {
            let mut next = *page;
            next.switch(delta);
            page.set_if_neq(next);
        }
        Line::Row(row) if row.enabled(&settings, *caps) => {
            change_row(row, delta, &mut settings, &mut save, *caps, &ladder);
        }
        Line::Row(_) => {}
    }
}

/// One Press observer handles arrows, slider grabs and tab clicks. Deepest-first bubbling
/// lets arrows or tracks stop propagation before their enclosing row; fills bubble to tracks.
/// Picking excludes the hidden card via InheritedVisibility.
///
/// Read the live track rect only on grab; [`ActiveDrag`] freezes it and [`track_fraction`]
/// requires the matching physical cursor.
#[expect(
    clippy::too_many_arguments,
    reason = "an observer spanning arrows, tracks, tabs, window, capabilities, ladder, page, drag, settings and save"
)]
fn page_pressed(
    mut press: On<Pointer<Press>>,
    arrows: Query<&ArrowGlyph>,
    tracks: Query<(&SliderTrack, &ComputedNode, &UiGlobalTransform)>,
    tabs: Query<&TabLabel>,
    window: Query<&Window, With<PrimaryWindow>>,
    caps: Res<PresentCaps>,
    ladder: Res<DisplayLadder>,
    mut page: ResMut<Page>,
    mut drag: ResMut<SliderDrag>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    let window = crate::state::primary_window(window.single());
    if let Ok(arrow) = arrows.get(press.entity) {
        press.propagate(false);
        if press.button == PointerButton::Primary && arrow.row.enabled(&settings, *caps) {
            change_row(
                arrow.row,
                arrow.delta,
                &mut settings,
                &mut save,
                *caps,
                &ladder,
            );
        }
    } else if let Ok((track, node, transform)) = tracks.get(press.entity) {
        press.propagate(false);
        let cursor = window.and_then(|window| window.physical_cursor_position());
        if press.button != PointerButton::Primary || !track.0.enabled(&settings, *caps) {
            return;
        }
        let Some(cursor) = cursor else { return };
        page.select(track.0);
        let active = ActiveDrag::grab(track.0, node, transform);
        drag.0 = Some(active);
        // No save here — the release path in `mouse_input` writes the file once per drag.
        scrub(active, cursor.x, &mut settings);
    } else if let Ok(tab) = tabs.get(press.entity) {
        press.propagate(false);
        if press.button == PointerButton::Primary {
            let mut next = *page;
            next.open(tab.0);
            page.set_if_neq(next);
        }
    }
}

/// Scrub from PHYSICAL cursor x. Compare first: ResMut writes bump the change tick even if
/// unchanged, so a held-still drag would otherwise rerun apply_settings.
fn scrub(active: ActiveDrag, cursor_x: f32, settings: &mut ResMut<Settings>) {
    let mut next = **settings;
    active
        .row
        .set_from_fraction(&mut next, active.fraction(cursor_x));
    if next != **settings {
        **settings = next;
    }
}

/// Per-frame mouse work: carry a grabbed slider and select hovered rows. Press/release without
/// motion emits no Pointer<Drag>; closing the page under a held button must release too.
///
/// Hovered includes descendants (CSS :hover) and changes only on enter/leave, so crossing an
/// arrow selects its row without per-frame hover changes.
///
/// Use `physical_cursor_position()`, not hand-converted `cursor_position() * scale_factor()`,
/// for [`ActiveDrag`]'s physical rect. A node's inverse_scale_factor
/// also includes UiScale and converts to CSS, not window-logical pixels; see [`track_fraction`].
/// Source: bevy_ui-0.19.1/src/update.rs:151-159 (`propagate_ui_target_cameras`).
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning page visibility, buttons, window, hovered rows and strip, capabilities, ladder, page, drag, settings and save"
)]
fn mouse_input(
    visible: Res<SettingsPageVisible>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Query<&Window, With<PrimaryWindow>>,
    rows: Query<(&RowLine, &Hovered), Changed<Hovered>>,
    strip: Query<&Hovered, (With<TabStrip>, Changed<Hovered>)>,
    caps: Res<PresentCaps>,
    ladder: Res<DisplayLadder>,
    mut page: ResMut<Page>,
    mut drag: ResMut<SliderDrag>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    let window = crate::state::primary_window(window.single());
    // A drag owns selection until release/page close, saves once, and keeps its frozen rect.
    if let Some(active) = drag.0 {
        if !visible.0 || !buttons.pressed(MouseButton::Left) {
            drag.0 = None;
            save.write(SaveSettings);
            info!(
                "settings: {} -> {}",
                active.row.label(),
                active.row.value(&settings, &ladder)
            );
        } else if let Some(cursor) = window.and_then(|window| window.physical_cursor_position()) {
            scrub(active, cursor.x, &mut settings);
        }
        return;
    }
    // Skip disabled rows so mouse and keyboard describe the same selectable rows in the hint.
    for (line, hovered) in &rows {
        if hovered.get() && line.0.enabled(&settings, *caps) {
            page.select(line.0);
        }
    }
    if strip.iter().any(Hovered::get) {
        page.line = Line::Tabs;
    }
}

/// Slider position, `0.0..=1.0`, from PHYSICAL cursor/centre/width. A logical cursor would
/// silently misplace the handle on Retina (2x scale). Pure for tests without layout.
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
    ladder: &DisplayLadder,
) {
    let before = **settings;
    let mut next = before;
    row.step(&mut next, delta, caps, ladder);
    if next == before {
        return;
    }
    // Through `ResMut`'s `DerefMut` so change detection fires and `apply_settings` runs.
    **settings = next;
    save.write(SaveSettings);
    info!("settings: {} -> {}", row.label(), row.value(&next, ladder));
}

/// Wheel delta in CSS px for [`ScrollPosition`]; to_css is the viewport's inverse_scale_factor.
/// Line is a dimensionless notch count scaled by the authored CSS distance, so it grows with the rows.
/// Pixel is winit PhysicalPosition copied unchanged by Bevy; convert once to CSS:
/// winit-0.30.13/src/event.rs:961-975 (`PixelDelta`),
/// bevy_winit-0.19.1/src/state.rs:342-349 (`MouseWheel`). Layout
/// multiplies by target scale × UiScale, so omitting conversion scales the flick twice
/// (0.75x/1.5x at 75/150%, doubled on Retina). Equal physical flicks must move equal physical
/// content distances. Pure for unit tests without selection-follow interference.
fn wheel_delta_css(unit: bevy::input::mouse::MouseScrollUnit, y: f32, to_css: f32) -> f32 {
    match unit {
        bevy::input::mouse::MouseScrollUnit::Line => y * page_look().wheel_line_px.0,
        bevy::input::mouse::MouseScrollUnit::Pixel => y * to_css,
    }
}

/// Follow the wheel, then keep the selected row visible. Keyboard selection must never vanish
/// while editing; this follow rule also makes a scrollbar unnecessary.
///
/// Update reads last frame's PostUpdate layout, already scrolled. Correct its delta rather
/// than duplicating taffy; the next frame converges and self-corrects after resize/UI-scale
/// changes or new rows.
///
/// ScrollPosition is CSS px; ComputedNode sizes and UiGlobalTransform positions are physical.
/// Convert rect differences with the viewport's inverse_scale_factor (includes UiScale). Layout
/// multiplies scroll by its reciprocal (bevy_ui-0.19.1/src/layout/mod.rs:347-367,
/// `update_uinode_geometry_recursive`).
///
/// Clamp the component here: Bevy clamps only its rendered clamped_scroll_position, leaving
/// the component free to bank offsets that wheel-up must unwind before anything moves.
fn scroll_rows(
    visible: Res<SettingsPageVisible>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    page: Res<Page>,
    mut viewport: Query<
        (&ComputedNode, &UiGlobalTransform, &mut ScrollPosition),
        With<RowViewport>,
    >,
    rows: Query<(&RowLine, &ComputedNode, &UiGlobalTransform)>,
) {
    let (node, transform, mut scroll) = viewport
        .single_mut()
        .expect("settings::ui::spawn_page creates exactly one RowViewport");
    if !visible.0 {
        // Drain, so a wheel spun over the game with the page shut does not arrive as a jump the
        // next time it opens.
        wheel.clear();
        return;
    }
    // Physical -> CSS px, the space `ScrollPosition` is in. See the doc above.
    let to_css = node.inverse_scale_factor();
    let mut wanted = scroll.0.y;

    for event in wheel.read() {
        // Wheel-up is positive and means "move the content down", i.e. a SMALLER offset.
        wanted -= wheel_delta_css(event.unit, event.y, to_css);
    }

    // Selection wins over wheel scroll; selecting the strip above the viewport asks no correction.
    if let Some(selected) = page.row()
        && let Some((_, row_node, row_transform)) =
            rows.iter().find(|(line, _, _)| line.0 == selected)
    {
        let view_half = node.size().y / 2.0;
        let row_half = row_node.size().y / 2.0;
        // Both rects are physical and share an origin, so their difference is a physical distance.
        let above =
            (transform.translation.y - view_half) - (row_transform.translation.y - row_half);
        let below =
            (row_transform.translation.y + row_half) - (transform.translation.y + view_half);
        if above > 0.0 {
            wanted -= above * to_css;
        } else if below > 0.0 {
            wanted += below * to_css;
        }
    }

    // Include scrollbar_size to match Bevy's max_possible_offset and keep the last row reachable.
    let overflow = (node.content_size().y - node.size().y + node.scrollbar_size.y).max(0.0);
    let wanted = wanted.clamp(0.0, overflow * to_css);
    // Guarded: `ScrollPosition` is a `Mut`, and a write is a change-tick bump even when the value
    // lands where it already was — which is every frame the page is merely open.
    if scroll.0.y != wanted {
        scroll.0.y = wanted;
    }
}

/// Rebuild every value, the selection highlight, the slider fills, the enabled/disabled colours and
/// the footer hint. Runs only while the page is up; the visibility write itself is `set_if_neq`, so
/// a closed page costs one comparison per frame.
#[expect(
    clippy::too_many_arguments,
    reason = "one query per disjoint page element plus the settings model, kept disjoint by their `Without` filters"
)]
fn refresh_page(
    visible: Res<SettingsPageVisible>,
    settings: Res<Settings>,
    // The greying reads the EFFECTIVE vsync rung, not the stored one — see [`Row::enabled`].
    caps: Res<PresentCaps>,
    ladder: Res<DisplayLadder>,
    page: Res<Page>,
    // A scrubbing hand is not pointing at anything: an arrow the cursor crosses mid-drag must not
    // light up, the same way hover does not move the selection then.
    drag: Res<SliderDrag>,
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
        (&ArrowGlyph, &Hovered, &mut TextColor),
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
    let mut card = card
        .single_mut()
        .expect("settings::ui::spawn_page creates exactly one SettingsCard");
    let mut hint = hint
        .single_mut()
        .expect("settings::ui::spawn_page creates exactly one HintText inside its card");
    card.set_if_neq(if visible.0 {
        Visibility::Visible
    } else {
        Visibility::Hidden
    });
    if !visible.0 {
        return;
    }

    let selected = page.row();
    for (value, mut text, mut color) in &mut values {
        let rendered = value.0.value(&settings, &ladder);
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
    for (arrow, hovered, mut color) in &mut arrow_glyphs {
        let lit = hovered.get() && drag.0.is_none() && arrow.row.enabled(&settings, *caps);
        color.set_if_neq(TextColor(if lit { TEXT } else { TEXT_DIM }));
    }
    for (fill, mut node, mut color) in &mut fills {
        let width = Val::Percent(
            fill.0
                .slider_fraction(&settings)
                .expect("every SliderFill row has a slider_fraction mapping")
                * 100.0,
        );
        if node.width != width {
            node.width = width;
        }
        color.set_if_neq(BackgroundColor(if fill.0.enabled(&settings, *caps) {
            page_look().slider_fill_color()
        } else {
            page_look().slider_fill_disabled()
        }));
    }
    for (line, mut background) in &mut lines {
        background.set_if_neq(BackgroundColor(if selected == Some(line.0) {
            page_look().row_bg_selected()
        } else {
            page_look().row_bg()
        }));
    }
    let rendered = selected.map_or(TAB_STRIP_HINT, Row::hint);
    if hint.0 != rendered {
        hint.0 = rendered.to_string();
    }
}

/// The tab strip's half of [`refresh_page`]: the open tab's panel in the layout and every other one
/// out of it, the names lit while open or hovered, the bar under the open tab, and the strip's own
/// selection highlight. Runs only while the page is up, like that system.
fn refresh_tabs(
    visible: Res<SettingsPageVisible>,
    page: Res<Page>,
    // A scrubbing hand is not pointing at anything — see `refresh_page`.
    drag: Res<SliderDrag>,
    mut panels: Query<(&TabPanel, &mut Node, &mut Visibility)>,
    mut names: Query<(&TabName, &Hovered, &mut TextColor)>,
    mut markers: Query<(&TabMarker, &mut BackgroundColor), Without<TabStrip>>,
    mut strip: Query<&mut BackgroundColor, (With<TabStrip>, Without<TabMarker>)>,
) {
    if !visible.0 {
        return;
    }
    for (panel, mut node, mut visibility) in &mut panels {
        let open = panel.0 == page.tab;
        let display = panel_display(open);
        // Guarded by hand: a `Node` write re-lays out the whole card.
        if node.display != display {
            node.display = display;
        }
        visibility.set_if_neq(panel_visibility(open));
    }
    for (name, hovered, mut color) in &mut names {
        let hovered = hovered.get() && drag.0.is_none();
        color.set_if_neq(TextColor(if name.0 == page.tab || hovered {
            TEXT
        } else {
            TEXT_DIM
        }));
    }
    for (marker, mut color) in &mut markers {
        color.set_if_neq(BackgroundColor(marker_color(marker.0 == page.tab)));
    }
    for mut background in &mut strip {
        background.set_if_neq(BackgroundColor(if page.line == Line::Tabs {
            page_look().row_bg_selected()
        } else {
            page_look().row_bg()
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Missing or duplicate card/hint nodes are construction bugs, never a silently disabled page.
    /// Exercise the production reconciler directly so a dropped cardinality check fails here.
    #[test]
    fn page_reconciliation_refuses_broken_card_and_hint_cardinalities() {
        // Fixture cardinalities cover both failure shapes of each singleton.
        for visible in [false, true] {
            for (cards, hints, expected) in [
                (0, 1, "exactly one SettingsCard"),
                (2, 1, "exactly one SettingsCard"),
                (1, 0, "exactly one HintText"),
                (1, 2, "exactly one HintText"),
            ] {
                let mut app = App::new();
                app.insert_resource(SettingsPageVisible(visible))
                    .init_resource::<Settings>()
                    .init_resource::<PresentCaps>()
                    .init_resource::<DisplayLadder>()
                    .init_resource::<Page>()
                    .init_resource::<SliderDrag>()
                    .add_systems(Update, refresh_page);
                for _ in 0..cards {
                    app.world_mut().spawn((SettingsCard, Visibility::Hidden));
                }
                for _ in 0..hints {
                    app.world_mut().spawn((HintText, Text::new("")));
                }
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()))
                    .expect_err("broken page construction must panic");
                let message = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .expect("the page's invariant panic has a message");
                assert!(
                    message.contains(expected),
                    "{cards} cards/{hints} hints: {message}"
                );
            }
        }
    }

    /// The unprobed capability state every pure test steps under: all rungs offered.
    const ALL_RUNGS: PresentCaps = PresentCaps::Unprobed;

    /// The display ladder of a world whose monitors have not been synchronised — every variant a
    /// file can hold, which is what the rows that are not the display row want to step under.
    fn unlearned() -> DisplayLadder {
        DisplayLadder::default()
    }

    /// Saturated steps must preserve whole-Settings equality, which change_row/scrub use to avoid
    /// repeated writes. Test accepted budgets including finite off-ladder values; the parse test
    /// `store::a_non_finite_budget_cannot_survive_the_parse` guards the boundary.
    #[test]
    fn a_saturated_step_on_any_row_is_a_no_op() {
        for budget in [
            PixelBudget::default(),
            PixelBudget(1.2),
            PixelBudget(PixelBudget::min()),
        ] {
            let settings = Settings {
                lod_pixel_budget: budget,
                ..default()
            };
            assert_eq!(
                settings, settings,
                "{budget:?}: settings must equal themselves, or every no-op guard on this page \
                 fails open and the file is written every frame",
            );
            // Walk every row to BOTH ends and then step past them: the last step of each walk must
            // land on a value that compares equal, which is what `change_row` returns early on.
            for row in Row::ORDER {
                for direction in [-1, 1] {
                    let mut walked = settings;
                    for _ in 0..64 {
                        row.step(&mut walked, direction, ALL_RUNGS, &unlearned());
                    }
                    let saturated = walked;
                    row.step(&mut walked, direction, ALL_RUNGS, &unlearned());
                    assert_eq!(
                        walked, saturated,
                        "{row:?} stepping {direction} past the end of its ladder must compare \
                         equal, or leaning on the key saves the file on every repeat",
                    );
                }
            }
        }
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
        Row::VSync.step(&mut settings, -1, metal, &unlearned());
        assert_eq!(
            settings.vsync,
            VsyncMode::Off,
            "one step down on Metal must skip the unoffered FAST"
        );
        Row::VSync.step(&mut settings, 1, metal, &unlearned());
        assert_eq!(settings.vsync, VsyncMode::On);

        let mut settings = Settings::default();
        Row::VSync.step(&mut settings, -1, ALL_RUNGS, &unlearned());
        assert_eq!(settings.vsync, VsyncMode::Fast, "unprobed offers all rungs");
        Row::VSync.step(&mut settings, -1, ALL_RUNGS, &unlearned());
        assert_eq!(settings.vsync, VsyncMode::Off);
        Row::VSync.step(&mut settings, -1, ALL_RUNGS, &unlearned());
        assert_eq!(settings.vsync, VsyncMode::Off, "the floor saturates");

        // A stored FAST on a Metal surface: not offered, so a step resolves into the offered set.
        let mut settings = Settings {
            vsync: VsyncMode::Fast,
            ..default()
        };
        Row::VSync.step(&mut settings, 1, metal, &unlearned());
        assert!(
            metal.offers(settings.vsync),
            "stepping an unoffered value must land inside the offered set, got {:?}",
            settings.vsync,
        );
    }

    /// An unpresentable stored OFF (Wayland refuses `Immediate`) must grey the frame-cap row,
    /// matching the limiter. Gate on the effective rung even before `settings::normalize_vsync`
    /// persists the correction. Every other row stays enabled under every rung on every surface.
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

        // Every other row is unconditional: no rung on any surface greys it.
        for caps in [
            wayland,
            metal,
            PresentCaps::Unprobed,
            PresentCaps::Unavailable,
        ] {
            for vsync in VsyncMode::ORDER {
                let settings = Settings { vsync, ..default() };
                for row in Row::ORDER.into_iter().filter(|&row| row != Row::FrameCap) {
                    assert!(
                        row.enabled(&settings, caps),
                        "{row:?} under {vsync:?} on {caps:?}"
                    );
                }
            }
        }
    }

    /// The selection walk skips disabled rows in BOTH directions and saturates at the bottom. With
    /// the default settings (vsync ON) the frame-cap row is a hole in the ladder: walking down from
    /// VSYNC lands on UI SCALE, walking back up from UI SCALE lands on VSYNC.
    #[test]
    fn the_selection_skips_disabled_rows() {
        let settings = Settings::default();
        assert_eq!(settings.vsync, VsyncMode::On, "the premise: cap disabled");
        let enabled = |row: Row| row.enabled(&settings, ALL_RUNGS);
        let on = |row: Row| Page {
            tab: row.tab(),
            line: Line::Row(row),
        };

        let mut page = on(Row::VSync);
        page.step(1, enabled);
        assert_eq!(
            page.row(),
            Some(Row::UiScale),
            "down from VSYNC must skip the disabled FRAME CAP"
        );
        page.step(-1, enabled);
        assert_eq!(
            page.row(),
            Some(Row::VSync),
            "up from UI SCALE must skip it too"
        );

        // With vsync OFF the row is an ordinary stop on the walk.
        let armed = Settings {
            vsync: VsyncMode::Off,
            ..default()
        };
        let mut page = on(Row::VSync);
        page.step(1, |row| row.enabled(&armed, ALL_RUNGS));
        assert_eq!(page.row(), Some(Row::FrameCap));

        // Holding down stops at the open tab's last row, never wrapping and never crossing into
        // the next tab's rows.
        for tab in Tab::ORDER {
            let last = tab.rows().last().expect("every tab has rows");
            let mut page = Page {
                tab,
                line: Line::Tabs,
            };
            for _ in 0..Row::ORDER.len() * 2 {
                page.step(1, |_| true);
            }
            assert_eq!(
                page.row(),
                Some(last),
                "{tab:?}: holding down must stop at the tab's last row",
            );
        }
    }

    /// Every row must render on exactly one nonempty tab. GAMEPLAY's four rows are the owner's
    /// split; AUDIO/CONTROLS must wait for their own rows.
    #[test]
    fn the_tabs_partition_the_rows() {
        for tab in Tab::ORDER {
            assert!(tab.rows().next().is_some(), "{tab:?} has no rows");
        }
        let drawn: usize = Tab::ORDER.iter().map(|tab| tab.rows().count()).sum();
        assert_eq!(drawn, Row::ORDER.len(), "a row is on no tab, or on two");
        assert_eq!(
            Tab::Gameplay.rows().collect::<Vec<_>>(),
            [
                Row::PenCamCorner,
                Row::KillCam,
                Row::ReduceMotion,
                Row::DebugPanel
            ],
        );
        assert_eq!(
            Row::ORDER[0].tab(),
            Tab::default(),
            "the default selection (`Page::default`) must be on the default tab",
        );
        assert_eq!(Page::default().row(), Some(Row::ORDER[0]));
    }

    /// **The tab strip is walked with the arrows, like a row.** `Up` from a tab's first row selects
    /// the strip, `Left`/`Right` there switch tabs (saturating, like every row), and `Down` enters
    /// the open tab's first row. A row of a closed tab can never be selected, by the walk or by
    /// [`Page::select`] — the hint would describe a row the player cannot see.
    #[test]
    fn the_tab_strip_is_the_line_above_every_tab() {
        let all = |_: Row| true;
        let mut page = Page::default();
        page.step(-1, all);
        assert_eq!(
            page.line,
            Line::Tabs,
            "up from the first row reaches the strip"
        );
        page.step(-1, all);
        assert_eq!(page.line, Line::Tabs, "the strip is the top: up stays put");

        page.switch(-1);
        assert_eq!(page.tab, Tab::Video, "the first tab saturates");
        for _ in 0..Tab::ORDER.len() * 2 {
            page.switch(1);
        }
        assert_eq!(
            page.tab,
            Tab::Gameplay,
            "the last tab saturates, never wraps"
        );
        assert_eq!(page.line, Line::Tabs, "switching keeps the strip selected");

        page.step(1, all);
        assert_eq!(page.row(), Tab::Gameplay.rows().next());
        let mut walked = vec![page.row()];
        for _ in 0..Row::ORDER.len() {
            page.step(1, all);
            walked.push(page.row());
        }
        assert!(
            walked
                .iter()
                .flatten()
                .all(|row| row.tab() == Tab::Gameplay),
            "the walk left the open tab: {walked:?}",
        );

        page.select(Row::Msaa);
        assert_ne!(
            page.row(),
            Some(Row::Msaa),
            "a closed tab's row must not be selectable"
        );
        page.open(Tab::Video);
        assert_eq!(
            page,
            Page {
                tab: Tab::Video,
                line: Line::Tabs
            }
        );
        page.select(Row::Msaa);
        assert_eq!(page.row(), Some(Row::Msaa));
    }

    /// Through Row::step, every driven-row change must derive CUSTOM or the exact matching tier;
    /// undriven edits preserve the tier. Test each tier, row and direction so no stored label can
    /// go stale.
    #[test]
    fn editing_a_driven_row_reads_custom_and_any_other_row_keeps_the_tier() {
        use super::super::preset::Look;

        for tier in Preset::ORDER {
            let mut base = Settings::default();
            tier.apply(&mut base);
            assert_eq!(Row::Preset.value(&base, &unlearned()), tier.label());
            for row in Row::ORDER.into_iter().filter(|row| *row != Row::Preset) {
                for delta in [-1, 1] {
                    let mut edited = base;
                    row.step(&mut edited, delta, ALL_RUNGS, &unlearned());
                    let label = Row::Preset.value(&edited, &unlearned());
                    let look = Look::of(&edited);
                    if look == Look::of(&base) {
                        assert_eq!(
                            label,
                            tier.label(),
                            "{row:?} {delta}: not a preset row, but it moved the label",
                        );
                        continue;
                    }
                    let matching = Preset::ORDER.into_iter().find(|t| t.look() == look);
                    assert_eq!(
                        label,
                        matching.map_or("CUSTOM", Preset::label),
                        "{row:?} {delta} from {tier:?}",
                    );
                }
            }
        }
        // And the row itself: a step from a tier lands on the next tier, all of its rows at once.
        let mut settings = Settings::default();
        Preset::ORDER[0].apply(&mut settings);
        Row::Preset.step(&mut settings, 1, ALL_RUNGS, &unlearned());
        assert_eq!(Look::of(&settings), Preset::ORDER[1].look());
    }

    /// **Nothing hidden: every value a preset drives has its own row, and those rows sit directly
    /// below PRESET** — which is what its hint promises. A row is found to be driven by stepping it
    /// and watching [`Look::ranks`] move, so the test names no row: each driven row moves exactly
    /// one of the driven values, and together they move all of them.
    #[test]
    fn the_rows_a_preset_drives_sit_directly_below_it() {
        use super::super::preset::{DRIVEN_ROWS, Look};

        let base = Settings::default();
        let mut covered = [false; DRIVEN_ROWS];
        let mut driven = Vec::new();
        for row in Row::ORDER.into_iter().filter(|row| *row != Row::Preset) {
            let mut moves = false;
            for delta in [-1, 1] {
                let mut edited = base;
                row.step(&mut edited, delta, ALL_RUNGS, &unlearned());
                let (before, after) = (Look::of(&base).ranks(), Look::of(&edited).ranks());
                let changed: Vec<usize> = (0..DRIVEN_ROWS)
                    .filter(|&i| before[i] != after[i])
                    .collect();
                assert!(
                    changed.len() <= 1,
                    "{row:?} moves driven values {changed:?}"
                );
                for value in changed {
                    covered[value] = true;
                    moves = true;
                }
            }
            if moves {
                driven.push(row);
            }
        }
        assert!(
            covered.iter().all(|value| *value),
            "a preset drives a value no row shows: {covered:?}",
        );
        let mut video = Tab::Video.rows();
        assert_eq!(video.next(), Some(Row::Preset), "PRESET heads VIDEO");
        let below: Vec<Row> = video.take(driven.len()).collect();
        assert_eq!(
            driven, below,
            "the driven rows must sit directly below PRESET"
        );
        assert_eq!(
            driven.len(),
            DRIVEN_ROWS,
            "one row per driven value — and the PRESET hint says \"nine rows\"",
        );
        assert_eq!(DRIVEN_ROWS, 9, "the PRESET hint says \"nine rows\"");
    }

    /// The slider mapping round-trips through the Row API for every ladder stop of every slider
    /// row — a drag to where the handle already sits is always a no-op — and each row's stops
    /// strictly ascend along its track.
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
                row.step(&mut settings, -1, ALL_RUNGS, &unlearned());
            }
            let mut previous: Option<f32> = None;
            loop {
                let fraction = row
                    .slider_fraction(&settings)
                    .expect("slider rows have fractions");
                assert!((0.0..=1.0).contains(&fraction), "{row:?} at {fraction}");
                assert!(
                    previous.is_none_or(|previous| fraction > previous),
                    "{row:?}: the stop at {fraction} does not ascend from {previous:?}",
                );
                previous = Some(fraction);
                let mut reconstructed = settings;
                row.set_from_fraction(&mut reconstructed, fraction);
                assert_eq!(
                    reconstructed, settings,
                    "{row:?}: a drag to the current handle position must be a no-op",
                );
                let mut next = settings;
                row.step(&mut next, 1, ALL_RUNGS, &unlearned());
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

    /// `step_in` clamps and tolerates a value that is not in the order (which the UI cannot produce,
    /// but a hand-edited config could hand us via serde).
    #[test]
    fn step_in_clamps_and_survives_an_unknown_current() {
        assert_eq!(step_in(&[1, 2, 3], 1, -5), 1);
        assert_eq!(step_in(&[1, 2, 3], 3, 5), 3);
        assert_eq!(step_in(&[1, 2, 3], 2, 1), 3);
        assert_eq!(step_in(&[1, 2, 3], 99, 1), 2, "unknown current starts at 0");
    }

    // --- The real page -------------------------------------------------------------------------
    //
    // Measured on the REAL page, headless. `DefaultPlugins` with the GPU backends and winit off (the
    // `headless_test` recipe) still runs the whole UI stack — taffy layout and parley text shaping
    // against the SHIPPED Barlow Condensed — so what these tests read out of `ComputedNode` is
    // bevy's own answer about the card, not an arithmetic re-derivation of it.

    /// How long to wait for the font asset (real, async, wall-clock file IO) before calling it a
    /// failure. Generous: the loop exits the moment the card has laid out.
    const LAYOUT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

    /// A headless app running the REAL page — [`plugin`], not just [`spawn_page`] — with `UiScale`
    /// set to `ui_scale`. The whole plugin, so the mouse test below exercises the wiring the game
    /// ships (observer included) rather than a hand-assembled lookalike.
    ///
    /// The window and camera are load-bearing rather than ceremony: the UI's scale factor and
    /// viewport come from the camera's target (`propagate_ui_target_cameras`), and with no target at
    /// all the root node is zero-sized, the fixed-width card flex-shrinks into it, and every hint
    /// wraps into nonsense. No winit and no GPU are needed for that — a `Window` COMPONENT carries
    /// its own resolution, which is all `Camera::physical_viewport_size` reads.
    fn headless_card_app(ui_scale: f32) -> App {
        headless_card_app_in(ui_scale, 1080)
    }

    /// [`headless_card_app`] in a window of a chosen HEIGHT — the knob the scroll tests need, since
    /// whether the row list overflows is a fact about the window, not about the card. The width
    /// stays 1920 for every caller: it only has to be wide enough that the card never flex-shrinks
    /// at any rung of the ladder (the top rung asks for `page_look().card_width_px.0 * 1.5` = 840 physical px).
    fn headless_card_app_in(ui_scale: f32, window_height: u32) -> App {
        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(Some(Window {
            resolution: bevy::window::WindowResolution::new(1920, window_height),
            ..default()
        })))
        .add_plugins(crate::ui_font::plugin)
        .init_resource::<Settings>()
        // What `settings::plugin` would have supplied around the page; the rest of that plugin is
        // disk IO and window observers this measurement has no use for.
        .init_resource::<PresentCaps>()
        .add_message::<SaveSettings>()
        .add_plugins(plugin)
        .insert_resource(UiScale(ui_scale))
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
        });

        // `App::run` normally drives plugin finish/cleanup; a bare `update()` loop must do it.
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        app
    }

    /// Wait for both font weights, then stable card/viewport/content geometry. Before shaping,
    /// unloaded text measures zero and the hint slot alone can make layout assertions pass.
    ///
    /// Independent Regular/SemiBold async loads can land in different frames. MEASURED CI failure:
    /// waiting only for Regular cached 242 px overflow; SemiBold arrived during wheel input and
    /// the correct clamp became 306 px. Fast machines masked the race. Labels/hint/legend use
    /// Regular; title/values/arrows need SemiBold.
    ///
    /// After loading, pump until stable rather than guessing Bevy's remeasurement pass count.
    fn settle_with_font(app: &mut App) {
        let started = std::time::Instant::now();
        loop {
            app.update();
            let fonts = app.world().resource::<crate::ui_font::UiFonts>().clone();
            let server = app.world().resource::<AssetServer>();
            let states = [
                ("SemiBold", server.load_state(&fonts.hud)),
                ("Regular", server.load_state(&fonts.body)),
            ];
            if states.iter().all(|(_, state)| state.is_loaded()) {
                break;
            }
            assert!(
                started.elapsed() < LAYOUT_DEADLINE
                    && !states.iter().any(|(_, state)| state.is_failed()),
                "the page's fonts never loaded headless after {:?} ({states:?}) — assets/fonts/\
                 BarlowCondensed-SemiBold.ttf and -Regular.ttf are what the whole measurement rests \
                 on",
                started.elapsed(),
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // The card's height, the viewport's rect and its content: the three numbers every
        // measurement below is taken from, so a frame that repeats all three is a settled layout.
        let mut last = None;
        for _ in 0..32 {
            app.update();
            let now = (card_height(app), viewport_state(app));
            if last == Some(now) {
                return;
            }
            last = Some(now);
        }
        panic!("the card never settled on a layout with both fonts loaded (last {last:?})");
    }

    /// Full settings composition without winit or a GPU. The explicit store path isolates IO.
    fn headless_settings_app(settings: Settings, path: Option<std::path::PathBuf>) -> App {
        let mut app = App::new();
        app.add_plugins(crate::gpu_less_default_plugins(None))
            .add_plugins(crate::ui_font::plugin)
            .insert_resource(State::new(crate::state::AppState::Paused))
            .init_resource::<bevy::winit::WinitMonitors>()
            .insert_resource(settings)
            .insert_resource(super::super::store::StorePath(path))
            .add_plugins(super::super::plugin(super::super::PageEntry::PauseState));
        app.finish();
        app.cleanup();
        app
    }

    /// The page and window reconcilers require one viewport, at most one primary window and
    /// mappings for every slider fill. Broken construction must panic rather than freeze controls.
    #[test]
    fn invalid_settings_page_construction_panics() {
        for (defect, expected) in [
            ("missing viewport", "exactly one RowViewport"),
            ("duplicate viewport", "exactly one RowViewport"),
            ("duplicate primary window", "at most one primary window"),
            ("stepper slider fill", "slider_fraction mapping"),
        ] {
            let mut app = headless_settings_app(Settings::default(), None);
            app.update();
            let world = app.world_mut();
            match defect {
                "missing viewport" => {
                    let viewport = world
                        .query_filtered::<Entity, With<RowViewport>>()
                        .single(world)
                        .unwrap();
                    world.entity_mut(viewport).remove::<RowViewport>();
                }
                "duplicate viewport" => {
                    world.spawn((RowViewport, Node::default(), ScrollPosition::default()));
                }
                "duplicate primary window" => {
                    world.spawn((Window::default(), PrimaryWindow));
                    world.spawn((Window::default(), PrimaryWindow));
                }
                "stepper slider fill" => {
                    world.spawn((
                        SliderFill(Row::Msaa),
                        Node::default(),
                        BackgroundColor::default(),
                    ));
                }
                _ => unreachable!("the fixture names every construction defect"),
            }
            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()))
                .expect_err("invalid page construction must panic");
            let message = panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap();
            assert!(message.contains(expected), "{defect}: {message}");
        }
    }

    /// Players must see every corrupt-file reset and the recovery location through the shipped
    /// settings plugin; a successful save and reopening must not erase that boot warning.
    #[test]
    fn corrupt_loads_show_the_reset_cause_and_kept_file_on_the_page() {
        use super::super::{StoreReport, store};

        let path = std::env::temp_dir().join(format!(
            "overmatch-settings-page-{}-{}.ron",
            std::process::id(),
            char::from_u32(0x1f4be).unwrap()
        ));
        for (text, preserve) in [("not ron {{{", true), ("(lod_pixel_budget: NaN)", false)] {
            let backup = path.with_extension("ron.bad");
            if !preserve {
                std::fs::create_dir(&backup).unwrap();
            }
            std::fs::write(&path, text).unwrap();
            let store::Load { settings, note } = store::load(Some(&path));
            assert!(
                matches!(&note, store::LoadNote::Corrupt { kept, .. } if kept.is_some() == preserve)
            );
            let mut app = headless_settings_app(settings, Some(path.clone()));
            app.insert_resource(StoreReport(note));
            app.update();
            let notice = |app: &mut App| {
                let world = app.world_mut();
                world
                    .query_filtered::<&Text, With<SaveNotice>>()
                    .single(world)
                    .unwrap()
                    .0
                    .clone()
            };
            let reset = notice(&mut app);
            assert!(reset.contains("Settings reset"), "{reset}");
            assert!(
                reset.contains("invalid"),
                "the cause must be visible: {reset}"
            );
            assert!(
                reset
                    .chars()
                    .all(|character| (' '..='~').contains(&character)),
                "{reset}"
            );
            assert!(reset.contains("\\u{1f4be}") || !preserve, "{reset}");
            if preserve {
                assert!(reset.contains("ron.bad"), "{reset}");
            } else {
                assert!(reset.contains("could NOT be preserved"), "{reset}");
            }
            app.world_mut().write_message(SaveSettings);
            app.update();
            app.insert_resource(State::new(crate::state::AppState::Playing));
            app.update();
            app.insert_resource(State::new(crate::state::AppState::Paused));
            app.update();
            assert_eq!(notice(&mut app), reset, "saving cannot undo the boot reset");
            if preserve {
                std::fs::remove_file(backup).unwrap();
            } else {
                std::fs::remove_dir(backup).unwrap();
            }
            std::fs::remove_file(&path).unwrap();
        }
    }

    /// The shipped page must replace its success legend with the retained failure, including after
    /// reopening, and restore the legend when a successful retry clears that failure.
    #[test]
    fn save_failures_replace_the_pages_automatic_save_promise_until_recovery() {
        let mut app = headless_card_app(1.0);
        app.insert_resource(SaveStatus(Some(super::super::store::SaveError::NoLocation)));
        app.update();
        let notice = |app: &mut App| {
            let world = app.world_mut();
            let mut notices = world.query_filtered::<&Text, With<SaveNotice>>();
            notices
                .single(world)
                .expect("the page owns one save notice")
                .0
                .clone()
        };
        assert!(notice(&mut app).contains("not saved"));
        app.insert_resource(SettingsPageVisible(true));
        app.update();
        assert!(
            notice(&mut app).contains("not saved"),
            "reopening must preserve the failure"
        );
        app.world_mut().resource_mut::<SaveStatus>().0 = None;
        app.update();
        assert_eq!(notice(&mut app), SAVE_LEGEND);
    }

    /// A synthetic pointer on a laid-out arrow must step its setting through the shipped page
    /// plugin: backend UiStack hit test, arrow priority (including padding), observer and change_row.
    /// A lost observer/backend/visibility path otherwise fails silently, as dead Retina mouse
    /// support did. PointerInput is the mouse plugin's own input; no winit or real cursor.
    #[test]
    fn a_picked_press_on_an_arrow_steps_its_row() {
        let mut app = headless_card_app(1.0);
        // Mid-ladder, so a step in either direction has somewhere to go.
        app.insert_resource(Settings {
            msaa: MsaaLevel::X2,
            ..default()
        });
        app.insert_resource(SettingsPageVisible(true));
        settle_with_font(&mut app);

        let world = app.world_mut();
        let mut arrows = world.query::<(Entity, &ArrowGlyph)>();
        let arrow = arrows
            .iter(world)
            .find(|(_, arrow)| arrow.row == Row::Msaa && arrow.delta == 1)
            .map(|(entity, _)| entity)
            .expect("the MSAA row spawns a `>` arrow");
        let location = pointer_on(&mut app, arrow);
        click(&mut app, location);

        assert_eq!(
            app.world().resource::<Settings>().msaa,
            MsaaLevel::X4,
            "a press on the MSAA row's `>` must step it — the mouse is not reaching the page",
        );
        assert_eq!(
            app.world().resource::<Page>().row(),
            Some(Row::Msaa),
            "and the hovered row must be the selected one, so the footer hint agrees with the hand",
        );
    }

    /// Where `entity` laid out, as the location a pointer at its centre reports. Physical px, and
    /// the headless window is scale factor 1, so this doubles as the LOGICAL position.
    fn pointer_on(app: &mut App, entity: Entity) -> bevy::picking::pointer::Location {
        let world = app.world_mut();
        let at = world
            .get::<UiGlobalTransform>(entity)
            .expect("a laid-out UI node")
            .translation;
        let mut windows = world.query_filtered::<Entity, With<PrimaryWindow>>();
        let window = windows.single(world).expect("the harness made one window");
        bevy::picking::pointer::Location {
            target: bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
                .normalize(Some(window))
                .expect("the primary window is a render target"),
            position: at,
        }
    }

    /// A primary-button click at `location`: a move frame first, because a press is only routed
    /// to what the previous frame's hover map says is under the pointer.
    fn click(app: &mut App, location: bevy::picking::pointer::Location) {
        use bevy::picking::pointer::{PointerAction, PointerId, PointerInput};
        app.world_mut().write_message(PointerInput::new(
            PointerId::Mouse,
            location.clone(),
            PointerAction::Move { delta: Vec2::ZERO },
        ));
        app.update();
        app.world_mut().write_message(PointerInput::new(
            PointerId::Mouse,
            location,
            PointerAction::Press(PointerButton::Primary),
        ));
        app.update();
    }

    /// **A click on a tab's name opens it, and only the open tab's rows can be seen or picked.**
    ///
    /// Through the real picking chain, like the arrow test above, because a dead tab strip fails
    /// the same silent way a dead arrow does. The closed tab's rows must be out of picking too
    /// (`InheritedVisibility` false), or a click on empty space could step a row the player cannot
    /// see.
    #[test]
    fn a_picked_press_on_a_tab_opens_it_and_hides_the_others() {
        let mut app = headless_card_app(1.0);
        app.insert_resource(SettingsPageVisible(true));
        settle_with_font(&mut app);

        let gameplay = {
            let world = app.world_mut();
            let mut names = world.query::<(Entity, &TabLabel)>();
            names
                .iter(world)
                .find(|(_, label)| label.0 == Tab::Gameplay)
                .map(|(entity, _)| entity)
                .expect("the strip spawns a GAMEPLAY name")
        };
        let location = pointer_on(&mut app, gameplay);
        click(&mut app, location);
        app.update();

        let page = *app.world().resource::<Page>();
        assert_eq!(
            page,
            Page {
                tab: Tab::Gameplay,
                line: Line::Tabs
            },
            "a press on GAMEPLAY must open it — the mouse is not reaching the strip",
        );
        let world = app.world_mut();
        let mut lines = world.query::<(&RowLine, &InheritedVisibility)>();
        for (line, visibility) in lines.iter(world) {
            assert_eq!(
                visibility.get(),
                line.0.tab() == Tab::Gameplay,
                "{:?} on {:?}: only the open tab's rows may be visible to the eye and to picking",
                line.0,
                line.0.tab(),
            );
        }
    }

    /// Card height in PHYSICAL px, measured on the wrapper's child (the 100% wrapper would just
    /// report window height). Do not divide out inverse_scale_factor: it includes UiScale and
    /// would cancel the rescale under test (source on [`mouse_input`]). The headless target scale
    /// is 1, so this is also the logical window height occupied.
    fn card_height(app: &mut App) -> f32 {
        let world = app.world_mut();
        let mut wrappers = world.query_filtered::<&Children, With<SettingsCard>>();
        let card = *wrappers
            .single(world)
            .expect("the page spawns exactly one wrapper")
            .first()
            .expect("the wrapper holds the card");
        world
            .get::<ComputedNode>(card)
            .expect("the card is a laid-out node")
            .size()
            .y
    }

    /// The row viewport's laid-out geometry: `(height, content height, scroll position)`, the first
    /// two in PHYSICAL px and the third in the CSS px [`ScrollPosition`] is stored in.
    fn viewport_state(app: &mut App) -> (f32, f32, f32) {
        let world = app.world_mut();
        let mut viewports =
            world.query_filtered::<(&ComputedNode, &ScrollPosition), With<RowViewport>>();
        let (node, scroll) = viewports
            .single(world)
            .expect("the card carries exactly one row viewport");
        (node.size().y, node.content_size().y, scroll.0.y)
    }

    /// Whether the selected row is fully inside the viewport's rect — the contract
    /// [`scroll_rows`] exists to keep. Both rects are physical, straight off the layout.
    fn selected_row_is_visible(app: &mut App) -> bool {
        let selected = app
            .world()
            .resource::<Page>()
            .row()
            .expect("a row is selected");
        let world = app.world_mut();
        let mut viewports =
            world.query_filtered::<(&ComputedNode, &UiGlobalTransform), With<RowViewport>>();
        let (view_node, view_transform) = viewports.single(world).expect("one viewport");
        let (view_center, view_half) = (view_transform.translation.y, view_node.size().y / 2.0);
        let mut rows = world.query::<(&RowLine, &ComputedNode, &UiGlobalTransform)>();
        let (row_center, row_half) = rows
            .iter(world)
            .find(|(line, _, _)| line.0 == selected)
            .map(|(_, node, transform)| (transform.translation.y, node.size().y / 2.0))
            .expect("the selected row is spawned");
        // A whole pixel of slack for the `floor()` bevy applies to the rendered scroll offset.
        row_center - row_half >= view_center - view_half - 1.0
            && row_center + row_half <= view_center + view_half + 1.0
    }

    /// Both above/below correction branches must keep the keyboard-selected row visible. Fixture
    /// window height 400 px leaves a 360 px card and few visible rows; at 1080 px most fit and the
    /// walk proves little.
    #[test]
    fn walking_the_selection_keeps_every_row_inside_the_viewport() {
        let mut app = headless_card_app_in(1.0, 400);
        app.insert_resource(SettingsPageVisible(true));
        settle_with_font(&mut app);

        let (view, content, _) = viewport_state(&mut app);
        assert!(
            content > view + 1.0,
            "the premise: in a 400 px window the {} VIDEO rows must overflow the {view:.1} px \
             viewport (content {content:.1} px), or this test proves nothing",
            Tab::Video.rows().count(),
        );

        for tab in Tab::ORDER {
            let rows: Vec<Row> = tab.rows().collect();
            for &row in rows.iter().chain(rows.iter().rev()) {
                *app.world_mut().resource_mut::<Page>() = Page {
                    tab,
                    line: Line::Row(row),
                };
                // `scroll_rows` corrects against the PREVIOUS frame's layout, so the fix lands on
                // the next pass; a couple of pumps is convergence, not patience.
                for _ in 0..4 {
                    app.update();
                    if selected_row_is_visible(&mut app) {
                        break;
                    }
                }
                assert!(
                    selected_row_is_visible(&mut app),
                    "{row:?} on {tab:?} never scrolled into the viewport",
                );
            }
        }
    }

    /// Wheel input must scroll, clamp at both ends and stay inert while closed. Otherwise Bevy's
    /// render-only clamp banks overscroll in the component (see [`scroll_rows`]).
    #[test]
    fn the_wheel_scrolls_the_rows_and_stops_at_both_ends() {
        use bevy::input::mouse::{MouseScrollUnit, MouseWheel};

        let mut app = headless_card_app_in(1.0, 400);
        app.insert_resource(SettingsPageVisible(true));
        settle_with_font(&mut app);
        // Park the selection on the first row so its own follow-correction pins the top, and the
        // wheel is the only thing moving the list.
        *app.world_mut().resource_mut::<Page>() = Page::default();
        app.update();
        let window = {
            let world = app.world_mut();
            let mut windows = world.query_filtered::<Entity, With<PrimaryWindow>>();
            windows.single(world).expect("the harness made one window")
        };
        let spin = |app: &mut App, y: f32, times: usize| {
            for _ in 0..times {
                app.world_mut().write_message(MouseWheel {
                    unit: MouseScrollUnit::Line,
                    x: 0.0,
                    y,
                    window,
                    phase: bevy::input::touch::TouchPhase::Moved,
                });
                app.update();
            }
        };

        let (_, _, top) = viewport_state(&mut app);
        assert_eq!(top, 0.0, "the list starts at the top");

        // Wheel DOWN (negative y) moves the content up, i.e. a larger offset. The selection is on
        // row 0, so it is pushed off the top and dragged back — the scroll must still have moved.
        spin(&mut app, -1.0, 1);
        let (_, _, scrolled) = viewport_state(&mut app);
        assert!(
            scrolled > 0.0,
            "one wheel notch must move the list, got {scrolled} px",
        );

        // Far past the end: the clamp is the content overflow, not infinity. The overflow is read
        // in the SAME breath as the offset it bounds, because that is what the clamp is a statement
        // about — the card the player is looking at now, not the one measured twenty frames ago.
        app.world_mut().resource_mut::<Page>().line =
            Line::Row(Tab::Video.rows().last().expect("VIDEO has rows"));
        spin(&mut app, -1.0, 50);
        let (view, content, bottom) = viewport_state(&mut app);
        let overflow = content - view;
        assert!(
            bottom <= overflow + 1.0,
            "the offset ran past the {overflow:.1} px of content there is to scroll ({bottom:.1}) \
             — bevy clamps only what it renders, so an unclamped component silently banks the \
             surplus",
        );
        assert!(bottom > 0.0, "the bottom of the list is not the top");

        // And back, in one wheel travel rather than in however many notches were banked above.
        *app.world_mut().resource_mut::<Page>() = Page::default();
        spin(&mut app, 1.0, 50);
        let (_, _, back) = viewport_state(&mut app);
        assert_eq!(back, 0.0, "wheel-up must reach the top again");

        // Shut the page and spin: nothing may be banked for the next open.
        app.insert_resource(SettingsPageVisible(false));
        spin(&mut app, -1.0, 10);
        let (_, _, shut) = viewport_state(&mut app);
        assert_eq!(
            shut, 0.0,
            "a wheel spun over the game with the page closed must not scroll the page",
        );
    }

    /// Pixel wheel input must move equal PHYSICAL content distance at every target/UI scale; Line
    /// notches must remain CSS-sized and scale with rows. Test pure conversion because selection
    /// follow runs after wheel input and would confound a laid-out measurement. See [`wheel_delta_css`]
    /// for the winit/Bevy unit contract and double-scaling counterexample.
    #[test]
    fn a_pixel_wheel_delta_is_converted_out_of_physical_space() {
        use bevy::input::mouse::MouseScrollUnit;

        const FLICK_PHYSICAL_PX: f32 = 120.0;
        // `to_css` is the viewport's `inverse_scale_factor`: 1 / (target scale factor * UiScale).
        // The three UI rungs on a 1x target, then the same three on a 2x Retina target.
        for (ui_scale, target_scale) in [
            (0.75, 1.0),
            (1.0, 1.0),
            (1.5, 1.0),
            (0.75, 2.0),
            (1.0, 2.0),
            (1.5, 2.0),
        ] {
            let combined = ui_scale * target_scale;
            let to_css = 1.0 / combined;
            let css = wheel_delta_css(MouseScrollUnit::Pixel, FLICK_PHYSICAL_PX, to_css);
            // What the layout will actually move: the CSS delta re-multiplied by the very factor
            // `bevy_ui` applies to `ScrollPosition`.
            let physical_travel = css * combined;
            assert!(
                (physical_travel - FLICK_PHYSICAL_PX).abs() <= 1e-3,
                "ui {ui_scale}x on a {target_scale}x target: a {FLICK_PHYSICAL_PX} px flick moved \
                 {physical_travel} physical px of content. A trackpad delta arrives in PHYSICAL \
                 pixels, so it must be converted to CSS before it reaches ScrollPosition — \
                 otherwise the UI scale is applied to it twice.",
            );
            // CONTROL: the stored CSS value therefore has to DIFFER by rung. If it did not, nothing
            // would be being converted and the assertion above would pass on a coincidence.
            assert!(
                (css - FLICK_PHYSICAL_PX * to_css).abs() <= 1e-3,
                "ui {ui_scale}x on a {target_scale}x target: stored {css} css px",
            );

            // The LINE unit is the opposite case and must NOT be converted.
            assert_eq!(
                wheel_delta_css(MouseScrollUnit::Line, 1.0, to_css),
                page_look().wheel_line_px.0,
                "ui {ui_scale}x on a {target_scale}x target: a wheel NOTCH is a dimensionless \
                 count and must keep its authored CSS distance, scaling with the UI like the rows it \
                 steps past",
            );
        }
        // Both units agree that positive is up (the smaller offset `scroll_rows` subtracts toward),
        // symmetrically, with a still hand moving nothing.
        for unit in [MouseScrollUnit::Line, MouseScrollUnit::Pixel] {
            assert!(wheel_delta_css(unit, 1.0, 0.5) > 0.0, "{unit:?}");
            assert_eq!(
                wheel_delta_css(unit, -1.0, 0.5),
                -wheel_delta_css(unit, 1.0, 0.5),
                "{unit:?}: the two directions must be symmetric",
            );
            assert_eq!(wheel_delta_css(unit, 0.0, 0.5), 0.0, "{unit:?}");
        }
    }
}
