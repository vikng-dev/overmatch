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
/// Re-exported for the spike bins (step 7): `Added<ShellPath>` is the observable "a shell/tracer
/// spawned" signal — the forced-rollback pass counts these to record whether a fire replayed
/// during rollback duplicates the local tracer (the accepted pre-`PreSpawned` wart).
#[cfg(feature = "net")]
pub use ballistics::ShellPath;
mod branding;
mod camera;
/// The command layer: device reads → player bindings → per-tank serializable `TankCommand`. The
/// seam authoritative multiplayer hangs off; sim modules consume commands, never devices.
mod command;
/// Re-exported for the `net` spike bins, which need `TankCommand`/`CrewSwap` to register the
/// lightyear input protocol and construct commands directly (no device gather in a headless bin).
#[cfg(feature = "net")]
pub use command::{CrewSwap, TankCommand};
/// The controlled tank's crew bar + swap input — a shared piece of the fixed player UI, mounted by
/// both `GamePlugin` and the sandbox (each scoped to the `Controlled` tank).
mod crew_ui;
pub(crate) mod damage;
#[cfg(debug_assertions)]
mod debug;
mod driving;
/// Re-exported for `net.rs` (step 7): `local_rollback::<DriveState>()`/`<Suspension>()`
/// registration and the `TankCommand`→sim bridge need these types by name.
#[cfg(feature = "net")]
pub use driving::{DriveState, Suspension};
/// Fire control: per-weapon superelevation range tables + the player-dialed range. Sits atop
/// `ballistics`; the aim commit reads it to lob the aim point so the bore elevates for range.
mod firecontrol;
/// The dedicated-server guard: boots `SimPlugin` headless (no GPU/window/winit) and drives the
/// tank via `TankCommand` — fails first if sim code grows a hard render dependency.
#[cfg(test)]
mod headless_test;
/// The shared tank-state HUD (world-anchored capability/crew/damage readouts). Mounted by both
/// `GamePlugin` and the sandbox; each tags its own world camera with `hud::HudCamera`.
mod hud;
/// Networking spike protocol registration (`net` feature only). Public so `spike_server`/
/// `spike_client` can mount it; not part of `GamePlugin`.
#[cfg(feature = "net")]
pub mod net;
/// The armor ballistics sandbox (`bin/armor_sandbox`). Public so the binary can mount it; not part
/// of `GamePlugin`.
pub mod sandbox;
mod shooting;
/// Re-exported for `net.rs` (step 7): `local_rollback::<Reload>()` registration.
#[cfg(feature = "net")]
pub use shooting::Reload;
mod sight;
mod spec;
/// Re-exported for the spike bins (increment 6): `spec::plugin` registers the `.tank.ron` asset
/// loader both `on_tank_ready` and the spawn systems depend on; `TankSpec`/`TankSpecHandle` are
/// the load-dependency pair the bins spawn against, matching `sandbox.rs`'s `load_target` pattern.
#[cfg(feature = "net")]
pub use spec::{TankSpec, TankSpecHandle, plugin as spec_plugin};
mod state;
/// Re-exported for the spike bins (step 7): `SimPlugin` mounts `state::sim_plugin`, which gates
/// `GameplaySet` on `AppState::Playing` — the bins must open that gate themselves once their spec
/// loads (they have no menu/loading-screen flow of their own to drive the transition).
/// `GameplaySet` is what `net.rs`'s input bridge orders itself before (every sim consumer is in it).
#[cfg(feature = "net")]
pub use state::{AppState, GameplaySet};
mod tank;
/// Re-exported for `spike_server`, which logs bound-rig roadwheel count as its step-2 success
/// criterion — the same signal `headless_test.rs` polls for, from outside the crate.
#[cfg(feature = "net")]
pub use tank::Roadwheel;
/// Re-exported for the spike bins (increment 6): the real Tiger rig replaces the increment-5
/// primitive on both ends. `on_tank_ready` is the binder observer (unchanged, per the task's "do
/// not modify sim module logic"); `Tank`/`Rig`/`Turret`/`Hull` are what the spike's verdict-2 log
/// (child collider tracking through rollback) reads back.
#[cfg(feature = "net")]
pub use tank::{Hull, Rig, ServoState, Tank, Turret, on_tank_ready};
/// The track-model sandbox (`bin/track_sandbox`). Public so the binary can mount it; not part of
/// `GamePlugin`. Self-contained: its own code-generated primitive rig + locomotion, for developing
/// the continuous-track model in isolation.
pub mod track_sandbox;
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

