//! The armor ballistics sandbox binary — a runtime shell, like `main.rs`, that mounts the sandbox
//! plugin (`overmatch::sandbox`) on `DefaultPlugins`. Run with `cargo run --bin armor_sandbox`.
//! See `.agents/docs/design/armor-penetration-and-damage.md` §11.

use bevy::prelude::*;

fn main() {
    App::new()
        // `AssetPlugin`'s default `file_path` is "assets" relative to the working directory, which
        // is the repo root under `cargo run` — no override needed (the sandbox isn't bundled).
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::sandbox::plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
