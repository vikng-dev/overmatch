//! Shared asset-root resolver for Bevy loading and geometry extraction.
//!
//! Invariant: both consumers resolve the same `assets/` tree, including packaged macOS bundles.

use std::path::{Path, PathBuf};

/// Resolve the `assets/` directory from explicit inputs — **pure**, so the resolution rule is unit-
/// testable. The macOS bug survived precisely because the rule lived only inside `std::env` reads
/// and a `current_exe()` call, a shape nothing but a real `.app` ever exercised.
///
/// Precedence extends Bevy's own [`FileAssetReader::get_base_path`] (`BEVY_ASSET_ROOT` →
/// `CARGO_MANIFEST_DIR` → the executable's directory) by one rung, so the bake and the asset server
/// always land on the same tree: env root → runtime manifest dir → `baked_manifest_dir` (the
/// manifest dir a dev-profile build captured at compile time, [`BAKED_MANIFEST_DIR`]) → the platform
/// bundle layout for the env-less packaged case:
/// - macOS `.app`: `exe_dir` = `<App>.app/Contents/MacOS` → the sibling `Contents/Resources/assets`
///   (where `scripts/package-macos.sh` copies them — see its `Contents/Resources/assets` line).
/// - flat archive (Windows/Linux): binary and `assets/` extract into one folder → `exe_dir/assets`.
///
/// Deliberately does no `is_dir()` probing and has no silent `"assets"`-in-cwd fallback: the path is
/// resolved deterministically from the build and the layout, and if it is wrong the bake panics
/// naming it (ADR-0011 fail-fast) instead of quietly loading a different tree.
///
/// [`FileAssetReader::get_base_path`]: bevy::asset::io::file::FileAssetReader::get_base_path
fn asset_root_from(
    exe_dir: &Path,
    manifest_dir: Option<&str>,
    env_root: Option<&str>,
    baked_manifest_dir: Option<&str>,
) -> PathBuf {
    // `BEVY_ASSET_ROOT` wins over everything — the server image sets it
    // (`.github/actions/server-image/Dockerfile`), and every harness script sets it too.
    if let Some(root) = env_root {
        return Path::new(root).join("assets");
    }
    // `CARGO_MANIFEST_DIR`: cargo sets it for `cargo run` / `cargo test`, so the dev loop and CI
    // resolve to `<repo>/assets` regardless of the exe's location under `target/`.
    if let Some(dir) = manifest_dir {
        return Path::new(dir).join("assets");
    }
    // A dev-profile binary launched by hand (`target/debug/overmatch --offline`) has neither env
    // var; its assets are the checkout that compiled it, not a directory beside the exe.
    if let Some(dir) = baked_manifest_dir {
        return Path::new(dir).join("assets");
    }
    // No env and nothing baked — a shipped build. macOS `.app`: the exe sits in `Contents/MacOS`,
    // but assets ship one level over in `Contents/Resources/assets`.
    if exe_dir.ends_with("Contents/MacOS")
        && let Some(contents) = exe_dir.parent()
    {
        return contents.join("Resources").join("assets");
    }
    // Flat archive: `assets/` is beside the binary.
    exe_dir.join("assets")
}

/// The checkout that compiled this binary, kept only by dev profiles. Every shipped binary is a
/// release-derived profile (`scripts/package-macos.sh`, `scripts/build-linux.sh`,
/// `.github/workflows/release.yml` all build `--release`), where `debug_assertions` is off, so a
/// player's binary carries no build-machine path and resolves by layout alone.
const BAKED_MANIFEST_DIR: Option<&str> = if cfg!(debug_assertions) {
    Some(env!("CARGO_MANIFEST_DIR"))
} else {
    None
};

/// THE SELECTION: the id of the one tank this build spawns. The tank's trio lives at
/// `assets/<id>/<id>.{glb,sim.glb,tank.ron,lod.json}`; every path in the program is derived from
/// this id by [`tank_file!`], so choosing another vehicle is this literal and nothing else. It is
/// a macro, not a `const`, because `concat!` takes only literals and the paths are `const`.
macro_rules! tank_id {
    () => {
        "tiger_1"
    };
}

/// `<id>/<id><suffix>` for the selected tank, as the asset server addresses it.
macro_rules! tank_file {
    ($suffix:literal) => {
        concat!(
            $crate::assets::tank_id!(),
            "/",
            $crate::assets::tank_id!(),
            $suffix
        )
    };
}

pub(crate) use {tank_file, tank_id};

/// The selected tank's asset id.
pub(crate) const TANK_ID: &str = tank_id!();

