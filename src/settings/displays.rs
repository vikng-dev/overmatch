//! What the display row OFFERS: `AUTO`, plus one rung per monitor bevy has synchronised, each named
//! the way the operating system names it.
//!
//! # Offered and stored are different questions
//!
//! [`DisplaySelection`] is the persisted enum and it never narrows — a `video.ron` holding
//! `Display3` on a two-display machine must still parse, because a parse failure costs the player
//! their whole video file. This module decides only what the row offers THIS MINUTE, and that is the
//! fact the enum cannot state: a rung the machine does not have is a rung a player can land on by
//! accident and then cannot tell apart from one that works.
//!
//! Nothing here changes what a stored rung MEANS. `DisplaySelection::resolve` is still the one
//! read-side fallback, still logs and still never writes; the ladder resolves through it so the row
//! renders the display the window will actually be placed on.
//!
//! # The primary display has no rung of its own
//!
//! `PRIMARY` and `DISPLAY 1` named the same panel and rendered as two entries, which is half of how
//! a settings file goes bad. So the rung for whichever monitor bevy marked `PrimaryMonitor` STORES
//! [`DisplaySelection::Primary`] instead of its ordinal name, and the ordinal is offered only for
//! the displays that are not the primary. That keeps the variant alive on disk and keeps its one
//! functional advantage: `DisplaySelection::at_window_creation` passes `Primary` through to window
//! creation, so the window is BORN on that display rather than created elsewhere and moved.
//!
//! # An empty monitor list is not an empty machine
//!
//! No [`Monitor`] entities means the list has not been synchronised yet (or there is no winit at
//! all), which is "could not learn" — the same tri-state discipline `PresentCaps` and
//! `DisplaySelection::resolve` are built on. The ladder then offers every variant a file can hold,
//! because narrowing on no evidence would take a player's display away from a frame that has never
//! seen one.

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
    /// ASCII plus `…`, per [`sanitize`] — it reaches `Text`.
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

    /// Which rung a stored selection sits on.
    ///
    /// Two lookups, because a stored value need not be a value the ladder offers. It is RESOLVED
    /// first, so a rung naming a display that is not attached reads as the one that will actually be
    /// used. Then, if the resolved rung is still not offered, by the MONITOR it names — the primary
    /// display's rung stores `Primary`, and a file holding that same display's ordinal name is
    /// talking about the same panel.
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
        // A monitor past the enum's indexed rungs gets none: `DisplaySelection` cannot name it, and
        // a rung that stored some other display's variant would be a mislabelled display rather than
        // a missing one. See `DisplaySelection::INDEXED`.
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
/// one this font can draw, its geometry otherwise.
fn rung_label(panel: &Panel<'_>) -> String {
    let named = panel.name.as_deref().and_then(sanitize);
    let label = named.unwrap_or_else(|| geometry_label(panel.monitor));
    let label = if panel.primary {
        format!("{PRIMARY_MARK}{label}")
    } else {
        label
    };
    truncate(&label)
}