/// The simulation — the authority layer, in the client/server sense (see the memory note and
/// bevy_replicon's "abstracting over configurations"): everything the server must run to be the
/// truth, and everything a predicting client re-runs. Consumes `TankCommand`s, never devices;
/// steps on the fixed clock. A dedicated server mounts exactly this (plus netcode) on
/// `MinimalPlugins`; the single-player game mounts it alongside [`ClientPlugin`].
pub struct SimPlugin;

impl Plugin for SimPlugin {
    fn build(&self, app: &mut App) {
        // NOTE: physics (avian `PhysicsPlugins`) is deliberately NOT mounted here — its
        // configuration is the one thing that legitimately differs per composition root:
        // single-player wants `PhysicsInterpolationPlugin::interpolate_all()` (ADR-0004), the
        // networked bins must disable exactly that plugin for `LightyearAvianPlugin` (spike log,
        // increment 5). The composition root (GamePlugin / the net bins) owns the choice.
        app.add_plugins((
            state::sim_plugin,
            world::plugin,
            // `spec` registers the `.tank.ron` data-asset loader before `tank` spawns the tank
            // and requests one (ADR-0010).
            spec::plugin,
            tank::sim_plugin,
            // Commands are the sim's only input: `core_plugin` puts a `TankCommand` on every tank
            // and consumes latched edges each tick; `driving`/`shooting`/`aim` read it.
            command::core_plugin,
            driving::plugin,
            aim::sim_plugin,
            // `ballistics` owns the shell trajectory + impact seam; `shooting` is the gun control
            // that drives it (the sandbox drives the same `FireShell` from its camera).
            ballistics::plugin,
            // Range tables at bind: the servo bridge lobs each tank's aim by its commanded range.
            firecontrol::sim_plugin,
            damage::plugin,
            shooting::plugin,
        ));
    }
}

/// The client — command generation (devices → `TankCommand`) and presentation (state → screen).
/// Requires [`SimPlugin`] in the same app (single-player and listen-server mount both; a pure
/// network client will too, for interpolation/prediction).
pub struct ClientPlugin;

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            branding::plugin,
            // Pause/cursor handling (drives the states that `state::sim_plugin` owns).
            state::client_plugin,
            // Device gather: the only device→command translation.
            command::client_plugin,
            tank::client_plugin,
            camera::plugin,
            aim::client_plugin,
            // `sight` owns the gunner-view toggle/mode that `camera` and `aim` branch on.
            sight::plugin,
            // The player's range dial (rides to the sim inside the command).
            firecontrol::client_plugin,
            // The tank-state HUD and the controlled tank's crew bar + `1`–`5` swap input.
            hud::plugin,
            crew_ui::plugin,
        ));

        // Dev-only physics visualization (collider/ray wireframes) + debug toggles. Off in release
        // builds.
        #[cfg(debug_assertions)]
        app.add_plugins((avian3d::prelude::PhysicsDebugPlugin, debug::plugin));
    }
}

/// Every gameplay feature, composed — the single-player configuration: the full sim plus the
/// local client, one app, no netcode. Add to an `App` that already has the runtime plugins.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            // The single-player physics choice: bodies render at an interpolated pose between
            // fixed steps (ADR-0004). The networked bins mount lightyear's config instead.
            PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all()),
            SimPlugin,
            // The single-player scenario: two-tank duel spawn, first tank controlled.
            tank::sp_spawn_plugin,
            ClientPlugin,
        ));
    }
}
