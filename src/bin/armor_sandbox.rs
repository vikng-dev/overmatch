//! The armor ballistics sandbox binary — a runtime shell, like `main.rs`, that mounts the sandbox
//! plugin (`overmatch::sandbox`) on `overmatch::windowed_runtime`, so `OVERMATCH_WINDOW` and
//! `OVERMATCH_UNATTENDED` hold here as on every windowed root. Run with `cargo run --bin
//! armor_sandbox`. See `.agents/docs/design/armor-penetration-and-damage.md` §11.

use bevy::prelude::*;

fn main() {
    let mut app = App::new();
    overmatch::windowed_runtime(&mut app, Window::default());
    app.add_plugins(overmatch::sandbox::plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
