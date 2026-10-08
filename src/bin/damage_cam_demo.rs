//! The damage cam clip player — a runtime shell that mounts `overmatch::damage_cam::player` on
//! `DefaultPlugins` and plays the hand-authored scenarios under `assets/dev/damage_cam/`. Run with
//! `cargo run --bin damage_cam_demo [through|ricochet|stopped|cookoff]` from the repo root;
//! the optional argument picks the starting scenario.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(overmatch::damage_cam::player::demo_plugin)
        .run();
}
