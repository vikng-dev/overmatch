//! The suspension-editor binary — a runtime shell that mounts the editor plugin
//! (`overmatch::suspension_editor`) on `DefaultPlugins`. Run with `cargo run --bin suspension_editor`.
//! A dev tool for visualizing (and live-tweaking) the Tiger's derived suspension/track geometry —
//! the gap between the Blender rest pose and the in-game result. See `src/suspension_editor/mod.rs`.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::suspension_editor::plugin)
        .run();
}
