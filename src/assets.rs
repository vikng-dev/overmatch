//! Where this process reads its `assets/` from — one resolver, shared by the two subsystems that
//! must agree on it: Bevy's `AssetPlugin` (wired in `net::client`) and the tank-geometry bake
//! (`bake::extract_at_startup`). Both open the SAME `tiger_1.glb`; before the rule was shared they
//! diverged on macOS — the bake used Bevy's raw [`FileAssetReader::get_base_path`], which in a
//! double-clicked `.app` (no `BEVY_ASSET_ROOT` / `CARGO_MANIFEST_DIR`) falls through to the exe dir
//! `…/Contents/MacOS`, while the asset server special-cased `…/Contents/Resources/assets` (where the
//! packager actually puts them). The asset server found the meshes; the bake did not, and the client
//! panicked on startup. That was the v0.3.0-alpha.2 macOS `.dmg` crash.
//!
//! [`FileAssetReader::get_base_path`]: bevy::asset::io::file::FileAssetReader::get_base_path
//!
//! Lives at the crate root (not under `net`) because `bake` shares it: it was once a private copy
//! inside `net::client`, unreachable from `bake`, and was lifted here so both subsystems resolve the
//! asset root by the one rule.

use std::path::{Path, PathBuf};

/// Resolve the `assets/` directory from explicit inputs — **pure**, so the resolution rule is unit-
/// testable. The macOS bug survived precisely because the rule lived only inside `std::env` reads
/// and a `current_exe()` call, a shape nothing but a real `.app` ever exercised.
///
/// Precedence mirrors Bevy's own [`FileAssetReader::get_base_path`] (`BEVY_ASSET_ROOT` →
/// `CARGO_MANIFEST_DIR` → the executable's directory) so the bake and the asset server always land
/// on the same tree, then resolves the platform bundle layout for the env-less packaged case:
/// - macOS `.app`: `exe_dir` = `<App>.app/Contents/MacOS` → the sibling `Contents/Resources/assets`
///   (where `scripts/package-macos.sh` copies them — see its `Contents/Resources/assets` line).
/// - flat archive (Windows/Linux): binary and `assets/` extract into one folder → `exe_dir/assets`.
///
/// Deliberately does no `is_dir()` probing and has no silent `"assets"`-in-cwd fallback: the path is
/// resolved deterministically from the layout, and if it is wrong the bake panics naming it
/// (ADR-0011 fail-fast) instead of quietly loading a different tree.
///
/// [`FileAssetReader::get_base_path`]: bevy::asset::io::file::FileAssetReader::get_base_path
fn asset_root_from(exe_dir: &Path, manifest_dir: Option<&str>, env_root: Option<&str>) -> PathBuf {
    // `BEVY_ASSET_ROOT` wins over everything — the dedicated server sets it (DEPLOY.md), and every
    // test/harness run inherits it or `CARGO_MANIFEST_DIR`, which is why the exe-dir path below is
    // only ever taken by a real double-clicked bundle.
    if let Some(root) = env_root {
        return Path::new(root).join("assets");
    }
    // `CARGO_MANIFEST_DIR`: cargo sets it for `cargo run` / `cargo test`, so the dev loop and CI
    // resolve to `<repo>/assets` regardless of the exe's location under `target/`.
    if let Some(dir) = manifest_dir {
        return Path::new(dir).join("assets");
    }
    // No env at all — the packaged, double-clicked case. macOS `.app`: the exe sits in
    // `Contents/MacOS`, but assets ship one level over in `Contents/Resources/assets`.
    if exe_dir.ends_with("Contents/MacOS")
        && let Some(contents) = exe_dir.parent()
    {
        return contents.join("Resources").join("assets");
    }
    // Flat archive: `assets/` is beside the binary.
    exe_dir.join("assets")
}

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
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BEVY_ASSET_ROOT` wins over a set `CARGO_MANIFEST_DIR` and over any exe layout — the
    /// dedicated server relies on this (it sets `BEVY_ASSET_ROOT=/opt/overmatch-server`).
    #[test]
    fn env_root_wins_over_everything() {
        let got = asset_root_from(
            Path::new("/App.app/Contents/MacOS"),
            Some("/repo"),
            Some("/opt/overmatch-server"),
        );
        assert_eq!(got, Path::new("/opt/overmatch-server/assets"));
    }

    /// No `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR` set → `<manifest>/assets`. This is the dev loop
    /// and every `cargo test`/`cargo run`, which is why CI never hit the packaged-app bug.
    #[test]
    fn manifest_dir_when_no_env_root() {
        let got = asset_root_from(Path::new("/repo/target/debug"), Some("/repo"), None);
        assert_eq!(got, Path::new("/repo/assets"));
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

        let got = asset_root_from(&macos, None, None);
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
        let got = asset_root_from(Path::new("/opt/overmatch"), None, None);
        assert_eq!(got, Path::new("/opt/overmatch/assets"));
    }
}
