//! Shared asset-root resolver for Bevy loading and geometry extraction.
//!
//! Invariant: both consumers resolve the same `assets/` tree, including packaged macOS bundles.

use std::path::{Path, PathBuf};

/// Resolve the `assets/` directory from explicit inputs — **pure**, so the resolution rule is unit-
/// testable. The macOS bug survived precisely because the rule lived only inside `std::env` reads
/// and a `current_exe()` call, a shape nothing but a real `.app` ever exercised.
///
/// Precedence extends Bevy's own [`FileAssetReader::get_base_path`] (`BEVY_ASSET_ROOT` →
/// `CARGO_MANIFEST_DIR` → the executable's directory) by one rung, and the asset server is handed
/// the result as an absolute path (`crate::default_plugins`), so Bevy's own chain never picks the
/// tree: env root → runtime manifest dir → `baked_manifest_dir` (the
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
    manifest_dir: Option<&Path>,
    env_root: Option<&Path>,
    baked_manifest_dir: Option<&str>,
) -> PathBuf {
    // `BEVY_ASSET_ROOT` wins over everything: it is the only rung that places assets away from a
    // release-derived binary (perf scripts on `target/release`, bitprobe CI, the Windows smoke's
    // server fixture).
    if let Some(root) = env_root {
        return root.join("assets");
    }
    // `CARGO_MANIFEST_DIR`: cargo sets it for `cargo run` / `cargo test`, so the dev loop and CI
    // resolve to `<repo>/assets` regardless of the exe's location under `target/`.
    if let Some(dir) = manifest_dir {
        return dir.join("assets");
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
/// [`asset_root_from`]. Used both as `AssetPlugin`'s `file_path` on every composition root
/// (`crate::default_plugins`) and as the bake extractor's glb base (`bake::extract_at_startup`), so
/// the two never disagree on which file they open. Both env vars are read as OS paths, so a root
/// that is not UTF-8 is still that root, never "unset". A process whose own executable the OS
/// cannot locate refuses to start, naming the OS error and the lever that bypasses the lookup.
pub(crate) fn asset_root() -> PathBuf {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let env_root = std::env::var_os("BEVY_ASSET_ROOT").map(PathBuf::from);
    let exe = std::env::current_exe().unwrap_or_else(|err| {
        panic!(
            "cannot locate this executable to find its assets/ beside it: {err} — set \
             BEVY_ASSET_ROOT to the directory that holds assets/"
        )
    });
    let exe_dir = exe
        .parent()
        .expect("the OS names an executable by a file path, which has a parent directory");
    asset_root_from(
        exe_dir,
        manifest_dir.as_deref(),
        env_root.as_deref(),
        BAKED_MANIFEST_DIR,
    )
}

/// Client-only dependencies collected in `PreStartup`, before tank admission starts in `Startup`.
/// Each view plugin contributes its own handles. A headless composition with no view plugins has
/// no such resource and no presentation dependencies.
#[derive(bevy::prelude::Resource, Default)]
pub(crate) struct PresentationLoads(pub Vec<(String, bevy::asset::UntypedHandle)>);

/// Refuse an authored presentation dependency before play. These paths use the same root as
/// Bevy's file reader; a missing file, an empty export or an unhydrated LFS pointer is an asset
/// fault. The asynchronous loader still checks the format and refuses a failed decode.
pub(crate) fn require_file(root: &Path, path: &str, field: &str) -> Vec<u8> {
    assert!(
        !path.is_empty()
            && Path::new(path)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_))),
        "{field}: {path:?} must be an asset-relative file path — author a path within assets/"
    );
    let resolved = root.join(path);
    let bytes = std::fs::read(&resolved).unwrap_or_else(|err| {
        panic!(
            "{}: {field}: cannot read required asset: {err} — restore the file or correct the authored path",
            resolved.display()
        )
    });
    assert!(
        !bytes.is_empty() && !bytes.starts_with(b"version https://git-lfs.github.com/spec/"),
        "{}: {field}: required asset is empty or an LFS pointer — export the asset or hydrate it with git lfs pull",
        resolved.display()
    );
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client admission relies on readable exports. A missing path or an LFS pointer must refuse
    /// at the file gate rather than becoming silent audio or a blank effect during play.
    #[test]
    fn required_presentation_files_refuse_missing_empty_and_lfs_inputs() {
        let root =
            std::env::temp_dir().join(format!("overmatch-view-files-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        for bytes in [
            b"".as_slice(),
            b"version https://git-lfs.github.com/spec/v1\n".as_slice(),
        ] {
            std::fs::write(root.join("clip.ogg"), bytes).unwrap();
            assert!(
                std::panic::catch_unwind(|| require_file(
                    &root,
                    "clip.ogg",
                    "fixture.report_clips"
                ))
                .is_err()
            );
        }
        assert!(
            std::panic::catch_unwind(|| require_file(&root, "missing.ogg", "fixture.report_clips"))
                .is_err()
        );
        for path in ["", "../clip.ogg", "/clip.ogg"] {
            assert!(
                std::panic::catch_unwind(|| require_file(&root, path, "fixture.report_clips"))
                    .is_err()
            );
        }
        std::fs::write(
            root.join("clip.ogg"),
            b"a nonempty fixture; the loader checks its codec",
        )
        .unwrap();
        require_file(&root, "clip.ogg", "fixture.report_clips");
        std::fs::remove_dir_all(root).unwrap();
    }

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
    /// any exe layout — a release-derived binary run away from its assets (perf scripts, bitprobe
    /// CI, the Windows smoke's server fixture) relies on this.
    #[test]
    fn env_root_wins_over_everything() {
        let got = asset_root_from(
            Path::new("/App.app/Contents/MacOS"),
            Some(Path::new("/repo")),
            Some(Path::new("/opt/overmatch-server")),
            Some("/build-machine/repo"),
        );
        assert_eq!(got, Path::new("/opt/overmatch-server/assets"));
    }

    /// No `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR` set → `<manifest>/assets`. This is the dev loop
    /// and every `cargo test`/`cargo run`, which is why CI never hit the packaged-app bug.
    #[test]
    fn manifest_dir_when_no_env_root() {
        let got = asset_root_from(
            Path::new("/repo/target/debug"),
            Some(Path::new("/repo")),
            None,
            None,
        );
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
            Some(Path::new("/wt")),
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
