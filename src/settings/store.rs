//! **The persistence seam.** Everything about WHERE settings live, in WHAT format, how they are
//! written safely, and how tolerant reading them is, lives behind this module's three functions —
//! [`load`], [`save`], [`location`]. Nothing above it (the [`Settings`] model, the reconcilers, the
//! settings page) names a path, a file format, or a serializer.
//!
//! Everything here is applied from, and justified in, the research brief at
//! `.agents/docs/design/display/persistence-brief-2026-07.md` (source-verified against bevy 0.19.0,
//! ron 0.12, serde 1.0.229, and Apple/Microsoft primary docs). The brief is the reference; this doc
//! records only what the code does and the few rules a future editor must not break.
//!
//! # The file is MACHINE-LOCAL, and its name says so
//!
//! `video.ron`, not `settings.ron`. The split axis for config files is **machine-local vs
//! portable**, not settings vs stats: video/GPU/monitor facts must never roam or cloud-sync, while
//! keybindings must follow the player to any machine. Valve's Auto-Cloud docs say it outright
//! ("avoid machine-specific configurations such as video settings"), and Tunnet — the flagship
//! shipped commercial Bevy game — had to HOTFIX exactly this, moving its video settings into a
//! separate file the cloud ignores.
//!
//! One config directory, split by FILENAME, so a future Steam Cloud manifest can include/exclude by
//! name. Every field in [`Settings`] today — the render ladders and the V2 display fields (window
//! mode, frame cap, UI scale) alike — is machine-local and belongs here.
//!
//! **Binding for whoever adds keybindings: they go in a NEW `controls.ron`, never in this file.**
//! And the on-disk key type must be OUR OWN enum, never `bevy::input::keyboard::KeyCode` — bevy
//! 0.12→0.13 renamed the variants (`KeyCode::W` → `KeyCode::KeyW`, `Up` → `ArrowUp`) and broke
//! deserialization in every player's file. General rule: **on-disk types contain only primitives,
//! `String`, and types we define.** Use `BTreeMap` (a `HashMap`'s randomly-seeded hasher writes
//! different bytes every save) and per-action `Option<Key>` so one bad binding degrades to one
//! reverted action rather than a full reset.
//!
//! # Why hand-rolled, settled
//!
//! Verified at the v0.19.0 tag: bevy's own first-party `bevy_settings` writes to macOS's
//! `~/Library/Preferences`, which Apple's File System Programming Guide explicitly forbids
//! self-written files in; destroys unknown keys and comments on every save; reverts a bad field
//! silently with no log; and has no version field at all. `bevy-persistent`'s native write is
//! truncate-in-place, NOT atomic. `bevy_pkv` silently falls back to writing its database into the
//! CWD when directory resolution fails. Tunnet hand-rolls exactly this shape. See brief §1.

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

/// Reads ONLY the version, before the schema. The whole point is that it cannot fail on a file whose
/// SHAPE changed: a missing `version` defaults to 0, and every other key is ignored.
///
/// Without this the refuse-newer gate cannot fire where it matters — a future file that renamed or
/// restructured a field fails the full parse first and gets reported as "corrupt", which is exactly
/// the case the version number exists to distinguish. Godot (`project_settings.cpp` reads
/// `config_version` during the streaming parse) and Minecraft (`Options.java` probes out of an
/// untyped bag, `catch (RuntimeException) ⇒ 0`) both do precisely this.
#[derive(Deserialize)]
struct VersionProbe {
    #[serde(default)]
    version: u32,
}

/// What happened while reading the settings, as DATA rather than a log line.
///
/// The distinction is load-bearing, not stylistic: [`load`] runs BEFORE `DefaultPlugins`, because
/// the present mode has to be known when the primary `Window` is described — and that is also
/// before `LogPlugin` installs a tracing subscriber, so anything logged there goes nowhere. (MEASURED
/// 2026-07-27: the first cut of this module logged directly from the boot read and produced not one
/// line in a real `--offline` launch.) So the outcome is carried as a value and reported by
/// `settings::report_store_load` once the app is up.
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

