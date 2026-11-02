//! Persistence seam: [`load`], [`save`], [`location`] hide path, format, serialization, tolerance
//! and write safety from model, reconcilers and page. Evidence and rejected crates:
//! `.agents/docs/design/display/persistence-brief-2026-07.md` (verified against Bevy 0.19.0,
//! RON 0.12, serde 1.0.229 and Apple/Microsoft docs).
//!
//! `video.ron` is machine-local; future keybindings belong in portable `controls.ron`. Split by
//! filename in one config directory so Steam Cloud can exclude video/GPU/monitor facts. Valve
//! requires that separation; Tunnet had to hotfix its video file out of cloud sync.
//!
//! On-disk types use primitives, String and our enums, never Bevy's KeyCode: the 0.12→0.13
//! variant rename broke player files. Use BTreeMap for stable bytes and per-action Option<Key>
//! so one bad binding reverts one action, not the whole file.
//!
//! Hand-rolling is settled (brief §1): `bevy_settings` at v0.19.0 self-writes to macOS Preferences
//! against Apple's rule, destroys unknown keys/comments, silently reverts bad fields and has no
//! version. `bevy-persistent` truncates in place; `bevy_pkv` silently falls back to CWD. Tunnet
//! uses this shape too.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{SETTINGS_VERSION, Settings};

/// Directory name under the platform config root.
///
/// A **deliberate deviation** from Apple's reverse-DNS convention (`com.vikngdev.overmatch`): a
/// player hunting for their config finds `Overmatch`, and this is a game, not a system service.
const CONFIG_DIR_NAME: &str = "Overmatch";

/// The machine-local video/graphics file — see the module doc on why the name matters.
const CONFIG_FILE_NAME: &str = "video.ron";

/// Overrides the whole resolved config directory. Exists for the same reason `BEVY_ASSET_ROOT` does:
/// a test (or a packaged/CI run) needs to point the game at a scratch directory without inheriting
/// the developer's real settings.
const CONFIG_DIR_ENV: &str = "OVERMATCH_CONFIG_DIR";

/// How long to keep retrying a rename that a Windows file-scanner is transiently blocking, in total.
/// Doubling backoff from 1 ms; ~500 ms is the figure rustup's long history with this settled on
/// (rustup itself needs 28 s only because it replaces a *running executable*).
const RENAME_RETRY_BUDGET: std::time::Duration = std::time::Duration::from_millis(512);

/// Windows error codes raised when AV, Search Indexer, OneDrive or Dropbox is holding the target
/// handle: `ERROR_ACCESS_DENIED`, `ERROR_SHARING_VIOLATION`, `ERROR_LOCK_VIOLATION`. Transient — the
/// scanner lets go within milliseconds. Never produced by the platforms where they'd be permanent.
const WINDOWS_TRANSIENT_RENAME_ERRORS: [i32; 3] = [5, 32, 33];

/// Probe version before schema: missing means 0, other keys are ignored. A future file with
/// restructured fields must report FutureVersion, not Corrupt. Godot's streaming config_version
/// and Minecraft's untyped Options probe use the same ordering.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: u32,
}

/// Boot outcome as data: `load` precedes DefaultPlugins and the tracing subscriber. MEASURED:
/// logging there produced no line in a real `--offline` launch. Carry it to
/// `settings::report_store_load` at Startup instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum LoadNote {
    /// The file was read and applied.
    Loaded,
    /// No file yet — a first launch, not a problem.
    FirstLaunch,
    /// No platform config directory at all (no `HOME`/`APPDATA`). Settings will not persist.
    NoLocation,
    /// The file exists but could not be read (permissions, IO).
    Unreadable(String),
    /// The file did not parse. Carries the parse error and where the original was preserved.
    Corrupt {
        error: String,
        kept: Option<PathBuf>,
    },
    /// Written by a newer build than this one. Carries the version and where it was preserved.
    FutureVersion { version: u32, kept: Option<PathBuf> },
}

/// A settings read: the values to use, and what happened getting them. Defaults are ALWAYS usable —
/// there is no failure mode that stops the game starting. (ADR-0011's fail-fast stance covers
/// required in-repo assets; a user's mutable config is the opposite case, and the honest response is
/// to fall back loudly, not to refuse to launch.)
pub(super) struct Load {
    pub(super) settings: Settings,
    pub(super) note: LoadNote,
}

