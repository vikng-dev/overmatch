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
use winit::window::Icon;

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
    let icon = match load_icon() {
        Ok(icon) => icon,
        Err(err) => {
            warn!("could not build window icon, falling back to default: {err}");
            return;
        }
    };
    WINIT_WINDOWS.with_borrow(|winit_windows| {
        for window in winit_windows.windows.values() {
            window.set_window_icon(Some(icon.clone()));
        }
    });
}

fn load_icon() -> Result<Icon, Box<dyn std::error::Error>> {
    let rgba = image::load_from_memory(ICON_PNG)?.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Icon::from_rgba(rgba.into_raw(), width, height)?)
}
