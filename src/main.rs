use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use overmatch::GamePlugin;

fn main() {
    App::new()
        // Runtime (window, rendering, input). Game features live in GamePlugin so the game
        // logic can also be mounted on a headless App (MinimalPlugins) for tests later.
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: asset_root(),
            ..default()
        }))
        .add_plugins(GamePlugin)
        .run();
}

/// Where to load runtime assets from. Defaults to `assets` (relative to the working
/// directory) for `cargo run` and the Linux/Windows bundles. A macOS `.app` is
/// launched with an unrelated working directory (often `/`), so when we detect we're
/// running inside a bundle we resolve assets at `Contents/Resources/assets`, derived
/// from the executable's own path.
fn asset_root() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(exe) = std::env::current_exe() {
        // exe = <App>.app/Contents/MacOS/<bin>  ->  ../Resources/assets
        if let Some(resources) = exe
            .parent()
            .and_then(|macos| macos.parent())
            .map(|contents| contents.join("Resources").join("assets"))
            && resources.is_dir()
        {
            return resources.to_string_lossy().into_owned();
        }
    }
    "assets".to_string()
}