/// `name` reduced to what the shipped font can draw: printable ASCII, upper-cased to match the rest
/// of the page, every run of anything else collapsed to a single space so a dropped glyph cannot
/// weld two words together.
///
/// `None` where nothing legible survives. `NSScreen::localizedName` is a LOCALIZED string, so a
/// machine in a non-Latin locale can hand us a name that sanitizes to punctuation or to nothing, and
/// a rung labelled `--` states less than its resolution does.
fn sanitize(name: &str) -> Option<String> {
    let mut sanitized = String::new();
    for character in name.chars() {
        let kept = match character {
            '!'..='~' => character.to_ascii_uppercase(),
            _ => ' ',
        };
        if kept != ' ' || !(sanitized.is_empty() || sanitized.ends_with(' ')) {
            sanitized.push(kept);
        }
    }
    let sanitized = sanitized.trim_end().to_string();
    sanitized
        .chars()
        .any(|character| character.is_ascii_alphanumeric())
        .then_some(sanitized)
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
/// font's verified coverage (see AGENTS.md); nothing else here leaves ASCII.
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
    // `MonitorSelection::Index(n)` counts in WINIT's list, not in the ECS's: bevy resolves it with
    // `WinitMonitors::nth` (vendored bevy_winit-0.19.0/src/winit_windows.rs:522). Query order is
    // spawn order only until a monitor is unplugged, and a rung labelled from the wrong monitor is
    // the one failure this whole module exists to prevent. Stable, so a world with no `WinitMonitors`
    // at all (every bare-`App` test) keeps query order rather than shuffling.
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

/// The operating system's own name for each handle, positionally. `None` means "this platform has
/// nothing better than the geometry", never "that display has no name".
///
/// macOS is the only platform asked, and it is not asked through winit: `MonitorHandle::name` there
/// is `format!("Monitor #{}", CGDisplay::model_number())` under a `TODO: Be smarter about this`
/// (vendored winit-0.30.13/src/platform_impl/macos/monitor.rs:222-228). That is a MODEL number, so
/// two identical panels share it and no player recognises any of them. `NSScreen::localizedName` is
/// the string the Displays pane shows.
///
/// **The pairing is by `CGDirectDisplayID`, which both sides already carry** — winit's `native_id`
/// and AppKit's `NSScreenNumber` device-description key — so a rung is named by its OWN display or
/// not named at all. Pairing on geometry instead also works here (MEASURED 2026-08-17 on a two
/// display machine: flipping AppKit's bottom-left origin and scaling by `backingScaleFactor`
/// reproduced winit's physical position and size exactly for both) but it pairs by coincidence
/// rather than by identity, and it needs a main-screen height that is only correct while the primary
/// is `screens[0]`.
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

/// The ladder a machine with these named monitors builds, the `primary`th of them marked. Fixture
/// data — the geometry is arbitrary; only the names and the primary position are asserted on.
#[cfg(test)]
pub(super) fn attached(names: &[&str], primary: Option<usize>) -> DisplayLadder {
    let monitors: Vec<Monitor> = names
        .iter()
        .map(|_| Monitor {
            name: None,
            physical_width: 1920,
            physical_height: 1080,
            physical_position: IVec2::ZERO,
            refresh_rate_millihertz: Some(60_000),
            scale_factor: 1.0,
            video_modes: Vec::new(),
        })
        .collect();
    let panels: Vec<Panel<'_>> = names
        .iter()
        .zip(&monitors)
        .enumerate()
        .map(|(index, (name, monitor))| Panel {
            primary: primary == Some(index),
            name: Some((*name).to_string()),
            monitor,
        })
        .collect();
    let displays = AttachedDisplays {
        count: names.len(),
        primary: primary.map(|_| Entity::PLACEHOLDER),
    };
    build(displays, &panels)
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

    /// A stored rung this machine cannot reach still loads, still renders, and still steps — it reads
    /// as the display it will actually be placed on. Nothing here writes it back; the file keeps the
    /// player's choice for the day the display returns (see `DisplaySelection::resolve`).
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
    }

    /// Stepping saturates at both ends, and `AUTO` is reachable by holding left from anywhere — the
    /// only rung that hands placement back to the window manager.
    #[test]
    fn stepping_saturates_and_auto_is_always_reachable() {
        let panel = monitor(1920, 1080, Some(60_000));
        let ladder = ladder(&[(true, None, &panel), (false, None, &panel)]);
        let mut selection = DisplaySelection::Auto;
        for _ in 0..8 {
            selection = ladder.step(selection, 1);
        }
        assert_eq!(selection, DisplaySelection::Display2);
        for _ in 0..8 {
            selection = ladder.step(selection, -1);
        }
        assert_eq!(selection, DisplaySelection::Auto);
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

    /// With no OS name, the label is what the display IS. The refresh rate is dropped where the
    /// platform reports none rather than rendered as a zero.
    #[test]
    fn a_nameless_display_is_labelled_by_its_geometry() {
        let panel = monitor(3024, 1964, Some(119_880));
        assert_eq!(
            rung_label(&Panel {
                primary: false,
                name: None,
                monitor: &panel,
            }),
            "3024X1964 120HZ",
        );
        let rateless = monitor(2560, 1600, None);
        assert_eq!(
            rung_label(&Panel {
                primary: true,
                name: None,
                monitor: &rateless,
            }),
            "* 2560X1600",
        );
    }

    /// The sanitizer's whole contract: printable ASCII, upper case, collapsed runs — and `None`
    /// wherever nothing legible is left, which is what sends the rung back to its geometry.
    #[test]
    fn the_sanitizer_keeps_only_what_the_font_can_draw() {
        assert_eq!(
            sanitize("Built-in Retina Display").as_deref(),
            Some("BUILT-IN RETINA DISPLAY"),
        );
        assert_eq!(
            sanitize("DELL\u{a0}U2723QE").as_deref(),
            Some("DELL U2723QE")
        );
        // Escaped, not literal: this file is scanned by the rendered-coverage guard
        // (`tests/ui_ascii.rs`), and these are the inputs that must NOT survive.
        assert_eq!(
            sanitize("  \u{c9}cran  externe  ").as_deref(),
            Some("CRAN EXTERNE")
        );
        assert_eq!(sanitize("\u{30c7}\u{30a3}\u{30b9}"), None);
        assert_eq!(sanitize("--"), None);
        assert_eq!(sanitize(""), None);
        // Every surviving character is drawable, whatever went in.
        for name in [
            "\u{3a9} display 4K\u{2122}",
            "\u{1f4fa} TV",
            "tab\tseparated",
        ] {
            let sanitized = sanitize(name).expect("these all keep letters");
            assert!(
                sanitized.chars().all(|c| (' '..='~').contains(&c)),
                "{sanitized:?} left printable ASCII",
            );
        }
    }

    /// A name with no length bound cannot push the row's value out of its well.
    #[test]
    fn an_overlong_name_is_cut_to_the_well() {
        let panel = monitor(1920, 1080, None);
        let label = rung_label(&Panel {
            primary: true,
            name: Some("A Ludicrously Overlong Display Name".to_string()),
            monitor: &panel,
        });
        assert_eq!(label.chars().count(), LABEL_MAX_CHARS);
        assert!(label.ends_with('\u{2026}'), "{label:?}");
        assert!(label.starts_with(PRIMARY_MARK), "{label:?}");
    }
}
