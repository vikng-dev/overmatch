//! The armor ballistics sandbox binary — a runtime shell, like `main.rs`, that mounts the sandbox
//! plugin (`overmatch::sandbox`) on `overmatch::default_plugins`. Run with `cargo run --bin
//! armor_sandbox`. See `.agents/docs/design/armor-penetration-and-damage.md` §11.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(overmatch::default_plugins())
        .add_plugins(overmatch::sandbox::plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
