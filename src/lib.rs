//! Overmatch — a realistic 3D multiplayer tank game (single-player vertical slice).
//!
//! Organized one plugin per feature. `GamePlugin` composes them; `main.rs` only supplies
//! the runtime and runs the app. Each feature module owns its components, systems, and its
//! own wiring (a `pub fn plugin(app: &mut App)`), so this list reads as a table of contents.

// Two clippy lints fight Bevy's ECS paradigm and are allowed crate-wide (as Bevy's own codebase
// does): `type_complexity` fires on ordinary multi-component query tuples, and `too_many_arguments`
// on systems that legitimately need many params. We de-duplicate the genuinely-repeated query shapes
// behind named `QueryData`/`SystemParam` (e.g. `damage::VolumeFacets`, `damage::ControlledTank`);
// what remains is irreducible ECS shape, not a smell.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use avian3d::prelude::{PhysicsInterpolationPlugin, PhysicsLayer, PhysicsPlugins};
use bevy::prelude::*;

mod aim;
mod ballistics;
mod branding;
mod camera;
pub(crate) mod damage;
#[cfg(debug_assertions)]
mod debug;
mod driving;
/// The shared tank-state HUD (world-anchored capability/crew/damage readouts). Mounted by both
/// `GamePlugin` and the sandbox; each tags its own world camera with `hud::HudCamera`.
mod hud;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
pub mod sandbox;
mod shooting;
mod sight;
mod spec;
mod state;
mod tank;
mod world;

/// Physics collision layers. Wheel suspension rays filter to `Terrain` only, so they ignore
/// the vehicle's own hull collider (ADR-0005). Shared infra, hence at the crate root.
#[derive(PhysicsLayer, Default, Clone, Copy, Debug)]
pub(crate) enum Layer {
    #[default]
    Default,
    Terrain,
    Vehicle,
    /// Ballistic volumes (armor plates + modules): what the penetration march raycasts against,
    /// distinct from `Vehicle` (the dynamic collision proxy). "Same geometry, two layers" (ADR-0008).
    Armor,
}

/// Every gameplay feature, composed. Add to an `App` that already has the runtime plugins
/// (`DefaultPlugins` for the game, `MinimalPlugins` for headless tests).
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // Avian physics — foundational infra for world/tank/shooting. Runs in
            // FixedPostUpdate by default, consistent with our sim-in-fixed bet (ADR-0004).
            // `interpolate_all` renders bodies at an interpolated pose between fixed steps, so
            // motion stays smooth when the display rate differs from the physics tick rate.
            PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
            branding::plugin,
            state::plugin,
            world::plugin,
            // `spec` registers the `.tank.ron` data-asset loader before `tank` spawns the tank
            // and requests one (ADR-0010).
            spec::plugin,
            tank::plugin,
            driving::plugin,
            camera::plugin,
            aim::plugin,
            // `sight` owns the gunner-view toggle/mode that `camera` and `aim` branch on.
            sight::plugin,
            // `ballistics` owns the shell trajectory + impact seam; `shooting` is the player's gun
            // control that drives it (the sandbox drives the same `FireShell` from its camera).
            ballistics::plugin,
            damage::plugin,
            shooting::plugin,
            // The shared tank-state HUD — capability/crew/damage readouts floated over each tank.
            hud::plugin,
        ));

        // Dev-only physics visualization (collider/ray wireframes) + debug toggles. Off in release
        // builds.
        #[cfg(debug_assertions)]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}
