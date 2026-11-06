//! Shared asset-root resolver for Bevy loading and geometry extraction.
//!
//! Invariant: both consumers resolve the same `assets/` tree, including packaged macOS bundles.

use std::path::{Path, PathBuf};

/// The checkout arm requires a Git marker before its assets can be used. A player's folder
/// can imitate Cargo's path shape; that ambiguity must refuse startup, never select another tree.
#[derive(Debug, PartialEq, Eq)]
enum AssetLayout {
    Checkout {
        root: PathBuf,
        adjacent_assets: PathBuf,
    },
    Override(PathBuf),
    Assets(PathBuf),
}

/// Resolve by layout alone, without filesystem probing or build-machine paths.
/// Cargo outputs live in `target/<profile>[/deps|examples]`, optionally below a target triple.
/// Target names have hyphen-separated fields (including abbreviated names such as wasm32-wasip1);
/// profile names are otherwise unrestricted. macOS bundles take precedence over an enclosing `target/`.
fn asset_root_from(exe_dir: &Path, env_root: Option<&Path>) -> AssetLayout {
    if let Some(root) = env_root {
        return AssetLayout::Override(root.join("assets"));
    }
    if exe_dir.ends_with("Contents/MacOS") {
        let contents = exe_dir
            .parent()
            .expect("Contents/MacOS has a Contents parent");
        return AssetLayout::Assets(contents.join("Resources/assets"));
    }
    for target in exe_dir.ancestors().filter(|dir| dir.ends_with("target")) {
        let below: Vec<_> = exe_dir
            .strip_prefix(target)
            .expect("an ancestor is a path prefix")
            .iter()
            .collect();
        let below = match below.as_slice() {
            [rest @ .., leaf] if *leaf == "deps" || *leaf == "examples" => rest,
            rest => rest,
        };
        let cargo_output = match below {
            [_profile] => true,
            [triple, _profile] => triple.as_encoded_bytes().contains(&b'-'),
            _ => false,
        };
        if cargo_output {
            return AssetLayout::Checkout {
                root: target
                    .parent()
                    .expect("a target directory with profile children has a checkout parent")
                    .to_owned(),
                adjacent_assets: exe_dir.join("assets"),
            };
        }
    }
    AssetLayout::Assets(exe_dir.join("assets"))
}

#[derive(Debug)]
enum AssetRootError {
    InvalidOverride {
        path: PathBuf,
        source: std::io::Error,
    },
    LayoutIo {
        path: PathBuf,
        source: std::io::Error,
    },
    PackagedAssetsInTarget(PathBuf),
}

impl std::fmt::Display for AssetRootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidOverride { path, source } => {
                return write!(
                    f,
                    "BEVY_ASSET_ROOT: {} is not an accessible assets directory: {source}; \
                     set BEVY_ASSET_ROOT to the directory holding assets/ or unset it",
                    path.display()
                );
            }
            Self::LayoutIo { path, source } => write!(
                f,
                "cannot verify checkout layout at {}: {source}",
                path.display()
            ),
            Self::PackagedAssetsInTarget(path) => write!(
                f,
                "packaged assets at {} conflict with the enclosing Cargo target layout",
                path.display()
            ),
        }?;
        write!(
            f,
            "; move the packaged game outside target/<profile> or \
             target/<triple>/<profile>, or set BEVY_ASSET_ROOT to the directory holding its assets/"
        )
    }
}

impl AssetLayout {
    /// Validation never changes the selected root. Both `.git/` and a worktree's `.git` file
    /// witness a checkout; assets beside a Cargo binary conflict even inside a real checkout.
    fn resolve(self) -> Result<PathBuf, AssetRootError> {
        match self {
            Self::Override(assets) => {
                std::fs::metadata(&assets)
                    .and_then(|metadata| {
                        if metadata.is_dir() {
                            Ok(())
                        } else {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::NotADirectory,
                                "assets path must be a directory",
                            ))
                        }
                    })
                    .map_err(|source| AssetRootError::InvalidOverride {
                        path: assets.clone(),
                        source,
                    })?;
                Ok(assets)
            }
            Self::Assets(assets) => Ok(assets),
            Self::Checkout {
                root,
                adjacent_assets,
            } => {
                let marker = root.join(".git");
                std::fs::metadata(&marker).map_err(|source| AssetRootError::LayoutIo {
                    path: marker,
                    source,
                })?;
                match std::fs::symlink_metadata(&adjacent_assets) {
                    Ok(_) => Err(AssetRootError::PackagedAssetsInTarget(adjacent_assets)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        Ok(root.join("assets"))
                    }
                    Err(source) => Err(AssetRootError::LayoutIo {
                        path: adjacent_assets,
                        source,
                    }),
                }
            }
        }
    }
}

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

