//! The avian3d 0.8.0 fork's guard crate (see `Cargo.toml`). The game-side seams are the game's own
//! `src/avian_ext/` and the guards are the game's own test files, included by path; what they name
//! of the game crate is stubbed here under the game's paths.

#[path = "../../../src/avian_ext/mod.rs"]
pub mod avian_ext;

#[cfg(test)]
mod tests;

/// The game's fixture helpers (`headless_test`): one fixed loop per `update`, and the plugin finish.
#[cfg(test)]
mod headless_test {
    pub(crate) fn start_fixed_clock(app: &mut bevy::prelude::App) {
        app.insert_resource(bevy::time::TimeUpdateStrategy::FixedTimesteps(1));
    }

    /// The game's plugin finish (`headless_test::finish_plugins`).
    pub(crate) fn finish_plugins(app: &mut bevy::prelude::App) {
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
    }
}

/// The game's gravity (`physical_laws::gravity_mps2`), read from the same shipped file.
#[cfg(test)]
mod physical_laws {
    pub(crate) fn gravity_mps2() -> f32 {
        let laws = include_str!("../../../assets/ballistics/physical_laws.ron");
        let (_, sourced) = laws
            .split_once("gravity_mps2: (")
            .expect("the shipped laws name gravity");
        let (value, _) = sourced.split_once(',').expect("a sourced value");
        value.trim().parse().expect("gravity is a number")
    }
}
