//! Branding: applies the game's window icon at startup.
//!
//! Bevy 0.19 has no native window-icon field on `Window`, so we reach through to
//! winit — the documented approach. The PNG is embedded with `include_bytes!`
//! rather than loaded via `AssetServer`, so the binary is self-contained and works
//! regardless of working directory (the release bundle runs as `./overmatch` from
//! its own folder). The icon is a generated branding asset under `build/icons/`
//! (regenerate with `scripts/gen-icons.sh`), not `assets/` (runtime-loaded content),
//! so it never ships as a runtime asset.

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::winit::WINIT_WINDOWS;
use winit::window::{BadIcon, Icon};

/// 256² window icon, baked into the binary at compile time from the branding sources.
const ICON_PNG: &[u8] = include_bytes!("../build/icons/window_icon.png");

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, set_window_icon);
}

/// Decode the embedded PNG and apply it to every window. In Bevy 0.19 the winit
/// windows live in a thread-local (`WINIT_WINDOWS`), not an ECS `NonSend` resource
/// (a stopgap pending bevyengine/bevy#17667). The `NonSendMarker` param is
/// load-bearing: it forces this system onto the main thread, where that thread-local
/// is populated and where winit's `set_window_icon` must be called — off the main
/// thread the thread-local is empty and the call can hang.
fn set_window_icon(_non_send_marker: NonSendMarker) {
    let icon = load_icon().unwrap_or_else(|err| match err {
        BadIcon::OsError(source) => {
            eprintln!("window icon: the OS refused the embedded icon: {source}");
            std::process::exit(2);
        }
        invalid => panic!("a decoded RGBA image has matching dimensions: {invalid}"),
    });
    WINIT_WINDOWS.with_borrow(|winit_windows| {
        for window in winit_windows.windows.values() {
            window.set_window_icon(Some(icon.clone()));
        }
    });
}

fn load_icon() -> Result<Icon, BadIcon> {
    let rgba = image::load_from_memory(ICON_PNG)
        .expect("build/icons/window_icon.png is an embedded PNG; regenerate it with scripts/gen-icons.sh")
        .into_rgba8();
    let (width, height) = rgba.dimensions();
    Icon::from_rgba(rgba.into_raw(), width, height)
}

#[cfg(test)]
mod tests {
    /// Window startup relies on a valid embedded icon; a broken export must fail before shipping.
    #[test]
    fn the_embedded_window_icon_decodes() {
        super::load_icon().expect("the embedded icon has valid dimensions and the OS accepts it");
    }
}
