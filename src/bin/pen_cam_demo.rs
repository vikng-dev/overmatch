//! The pen cam demo: the in-battle flow of a hit, over the recorded scenarios in
//! `assets/dev/pen_cam/`. Run from the repo root with `cargo run --bin pen_cam_demo`; an optional
//! scenario tag (`s4`) picks the first scenario and `--once` stops after it. Keys and the frame
//! grammar: `overmatch::damage_cam::pen_cam`.

use bevy::prelude::*;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Overmatch pen cam demo".into(),
                resolution: (1280, 720).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(overmatch::damage_cam::pen_cam::demo_plugin)
        // Documentation frames; inert without `OVERMATCH_CAPTURE`.
        .add_plugins(overmatch::dev_capture::plugin)
        .run();
}