/// Resolve the config directory from explicit inputs — **pure**, so the platform rule is unit
/// testable on any OS. Mirrors `assets::asset_root_from`, and for the same reason: the last
/// path-resolution bug in this repo survived because the rule lived only inside `std::env` reads.
///
/// Precedence: [`CONFIG_DIR_ENV`] wins outright, then the platform convention:
/// - macOS: `$HOME/Library/Application Support/Overmatch`
/// - Windows: `%APPDATA%\Overmatch`
/// - Linux/other: `$XDG_CONFIG_HOME/overmatch`, else `$HOME/.config/overmatch`
///
/// Deliberately NOT beside the executable, and the reasons are stronger than "discouraged":
/// - a signed macOS `.app` is read-only by Apple's own rule (TN2206: *"Bundles should be treated as
///   read-only once they have been signed"*), and `scripts/package-macos.sh` signs and notarizes;
/// - macOS **App Translocation** additionally runs a quarantined, never-Finder-moved `.app` from a
///   read-only randomized mount whose path CHANGES every launch — exactly the GitHub-release
///   `.dmg`/`.zip` path;
/// - a 64-bit Windows game under `Program Files` gets `ACCESS_DENIED` with no fallback (UAC's
///   VirtualStore redirection is disabled for 64-bit and manifested apps).
///
/// The asset root can be exe-relative because it is READ-ONLY; anything written cannot.
///
/// Returns `None` when the platform's home/appdata variable is missing — the caller then runs on
/// defaults and reports [`LoadNote::NoLocation`], rather than inventing a path.
fn config_dir_from(
    target_os: &str,
    env_override: Option<&str>,
    home: Option<&str>,
    app_data: Option<&str>,
    xdg_config_home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(dir) = env_override {
        return Some(PathBuf::from(dir));
    }
    match target_os {
        "macos" => Some(
            Path::new(home?)
                .join("Library")
                .join("Application Support")
                .join(CONFIG_DIR_NAME),
        ),
        "windows" => Some(Path::new(app_data?).join(CONFIG_DIR_NAME)),
        _ => {
            let base = match xdg_config_home {
                Some(xdg) if !xdg.is_empty() => PathBuf::from(xdg),
                _ => Path::new(home?).join(".config"),
            };
            // Lower-case on Linux: `~/.config` is conventionally lower-case, unlike the two
            // platforms above whose directories are user-visible and title-cased. (XDG also puts
            // CONFIG here and DATA in `~/.local/share` — saves, if they ever exist, belong there.)
            Some(base.join(CONFIG_DIR_NAME.to_ascii_lowercase()))
        }
    }
}