/// Pure path resolution, unit-testable without env reads (as `assets::asset_root_from`).
/// Precedence: [`CONFIG_DIR_ENV`], then platform convention:
/// - macOS: $HOME/Library/Application Support/Overmatch
/// - Windows: %APPDATA%\Overmatch
/// - Linux/other: $XDG_CONFIG_HOME/overmatch, else $HOME/.config/overmatch
///
/// Never exe-relative: signed .apps are read-only (Apple TN2206), App Translocation adds a
/// randomized read-only mount changing each launch, and 64-bit/manifested Program Files apps
/// get `ACCESS_DENIED` without VirtualStore. The read-only asset root may be exe-relative; writes
/// cannot. Missing home/appdata returns None for defaults plus [`LoadNote::NoLocation`], never
/// an invented path.
fn config_dir_from(
    target_os: &str,
    env_override: Option<&Path>,
    home: Option<&Path>,
    app_data: Option<&Path>,
    xdg_config_home: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(dir) = env_override {
        return Some(dir.to_path_buf());
    }
    match target_os {
        "macos" => Some(
            home?
                .join("Library")
                .join("Application Support")
                .join(CONFIG_DIR_NAME),
        ),
        "windows" => Some(app_data?.join(CONFIG_DIR_NAME)),
        _ => {
            let base = match xdg_config_home {
                Some(xdg) if !xdg.as_os_str().is_empty() => xdg.to_path_buf(),
                _ => home?.join(".config"),
            };
            // Lower-case on Linux: `~/.config` is conventionally lower-case, unlike the two
            // platforms above whose directories are user-visible and title-cased. (XDG also puts
            // CONFIG here and DATA in `~/.local/share` — saves, if they ever exist, belong there.)
            Some(base.join(CONFIG_DIR_NAME.to_ascii_lowercase()))
        }
    }
}

/// The `video.ron` path for this process, or `None` when the platform home is unset. The one
/// reader of the process environment in this module: the production callers resolve it and hand it
/// to [`load`], [`save`] and [`reset`], which read nothing but their `path`. Every variable is read
/// as an OS path, so a directory whose name is not UTF-8 is still that directory, never "unset".
pub(super) fn location() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let (env_override, home, app_data, xdg) = (
        var(CONFIG_DIR_ENV),
        var("HOME"),
        var("APPDATA"),
        var("XDG_CONFIG_HOME"),
    );
    config_dir_from(
        std::env::consts::OS,
        env_override.as_deref(),
        home.as_deref(),
        app_data.as_deref(),
        xdg.as_deref(),
    )
    .map(|dir| dir.join(CONFIG_FILE_NAME))
}

/// The serializer config, in one place. **`struct_names` MUST stay `false`** (which is
/// `PrettyConfig::default()`): RON 0.12 does not merely allow an omitted struct name, it *enforces*
/// the name when one IS present — `Config(version: 3)` read into a differently-named struct fails
/// with ``Expected struct `VersionProbe` but found `Config` ``. The anonymous `(…)` form is what
/// makes [`VersionProbe`] work at all. `new_line("\n")` is pinned because the default is `"\r\n"`
/// on Windows, which would make the same settings differ on every line between a Windows and a macOS
/// build.
fn pretty_config() -> ron::ser::PrettyConfig {
    ron::ser::PrettyConfig::default().new_line("\n")
}

/// Move a file we are about to stop honouring out of the way, returning where it went.
///
/// Firefox's `Invalidprefs.js` policy. It does two jobs: a corrupt file is PRESERVED as the only
/// evidence of what went wrong instead of being destroyed by the next save; and a file from a NEWER
/// build is moved aside so the player's next change cannot silently overwrite their newer settings
/// with this build's older shape (the silent-downgrade data loss).
fn keep_aside(path: &Path, suffix: &str) -> Option<PathBuf> {
    let kept = path.with_extension(format!("ron.{suffix}"));
    std::fs::rename(path, &kept).ok().map(|()| kept)
}