/// The `assets/` directory this process reads from — the thin env-reading wrapper over the pure
/// [`asset_root_from`]. Used both as `AssetPlugin`'s `file_path` (`net::client`) and as the bake
/// extractor's glb base (`bake::extract_at_startup`), so the two never disagree on which file they
/// open. Falls back to `.` for the exe dir only if `current_exe()` itself fails (a case cargo's
/// `CARGO_MANIFEST_DIR` already covers under `cargo run`/`cargo test`).
pub(crate) fn asset_root() -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    asset_root_from(
        &exe_dir,
        std::env::var("CARGO_MANIFEST_DIR").ok().as_deref(),
        std::env::var("BEVY_ASSET_ROOT").ok().as_deref(),
        BAKED_MANIFEST_DIR,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FIXTURE DATA (the shipped tank): the id and the trio paths the program derives. Every
    /// consumer reaches the files only through these, so a wrong suffix or a dropped `<id>/`
    /// directory fails here and not at a missing-file panic.
    #[test]
    fn the_selected_tank_derives_its_trio_paths() {
        assert_eq!(TANK_ID, "tiger_1");
        assert_eq!(tank_file!(".glb"), "tiger_1/tiger_1.glb");
        assert_eq!(tank_file!(".sim.glb"), "tiger_1/tiger_1.sim.glb");
        assert_eq!(tank_file!(".tank.ron"), "tiger_1/tiger_1.tank.ron");
        assert_eq!(tank_file!(".lod.json"), "tiger_1/tiger_1.lod.json");
    }

    /// `BEVY_ASSET_ROOT` wins over a set `CARGO_MANIFEST_DIR`, over a baked manifest dir and over
    /// any exe layout — the dedicated server relies on this (it sets
    /// `BEVY_ASSET_ROOT=/opt/overmatch-server`).
    #[test]
    fn env_root_wins_over_everything() {
        let got = asset_root_from(
            Path::new("/App.app/Contents/MacOS"),
            Some("/repo"),
            Some("/opt/overmatch-server"),
            Some("/build-machine/repo"),
        );
        assert_eq!(got, Path::new("/opt/overmatch-server/assets"));
    }

    /// No `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR` set → `<manifest>/assets`. This is the dev loop
    /// and every `cargo test`/`cargo run`, which is why CI never hit the packaged-app bug.
    #[test]
    fn manifest_dir_when_no_env_root() {
        let got = asset_root_from(Path::new("/repo/target/debug"), Some("/repo"), None, None);
        assert_eq!(got, Path::new("/repo/assets"));
    }

    /// A dev-profile binary launched by hand has no env at all; the baked manifest dir resolves it to
    /// the checkout that compiled it, never to `target/debug/assets`. The runtime env still wins
    /// when set, so `cargo run` from a worktree reads that worktree, not the baked one.
    #[test]
    fn baked_manifest_dir_when_launched_by_hand() {
        let got = asset_root_from(Path::new("/repo/target/debug"), None, None, Some("/repo"));
        assert_eq!(got, Path::new("/repo/assets"));
        assert_ne!(got, Path::new("/repo/target/debug/assets"));

        let got = asset_root_from(
            Path::new("/wt/target/debug"),
            Some("/wt"),
            None,
            Some("/repo"),
        );
        assert_eq!(got, Path::new("/wt/assets"));
    }

    /// THE REGRESSION GUARD for the v0.3.0-alpha.2 startup panic. A double-clicked macOS `.app` has
    /// neither env var; the exe lives in `Contents/MacOS`, and assets ship in the sibling
    /// `Contents/Resources/assets` — NOT `Contents/MacOS/assets`, which is what the buggy bake
    /// resolved and then panicked on. Built as a real on-disk `.app`-shaped tree so the guard runs
    /// on any OS, not just macOS.
    #[test]
    fn macos_app_resolves_to_resources_not_macos_dir() {
        let base = std::env::temp_dir().join(format!(
            "overmatch-assets-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let macos = base.join("Overmatch.app/Contents/MacOS");
        let resources_assets = base.join("Overmatch.app/Contents/Resources/assets");
        std::fs::create_dir_all(&macos).unwrap();
        std::fs::create_dir_all(&resources_assets).unwrap();

        let got = asset_root_from(&macos, None, None, None);
        assert_eq!(got, resources_assets);
        // And explicitly NOT the exe-dir/assets path the buggy resolver produced.
        assert_ne!(got, macos.join("assets"));

        std::fs::remove_dir_all(&base).ok();
    }

    /// Flat layout (Windows/Linux ship the binary and `assets/` in one extracted folder), no env →
    /// `<exe_dir>/assets`. This is why the bug never showed on those platforms: exe-dir/assets is
    /// where the assets actually are.
    #[test]
    fn flat_layout_resolves_beside_exe() {
        let got = asset_root_from(Path::new("/opt/overmatch"), None, None, None);
        assert_eq!(got, Path::new("/opt/overmatch/assets"));
    }
}
