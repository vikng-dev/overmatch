//! The damage cam clip player — a runtime shell that mounts `overmatch::damage_cam::player` on
//! `DefaultPlugins` and plays the hand-authored scenarios under `assets/dev/damage_cam/`. Run with
//! `cargo run --bin damage_cam_demo [through|ricochet|stopped|cookoff|overpen] [section]` from the
//! repo root; the optional arguments pick the starting scenario and start in section view.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::damage_cam::player::demo_plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