/// Read the settings at `path` (`None`: no platform config directory), or return defaults. Logs
/// NOTHING — see [`LoadNote`].
pub(super) fn load(path: Option<&Path>) -> Load {
    let Some(path) = path else {
        return Load {
            settings: Settings::default(),
            note: LoadNote::NoLocation,
        };
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Load {
                settings: Settings::default(),
                note: LoadNote::FirstLaunch,
            };
        }
        Err(err) => {
            return Load {
                settings: Settings::default(),
                note: LoadNote::Unreadable(err.to_string()),
            };
        }
    };
    match parse(&text) {
        Parsed::Ok(settings) => Load {
            settings,
            note: LoadNote::Loaded,
        },
        // Refuse to guess forward — nobody can know what a newer build's values mean. Every engine
        // that has faced this (Godot, Minecraft, Factorio, RimWorld, Unity) refuses loudly.
        Parsed::FromTheFuture(version) => Load {
            settings: Settings::default(),
            note: LoadNote::FutureVersion {
                version,
                kept: keep_aside(path, "newer"),
            },
        },
        Parsed::Corrupt(error) => Load {
            settings: Settings::default(),
            note: LoadNote::Corrupt {
                error,
                kept: keep_aside(path, "bad"),
            },
        },
    }
}

/// The outcome of reading settings TEXT, with no filesystem in it — so every branch is unit
/// testable.
#[derive(Debug, PartialEq)]
enum Parsed {
    Ok(Settings),
    /// Version stamp ahead of this build.
    FromTheFuture(u32),
    Corrupt(String),
}

/// Parse settings text: probe the version FIRST, then the schema.
fn parse(text: &str) -> Parsed {
    // The probe cannot fail on shape, only on syntax — so a future file that restructured a field
    // is still correctly identified as "from the future" rather than "corrupt".
    if let Ok(probe) = ron::de::from_str::<VersionProbe>(text)
        && probe.version > SETTINGS_VERSION
    {
        return Parsed::FromTheFuture(probe.version);
    }
    match ron::de::from_str::<Settings>(text) {
        // Stamp the current version so the next save records the shape actually written, and fold
        // the retired v1 `vsync: bool` key into the live ladder HERE — the legacy shadow field
        // never escapes the persistence seam.
        Ok(settings) => Parsed::Ok(
            Settings {
                version: SETTINGS_VERSION,
                ..settings
            }
            .absorb_legacy_vsync(),
        ),
        Err(err) => Parsed::Corrupt(err.to_string()),
    }
}

/// `rename`, retrying the transient Windows holds. `std::fs::rename` DOES atomic-replace on Windows
/// (std calls `MoveFileExW` / `SetFileInformationByHandle`; the "fails if destination exists"
/// folklore is about raw `MoveFileW`), so the only thing to handle is a scanner holding the handle
/// for a few milliseconds. The retry compiles everywhere and is inert off Windows — the codes it
/// matches are never produced elsewhere — which keeps one code path instead of two.
fn rename_with_retry(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut wait = std::time::Duration::from_millis(1);
    let mut spent = std::time::Duration::ZERO;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(err) => {
                let transient = err
                    .raw_os_error()
                    .is_some_and(|code| WINDOWS_TRANSIENT_RENAME_ERRORS.contains(&code));
                if !transient || spent >= RENAME_RETRY_BUDGET {
                    return Err(err);
                }
                std::thread::sleep(wait);
                spent += wait;
                wait *= 2;
            }
        }
    }
}

