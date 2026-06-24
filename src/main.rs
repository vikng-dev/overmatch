use bevy::prelude::*;
use overmatch::GamePlugin;

fn main() {
    App::new()
        // Runtime (window, rendering, input). Game features live in GamePlugin so the game
        // logic can also be mounted on a headless App (MinimalPlugins) for tests later.
        .add_plugins(DefaultPlugins)
        .add_plugins(GamePlugin)
        .run();
}
