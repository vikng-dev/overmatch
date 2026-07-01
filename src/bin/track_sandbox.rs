//! The track-model sandbox binary — a runtime shell that mounts the sandbox plugin
//! (`overmatch::track_sandbox`) on `DefaultPlugins`. Run with `cargo run --bin track_sandbox`.
//! See `.agents/docs/design/track-model/HQ.md`.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::track_sandbox::plugin)
        .run();
}
