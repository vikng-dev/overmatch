//! The track-model sandbox binary — a runtime shell that mounts the sandbox plugin
//! (`overmatch::track_sandbox`) on `overmatch::default_plugins`.
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
    // A capture run (`OVERMATCH_CAPTURE`) creates its window invisible: `visible: false` is the
    // one macOS lever that keeps a run from taking the screen (see `overmatch::run_offline`).
    let hidden = std::env::var_os("OVERMATCH_CAPTURE").is_some();
    App::new()
        .add_plugins(overmatch::default_plugins().set(WindowPlugin {
            primary_window: Some(Window {
                visible: !hidden,
                ..default()
            }),
            ..default()
        }))
        .add_plugins(overmatch::track_sandbox::plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
