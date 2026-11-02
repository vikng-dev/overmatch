//! Offered display ladder: AUTO plus each synchronized monitor, named as the OS names it.
//!
//! Persisted [`DisplaySelection`] never narrows: an old Display3 must still parse on a two-panel
//! machine or the whole video file is lost. Resolve it read-side through `DisplaySelection::resolve`
//! so row and window agree, without rewriting a choice that may return after hot-plug.
//!
//! Offer the marked primary as `Primary`, not its ordinal too: two entries would name one panel.
//! This retains `Primary`'s creation-time placement advantage; indexed selections move after boot.
//!
//! An empty monitor list means unsynchronized/no winit, not evidence of no displays. Offer every
//! persisted variant until monitors are learned, as `PresentCaps` does for unknown capabilities.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{Monitor, PrimaryMonitor};
use bevy::winit::WinitMonitors;
use winit::monitor::MonitorHandle;

use super::{AttachedDisplays, DisplaySelection, read_displays};

/// The ladder, rebuilt whenever the monitor list moves. Mounted by `ui::plugin`, whose page is its
/// only reader, and ordered ahead of that page's own chain (which runs `.after(DeclareSettingsPage)`)
/// so a row opened the same frame a display was plugged in reads this frame's ladder.
pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DisplayLadder>().add_systems(
        Update,
        refresh_ladder
            .run_if(monitors_changed)
            .before(super::ui::DeclareSettingsPage),
    );
}

/// The widest a rung may render, in characters — the row's value well is a fixed width
/// (`ui::Row::value_well_px`) and an OS display name has no length bound at all.
const LABEL_MAX_CHARS: usize = 26;

/// What sits in front of the primary display's rung. ASCII, and one glyph, because it prefixes a
/// name that already spends most of the well.
const PRIMARY_MARK: &str = "* ";

/// One offered rung: the value the row stores when it is chosen, the attached monitor it names, and
/// the text it renders.
#[derive(PartialEq, Eq, Debug)]
struct Rung {
    selection: DisplaySelection,
    /// Zero-based position in the attached-monitor list — `None` for [`DisplaySelection::Auto`],
    /// which names no monitor at all.
    monitor: Option<usize>,
    /// What [`crate::ui_font::drawable`] keeps, plus `…` — it reaches `Text`.
    label: String,
}

/// The rungs the display row offers, and the display list they were built from.
#[derive(Resource, PartialEq, Eq, Debug)]
pub(super) struct DisplayLadder {
    /// What [`DisplaySelection::resolve`] is resolved against here, so the row and the two window
    /// writers cannot form different opinions of the same machine.
    displays: AttachedDisplays,
    /// Never empty: [`DisplaySelection::Auto`] is always rung 0, which is what makes every index
    /// below infallible.
    rungs: Vec<Rung>,
}

impl Default for DisplayLadder {
    fn default() -> Self {
        build(AttachedDisplays::default(), &[])
    }
}

impl DisplayLadder {
    /// The text the row renders for `selection`. ASCII plus `…` — it reaches `Text`.
    pub(super) fn label(&self, selection: DisplaySelection) -> &str {
        &self.rungs[self.rung_of(selection)].label
    }

    /// `selection` moved `delta` rungs, saturating at the ends rather than wrapping — the property
    /// the whole page shares, and the one that lets "hold left" always reach `AUTO`.
    pub(super) fn step(&self, selection: DisplaySelection, delta: i32) -> DisplaySelection {
        let last = self.rungs.len() as i32 - 1;
        let next = (self.rung_of(selection) as i32 + delta).clamp(0, last);
        self.rungs[next as usize].selection
    }

    /// Resolve an absent display first, then find its rung by selection or monitor ordinal: a file
    /// may name the primary by its index while the offered rung stores `Primary`.
    fn rung_of(&self, selection: DisplaySelection) -> usize {
        let resolved = selection.resolve(self.displays);
        self.rungs
            .iter()
            .position(|rung| rung.selection == resolved)
            .or_else(|| {
                let named = resolved.index()?;
                self.rungs
                    .iter()
                    .position(|rung| rung.monitor == Some(named))
            })
            .unwrap_or(0)
    }
}