/// Write the settings to `path` (`None`: no platform config directory) ATOMICALLY. Returns whether
/// the write landed.
///
/// The sequence, each step answering a specific documented failure:
/// 1. serialize (sparse — see [`Settings`]'s `skip_serializing_if` note);
/// 2. write a **pid-unique** temp file in the same directory. Unique, not fixed: two instances of
///    the game sharing one fixed temp name can have instance B's partial write renamed into place by
///    instance A. That was the only live corruption path in the first cut of this module. (A unique
///    temp name is also why no lock file is needed — last-writer-wins is correct here, and it is
///    what VS Code does across N windows over one `settings.json`.)
/// 3. `sync_all` the temp before renaming. `fs::write` never syncs, and that is the ext4
///    delayed-allocation shape that yields a **zero-length settings file** after a power cut;
///    deliberately NOT macOS's `F_FULLFSYNC`, which SQLite measures as "profoundly slow" for a worst
///    case that is merely "revert to the previous whole file";
/// 4. `rename` over the target, retrying the transient Windows holds ([`rename_with_retry`]);
/// 5. on unix, sync the DIRECTORY so the rename itself is durable — near-free.
///
/// Errors are logged, never propagated: failing to save a graphics preference must not interrupt
/// play. Unlike [`load`] this DOES log directly, and may — it only ever runs long after boot, from a
/// settings-page edit.
pub(super) fn save(path: Option<&Path>, settings: &Settings) -> bool {
    use std::io::Write;

    use bevy::prelude::{error, info, warn};

    let Some(path) = path else {
        warn!("settings: no platform config directory — not saved");
        return false;
    };
    let Some(dir) = path.parent() else {
        return false;
    };
    if let Err(err) = std::fs::create_dir_all(dir) {
        warn!(
            "settings: cannot create {} ({err}) — not saved",
            dir.display()
        );
        return false;
    }
    let text = match ron::ser::to_string_pretty(settings, pretty_config()) {
        Ok(text) => text,
        Err(err) => {
            error!("settings: cannot serialize ({err}) — not saved");
            return false;
        }
    };

    let temporary = path.with_extension(format!("ron.{}.tmp", std::process::id()));
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(text.as_bytes())?;
        // Before the rename, not after: the rename is only atomic with respect to CONTENT that has
        // actually reached the disk.
        file.sync_all()
    });
    if let Err(err) = written {
        warn!(
            "settings: cannot write {} ({err}) — not saved",
            temporary.display()
        );
        std::fs::remove_file(&temporary).ok();
        return false;
    }
    if let Err(err) = rename_with_retry(&temporary, path) {
        warn!(
            "settings: cannot replace {} ({err}) — not saved",
            path.display()
        );
        std::fs::remove_file(&temporary).ok();
        return false;
    }
    #[cfg(unix)]
    if let Ok(handle) = std::fs::File::open(dir) {
        // Makes the rename itself durable, not just the bytes. Best-effort: a filesystem that
        // refuses to sync a directory handle is not a reason to report the save as failed.
        handle.sync_all().ok();
    }
    info!("settings: saved {}", path.display());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        BloomLevel, DebugPanel, DisplaySelection, FrameCap, KillCam, MsaaLevel, PenCamCorner,
        PixelBudget, ReduceMotion, RenderScaleLevel, ShadowCascades, ShadowDistance,
        ShadowResolution, TrackPrintRange, TrackPrintRelief, UiScalePercent, VsyncMode,
        WindowModeSetting,
    };

    /// Each platform lands on its own convention — and NEVER beside the executable, which is
    /// unwritable in both shipping layouts (signed/translocated `.app`, `Program Files`). The env
    /// override wins on every platform — the hook a packaged or scripted run needs so it cannot
    /// touch the developer's real settings — and a missing platform home yields `None` rather than
    /// a guessed path: the caller reports `NoLocation` and runs on defaults instead of writing
    /// somewhere arbitrary.
    #[test]
    fn platform_config_dirs_follow_convention() {
        for os in ["macos", "windows", "linux"] {
            let got = config_dir_from(
                os,
                Some(Path::new("/scratch/cfg")),
                Some(Path::new("/home/y")),
                None,
                None,
            );
            assert_eq!(got, Some(PathBuf::from("/scratch/cfg")), "os {os}");
            assert_eq!(config_dir_from(os, None, None, None, None), None, "os {os}");
        }
        assert_eq!(
            config_dir_from("macos", None, Some(Path::new("/Users/yan")), None, None),
            Some(PathBuf::from(
                "/Users/yan/Library/Application Support/Overmatch"
            )),
        );
        assert_eq!(
            config_dir_from(
                "windows",
                None,
                None,
                Some(Path::new("C:\\Users\\yan\\AppData\\Roaming")),
                None
            ),
            Some(PathBuf::from("C:\\Users\\yan\\AppData\\Roaming").join("Overmatch")),
        );
        assert_eq!(
            config_dir_from("linux", None, Some(Path::new("/home/yan")), None, None),
            Some(PathBuf::from("/home/yan/.config/overmatch")),
        );
        assert_eq!(
            config_dir_from(
                "linux",
                None,
                Some(Path::new("/home/yan")),
                None,
                Some(Path::new("/xdg"))
            ),
            Some(PathBuf::from("/xdg/overmatch")),
            "XDG_CONFIG_HOME wins over ~/.config when set",
        );
        assert_eq!(
            config_dir_from(
                "linux",
                None,
                Some(Path::new("/home/yan")),
                None,
                Some(Path::new(""))
            ),
            Some(PathBuf::from("/home/yan/.config/overmatch")),
            "an EMPTY XDG_CONFIG_HOME is unset, not a root-relative path",
        );
    }

    /// **`struct_names` must stay false.** RON 0.12 ENFORCES a struct name when one is present, so
    /// emitting names would break `VersionProbe` (and any future snapshot struct) with
    /// ``Expected struct `X` but found `Y` ``. The assertion is on the real serializer config,
    /// and the round-trip below is the behaviour it protects.
    #[test]
    fn serializer_is_anonymous_and_newline_pinned() {
        let text = ron::ser::to_string_pretty(&Settings::default(), pretty_config()).unwrap();
        assert!(
            !text.contains("Settings("),
            "struct_names must stay false or the version probe cannot read our own files: {text}"
        );
        assert!(
            !text.contains('\r'),
            "new_line must be pinned to \\n so Windows and macOS write byte-identical files: {text:?}"
        );
        // The property that actually depends on it.
        assert!(ron::de::from_str::<VersionProbe>(&text).is_ok());
    }

    /// Sparse saves must omit defaults so future defaults can reach existing players, while always
    /// writing version (otherwise a default file probes as 0). See [`Settings`] for the rationale.
    /// Assert exactly one key, not an absent-key list: a new field missing skip_serializing_if must
    /// fail immediately rather than silently freeze its default.
    #[test]
    fn defaults_are_not_written_but_the_version_always_is() {
        // `pretty_config` puts one `key: value,` per line, so the keys are the text left of the
        // first colon on every line that has one — the `(` and `)` lines have none.
        fn keys_of(settings: &Settings) -> Vec<String> {
            let text = ron::ser::to_string_pretty(settings, pretty_config()).unwrap();
            text.lines()
                .filter_map(|line| line.split_once(':'))
                .map(|(key, _)| key.trim().to_string())
                .collect()
        }

        assert_eq!(
            keys_of(&Settings::default()),
            ["version"],
            "a fully-default file is the version stamp and NOTHING else",
        );
        // Non-default shadow rows serialize independently.
        assert_eq!(
            keys_of(&Settings {
                shadow_distance: ShadowDistance::M1000,
                ..Settings::default()
            }),
            ["version", "shadow_distance"],
        );
        assert_eq!(
            keys_of(&Settings {
                shadow_cascades: ShadowCascades::Two,
                ..Settings::default()
            }),
            ["version", "shadow_cascades"],
            "the cascade row diffs sparsely like every other field — the default 4 writes nothing",
        );
    }

    /// The load report must identify and refuse a nonfinite budget, preserving the player's file;
    /// otherwise the page's whole-Settings equality guards can amplify writes. Finite off-ladder
    /// edits remain valid and exact.
    #[test]
    fn a_non_finite_budget_cannot_survive_the_parse() {
        for value in ["NaN", "inf", "-inf"] {
            let file = format!("(version: {SETTINGS_VERSION}, lod_pixel_budget: {value})");
            let Parsed::Corrupt(error) = parse(&file) else {
                panic!("{file} must refuse a nonfinite budget");
            };
            assert!(error.contains("lod_pixel_budget"), "{error}");
            assert!(error.contains("finite"), "{error}");

            let scratch = ScratchDir::new("nonfinite");
            std::fs::create_dir_all(&scratch.dir).unwrap();
            let path = scratch.file();
            std::fs::write(&path, &file).unwrap();
            let loaded = load(Some(&path));
            let LoadNote::Corrupt { error, kept } = loaded.note else {
                panic!("a nonfinite budget must reach the visible corrupt-file report");
            };
            assert!(error.contains("lod_pixel_budget"), "{error}");
            assert_eq!(loaded.settings, Settings::default());
            assert_eq!(std::fs::read_to_string(kept.unwrap()).unwrap(), file);
        }
        let Parsed::Ok(edited) = parse("(version: 1, lod_pixel_budget: 1.2)") else {
            panic!("a finite off-ladder value must parse");
        };
        assert_eq!(edited.lod_pixel_budget, PixelBudget(1.2));
    }

    /// Retired shadow tokens migrate through a whole file to the nearest surviving rung in ORDER.
    /// Off-ladder values would step to Off; other fields must survive. The migration is lossy and
    /// one-way: the next save writes no retired token. Meaning under a surviving name is unchanged,
    /// so [`SETTINGS_VERSION`] does not move.
    #[test]
    fn the_retired_shadow_distance_rungs_load_and_migrate() {
        for retired in ["M100", "M150", "M300"] {
            let text = format!("(version: 1, shadow_distance: {retired}, msaa: X2)");
            let Parsed::Ok(settings) = parse(&text) else {
                panic!("{text} must still load — it is on players' disks");
            };
            assert_eq!(
                settings.shadow_distance,
                ShadowDistance::M350,
                "{retired} must land on the nearest surviving rung",
            );
            assert!(
                ShadowDistance::ORDER.contains(&settings.shadow_distance),
                "{retired} migrated to an off-ladder value, which `step_in` would resolve to Off",
            );
            assert_eq!(
                settings.msaa,
                MsaaLevel::X2,
                "{retired} must not disturb the rest of the file",
            );

            // Round trip: what the next save writes must re-read as the migrated value, with no
            // trace of the retired token left behind.
            let written = ron::ser::to_string_pretty(&settings, pretty_config()).unwrap();
            assert!(
                !written.contains(retired),
                "a retired token must never be written back: {written}",
            );
            assert_eq!(parse(&written), Parsed::Ok(settings), "{written}");
        }
    }

    /// Round-trip through the real serializer the save path uses, including the sparse form.
    #[test]
    fn settings_round_trip_through_ron() {
        for original in [
            Settings::default(),
            Settings {
                version: SETTINGS_VERSION,
                shadow_distance: ShadowDistance::Off,
                // Non-default, so the round-trip exercises a written key.
                shadow_resolution: ShadowResolution::X1024,
                shadow_cascades: ShadowCascades::Two,
                msaa: MsaaLevel::Off,
                bloom: BloomLevel::Low,
                vsync: VsyncMode::Off,
                legacy_vsync: None,
                render_scale: RenderScaleLevel::Percent67,
                window_mode: WindowModeSetting::Fullscreen,
                display: DisplaySelection::Display2,
                lod_pixel_budget: PixelBudget(2.5),
                frame_cap: FrameCap(120),
                ui_scale: UiScalePercent(125),
                debug_panel: DebugPanel::Off,
                pen_cam_corner: PenCamCorner::ThroughOnly,
                kill_cam: KillCam::Off,
                reduce_motion: ReduceMotion::On,
                track_prints: TrackPrintRelief::Lighting,
                track_print_range: TrackPrintRange::Near,
            },
            // The FAST rung — the value the retired bool field could not carry, i.e. the whole
            // reason the key moved.
            Settings {
                vsync: VsyncMode::Fast,
                ..Settings::default()
            },
        ] {
            let text = ron::ser::to_string_pretty(&original, pretty_config()).unwrap();
            assert_eq!(parse(&text), Parsed::Ok(original), "{text}");
        }
    }

    /// Old settings files must select SUN SHADOW at 30 m, and every relief/range row choice must
    /// survive the persistence seam. Comparing only with Settings::default would silently approve
    /// a changed default, while checking only one non-default misses enum tokens a player can save.
    #[test]
    fn track_print_settings_keep_the_fixed_defaults_and_every_saved_choice() {
        let Parsed::Ok(loaded) = parse("()") else {
            panic!("an older file without print settings loads");
        };
        assert_eq!(loaded.track_prints, TrackPrintRelief::SunShadow);
        assert_eq!(loaded.track_print_range.metres(), 30.0);
        for relief in TrackPrintRelief::ORDER {
            for range in TrackPrintRange::ORDER {
                let settings = Settings {
                    track_prints: relief,
                    track_print_range: range,
                    ..Settings::default()
                };
                let text = ron::ser::to_string_pretty(&settings, pretty_config()).unwrap();
                assert_eq!(parse(&text), Parsed::Ok(settings), "{text}");
            }
        }
    }

    /// Missing keys default; unknown keys are skipped. New fields must preserve this compatibility.
    #[test]
    fn unknown_and_missing_fields_both_load_cleanly() {
        let Parsed::Ok(missing) = parse("(shadow_distance: M700)") else {
            panic!("an old file must load");
        };
        assert_eq!(missing.shadow_distance, ShadowDistance::M700);
        assert_eq!(
            missing.msaa,
            MsaaLevel::default(),
            "an absent key must take its default, not fail the load"
        );
        assert_eq!(
            missing.vsync,
            VsyncMode::default(),
            "absent vsync takes the default"
        );

        // Unknown keys a future build could write.
        let Parsed::Ok(unknown) =
            parse("(shadow_distance: M350, film_grain: On, motion_blur: 2, hdr_output: true)")
        else {
            panic!("keys this build does not know must be skipped, not abort the parse");
        };
        assert_eq!(unknown.shadow_distance, ShadowDistance::M350);

        // Files predating the display fields load their defaults.
        let Parsed::Ok(pre_v2) = parse("(version: 1, msaa: X2)") else {
            panic!("a file from before the V2 display fields must load");
        };
        assert_eq!(pre_v2.window_mode, WindowModeSetting::default());
        assert_eq!(pre_v2.frame_cap, FrameCap::OFF);
        assert_eq!(pre_v2.ui_scale, UiScalePercent::default());

        // Files predating render scale load native.
        let Parsed::Ok(pre_render_scale) = parse("(version: 1, msaa: X2)") else {
            panic!("a file from before the render-scale row must load");
        };
        assert_eq!(
            pre_render_scale.render_scale,
            RenderScaleLevel::default(),
            "an absent render_scale must take the native default, not a rung a player never chose"
        );
    }

    /// Keep the shipped version-1 bytes unchanged; add a fixture per shipped version. Known keys
    /// must survive, while removed `shadows` leaves both successors at defaults rather than guessing
    /// what Low meant. Removal needs no version bump: meaning under a surviving name is unchanged.
    #[test]
    fn version_1_fixture_still_loads() {
        const V1: &str =
            "(\n    version: 1,\n    shadows: Low,\n    msaa: X2,\n    vsync: false,\n)\n";
        let Parsed::Ok(settings) = parse(V1) else {
            panic!("the version-1 fixture must keep loading");
        };
        assert_eq!(settings.msaa, MsaaLevel::X2);
        assert_eq!(settings.vsync, VsyncMode::Off);
        assert_eq!(
            settings.legacy_vsync, None,
            "the legacy bool must be absorbed inside the parse, never handed onward"
        );
        assert_eq!(
            settings.shadow_distance,
            ShadowDistance::default(),
            "a retired key must leave its successor at the default, never abort the load"
        );
        assert_eq!(settings.shadow_resolution, ShadowResolution::default());
    }

    /// The vsync key migration THROUGH the real parse path: old key alone, new key alone, and the
    /// hand-edited both-keys file. The write side moves too: a rung is written under `vsync_mode`,
    /// never under the retired `vsync`.
    #[test]
    fn the_vsync_key_migration_reads_old_new_and_both() {
        let written = ron::ser::to_string(&Settings {
            vsync: VsyncMode::Off,
            ..Settings::default()
        })
        .expect("settings serialize");
        assert!(
            written.contains("vsync_mode:Off"),
            "the off rung must write the new key: {written}",
        );
        assert!(
            !written.contains("vsync:"),
            "the retired bool key must never be written again: {written}",
        );
        let vsync_of = |text: &str| match parse(text) {
            Parsed::Ok(settings) => settings.vsync,
            other => panic!("{text} must parse: {other:?}"),
        };
        assert_eq!(vsync_of("(vsync: false)"), VsyncMode::Off);
        assert_eq!(vsync_of("(vsync: true)"), VsyncMode::On);
        assert_eq!(vsync_of("(vsync_mode: Fast)"), VsyncMode::Fast);
        assert_eq!(vsync_of("(vsync_mode: Off)"), VsyncMode::Off);
        assert_eq!(
            vsync_of("(vsync: false, vsync_mode: Fast)"),
            VsyncMode::Fast,
            "a non-default new key is the more specific statement and wins",
        );
        assert_eq!(
            vsync_of("(vsync: false, vsync_mode: On)"),
            VsyncMode::Off,
            "a DEFAULT new key is indistinguishable from an absent one, so the legacy bool is \
             honoured — the documented edge of the migration",
        );
    }

    /// The refuse-newer gate must fire even when the future file's SHAPE changed — which is the
    /// whole reason the version is probed before the schema. Without the probe this reports
    /// "corrupt", i.e. the gate fails exactly where it exists.
    #[test]
    fn a_future_file_is_refused_even_when_its_shape_changed() {
        assert_eq!(
            parse("(version: 9999, shadow_distance: M100)"),
            Parsed::FromTheFuture(9999)
        );
        assert_eq!(
            parse("(version: 2, shadow_distance: (kind: Volumetric, steps: 8), new_thing: [1, 2])"),
            Parsed::FromTheFuture(2),
            "a restructured future file must read as FROM THE FUTURE, not as corrupt",
        );
        // A missing version is 0 — an ancient file, not an error.
        assert!(matches!(parse("(shadow_distance: M100)"), Parsed::Ok(_)));
    }

    /// A scratch config directory, removed on drop. The tests hand [`ScratchDir::file`] to the store
    /// directly, so nothing here touches the process environment.
    struct ScratchDir {
        dir: PathBuf,
    }

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "overmatch-settings-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            Self { dir }
        }

        /// The `video.ron` inside this directory: where [`location`] would point with the
        /// directory as the override.
        fn file(&self) -> PathBuf {
            config_dir_from(std::env::consts::OS, Some(&self.dir), None, None, None)
                .expect("the override always resolves")
                .join(CONFIG_FILE_NAME)
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    /// The whole seam against a real filesystem: first launch, save, reload, and no temp left
    /// behind. The temp check is the atomic-write contract's observable half.
    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let scratch = ScratchDir::new("roundtrip");
        let path = scratch.file();
        assert_eq!(
            load(Some(&path)).note,
            LoadNote::FirstLaunch,
            "an empty directory is a first launch, not an error"
        );
        let wanted = Settings {
            version: SETTINGS_VERSION,
            // Every shadow row deliberately off its default, so the reload proves a WRITTEN key came
            // back rather than an absent one landing on the same value. `Three` used to serve that
            // purpose here; it became the default on 2026-07-28, hence `Two`.
            shadow_distance: ShadowDistance::M350,
            shadow_resolution: ShadowResolution::X1024,
            shadow_cascades: ShadowCascades::Two,
            msaa: MsaaLevel::X2,
            bloom: BloomLevel::Low,
            vsync: VsyncMode::Fast,
            legacy_vsync: None,
            render_scale: RenderScaleLevel::Percent75,
            window_mode: WindowModeSetting::Fullscreen,
            display: DisplaySelection::Display1,
            lod_pixel_budget: PixelBudget(3.0),
            frame_cap: FrameCap(60),
            ui_scale: UiScalePercent(110),
            debug_panel: DebugPanel::Off,
            pen_cam_corner: PenCamCorner::Off,
            kill_cam: KillCam::Off,
            reduce_motion: ReduceMotion::On,
            track_prints: TrackPrintRelief::Parallax,
            track_print_range: TrackPrintRange::Farthest,
        };
        assert!(save(Some(&path), &wanted), "the save must land");
        let reloaded = load(Some(&path));
        assert_eq!(reloaded.settings, wanted);
        assert_eq!(reloaded.note, LoadNote::Loaded);

        let leftovers: Vec<_> = std::fs::read_dir(&scratch.dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the atomic write must leave no temporary behind: {leftovers:?}"
        );
    }

    /// A corrupt file is PRESERVED as `.bad` rather than destroyed by the next save — the only
    /// evidence of what went wrong. Firefox's `Invalidprefs.js` policy.
    #[test]
    fn a_corrupt_file_is_kept_aside_and_survives_the_next_save() {
        let scratch = ScratchDir::new("corrupt");
        std::fs::create_dir_all(&scratch.dir).unwrap();
        let path = scratch.file();
        std::fs::write(&path, "not ron at all {{{").unwrap();

        let loaded = load(Some(&path));
        assert_eq!(loaded.settings, Settings::default());
        let LoadNote::Corrupt { kept, .. } = loaded.note else {
            panic!("a damaged file must report Corrupt");
        };
        let kept = kept.expect("the damaged file must be preserved");
        assert!(kept.exists());
        assert!(!path.exists(), "the damaged file is moved, not copied");

        // And the next save cannot destroy the evidence.
        assert!(save(Some(&path), &Settings::default()));
        assert!(kept.exists(), "the .bad copy must survive a save");
    }

    /// A file from a NEWER build is moved aside before we fall back, so the player's next change
    /// cannot silently overwrite their newer settings with this build's older shape.
    #[test]
    fn a_future_file_is_kept_aside_before_falling_back() {
        let scratch = ScratchDir::new("future");
        std::fs::create_dir_all(&scratch.dir).unwrap();
        let path = scratch.file();
        std::fs::write(&path, "(version: 9999, shadow_distance: M100)").unwrap();

        let loaded = load(Some(&path));
        assert_eq!(loaded.settings, Settings::default());
        let LoadNote::FutureVersion { version, kept } = loaded.note else {
            panic!("a newer file must report FutureVersion");
        };
        assert_eq!(version, 9999);
        let kept = kept.expect("the newer file must be preserved");
        assert!(kept.exists());
        assert!(
            kept.to_string_lossy().ends_with(".newer"),
            "kept at {}",
            kept.display()
        );
    }
}