/// Shared root for Bevy and direct readers. `BEVY_ASSET_ROOT` deliberately places assets away
/// from the binary (the packaged Windows smoke server uses it); otherwise the layout owns it.
/// OS paths preserve non-UTF-8 overrides. A matching Cargo layout must prove its checkout.
pub(crate) fn asset_root() -> PathBuf {
    let env_root = std::env::var_os("BEVY_ASSET_ROOT").map(PathBuf::from);
    let layout = match env_root.as_deref() {
        // An explicit root needs no executable lookup; only the layout resolver appends assets/.
        Some(root) => asset_root_from(Path::new(""), Some(root)),
        None => {
            let exe = std::env::current_exe().unwrap_or_else(|error| {
                crate::refuse_start(format!(
                    "cannot locate this executable to find assets: {error}; set BEVY_ASSET_ROOT to the directory holding assets/"
                ))
            });
            let exe_dir = exe
                .parent()
                .expect("the OS names an executable by a file path, which has a parent directory");
            asset_root_from(exe_dir, None)
        }
    };
    layout
        .resolve()
        .unwrap_or_else(|error| crate::refuse_start(error.to_string()))
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

    /// Asset readers rely on paths derived from the selected ID, so selecting another valid
    /// tank must preserve this test while a missing ID directory or wrong suffix fails it.
    #[test]
    fn the_selected_tank_derives_its_trio_paths() {
        for (path, suffix) in [
            (tank_file!(".glb"), ".glb"),
            (tank_file!(".sim.glb"), ".sim.glb"),
            (tank_file!(".tank.ron"), ".tank.ron"),
            (tank_file!(".lod.json"), ".lod.json"),
        ] {
            assert_eq!(path, format!("{TANK_ID}/{TANK_ID}{suffix}"));
        }
    }

    /// Bare builds, cargo run and cargo test share one checkout rule in every profile.
    #[test]
    fn cargo_layouts_identify_the_owning_checkout() {
        for profile in ["debug", "release", "edit", "custom-profile"] {
            for triple in [
                "",
                "aarch64-apple-darwin/",
                "x86_64-unknown-linux-gnu/",
                "wasm32-wasip1/",
            ] {
                for leaf in ["", "/deps", "/examples"] {
                    let dir = format!("/repo/target/{triple}{profile}{leaf}");
                    assert_eq!(
                        asset_root_from(Path::new(&dir), None),
                        AssetLayout::Checkout {
                            root: PathBuf::from("/repo"),
                            adjacent_assets: Path::new(&dir).join("assets")
                        },
                        "{dir}"
                    );
                }
            }
        }
    }

    /// A directory named target alone, or a deeper unrelated tree, is a flat package layout.
    #[test]
    fn unrelated_target_paths_keep_assets_beside_the_binary() {
        for dir in [
            "/games/target",
            "/games/target/game/bin",
            "/games/target/release/nested/bin",
        ] {
            assert_eq!(
                asset_root_from(Path::new(dir), None),
                AssetLayout::Assets(Path::new(dir).join("assets"))
            );
        }
    }

    /// Packaged clients must retain their assets when relocated, even into a checkout's target.
    #[test]
    fn packages_resolve_by_layout() {
        for (dir, assets) in [
            ("/opt/overmatch", "/opt/overmatch/assets"),
            (
                "/App.app/Contents/MacOS",
                "/App.app/Contents/Resources/assets",
            ),
            (
                "/repo/target/release/App.app/Contents/MacOS",
                "/repo/target/release/App.app/Contents/Resources/assets",
            ),
        ] {
            assert_eq!(
                asset_root_from(Path::new(dir), None),
                AssetLayout::Assets(PathBuf::from(assets))
            );
        }
    }

    /// Operators can deliberately override every layout, including an ambiguous player folder.
    #[test]
    fn env_root_wins_over_every_layout() {
        for dir in [
            "/repo/target/release",
            "/App.app/Contents/MacOS",
            "/opt/overmatch",
        ] {
            assert_eq!(
                asset_root_from(Path::new(dir), Some(Path::new("/chosen"))),
                AssetLayout::Override(PathBuf::from("/chosen/assets"))
            );
        }
    }

    /// Operators must see an invalid override refused before either asset reader runs;
    /// only an existing assets directory can pass, without selecting another layout.
    #[test]
    fn env_root_refuses_missing_or_non_directory_assets() {
        let root = std::env::temp_dir().join(format!("overmatch-override-{}", std::process::id()));
        std::fs::create_dir_all(root.join("file")).unwrap();
        std::fs::write(
            root.join("file/assets"),
            b"fixture: a file is not an assets directory",
        )
        .unwrap();
        for (name, kind) in [
            ("missing", std::io::ErrorKind::NotFound),
            ("file", std::io::ErrorKind::NotADirectory),
        ] {
            let chosen = root.join(name);
            let error = asset_root_from(Path::new("/repo/target/release"), Some(&chosen))
                .resolve()
                .unwrap_err();
            match &error {
                AssetRootError::InvalidOverride { path, source } => {
                    assert_eq!(path, &chosen.join("assets"));
                    assert_eq!(source.kind(), kind);
                }
                other => panic!("invalid operator override must refuse: {other}"),
            }
            let message = error.to_string();
            assert!(message.contains("BEVY_ASSET_ROOT"));
            assert!(message.contains(&chosen.join("assets").display().to_string()));
            assert!(
                message
                    .contains("set BEVY_ASSET_ROOT to the directory holding assets/ or unset it")
            );
        }
        std::fs::create_dir(root.join("assets")).unwrap();
        assert_eq!(
            asset_root_from(Path::new("/repo/target/release"), Some(&root))
                .resolve()
                .unwrap(),
            root.join("assets")
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A player extracting into target/release must not silently read an ancestor's assets.
    /// The pure resolver marks that residual ambiguity; the startup gate requires a Git marker.
    #[test]
    fn cargo_shaped_package_requires_a_checkout_marker() {
        let root = std::env::temp_dir().join(format!("overmatch-layout-{}", std::process::id()));
        std::fs::create_dir_all(root.join("assets")).unwrap();
        let dir = root.join("target/release");
        assert_eq!(
            asset_root_from(&dir, None),
            AssetLayout::Checkout {
                root: root.clone(),
                adjacent_assets: dir.join("assets")
            }
        );
        match asset_root_from(&dir, None).resolve().unwrap_err() {
            AssetRootError::LayoutIo { path, source } => {
                assert_eq!(path, root.join(".git"));
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("missing checkout marker must refuse: {other}"),
        }
        // A linked worktree's marker is a file; a regular checkout's marker is a directory.
        std::fs::write(root.join(".git"), "gitdir: /fixture/worktree").unwrap();
        assert_eq!(
            asset_root_from(&dir, None).resolve().unwrap(),
            root.join("assets")
        );
        std::fs::remove_file(root.join(".git")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        assert_eq!(
            asset_root_from(&dir, None).resolve().unwrap(),
            root.join("assets")
        );
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        match asset_root_from(&dir, None).resolve().unwrap_err() {
            AssetRootError::PackagedAssetsInTarget(path) => assert_eq!(path, dir.join("assets")),
            other => panic!("a package inside a checkout must refuse: {other}"),
        }
        assert_eq!(
            asset_root_from(&dir, Some(&dir)).resolve().unwrap(),
            dir.join("assets"),
            "an operator can deliberately select the packaged assets"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