/// One attached monitor, in the order `MonitorSelection::Index` counts in.
struct Panel<'a> {
    /// Whether bevy marked this one `PrimaryMonitor`.
    primary: bool,
    /// The operating system's own name for it, where this platform has one.
    name: Option<String>,
    monitor: &'a Monitor,
}

/// The ladder `panels` deserves. Pure: which monitor is which, and what the OS calls it, is
/// [`refresh_ladder`]'s half.
fn build(displays: AttachedDisplays, panels: &[Panel<'_>]) -> DisplayLadder {
    if panels.is_empty() {
        return DisplayLadder {
            displays,
            rungs: DisplaySelection::ORDER.into_iter().map(unnamed).collect(),
        };
    }
    let mut rungs = vec![unnamed(DisplaySelection::Auto)];
    for (index, panel) in panels.iter().enumerate() {
        // Skip unaddressable monitors; borrowing another display's variant would mislabel it.
        // See `DisplaySelection::INDEXED`.
        let Some(selection) = (if panel.primary {
            Some(DisplaySelection::Primary)
        } else {
            DisplaySelection::at_index(index)
        }) else {
            continue;
        };
        rungs.push(Rung {
            selection,
            monitor: Some(index),
            label: rung_label(panel),
        });
    }
    DisplayLadder { displays, rungs }
}

/// A rung named by its variant alone — `AUTO`, and every rung of the unsynchronised ladder.
fn unnamed(selection: DisplaySelection) -> Rung {
    Rung {
        selection,
        monitor: selection.index(),
        label: selection.label().to_string(),
    }
}

/// What one attached display is called on the row: the operating system's own name where there is
/// one this font can draw, upper-cased to match the rest of the page, its geometry otherwise.
/// `NSScreen::localizedName` is a LOCALIZED string, so a machine in a non-Latin locale can hand us
/// a name with nothing legible left in it, and a rung labelled `--` states less than its
/// resolution does.
fn rung_label(panel: &Panel<'_>) -> String {
    let named = panel
        .name
        .as_deref()
        .and_then(crate::ui_font::drawable)
        .map(|name| name.to_ascii_uppercase());
    let label = named.unwrap_or_else(|| geometry_label(panel.monitor));
    let label = if panel.primary {
        format!("{PRIMARY_MARK}{label}")
    } else {
        label
    };
    truncate(&label)
}

/// A display named by what it is: its physical size, and its refresh rate where the platform reports
/// one. Whole hertz — the row is a picker, and `119.88` on it is noise.
fn geometry_label(monitor: &Monitor) -> String {
    let size = format!("{}X{}", monitor.physical_width, monitor.physical_height);
    match monitor.refresh_rate_millihertz {
        Some(millihertz) => format!("{size} {}HZ", (millihertz + 500) / 1000),
        None => size,
    }
}

/// `label` cut to [`LABEL_MAX_CHARS`], ellipsised where it had to be cut. `…` is inside the shipped
/// font's verified coverage (see AGENTS.md).
fn truncate(label: &str) -> String {
    if label.chars().count() <= LABEL_MAX_CHARS {
        return label.to_string();
    }
    label
        .chars()
        .take(LABEL_MAX_CHARS - 1)
        .chain(['\u{2026}'])
        .collect()
}

/// Whether the monitor list has moved since the ladder was built. `Changed<Monitor>` catches a
/// display appearing — bevy_winit SPAWNS a new entity rather than editing an existing one (vendored
/// bevy_winit-0.19.0/src/system.rs:184-215) — and the count catches one going away, which is a
/// despawn that changes nothing still in the world.
fn monitors_changed(
    changed: Query<(), Changed<Monitor>>,
    monitors: Query<(), With<Monitor>>,
    ladder: Res<DisplayLadder>,
) -> bool {
    !changed.is_empty() || monitors.iter().count() != ladder.displays.count
}

/// Rebuild the ladder from the monitors bevy has synchronised.
///
/// [`NonSendMarker`] pins this to the main thread (the repo's `settings::probe` idiom): the name
/// lookup below is an AppKit call, and its main-thread check answers `None` anywhere else — which
/// would cost every rung its name on a schedule that happened to run this elsewhere.
fn refresh_ladder(
    _main_thread: NonSendMarker,
    counted: Query<(), With<Monitor>>,
    primaries: Query<Entity, (With<Monitor>, With<PrimaryMonitor>)>,
    monitors: Query<(Entity, &Monitor)>,
    winit_monitors: Option<Res<WinitMonitors>>,
    mut ladder: ResMut<DisplayLadder>,
) {
    let displays = read_displays(&counted, &primaries);
    let mut ordered: Vec<(Entity, &Monitor, Option<MonitorHandle>)> = monitors
        .iter()
        .map(|(entity, monitor)| {
            let handle = winit_monitors
                .as_ref()
                .and_then(|winit| winit.find_entity(entity));
            (entity, monitor, handle)
        })
        .collect();
    // Sort by WINIT order (`WinitMonitors::nth`), which `MonitorSelection::Index` uses. ECS query
    // order diverges after unplugging; stable sort preserves it in bare Apps without handles.
    ordered.sort_by_key(|(_, _, handle)| {
        let index = winit_monitors
            .as_ref()
            .zip(handle.as_ref())
            .and_then(|(winit, handle)| winit_index(winit, handle));
        index.unwrap_or(usize::MAX)
    });

    let handles: Vec<Option<MonitorHandle>> = ordered
        .iter()
        .map(|(_, _, handle)| handle.clone())
        .collect();
    let panels: Vec<Panel<'_>> = ordered
        .iter()
        .zip(os_display_names(&handles))
        .map(|((entity, monitor, _), name)| Panel {
            primary: displays.primary == Some(*entity),
            name,
            monitor,
        })
        .collect();
    *ladder = build(displays, &panels);
    info!(
        "settings: display ladder = [{}]",
        ladder
            .rungs
            .iter()
            .map(|rung| format!("{:?} {:?}", rung.selection, rung.label))
            .collect::<Vec<_>>()
            .join(", "),
    );
}

/// Where `handle` sits in winit's own monitor list. Bevy exposes that list only through
/// `WinitMonitors::nth`, so it is walked rather than indexed.
fn winit_index(monitors: &WinitMonitors, handle: &MonitorHandle) -> Option<usize> {
    (0..)
        .map_while(|n| monitors.nth(n))
        .position(|candidate| candidate == *handle)
}

/// OS names in handle order; None means geometry is the best available label.
///
/// Ask AppKit on macOS: winit's `MonitorHandle::name` uses a model number, so identical panels
/// share an unrecognizable name. `NSScreen::localizedName` matches the Displays pane.
/// Source: winit-0.30.13/src/platform_impl/macos/monitor.rs:222-228 (`MonitorHandle::name`).
///
/// Pair winit's `native_id` with AppKit's `NSScreenNumber` (`CGDirectDisplayID`). Geometry pairing
/// is rejected despite matching both panels in a MEASURED two-display check: flipping AppKit's
/// bottom-left origin and applying `backingScaleFactor` works by coincidence, and needs a main
/// screen height valid only while the primary is `screens[0]`.
#[cfg(target_os = "macos")]
fn os_display_names(handles: &[Option<MonitorHandle>]) -> Vec<Option<String>> {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;
    use winit::platform::macos::MonitorHandleExtMacOS;

    let Some(mtm) = MainThreadMarker::new() else {
        warn!("settings: display names need the main thread; falling back to resolutions");
        return vec![None; handles.len()];
    };
    let screens = NSScreen::screens(mtm);
    handles
        .iter()
        .map(|handle| {
            let wanted = handle.as_ref()?.native_id();
            let screen = screens
                .iter()
                .find(|screen| screen_number(screen) == Some(wanted))?;
            // SAFETY: a plain AppKit accessor with no invariants beyond main-thread, which `mtm`
            // proves; `unsafe` only because the objc2 0.2 generation did not audit it as safe.
            Some(unsafe { screen.localizedName() }.to_string())
        })
        .collect()
}

/// The `CGDirectDisplayID` behind an `NSScreen`.
#[cfg(target_os = "macos")]
fn screen_number(screen: &objc2_app_kit::NSScreen) -> Option<u32> {
    let description = screen.deviceDescription();
    let value = description.get(objc2_foundation::ns_string!("NSScreenNumber"))?;
    // SAFETY: AppKit documents `deviceDescription`'s `NSScreenNumber` entry as an `NSNumber`.
    let number: &objc2_foundation::NSNumber = unsafe { &*core::ptr::from_ref(value).cast() };
    Some(number.as_u32())
}

#[cfg(not(target_os = "macos"))]
fn os_display_names(handles: &[Option<MonitorHandle>]) -> Vec<Option<String>> {
    vec![None; handles.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A monitor as bevy reports one. Fixture data: the numbers name nothing this module branches
    /// on, they are only what a label is derived from.
    fn monitor(width: u32, height: u32, millihertz: Option<u32>) -> Monitor {
        Monitor {
            name: Some("Monitor #41054".to_string()),
            physical_width: width,
            physical_height: height,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: millihertz,
            scale_factor: 2.0,
            video_modes: Vec::new(),
        }
    }

    /// `panels` as `build` wants them, plus the [`AttachedDisplays`] that agrees with them.
    fn ladder(panels: &[(bool, Option<&str>, &Monitor)]) -> DisplayLadder {
        let displays = AttachedDisplays {
            count: panels.len(),
            primary: panels
                .iter()
                .any(|(primary, _, _)| *primary)
                .then_some(Entity::PLACEHOLDER),
        };
        let panels: Vec<Panel<'_>> = panels
            .iter()
            .map(|(primary, name, monitor)| Panel {
                primary: *primary,
                name: name.map(str::to_string),
                monitor,
            })
            .collect();
        build(displays, &panels)
    }

    fn offered(ladder: &DisplayLadder) -> Vec<(DisplaySelection, &str)> {
        ladder
            .rungs
            .iter()
            .map(|rung| (rung.selection, rung.label.as_str()))
            .collect()
    }

    /// **The ladder is the monitors that exist.** Two attached displays offer two display rungs —
    /// there is no third, whatever the enum can hold.
    #[test]
    fn the_ladder_offers_one_rung_per_attached_monitor() {
        let built_in = monitor(3024, 1964, Some(120_000));
        let external = monitor(2560, 1600, Some(60_000));
        let ladder = ladder(&[
            (true, Some("Built-in Retina Display"), &built_in),
            (false, Some("ARZOPA"), &external),
        ]);
        assert_eq!(
            offered(&ladder),
            vec![
                (DisplaySelection::Auto, "AUTO"),
                (DisplaySelection::Primary, "* BUILT-IN RETINA DISPLAY"),
                (DisplaySelection::Display2, "ARZOPA"),
            ],
        );
    }

    /// The primary display's rung stores `Primary` WHEREVER it sits, and its ordinal name is then
    /// not offered at all — the two-entries-one-panel bug this module exists to end.
    #[test]
    fn the_primary_monitor_owns_the_primary_rung_at_any_position() {
        let panel = monitor(1920, 1080, Some(60_000));
        let second_is_primary =
            ladder(&[(false, Some("Left"), &panel), (true, Some("Main"), &panel)]);
        assert_eq!(
            offered(&second_is_primary),
            vec![
                (DisplaySelection::Auto, "AUTO"),
                (DisplaySelection::Display1, "LEFT"),
                (DisplaySelection::Primary, "* MAIN"),
            ],
        );
        // And the ordinal a file may hold for that same panel reads as its rung, rather than as
        // nothing.
        assert_eq!(
            second_is_primary.label(DisplaySelection::Display2),
            "* MAIN"
        );
        assert_eq!(
            second_is_primary.step(DisplaySelection::Display2, -1),
            DisplaySelection::Display1,
        );
    }

    /// Where the window system names no primary — every Wayland session — every rung is ordinal, and
    /// `Primary` is not offered. `resolve`'s own substitution then makes a stored `Primary` read as
    /// the first display, which is the rung that exists.
    #[test]
    fn an_unmarked_primary_leaves_every_rung_ordinal() {
        let panel = monitor(1920, 1080, None);
        let ladder = ladder(&[
            (false, Some("Left"), &panel),
            (false, Some("Right"), &panel),
        ]);
        assert_eq!(
            offered(&ladder),
            vec![
                (DisplaySelection::Auto, "AUTO"),
                (DisplaySelection::Display1, "LEFT"),
                (DisplaySelection::Display2, "RIGHT"),
            ],
        );
        assert_eq!(ladder.label(DisplaySelection::Primary), "LEFT");
    }

    /// **An unsynchronised list is "could not learn", not "no displays".** It offers every variant a
    /// file can hold, under the names that say only where a rung sits.
    #[test]
    fn an_unsynchronised_list_narrows_nothing() {
        let ladder = DisplayLadder::default();
        assert_eq!(
            offered(&ladder),
            vec![
                (DisplaySelection::Auto, "AUTO"),
                (DisplaySelection::Primary, "PRIMARY"),
                (DisplaySelection::Display1, "DISPLAY 1"),
                (DisplaySelection::Display2, "DISPLAY 2"),
                (DisplaySelection::Display3, "DISPLAY 3"),
            ],
        );
    }

    /// An unreachable stored rung must render and step from the resolved panel without being
    /// rewritten, so hot-plug can restore the choice. Steps saturate and holding left reaches AUTO,
    /// returning placement to the window manager.
    #[test]
    fn a_persisted_unreachable_rung_renders_and_steps_without_being_rewritten() {
        let built_in = monitor(3024, 1964, Some(120_000));
        let external = monitor(2560, 1600, Some(60_000));
        let ladder = ladder(&[
            (true, Some("Built-in Retina Display"), &built_in),
            (false, Some("ARZOPA"), &external),
        ]);
        let stored = DisplaySelection::Display3;
        assert!(
            !ladder.rungs.iter().any(|rung| rung.selection == stored),
            "the third rung must not be offered on a two-display machine",
        );
        assert_eq!(ladder.label(stored), "* BUILT-IN RETINA DISPLAY");
        assert_eq!(ladder.step(stored, 1), DisplaySelection::Display2);
        assert_eq!(ladder.step(stored, -1), DisplaySelection::Auto);
        let mut selection = DisplaySelection::Auto;
        for _ in 0..8 {
            selection = ladder.step(selection, 1);
        }
        assert_eq!(selection, DisplaySelection::Display2, "the top saturates");
        for _ in 0..8 {
            selection = ladder.step(selection, -1);
        }
        assert_eq!(selection, DisplaySelection::Auto, "AUTO is reachable");
    }

    /// A monitor past the enum's indexed rungs gets none rather than another display's variant.
    #[test]
    fn a_monitor_the_enum_cannot_name_gets_no_rung() {
        let panel = monitor(1920, 1080, None);
        let ladder = ladder(&[
            (false, Some("One"), &panel),
            (false, Some("Two"), &panel),
            (false, Some("Three"), &panel),
            (false, Some("Four"), &panel),
        ]);
        assert_eq!(
            offered(&ladder)
                .into_iter()
                .map(|(selection, _)| selection)
                .collect::<Vec<_>>(),
            vec![
                DisplaySelection::Auto,
                DisplaySelection::Display1,
                DisplaySelection::Display2,
                DisplaySelection::Display3,
            ],
        );
    }
}
