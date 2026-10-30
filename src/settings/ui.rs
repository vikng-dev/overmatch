//! The player-facing settings page: a titled card with a tab strip and one tab's rows —
//! `LABEL  < VALUE >` steppers for the enum ladders, `LABEL  [====----] VALUE` sliders for the
//! scalar ones. Two tabs ([`Tab`]): VIDEO (display and graphics) and GAMEPLAY. A tab exists only
//! once it has rows.
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
//! * `Up`/`Down` — move the selection through the open tab's rows, SKIPPING disabled rows (the
//!   frame cap while vsync is not OFF). `Up` from the tab's first row selects the tab strip itself.
//!   `W`/`S` are deliberately NOT bound: they drive the tank, and a settings page that also reads
//!   the drive keys teaches the wrong reflex.
//! * `Left`/`Right` — change the selected row's value: one ladder rung on a stepper, one stop on a
//!   slider. On the tab strip they switch tabs, so the strip is walked exactly like a stepper row
//!   and the page needs no key beyond the arrows (`Tab` swaps the possessed tank in single-player).
//! * Mouse — hover selects; the `<` `>` glyphs are REAL affordances (visible, hover-highlighted,
//!   clickable — the invisible click-the-half-of-the-row scheme they replace was pure mystery
//!   meat); a slider's track is click-to-set and drag-to-scrub; a tab's name is click-to-open.
//!
//! **Hit testing is `bevy_picking`'s, not ours.** `UiPickingPlugin` ships inside `UiPlugin`, so the
//! backend already walks `UiStack` against every node on this card whether we ask or not; the page
//! used to re-walk the same rects by hand. It reads the answers instead — [`Hovered`] for the
//! highlight and the hover-selection, `Pointer<Press>` observers for the clicks — which deletes the
//! hand-rolled cursor conversion, the hand-ordered "the arrow inside the row wins" (bubbling from
//! the deepest node IS that rule) and the "not while hidden" gate (`InheritedVisibility`).
//!
//! Every change saves immediately — there is no Apply button to forget, and an atomic write makes
//! that safe (`settings::save`). The one refinement: a slider DRAG saves once on release, not once
//! per dragged frame. A drag also FREEZES its track's rect at the grab and maps the PHYSICAL cursor
//! through that for its whole life ([`ActiveDrag`]) — the UI SCALE row re-lays out the page it is
//! being dragged on, so a live rect would be an output of the value it computes. That is the one
//! place this file still does pixel arithmetic, and the one place the physical-vs-logical bug class
//! still bites; [`track_fraction`] records it.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::ui::{ComputedNode, Overflow, ScrollPosition, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use super::displays::DisplayLadder;
use super::preset::Preset;
use super::{
    BloomLevel, DebugPanel, FrameCap, KillCam, MsaaLevel, PenCamCorner, PixelBudget, PresentCaps,
    ReduceMotion, RenderScaleLevel, SaveSettings, Settings, ShadowCascades, ShadowDistance,
    ShadowResolution, TrackPrintRange, TrackPrintRelief, UiScalePercent, VsyncMode,
    WindowModeSetting,
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
/// closes.
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

/// Which row of the page a line is. The order here IS the on-screen order within each tab.
///
/// **The extension point**: add a variant, add it to [`Row::ORDER`], give it arms in
/// [`Row::tab`] / [`Row::label`] / [`Row::hint`] / [`Row::value`] / [`Row::step`] (and, for a
/// scalar, [`Row::kind`] + the two slider-fraction arms). Nothing else in this file, and nothing
/// outside it, needs to change — the card lays out from `ORDER`, and `Settings` already accepts new
/// fields without a migration (see the `settings` module doc).
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    /// LOW to ULTRA: writes every row directly below it at once, and reads CUSTOM whenever their
    /// values match no tier — see `settings::preset`. Heads VIDEO, its rows beneath it.
    Preset,
    RenderScale,
    /// The LOD screen-space error budget, in pixels. Beside render scale because they are the two
    /// rows that trade picture for frames by drawing LESS, rather than by drawing it differently.
    /// No consumer yet — see `settings::PixelBudget`.
    LodPixelBudget,
    /// Windowed / borderless fullscreen. Reflects OS-side toggles too — see
    /// `settings::observe_window_placement`.
    WindowMode,
    /// Which monitor the window is centred on. Gated like vsync, but by the machine's own display
    /// list: the rungs are `settings::displays::DisplayLadder`, one per attached monitor and named
    /// by the OS. See `settings::DisplaySelection` for why a rung naming an unplugged display falls
    /// back rather than being written back.
    Display,
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
    /// How the track prints' relief is drawn — see `settings::TrackPrintRelief`.
    TrackPrints,
    /// How far that relief reaches — see `settings::TrackPrintRange`.
    TrackPrintRange,
    Msaa,
    /// How finely the glow around tracers and bright light is drawn — see `settings::BloomLevel`.
    Bloom,
    /// Capability-gated: only the rungs the surface's probe reported are offered ([`PresentCaps`]).
    VSync,
    /// The one row that can be DISABLED: it renders greyed and is skipped by every input path
    /// unless vsync is OFF (see [`Row::enabled`]).
    FrameCap,
    UiScale,
    /// The online debug card in a match's corner — see `settings::DebugPanel`.
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
        // The rows the preset drives, directly beneath it — the PRESET hint says so, and
        // `the_rows_a_preset_drives_sit_directly_below_it` pins it.
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
            // Names what it leaves alone, because a preset that moved the window or the frame cap
            // would be the surprise. "nine" is `preset::DRIVEN_ROWS`, all directly below this row
            // (`the_rows_a_preset_drives_sit_directly_below_it`).
            Row::Preset => {
                "Sets the nine rows below it at once; editing one reads CUSTOM. \
                 Window, display, vsync, frame cap and UI scale are never touched."
            }
            // Says what it does NOT touch, because that is the surprising half: the crosshair, the
            // range dial and every label stay pin-sharp while the world softens. The MSAA advice is
            // the one honest limit a player can act on (the multisample resolve is full-res
            // whatever this row says, so 4X below 100% is paying twice for the same edges).
            Row::RenderScale => {
                "How much of the window the 3D world is drawn at, before upscaling. \
                 The HUD stays sharp. Below 100%, prefer 2X or OFF anti-aliasing."
            }
            // Honest about the unit and about which direction is which: the number is an ERROR
            // budget, so bigger is worse-looking and cheaper — the opposite of every other row on
            // this page, and exactly the thing a label alone cannot say.
            Row::LodPixelBudget => {
                "Largest on-screen error, in pixels, a simplified model may show. \
                 Lower is crisper and costs more."
            }
            Row::WindowMode => {
                "Borderless fullscreen on the current display. \
                 The OS's own fullscreen button is reflected here too."
            }
            // States the fallback, because unplugging a monitor is the common case and a row that
            // silently did something else would read as broken.
            Row::Display => {
                "Which monitor the window opens on, * marking your main display. \
                 One that is not attached falls back to it."
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
            // NOT independent of the distance row, despite reading like it: shadow bias is measured
            // in texels, so a longer envelope's coarser texels detach a distant shadow from its
            // caster by metres unless resolution rises with it. Measured 2026-07-28 — see
            // `ShadowDistance`'s detachment table.
            Row::ShadowResolution => {
                "How crisp shadows are, and how tightly distant ones stay attached. \
                 Higher costs more memory."
            }
            // Honest about the trade: each cascade is a shadow render pass, and fewer of them
            // means the near ones cover more ground per texel.
            Row::ShadowCascades => {
                "How many slices the shadow distance is split into. \
                 Fewer is cheaper but blockier up close."
            }
            // Says what every level keeps, because that is what a player lowering it worries
            // about: the trail is still there, only its depth is drawn more cheaply.
            Row::TrackPrints => {
                "How track prints show their depth: lit only, with parallax, or with parallax \
                 and the sun's shadow in the rut. Every level keeps the trail."
            }
            Row::TrackPrintRange => {
                "How far from you track prints keep their parallax depth. Beyond it they are lit \
                 only. Farther costs more."
            }
            Row::Msaa => "Smooths jagged edges. The cheapest setting to drop for more frames.",
            // Says what LOW keeps, because a player lowering it worries about losing the tracers:
            // every halo stays, drawn coarser.
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

    /// Stepper or slider — the scalar rows are sliders (the owner's explicit ask: a scalar deserves
    /// a scrubbable control, not a disguised ladder).
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
    const fn value_well_px(self) -> f32 {
        match (self, self.kind()) {
            (Row::Display, _) => DISPLAY_VALUE_WELL_PX,
            (_, RowKind::Stepper) => STEPPER_VALUE_WELL_PX,
            (_, RowKind::Slider) => SLIDER_VALUE_WELL_PX,
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

    /// The row's current value, as rendered. ASCII plus the verified typographic set — it reaches
    /// `Text`. A `String` since the scalar rows format live numbers.
    ///
    /// `ladder` is to the display row what `caps` is to vsync: the machine's own answer, so the row
    /// renders the DISPLAY a rung names rather than the rung's position in a list.
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
            Row::TrackPrintRange => settings.track_print_range.label().to_string(),
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

    /// Step this row by `delta` (`+1` / `-1`), saturating at the ends rather than wrapping.
    ///
    /// Saturating is the deliberate choice for a page a player reads: a wrapping list turns "press
    /// right until it stops" into an accidental jump from the highest setting to the lowest — and
    /// with `OFF` the floor of the shadow-distance ladder, a wrap would put "no shadows at all"
    /// one keypress past the most expensive setting.
    ///
    /// `caps` gates the vsync ladder and `ladder` the display one: both walks happen INSIDE the
    /// offered rungs, so a Metal surface steps ON <-> OFF without ever visiting the unoffered FAST,
    /// and a two-display machine never steps onto a third display.
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

/// The selected line: the tab strip, or one row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Line {
    Tabs,
    Row(Row),
}

/// Where the page is: the open tab and the selected line. One resource, because the invariant
/// between the two — **a selected row is always on the open tab** — is kept by these methods
/// rather than by every caller: the tab changes only through [`Page::switch`] / [`Page::open`],
/// which move the selection to the strip, and a row is selected only through [`Page::step`] /
/// [`Page::select`], which refuse a row of another tab.
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

    /// Move the selection one line in `delta`'s direction through the open tab's rows, SKIPPING
    /// rows `enabled` refuses, saturating at the bottom — "hold down" stops at the last enabled row
    /// rather than jumping back to the top, and a disabled row is never landed on. Running off the
    /// top lands on the tab strip, which is the line above every tab's first row.
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

/// The fixed-height box the hint is drawn into: always exactly two lines tall, whether the selected
/// row's hint fills one line or two, so walking the selection never moves the rows above it. See
/// [`HINT_SLOT_HEIGHT_PX`].
#[derive(Component)]
struct HintSlot;

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
/// The display row's own well — its values are OS display NAMES, not two-character rungs. MEASURED
/// against the shipped Barlow Condensed SemiBold at the row's 18 px: `* BUILT-IN RETINA DISPLAY`
/// draws 143 px, and 26 solid `M` (the `LABEL_MAX_CHARS` ceiling `settings::displays` truncates to)
/// draw 210 px. Sized for names, so that worst case overhangs the arrows rather than reflowing the
/// row. DERIVED: the widest row label plus this well plus both arrows is ~370 px inside the card's
/// 512 px interior, so no stepper row has to give up width for it.
const DISPLAY_VALUE_WELL_PX: f32 = 190.0;
const SLIDER_VALUE_WELL_PX: f32 = 64.0;
/// The space between two tab names, and the height of the bar under the open one.
const TAB_GAP_PX: f32 = 24.0;
const TAB_MARKER_HEIGHT_PX: f32 = 2.0;

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
///   shift the layout, and that is the honest failure) rather than silently clipped. No test
///   measures the copy against the slot.
///
/// **The `min_height` goes on a WRAPPER node ([`HintSlot`]), never on the text itself.** Bevy 0.19's
/// text measure resolves a node's effective height as `known.or(preferred.or(min))` — with no
/// `height` set, a `min_height` IS the answer, and the text's own measured height is never consulted
/// (`bevy_ui::measurement::resolve_axis`). A `min_height` on the `Text` node is therefore a FIXED
/// height in disguise: three lines of copy would be clipped to two, silently, which is the one
/// outcome the floor above is written to avoid. MEASURED: an overlong string laid out in such a
/// node never grew.
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

/// The gap between the card's own children AND between the rows inside each [`TabPanel`]. One
/// constant, because two `row_gap`s that happened to agree would be two numbers free to drift into
/// a visible seam at the top and bottom of the scrolling list.
const CARD_ROW_GAP_PX: f32 = 6.0;

/// How much of the window height the card may occupy before its row list starts scrolling.
///
/// **The page outgrew the screen and this is the measured fix.** MEASURED 2026-08-01, headless, at
/// the shipped 11 rows, in logical px: 506 at the 75% UI-scale rung, 666 at 100%, **1000 at 150%**
/// — against a 720 px window, the shortest a Windows or Linux player can be running. The top rung was already over that
/// floor at 9 rows (~875 px), so this is not a debt the two newest rows created, only one they made
/// impossible to keep ignoring: past the floor the card simply ran off the bottom of the screen,
/// taking the footer hint and the control legend — the two things that explain the page — with it.
///
/// 90% rather than 100% so the scrim reads as a scrim: a card flush against both screen edges looks
/// like a broken full-screen layout rather than a modal. The 10% also leaves the clipped row at the
/// bottom visibly clipped, which is the only affordance a scroll view gets here — there is no
/// scrollbar, deliberately (see [`scroll_rows`]: the keyboard walk carries the selection into view,
/// so the bar would be decoration nobody needs to aim at).
const CARD_MAX_HEIGHT_PERCENT: f32 = 90.0;

/// How far one LINE-unit wheel notch scrolls the row list, in CSS px — roughly one row, so a notch
/// moves the list by a readable unit instead of a hair or a page. Trackpads report
/// `MouseScrollUnit::Pixel` and are used as-is.
const WHEEL_LINE_PX: f32 = 40.0;

/// The shared page: spawn, input, refresh. Mounted by `settings::plugin`, which also adds exactly
/// one of the ENTRY declarers ([`declare_from_overlay_menu`] / [`declare_from_pause_state`]) — they
/// are the one thing the two windowed roots do differently, which is why they are that plugin's
/// single parameter rather than a system each root must remember.
pub(super) fn plugin(app: &mut App) {
    // The display row's offered rungs. Mounted here rather than beside the other settings plugins
    // because this page is the only thing that reads them.
    app.add_plugins(super::displays::plugin)
        .init_resource::<SettingsPageVisible>()
        .init_resource::<Page>()
        .init_resource::<SliderDrag>()
        .add_systems(Startup, spawn_page)
        // The click path is a `bevy_picking` observer, which runs in `PreUpdate` — ahead of
        // everything below, so a click and the repaint that shows it land in the same frame.
        .add_observer(page_pressed)
        .add_systems(
            Update,
            (
                // First, so a capture's pin overrides whatever the root's declarer just wrote.
                pin_for_capture.run_if(resource_exists::<CapturePage>),
                // Input first, then the refresh reads the same frame's result — so a keypress
                // repaints immediately instead of one frame late.
                (keyboard_input, mouse_input),
                // After the input, so the selection this follows is the one the arrow key just
                // moved rather than last frame's.
                scroll_rows,
                (refresh_page, refresh_tabs),
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

/// Holds the page open on one tab, its first row selected, whatever the pause surface declares —
/// the capture lever `dev_capture` inserts for `OVERMATCH_CAPTURE_SETTINGS`. Nothing else inserts
/// it, so a shipped run never sees it.
#[derive(Resource, Clone, Copy, Debug)]
pub(crate) struct CapturePage(Tab);

impl CapturePage {
    /// The tab whose label is `name`, any case.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Tab::ORDER
            .into_iter()
            .find(|tab| tab.label().eq_ignore_ascii_case(name.trim()))
            .map(Self)
    }
}

/// [`CapturePage`]'s system. Writes the tab once, so the page is otherwise left as the player's
/// input would leave it.
fn pin_for_capture(
    pin: Res<CapturePage>,
    mut visible: ResMut<SettingsPageVisible>,
    mut page: ResMut<Page>,
) {
    visible.set_if_neq(SettingsPageVisible(true));
    if page.tab != pin.0 {
        page.open(pin.0);
        page.step(1, |_| true);
    }
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
                        width: Val::Px(CARD_WIDTH_PX),
                        // The card may never be taller than the window it is centred in — see
                        // [`CARD_MAX_HEIGHT_PERCENT`]. This is what turns "the page is too tall"
                        // from a card with its footer off the bottom of the screen into a card
                        // whose row list scrolls.
                        max_height: Val::Percent(CARD_MAX_HEIGHT_PERCENT),
                        padding: UiRect::all(Val::Px(CARD_PADDING_PX)),
                        border_radius: BorderRadius::all(Val::Px(6.0)),
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(CARD_ROW_GAP_PX),
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
                            // The title, the hint slot and the legend are FIXED furniture: only the
                            // row list may give up height when the card is taller than the window
                            // (see [`RowViewport`]). Without this, flexbox's default `flex_shrink:
                            // 1` would squeeze the title and the footer first — the two things a
                            // player needs most when the page does not fit.
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
                                        // The card's own `row_gap` spaces the title, strip,
                                        // viewport, hint and legend; this one spaces the rows, at
                                        // the same value so the two cannot drift into a seam.
                                        row_gap: Val::Px(CARD_ROW_GAP_PX),
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

                    // The hint's fixed two-line SLOT, with the hint text inside it. See
                    // [`HINT_SLOT_HEIGHT_PX`] for why the reservation is a node of its own rather
                    // than a `min_height` on the text.
                    card.spawn((
                        HintSlot,
                        Node {
                            width: Val::Percent(100.0),
                            margin: UiRect::top(Val::Px(14.0)),
                            min_height: Val::Px(HINT_SLOT_HEIGHT_PX),
                            flex_shrink: 0.0,
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
            column_gap: Val::Px(TAB_GAP_PX),
            padding: UiRect::axes(Val::Px(10.0), Val::Px(7.0)),
            border_radius: BorderRadius::all(Val::Px(4.0)),
            // Fixed furniture, like the title: see the title's `flex_shrink`.
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(ROW_BG),
    ))
    .with_children(|strip| {
        for tab in Tab::ORDER {
            strip
                .spawn((
                    TabLabel(tab),
                    Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: Val::Px(3.0),
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
                            font_size: FontSize::Px(18.0),
                            ..default()
                        },
                        TextColor(if tab == open { TEXT } else { TEXT_DIM }),
                    ));
                    label.spawn((
                        TabMarker(tab),
                        Node {
                            width: Val::Percent(100.0),
                            height: Val::Px(TAB_MARKER_HEIGHT_PX),
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
const fn marker_color(open: bool) -> Color {
    if open { SLIDER_FILL_COLOR } else { Color::NONE }
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

/// Every click the page takes: an arrow steps its row, a slider's track sets the value there and
/// GRABS itself for [`mouse_input`] to scrub until the button comes up, and a tab's name opens that
/// tab.
///
/// One observer rather than a hand-ordered pair of loops is what makes "the arrow inside the row
/// wins over the row" a fact of the hierarchy: picking targets the DEEPEST node under the cursor and
/// the event bubbles outwards from there, so the arrow — or the fill inside a track, which is why
/// `press.entity` has become the TRACK by the time the second arm matches — is reached first and
/// stops the walk. It cannot fire on a closed page either: the backend skips anything whose
/// `InheritedVisibility` is false, which is exactly what [`refresh_page`] writes onto the card.
///
/// The grab is the one moment the live track rect is read, because it is the rect the player just
/// aimed at. Everything after maps through the frozen copy — see [`ActiveDrag`], and
/// [`track_fraction`] for why the cursor has to be the PHYSICAL one to match it.
#[expect(
    clippy::too_many_arguments,
    reason = "an observer spanning arrows, tracks, tabs, window, capabilities, ladder, page, drag, settings and save"
)]
fn page_pressed(
    mut press: On<Pointer<Press>>,
    arrows: Query<&ArrowGlyph>,
    tracks: Query<(&SliderTrack, &ComputedNode, &UiGlobalTransform)>,
    tabs: Query<&TabLabel>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    caps: Res<PresentCaps>,
    ladder: Res<DisplayLadder>,
    mut page: ResMut<Page>,
    mut drag: ResMut<SliderDrag>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
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

/// Put `active`'s row where `cursor_x` (PHYSICAL px) sits on its frozen track. Guarded, because
/// `Settings` is a `ResMut` and a write is a change-tick bump — i.e. a re-`apply_settings` — even
/// when the value lands where it already was, and a held-still drag is most of a drag's frames.
fn scrub(active: ActiveDrag, cursor_x: f32, settings: &mut ResMut<Settings>) {
    let mut next = **settings;
    active
        .row
        .set_from_fraction(&mut next, active.fraction(cursor_x));
    if next != **settings {
        **settings = next;
    }
}

/// The two things the mouse does per FRAME rather than per event: carry a grabbed slider, and move
/// the selection to the hovered row. The drag lives here rather than in a `Pointer<Drag>` observer
/// because a press-and-release that never moves the mouse produces no drag events at all, and the
/// release is a frame fact anyway (the page can close under a held button).
///
/// [`Hovered`] is picking's own per-entity flag, true for the row line AND anything inside it (the
/// CSS `:hover` rule — which is how hovering an ARROW still selects its row), and mutated only on
/// enter/leave, so `Changed` fires twice per row crossed rather than every frame.
///
/// **The cursor is the PHYSICAL one**, because the rect [`ActiveDrag`] froze is physical — see
/// [`track_fraction`] for the silent 2x that pins. `Window::physical_cursor_position` rather than a
/// hand-rolled `cursor_position() * scale_factor()`: it is the value the logical accessor divides.
/// A node's own `inverse_scale_factor` could NOT stand in — it folds in `UiScale` too
/// (`bevy_ui::update` builds it as `target_scaling_factor * ui_scale`), so it converts to CSS
/// pixels, not to the window-logical pixels a pointer reports.
#[expect(
    clippy::too_many_arguments,
    reason = "a system spanning page visibility, buttons, window, hovered rows and strip, capabilities, ladder, page, drag, settings and save"
)]
fn mouse_input(
    visible: Res<SettingsPageVisible>,
    buttons: Res<ButtonInput<MouseButton>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    rows: Query<(&RowLine, &Hovered), Changed<Hovered>>,
    strip: Query<&Hovered, (With<TabStrip>, Changed<Hovered>)>,
    caps: Res<PresentCaps>,
    ladder: Res<DisplayLadder>,
    mut page: ResMut<Page>,
    mut drag: ResMut<SliderDrag>,
    mut settings: ResMut<Settings>,
    mut save: MessageWriter<SaveSettings>,
) {
    // A drag owns the mouse outright: scrub while held, save ONCE on release (or on the page closing
    // under the drag), and never let the selection wander meanwhile. The mapping deliberately does
    // NOT re-read the track — the geometry frozen at the grab is the whole rect this drag will ever
    // use, for the layout feedback loop [`ActiveDrag`] records.
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
    // Hover alone moves the selection, disabled rows excluded — a disabled row cannot be "described"
    // by the footer hint it is excluded from acting on, so keyboard and mouse never disagree about
    // which row the hint is describing.
    for (line, hovered) in &rows {
        if hovered.get() && line.0.enabled(&settings, *caps) {
            page.select(line.0);
        }
    }
    if strip.iter().any(Hovered::get) {
        page.line = Line::Tabs;
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

/// How far one wheel message asks the list to travel, in the CSS pixels [`ScrollPosition`] is
/// stored in. `to_css` is the viewport's `inverse_scale_factor`. Pure, so the unit question is
/// testable without a laid-out UI — the same reason [`track_fraction`] is.
///
/// **The two units are in DIFFERENT SPACES, and only one of them needs converting.**
///
/// A `Line` delta is a count of notches — dimensionless — so multiplying it by a CSS-px constant
/// already lands in the space wanted, and it SHOULD scale with the UI: a notch is "about one row",
/// and rows get bigger with `UiScale`.
///
/// A `Pixel` delta (every trackpad) is winit's `MouseScrollDelta::PixelDelta`, whose payload is a
/// `PhysicalPosition` (vendored winit-0.30.13/src/event.rs:961-966), and bevy copies its components
/// across unchanged (bevy_winit-0.19.0/src/state.rs:342-349). So it arrives in PHYSICAL pixels and
/// must be converted like every other physical measurement here. Without the conversion it is
/// scaled a SECOND time by layout — `UiScale` is folded into the factor `ScrollPosition` is
/// multiplied by (`bevy_ui::update`, `target_scaling_factor().unwrap_or(1.) * ui_scale.0`) — so the
/// same flick of a finger would scroll 0.75x at the 75% rung and 1.5x at 150%, doubled again on a
/// Retina target. A physical distance the hand moved must land the same physical distance of
/// content whatever the UI scale is.
fn wheel_delta_css(unit: bevy::input::mouse::MouseScrollUnit, y: f32, to_css: f32) -> f32 {
    match unit {
        bevy::input::mouse::MouseScrollUnit::Line => y * WHEEL_LINE_PX,
        bevy::input::mouse::MouseScrollUnit::Pixel => y * to_css,
    }
}

/// Keep the row list scrolled somewhere useful: follow the wheel, and always keep the SELECTED row
/// on screen.
///
/// **The selection-follow is the load-bearing half**, not the wheel. This page is walked with the
/// arrow keys, and a selection that can leave the viewport is worse than no scrolling at all — the
/// player presses Down, the highlight vanishes, and the value they are about to change is one they
/// cannot see. So the wheel is a convenience and the follow is the contract, which is also why
/// there is no scrollbar: nothing here needs to be aimed at with a mouse.
///
/// # The geometry is LAST frame's, and that is fine
///
/// `ComputedNode`/`UiGlobalTransform` are written in `PostUpdate`, so what this reads in `Update`
/// is the layout the player is currently looking at — including the scroll already applied to it.
/// The correction below is therefore a DELTA against a rect that already moved, which converges in
/// one frame and self-corrects if it does not (a resize, a UI-scale drag, a row appearing). Solving
/// it exactly would mean re-deriving taffy's layout here, to hide one frame nobody can see.
///
/// # Two pixel spaces, one of which is not the usual one
///
/// [`ScrollPosition`] is in CSS pixels — `bevy_ui`'s layout multiplies it by the node's scale
/// factor on the way to physical (`bevy_ui::layout::update_uinode_geometry_recursive`,
/// `scroll_pos.y * inverse_target_scale_factor.recip()`), and that factor folds `UiScale` in.
/// `ComputedNode` sizes and `UiGlobalTransform` translations are PHYSICAL. So every rect
/// measurement below is converted by the viewport's own `inverse_scale_factor` before it touches
/// the scroll value. This is the same space mismatch [`track_fraction`] documents for the slider,
/// with the opposite conversion; getting it wrong here is a scroll that overshoots by exactly the
/// UI scale.
///
/// The clamp is ours to apply, not bevy's: layout clamps the value it RENDERS with
/// (`clamped_scroll_position`) but writes that back only into `ComputedNode`, leaving the component
/// free to accumulate an unbounded value — after which a wheel-up would spend a hundred notches
/// doing nothing before the list moved.
fn scroll_rows(
    visible: Res<SettingsPageVisible>,
    mut wheel: MessageReader<bevy::input::mouse::MouseWheel>,
    page: Res<Page>,
    viewport: Option<
        Single<(&ComputedNode, &UiGlobalTransform, &mut ScrollPosition), With<RowViewport>>,
    >,
    rows: Query<(&RowLine, &ComputedNode, &UiGlobalTransform)>,
) {
    let Some(viewport) = viewport else {
        return;
    };
    let (node, transform, mut scroll) = viewport.into_inner();
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

    // Then the selection, which wins: it is applied last, so a wheel that scrolled the selected row
    // off screen is immediately undone rather than fighting the next arrow press. The tab strip is
    // above the viewport, so selecting it asks nothing of the scroll.
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

    // Ours to apply — layout clamps only what it renders. `scrollbar_size` is in the sum because
    // bevy's own `max_possible_offset` includes it, and a mismatch would leave the last row
    // unreachable if a scrollbar ever appears.
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
        background.set_if_neq(BackgroundColor(if selected == Some(line.0) {
            ROW_BG_SELECTED
        } else {
            ROW_BG
        }));
    }
    if let Ok(mut hint) = hint.single_mut() {
        let rendered = selected.map_or(TAB_STRIP_HINT, Row::hint);
        if hint.0 != rendered {
            hint.0 = rendered.to_string();
        }
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
            ROW_BG_SELECTED
        } else {
            ROW_BG
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unprobed capability state every pure test steps under: all rungs offered.
    const ALL_RUNGS: PresentCaps = PresentCaps::Unprobed;

    /// The display ladder of a world whose monitors have not been synchronised — every variant a
    /// file can hold, which is what the rows that are not the display row want to step under.
    fn unlearned() -> DisplayLadder {
        DisplayLadder::default()
    }

    /// **A saturated step is a no-op, and stays one whatever else is in the settings.**
    ///
    /// `change_row` and `scrub` both decide "nothing happened" by comparing whole [`Settings`]
    /// values, so any field that breaks reflexive equality turns every held key and every
    /// held-still drag frame into a settings write and a file save. A `NaN` budget did exactly that
    /// (`store::a_non_finite_budget_cannot_survive_the_parse` is the fix at the boundary); this is
    /// the property that fix protects, asserted where it is actually consumed.
    #[test]
    fn a_saturated_step_on_any_row_is_a_no_op() {
        // The budget as a parser could deliver it after sanitisation, plus an ordinary off-ladder
        // hand-edit, so this covers the shape the fix produces rather than only the default.
        for budget in [
            PixelBudget::default(),
            PixelBudget(1.2),
            PixelBudget(PixelBudget::MIN),
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

    /// A stored OFF the surface cannot present (Wayland
    /// refuses `Immediate`) must grey the frame-cap row, because that is precisely what the limiter
    /// does with it — the row used to stay lit and armed over a compositor-paced surface. The gate
    /// is the EFFECTIVE rung, so this holds in the frames before `settings::normalize_vsync` has
    /// written the correction back. Every other row is enabled under every rung on every surface.
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

    /// **Every row is on exactly one tab, and no tab is empty.** The page lays each tab out from
    /// [`Tab::rows`], so a row on no tab would never be drawn, and an empty tab is a tab the owner
    /// ruled out (AUDIO and CONTROLS arrive with their rows). GAMEPLAY is pinned by name: those
    /// four rows are the owner's split, and the rest is VIDEO.
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

    /// **Editing any driven row reads CUSTOM by itself, and editing any other row never moves the
    /// label** — through [`Row::step`], the path every key and click takes. From each tier, every
    /// row is stepped one rung both ways: a row that changed the driven values must read CUSTOM (or
    /// the tier whose table it happens to land on exactly), and a row that did not must still read
    /// the tier. No stored preset exists to go stale; this is the derivation doing it.
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

    /// The capture lever names a tab by its label, in any case, and nothing else.
    #[test]
    fn a_capture_names_a_tab_by_its_label() {
        for tab in Tab::ORDER {
            assert_eq!(
                CapturePage::named(&tab.label().to_lowercase()).map(|pin| pin.0),
                Some(tab),
            );
        }
        assert!(CapturePage::named("audio").is_none());
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
    /// at any rung of the ladder (the top rung asks for `CARD_WIDTH_PX * 1.5` = 840 physical px).
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

    /// Pump the app until BOTH bundled fonts have LOADED and the card has stopped moving. Nothing
    /// may be measured before this: an unshaped (or unfindable) font measures as nothing, so the
    /// reserved hint slot reports exactly the `min_height` the test is looking for and every
    /// assertion below would pass vacuously.
    ///
    /// # Both weights, because the card draws with both
    ///
    /// [`UiFonts`] resolves two handles and this page uses each of them: Regular for the row labels,
    /// the hint and the legend, SemiBold for the 30 px title, every value and both arrow glyphs. They
    /// are two independent async loads and the IO pool may finish them in different FRAMES, so a card
    /// waited on by Regular alone can still be laid out with every SemiBold node measuring zero — a
    /// real card that is simply too short: no title, and rows shrunk to their label's height.
    ///
    /// That is not a hypothetical. It is what failed
    /// [`the_wheel_scrolls_the_rows_and_stops_at_both_ends`] on a loaded CI runner and nowhere else:
    /// the test cached a 242 px overflow off the SemiBold-less card, SemiBold arrived during the 50
    /// wheel notches that followed, and [`scroll_rows`] then correctly clamped to the settled card's
    /// real 306 px. On a fast machine the two loads land in the same frame and the wrong height is
    /// never observed, which is exactly the shape of a race that only ever fails under contention.
    ///
    /// # And then the layout, run until it stops moving
    ///
    /// A font that lands is re-measured and re-laid-out over the following passes, and how many
    /// passes that takes is bevy's business rather than a number worth guessing.
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

    /// **The page's mouse is alive.** Hit testing moved to `bevy_picking`, and the whole failure
    /// mode of a hit test is SILENCE: the page's mouse support was once dead on every Retina display
    /// and said nothing, because "no node contains the cursor" and "no node was asked" look
    /// identical from the outside. This is the tripwire — a synthetic pointer put on the ARROW's own
    /// laid-out rect, pressed, and the setting expected to step.
    ///
    /// It exercises the real chain end to end: the backend walking `UiStack` against `ComputedNode`,
    /// the hit resolving to the arrow (not the row line it sits in, and not the text run inside its
    /// padding), the event reaching [`page_pressed`], and `change_row` moving [`Settings`]. Anything
    /// that unhooks the page from picking — a lost observer, a card that never becomes visible, a
    /// backend that stops running — fails here loudly instead of shipping a dead page.
    ///
    /// The pointer is driven by writing `PointerInput` directly, which is exactly what
    /// `bevy_picking`'s own mouse plugin writes; no winit, no real cursor.
    #[test]
    fn a_picked_press_on_an_arrow_steps_its_row() {
        use bevy::picking::pointer::{Location, PointerAction, PointerId, PointerInput};

        let mut app = headless_card_app(1.0);
        // Mid-ladder, so a step in either direction has somewhere to go.
        app.insert_resource(Settings {
            msaa: MsaaLevel::X2,
            ..default()
        });
        app.insert_resource(SettingsPageVisible(true));
        settle_with_font(&mut app);

        // Where the MSAA row's `>` glyph actually laid out. Physical px, and the headless window is
        // scale factor 1, so this doubles as the LOGICAL position a pointer reports.
        let world = app.world_mut();
        let mut arrows = world.query::<(&ArrowGlyph, &UiGlobalTransform)>();
        let at = arrows
            .iter(world)
            .find(|(arrow, _)| arrow.row == Row::Msaa && arrow.delta == 1)
            .map(|(_, transform)| transform.translation)
            .expect("the MSAA row spawns a `>` arrow");
        let mut windows = world.query_filtered::<Entity, With<PrimaryWindow>>();
        let window = windows.single(world).expect("the harness made one window");
        let location = Location {
            target: bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
                .normalize(Some(window))
                .expect("the primary window is a render target"),
            position: at,
        };

        // Move first: a press is only routed to what the previous frame's hover map says is under
        // the pointer, so the two have to be separate frames.
        world.write_message(PointerInput::new(
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

    /// **How tall the whole card actually is**, in the PHYSICAL pixels `ComputedNode` reports —
    /// i.e. how much window height the page needs to be fully readable at the UI scale the app was
    /// built with.
    ///
    /// Measured off the CARD (the wrapper's single child), not the wrapper: the wrapper is
    /// `100% x 100%` by construction and would report the window every time.
    ///
    /// **Deliberately NOT divided by `inverse_scale_factor`.** That value folds `UiScale` in
    /// (`bevy_ui::update` builds it as `target_scaling_factor * ui_scale` — the same trap
    /// [`mouse_input`]'s doc records), so dividing by it converts to CSS pixels and cancels the
    /// very rescale this measurement exists to see: the card would report the same design height at
    /// every rung and the ladder loop below would be pure ceremony. `headless_card_app`'s window is
    /// scale factor 1, so what is returned here IS the logical window height the card occupies.
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

    /// **Walking the selection can never lose it off the edge of the viewport.**
    ///
    /// This is the whole reason the page scrolls at all: it is driven with the arrow keys, so a
    /// selection that can leave the clipped box is strictly worse than no scrolling — the highlight
    /// vanishes and the player is editing a value they cannot see. The window here is deliberately
    /// short (400 px, so the card is 360 and only a few rows fit at once) because that is the state
    /// the contract is about; at 1080 px most of the list is visible and the walk proves little.
    ///
    /// Both directions, because the correction is two different branches of [`scroll_rows`] (a row
    /// above the top scrolls back, a row below the bottom scrolls forward) and a page that only
    /// followed downwards would look fine until the player pressed Up.
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

    /// The wheel scrolls the list, stops at both ends, and is INERT while the page is shut.
    ///
    /// The clamp is the half worth pinning: `bevy_ui` clamps only the offset it RENDERS with and
    /// leaves the component free to run away (see [`scroll_rows`]), so an unclamped page would
    /// answer the first wheel-up after a hard scroll-down by doing nothing at all, for as many
    /// notches as it took to get there.
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
            "one wheel notch must move the list ({scrolled} after a notch of {WHEEL_LINE_PX} px)",
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

    /// **A trackpad's PIXEL delta is physical, and must be converted like every other physical
    /// measurement here.**
    ///
    /// winit's `MouseScrollDelta::PixelDelta` carries a `PhysicalPosition`
    /// (winit-0.30.13/src/event.rs:961-966) and bevy copies its components across unchanged
    /// (bevy_winit-0.19.0/src/state.rs:342-349), while [`ScrollPosition`] is in CSS pixels — layout
    /// multiplies it by a factor that folds `UiScale` in (`bevy_ui::update`,
    /// `target_scaling_factor().unwrap_or(1.) * ui_scale.0`). Applying the raw delta therefore
    /// scaled a trackpad flick twice: 0.75x at the 75% rung and 1.5x at 150%, doubled again on a
    /// Retina target.
    ///
    /// Tested PURE, like [`track_fraction`], and for the usual reason plus one specific to this
    /// page: the selection follow deliberately runs AFTER the wheel and overrides it, so a wheel
    /// measured through a laid-out card measures the follow, not the unit conversion.
    ///
    /// The claim is an invariant rather than a table of magic numbers — the same physical flick
    /// must move the same PHYSICAL distance of content at every rung, because the player's finger
    /// moved the same distance across the same glass. The LINE unit is asserted in the OPPOSITE
    /// direction: it is a dimensionless notch count, authored in CSS px, and it should scale with
    /// the UI exactly as the rows it steps past do.
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
                WHEEL_LINE_PX,
                "ui {ui_scale}x on a {target_scale}x target: a wheel NOTCH is a dimensionless \
                 count and must stay {WHEEL_LINE_PX} css px, scaling with the UI like the rows it \
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