/// The `video.ron` path for this process, or `None` when the platform home is unreadable.
/// Exposed so the report line can name the file the player would edit.
pub(super) fn location() -> Option<PathBuf> {
    config_dir_from(
        std::env::consts::OS,
        std::env::var(CONFIG_DIR_ENV).ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
        std::env::var("APPDATA").ok().as_deref(),
        std::env::var("XDG_CONFIG_HOME").ok().as_deref(),
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

/// Read the settings, or return defaults. Logs NOTHING — see [`LoadNote`].
pub(super) fn load() -> Load {
    let Some(path) = location() else {
        return Load {
            settings: Settings::default(),
            note: LoadNote::NoLocation,
        };
    };
    let text = match std::fs::read_to_string(&path) {
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
                kept: keep_aside(&path, "newer"),
            },
        },
        Parsed::Corrupt(error) => Load {
            settings: Settings::default(),
            note: LoadNote::Corrupt {
                error,
                kept: keep_aside(&path, "bad"),
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

/// Write the settings ATOMICALLY. Returns whether the write landed.
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
pub(super) fn save(settings: &Settings) -> bool {
    use std::io::Write;

    use bevy::prelude::{error, info, warn};

    let Some(path) = location() else {
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
    if let Err(err) = rename_with_retry(&temporary, &path) {
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

/// Delete the settings file — the `--reset-display` escape hatch's implementation.
///
/// The failure it exists for: a persisted display choice that makes the game unlaunchable on THIS
/// machine (bevy panics on a `Fullscreen` whose monitor is unresolvable, so a config written while a
/// second display was attached can brick boot after it is unplugged). A player in that state cannot
/// reach the settings page to undo it, so the escape has to be outside the app's own UI.
///
/// Deleting rather than rewriting defaults keeps this honest: the next launch takes the ordinary
/// first-launch path, which is already proven.
pub(super) fn reset() -> bool {
    use bevy::prelude::{info, warn};

    let Some(path) = location() else {
        warn!("settings: no platform config directory — nothing to reset");
        return false;
    };
    match std::fs::remove_file(&path) {
        Ok(()) => {
            info!("settings: reset — deleted {}", path.display());
            true
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            info!(
                "settings: nothing to reset ({} does not exist)",
                path.display()
            );
            true
        }
        Err(err) => {
            warn!("settings: cannot delete {} ({err})", path.display());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{
        DisplaySelection, FrameCap, MsaaLevel, PixelBudget, RenderScaleLevel, ShadowCascades,
        ShadowDistance, ShadowResolution, UiScalePercent, VsyncMode, WindowModeSetting,
    };

    /// The env override wins on every platform — the hook a test or a packaged run needs so it
    /// cannot touch the developer's real settings.
    #[test]
    fn config_env_override_wins_everywhere() {
        for os in ["macos", "windows", "linux"] {
            let got = config_dir_from(os, Some("/scratch/cfg"), Some("/home/y"), None, None);
            assert_eq!(got, Some(PathBuf::from("/scratch/cfg")), "os {os}");
        }
    }

    /// Each platform lands on its own convention — and NEVER beside the executable, which is
    /// unwritable in both shipping layouts (signed/translocated `.app`, `Program Files`).
    #[test]
    fn platform_config_dirs_follow_convention() {
        assert_eq!(
            config_dir_from("macos", None, Some("/Users/yan"), None, None),
            Some(PathBuf::from(
                "/Users/yan/Library/Application Support/Overmatch"
            )),
        );
        assert_eq!(
            config_dir_from(
                "windows",
                None,
                None,
                Some("C:\\Users\\yan\\AppData\\Roaming"),
                None
            ),
            Some(PathBuf::from("C:\\Users\\yan\\AppData\\Roaming").join("Overmatch")),
        );
        assert_eq!(
            config_dir_from("linux", None, Some("/home/yan"), None, None),
            Some(PathBuf::from("/home/yan/.config/overmatch")),
        );
        assert_eq!(
            config_dir_from("linux", None, Some("/home/yan"), None, Some("/xdg")),
            Some(PathBuf::from("/xdg/overmatch")),
            "XDG_CONFIG_HOME wins over ~/.config when set",
        );
        assert_eq!(
            config_dir_from("linux", None, Some("/home/yan"), None, Some("")),
            Some(PathBuf::from("/home/yan/.config/overmatch")),
            "an EMPTY XDG_CONFIG_HOME is unset, not a root-relative path",
        );
    }

    /// A missing platform home yields `None` rather than a guessed path — the caller reports
    /// `NoLocation` and runs on defaults instead of writing somewhere arbitrary.
    #[test]
    fn missing_home_yields_no_path() {
        assert_eq!(config_dir_from("macos", None, None, None, None), None);
        assert_eq!(config_dir_from("windows", None, None, None, None), None);
        assert_eq!(config_dir_from("linux", None, None, None, None), None);
    }

    /// The file is named for what it IS — machine-local video state — so a Steam Cloud manifest can
    /// exclude it by name without a migration. Tunnet had to ship that migration; we should not.
    #[test]
    fn the_config_file_is_named_machine_local() {
        assert_eq!(CONFIG_FILE_NAME, "video.ron");
        assert_ne!(
            CONFIG_FILE_NAME, "settings.ron",
            "a generic name cannot be include/excluded from cloud sync by filename"
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

    /// **The sparse-write rule** (the highest-value item in the brief): a value equal to the current
    /// default is NOT written, so a default can still be changed for existing players later. Once
    /// `shadow_distance: M150` is on disk it is indistinguishable from a deliberate choice — that is the
    /// bug `television` had to write a 318-line archaeology system to undo.
    ///
    /// **That "later" arrived on 2026-07-28**, when all three shadow defaults moved off measurement.
    /// Every player who never opened the page picked the new picture up for free, and the only files
    /// that had to be reasoned about at all were the ones carrying a DELIBERATE choice — which is
    /// exactly the property this test exists to keep. It is the rule's first real payout, so it is
    /// recorded here rather than left as a hypothetical.
    ///
    /// `version` is the deliberate exception and must ALWAYS be written, or a fully-default file
    /// probes as version 0.
    ///
    /// **Stated as "exactly one key", not as a list of keys that must be absent.** The absent-list
    /// form was FAIL-OPEN: a field added later without a `skip_serializing_if` would not be on the
    /// list, so the test would keep passing while every player's file silently froze that field's
    /// current default. Counting keys makes a new field fail here the day it lands.
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
        // A non-default value IS written, and only that one — the shadow rows are separate
        // fields and diff separately. (`M1000` rather than `M350`: the probe only has to be a rung
        // that is NOT the default, and `M350` became the default on 2026-07-28.)
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

    /// The cascade-count row's persistence, end to end through the real parse path: every rung
    /// round-trips, the default writes no key (so today's 4 can still be changed for existing
    /// players later), and a file from before the row existed loads at the default.
    #[test]
    fn the_shadow_cascades_key_round_trips_and_is_absent_by_default() {
        for cascades in ShadowCascades::ORDER {
            let original = Settings {
                shadow_cascades: cascades,
                ..Settings::default()
            };
            let text = ron::ser::to_string_pretty(&original, pretty_config()).unwrap();
            assert_eq!(parse(&text), Parsed::Ok(original), "{text}");
            assert_eq!(
                text.contains("shadow_cascades"),
                cascades != ShadowCascades::default(),
                "only a non-default count may reach the disk: {text}",
            );
        }
        // The bare key, as a player's hand-edit would write it.
        let Parsed::Ok(edited) = parse("(version: 1, shadow_cascades: Two)") else {
            panic!("the bare key must parse");
        };
        assert_eq!(edited.shadow_cascades, ShadowCascades::Two);
        // A file from before the row existed: the standard absent-key path, at the default. The
        // `M300` is deliberately a REAL such file's contents — a retired rung — so this also crosses
        // the distance-ladder migration.
        let Parsed::Ok(pre_row) = parse("(version: 1, shadow_distance: M300)") else {
            panic!("a file from before the cascade row must load");
        };
        assert_eq!(pre_row.shadow_cascades, ShadowCascades::default());
        assert_eq!(pre_row.shadow_distance, ShadowDistance::M350);
    }

    /// **The two 2026-08-01 rows, end to end through the real parse path.** Both landed on the
    /// module doc's free path (a `Default`, a `skip_serializing_if`, a row in `ui::Row::ORDER`), and
    /// this is what "free" means at the level that matters: every rung round-trips, the default
    /// writes no key, and a file from before either row existed loads at the default.
    ///
    /// The bare-key half is not ceremony — it is the SUPPORTED INTERFACE for the frame sweep.
    /// `scripts/perf/run-frame-sweep.sh` generates a two-key `video.ron` per condition and relies on
    /// serde treating every omitted field as its default; pinning the display key alongside it is
    /// what makes "pin the window to a monitor from the file alone" a tested claim rather than a
    /// hope.
    #[test]
    fn the_display_and_detail_keys_round_trip_and_are_absent_by_default() {
        for display in DisplaySelection::ORDER {
            let original = Settings {
                display,
                ..Settings::default()
            };
            let text = ron::ser::to_string_pretty(&original, pretty_config()).unwrap();
            assert_eq!(parse(&text), Parsed::Ok(original), "{text}");
            assert_eq!(
                text.contains("display"),
                display != DisplaySelection::default(),
                "only a non-default display may reach the disk: {text}",
            );
        }
        for budget in [PixelBudget::MIN, 1.0, 2.5, PixelBudget::MAX] {
            let original = Settings {
                lod_pixel_budget: PixelBudget(budget),
                ..Settings::default()
            };
            let text = ron::ser::to_string_pretty(&original, pretty_config()).unwrap();
            assert_eq!(parse(&text), Parsed::Ok(original), "{text}");
            assert_eq!(
                text.contains("lod_pixel_budget"),
                PixelBudget(budget) != PixelBudget::default(),
                "only a non-default budget may reach the disk: {text}",
            );
        }

        // The sweep's own file shape: a version stamp plus exactly the keys the condition cares
        // about, everything else omitted.
        let Parsed::Ok(swept) = parse("(version: 1, vsync_mode: Off, display: Primary)") else {
            panic!("the sweep's generated file must parse");
        };
        assert_eq!(swept.display, DisplaySelection::Primary);
        assert_eq!(swept.vsync, VsyncMode::Off);
        assert_eq!(swept.lod_pixel_budget, PixelBudget::default());

        // The bare keys, as a player's hand-edit would write them — the budget as the transparent
        // float it is on disk, not as a wrapper.
        let Parsed::Ok(edited) = parse("(version: 1, display: Display2, lod_pixel_budget: 2.5)")
        else {
            panic!("the bare keys must parse");
        };
        assert_eq!(edited.display, DisplaySelection::Display2);
        assert_eq!(edited.lod_pixel_budget, PixelBudget(2.5));

        // A file from before either row existed: the standard absent-key path, at the defaults.
        let Parsed::Ok(pre_row) = parse("(version: 1, msaa: X2)") else {
            panic!("a file from before these rows must load");
        };
        assert_eq!(pre_row.display, DisplaySelection::default());
        assert_eq!(pre_row.lod_pixel_budget, PixelBudget::default());
        assert_eq!(pre_row.lod_pixel_budget.pixels(), 1.0);
    }

    /// **A non-finite budget cannot survive the parse** (Codex, 2026-08-01).
    ///
    /// RON accepts `NaN` and `inf`, and `Settings` derives `PartialEq` — so one `NaN` on disk makes
    /// the settings resource unequal to ITSELF, and the settings page's two no-op guards are
    /// equality tests (`change_row`'s "the step changed nothing" and `scrub`'s "the drag landed
    /// where it already was"). A NaN would turn a held-still drag into a file write every frame and
    /// a saturated keypress on any row into a write per repeat. So it is normalised where it enters
    /// — through the real `parse`, not a unit call on the sanitiser.
    #[test]
    fn a_non_finite_budget_cannot_survive_the_parse() {
        for (text, want) in [
            ("NaN", PixelBudget::default()),
            ("inf", PixelBudget(PixelBudget::MAX)),
            ("-inf", PixelBudget(PixelBudget::MIN)),
        ] {
            let file = format!("(version: 1, lod_pixel_budget: {text})");
            let Parsed::Ok(settings) = parse(&file) else {
                panic!("{file} must parse — a hand-edit is not a corrupt file");
            };
            assert_eq!(settings.lod_pixel_budget, want, "{file}");
            assert!(
                settings.lod_pixel_budget.0.is_finite(),
                "{file} left a non-finite value in the struct",
            );
            // The property that actually matters: the settings resource compares equal to a copy of
            // itself, so every no-op guard on the page still works.
            assert_eq!(
                settings, settings,
                "{file} poisoned whole-settings equality"
            );
        }
        // A finite off-ladder hand-edit is still honoured exactly, which is the behaviour this
        // shares with `FrameCap` — the sanitiser must not quietly become a ladder snap.
        let Parsed::Ok(edited) = parse("(version: 1, lod_pixel_budget: 1.2)") else {
            panic!("a finite off-ladder value must parse");
        };
        assert_eq!(edited.lod_pixel_budget, PixelBudget(1.2));
    }

    /// **The distance-ladder migration, through a whole file** (2026-07-28). The rungs went
    /// `100/150/300` → `350/700/1000`; the retired tokens are `#[serde(alias)]`es on `M350`, so
    /// every `video.ron` a player already has keeps loading, and keeps loading onto a rung the
    /// settings page can actually walk. `settings::tests::the_retired_distance_rungs_migrate_onto_the_ladder`
    /// carries the argument for migrating rather than hiding; this is the same claim at the level
    /// that matters to a player — the file on their disk.
    ///
    /// It is a MIGRATION and not a rename, so it is deliberately lossy and deliberately one-way: all
    /// three old rungs collapse onto the nearest survivor, and the next save writes the new token.
    /// That is within the module doc's policy (a value's meaning is unchanged — 350 m still means
    /// 350 m), so [`SETTINGS_VERSION`] does not move.
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
                // A non-default rung on purpose: `X4096` became the default on 2026-07-28, so it
                // would round-trip through an ABSENT key and stop exercising the written one.
                shadow_resolution: ShadowResolution::X1024,
                shadow_cascades: ShadowCascades::Two,
                msaa: MsaaLevel::Off,
                vsync: VsyncMode::Off,
                legacy_vsync: None,
                render_scale: RenderScaleLevel::Percent67,
                window_mode: WindowModeSetting::Fullscreen,
                display: DisplaySelection::Display2,
                lod_pixel_budget: PixelBudget(2.5),
                frame_cap: FrameCap(120),
                ui_scale: UiScalePercent(125),
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

    /// **The forward/backward-compatibility contract**, which is what lets the research-pending
    /// entries land as plain new fields. A file missing a key loads it at its default; a file
    /// carrying a key this build has never heard of is ignored, not rejected.
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

        // Genuinely unknown keys (the V2 display fields graduated to known ones, so these are the
        // next candidates a future build might write).
        let Parsed::Ok(unknown) =
            parse("(shadow_distance: M350, bloom: On, motion_blur: 2, hdr_output: true)")
        else {
            panic!("keys this build does not know must be skipped, not abort the parse");
        };
        assert_eq!(unknown.shadow_distance, ShadowDistance::M350);

        // The V2 display fields landing as plain new fields is the contract's second exercise: a
        // file from before them loads with every one at its default.
        let Parsed::Ok(pre_v2) = parse("(version: 1, msaa: X2)") else {
            panic!("a file from before the V2 display fields must load");
        };
        assert_eq!(pre_v2.window_mode, WindowModeSetting::default());
        assert_eq!(pre_v2.frame_cap, FrameCap::OFF);
        assert_eq!(pre_v2.ui_scale, UiScalePercent::default());

        // The render-scale row landing as a plain new field is the contract's first exercise: a
        // file written by the build BEFORE it existed must still load, at native.
        let Parsed::Ok(pre_render_scale) = parse("(version: 1, msaa: X2)") else {
            panic!("a file from before the render-scale row must load");
        };
        assert_eq!(
            pre_render_scale.render_scale,
            RenderScaleLevel::default(),
            "an absent render_scale must take the native default, not a rung a player never chose"
        );
    }

    /// A real file as shipped at version 1 — kept BYTE-FOR-BYTE, because it is what is actually on
    /// players' disks; the fixture will be joined by one per shipped version, so a future schema
    /// change proves it can still read what they have.
    ///
    /// It now also exercises a REMOVED field: `shadows` was one coupled preset until 2026-07-27 and
    /// is two fields today. The module doc's policy says removing a field is free, and this is what
    /// "free" means in practice — the file still loads, every key this build still knows survives,
    /// and the retired key's two successors come up at their defaults rather than at some guess at
    /// what `Low` used to mean. That is the deliberate cost of the removal, not a bug (which is also
    /// why [`SETTINGS_VERSION`] did not move: nothing changed MEANING under the same name).
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

    /// The vsync key migration THROUGH the real parse path (the pure absorb rule is pinned in
    /// `settings::tests`): old key alone, new key alone, and the hand-edited both-keys file.
    #[test]
    fn the_vsync_key_migration_reads_old_new_and_both() {
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

    /// Genuine syntax damage is corrupt, and says why.
    #[test]
    fn a_damaged_file_is_corrupt() {
        let Parsed::Corrupt(error) = parse("this is not ron {{{") else {
            panic!("a damaged file must be reported corrupt");
        };
        assert!(!error.is_empty());
    }

    /// A scratch config dir via the same override a packaged/CI run would use. Serialized by
    /// [`ENV_LEASE`] because `set_var` is process-global.
    struct ScratchDir {
        dir: PathBuf,
        previous: Option<String>,
        _lease: std::sync::MutexGuard<'static, ()>,
    }

    /// Serializes every test that writes `OVERMATCH_CONFIG_DIR` — the variable is process-global, so
    /// two of these running at once would each see the other's directory.
    static ENV_LEASE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl ScratchDir {
        fn new(tag: &str) -> Self {
            let lease = ENV_LEASE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "overmatch-settings-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let previous = std::env::var(CONFIG_DIR_ENV).ok();
            // SAFETY: the lease makes this body single-threaded with respect to the variable, and
            // `Drop` restores it.
            unsafe { std::env::set_var(CONFIG_DIR_ENV, &dir) };
            Self {
                dir,
                previous,
                _lease: lease,
            }
        }
    }

    impl Drop for ScratchDir {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
            // SAFETY: as above.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var(CONFIG_DIR_ENV, value),
                    None => std::env::remove_var(CONFIG_DIR_ENV),
                }
            }
        }
    }

    /// The whole seam against a real filesystem: first launch, save, reload, and no temp left
    /// behind. The temp check is the atomic-write contract's observable half.
    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let scratch = ScratchDir::new("roundtrip");
        assert_eq!(
            load().note,
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
            vsync: VsyncMode::Fast,
            legacy_vsync: None,
            render_scale: RenderScaleLevel::Percent75,
            window_mode: WindowModeSetting::Fullscreen,
            display: DisplaySelection::Display1,
            lod_pixel_budget: PixelBudget(3.0),
            frame_cap: FrameCap(60),
            ui_scale: UiScalePercent(110),
        };
        assert!(save(&wanted), "the save must land");
        let reloaded = load();
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
        let path = location().unwrap();
        std::fs::write(&path, "not ron at all {{{").unwrap();

        let loaded = load();
        assert_eq!(loaded.settings, Settings::default());
        let LoadNote::Corrupt { kept, .. } = loaded.note else {
            panic!("a damaged file must report Corrupt");
        };
        let kept = kept.expect("the damaged file must be preserved");
        assert!(kept.exists());
        assert!(!path.exists(), "the damaged file is moved, not copied");

        // And the next save cannot destroy the evidence.
        assert!(save(&Settings::default()));
        assert!(kept.exists(), "the .bad copy must survive a save");
    }

    /// A file from a NEWER build is moved aside before we fall back, so the player's next change
    /// cannot silently overwrite their newer settings with this build's older shape.
    #[test]
    fn a_future_file_is_kept_aside_before_falling_back() {
        let scratch = ScratchDir::new("future");
        std::fs::create_dir_all(&scratch.dir).unwrap();
        let path = location().unwrap();
        std::fs::write(&path, "(version: 9999, shadow_distance: M100)").unwrap();

        let loaded = load();
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

    /// `--reset-display`'s implementation: deletes the file so the next launch takes the ordinary,
    /// already-proven first-launch path. Deleting an absent file is a success, not an error.
    #[test]
    fn reset_deletes_the_file_and_is_idempotent() {
        let scratch = ScratchDir::new("reset");
        std::fs::create_dir_all(&scratch.dir).unwrap();
        assert!(save(&Settings {
            shadow_distance: ShadowDistance::M350,
            ..Settings::default()
        }));
        assert!(location().unwrap().exists());

        assert!(reset());
        assert!(!location().unwrap().exists());
        assert!(reset(), "resetting twice must not be an error");
        assert_eq!(load().note, LoadNote::FirstLaunch);
    }
}
