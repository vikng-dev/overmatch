//! The track-model sandbox binary — a runtime shell that mounts the sandbox plugin
//! (`overmatch::track_sandbox`) on `DefaultPlugins`.
//!
//! Run with `cargo run --bin track_sandbox --features dev_ui`. The `dev_ui` feature is REQUIRED
//! (declared as `required-features` on this bin in `Cargo.toml`): it gates the egui control panel
//! that is the sandbox's control surface, and — crucially — keeps `bevy_egui` out of the shipping
//! client, which builds with default features (`dev_ui` off). A plain `cargo build --bin
//! track_sandbox` is intentionally skipped by cargo for want of the feature.
//!
//! See `.agents/docs/design/track-model/HQ.md`.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::track_sandbox::plugin)
        .run();
}
