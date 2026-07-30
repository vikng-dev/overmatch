//! Shared real-loopback test floor for network integration probes.

use core::time::Duration;
use std::net::{Ipv4Addr, UdpSocket};
use std::sync::{Mutex, MutexGuard};

use avian3d::prelude::PhysicsPlugins;
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

/// Fixed step shared by every real-loopback app.
pub(super) const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

/// The real-UDP tests share loopback scheduling and open many sockets. Serializing them makes their
/// measurements repeatable and prevents a many-receiver probe from contending with another harness
/// test in the same binary.
static REAL_UDP_TEST_MUTEX: Mutex<()> = Mutex::new(());

/// Serialize loopback harnesses even after a prior assertion panicked. The poisoned state says that
/// test failed, not that the lock ceased to protect the sockets; recovering keeps later independent
/// UDP probes runnable and their own failures visible.
pub(super) fn lock_real_udp_test() -> MutexGuard<'static, ()> {
    REAL_UDP_TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The plugin floor shared by real-loopback apps: no rig, tank, or renderer, only the assets,
/// schedules, and physics needed by their production seams.
///
/// AVIAN'S DEFAULT PHYSICS COMPOSITION, which is NOT the network client's — see
/// [`net_physics_app`] for the difference and for when it matters.
pub(super) fn base_app() -> App {
    base_app_with(PhysicsPlugins::default().build())
}

/// The same floor with the NETWORK CLIENT's physics composition
/// (`net::physics::physics_plugins`), which disables `PhysicsTransformPlugin`.
///
/// The difference is load-bearing for anything that asserts on `Position` ACROSS A REPLAY. Avian's
/// `PhysicsTransformPlugin` puts `transform_to_position` in `FixedPostUpdate`, which is inside
/// `FixedMain` — the schedule `run_rollback` executes once per replayed tick. With it mounted, the
/// first replayed tick overwrites the pose `prepare_rollback` just restored with whatever `Transform`
/// held, undoing the restore; lightyear's own `LightyearAvianPlugin` warns about exactly this
/// ("in case a rollback updates Position, that change will be overridden by the transform->position",
/// `lightyear_avian3d-0.28.0/src/plugin.rs`). `net::physics` disables the plugin and owns the one
/// ordering edge it needed, which is why the shipping client's rollbacks survive their own replay.
///
/// [`base_app`] keeps the default composition because the fixtures built on it assert on
/// VELOCITIES, which that sync does not touch, and moving them is not this slice's business.
pub(super) fn net_physics_app() -> App {
    let mut app = base_app_with(super::physics::physics_plugins());
    super::physics::plugin(&mut app);
    app
}

fn base_app_with(physics: bevy::app::PluginGroupBuilder) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        // lightyear's plugins `init_state`, which needs the `StateTransition` schedule that only
        // `StatesPlugin` (folded into `DefaultPlugins`, absent from `MinimalPlugins`) adds.
        bevy::state::app::StatesPlugin,
    ))
    .init_asset::<Mesh>()
    .init_asset::<StandardMaterial>()
    .init_asset::<bevy::world_serialization::WorldAsset>()
    // One fixed tick per `update()` — the determinism the assertions rest on.
    .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
    .add_plugins(physics);
    app
}

/// Drive plugin finish/cleanup by hand — a bare `update()` loop skips it, and avian registers its
/// diagnostics resources (which the spatial-query systems require) in `Plugin::finish`.
pub(super) fn finish(app: &mut App) {
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
}

/// The `PredictionManager` every fixture in this tree spawns.
///
/// LIGHTYEAR DEFAULTS EXCEPT THE CORRECTION POLICY, which is the shipping one
/// (`net::client::shipping_correction_policy`). Until slice 4 every fixture carried
/// `PredictionManager::default()`, whose `CorrectionPolicy` is lightyear's 200 ms / 0.5 — a
/// smoothing configuration the game has never run. Correction is the half of the rollback
/// transaction `net::render_error` shares with lightyear, so a fixture that runs the default policy
/// is exercising a different visual seam from the client.
///
/// The ROLLBACK policy is deliberately left at lightyear's default rather than raised to
/// `net::client::shipping_rollback_policy`. Fixtures that care assert against it explicitly —
/// `net::adoption::the_shipping_client_disables_input_rollback` is the tripwire that holds the
/// shipping value in place, and `net::lead_zero_rollback::a_revalidation_that_never_passes_is_dropped_at_the_replay_window`
/// derives its boundary FROM the manager it built. Folding the shipping rollback policy in here
/// would silently change what those fixtures are measuring.
pub(super) fn prediction_manager() -> lightyear::prelude::PredictionManager {
    lightyear::prelude::PredictionManager {
        correction_policy: super::client::shipping_correction_policy(),
        ..default()
    }
}

/// Grab a free loopback UDP port by binding one and dropping it. A fixed port would collide with a
/// concurrent test binary (or a stray dev server) on the same machine.
pub(super) fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("loopback UDP must be bindable")
        .local_addr()
        .expect("a bound socket has a local address")
        .port()
}
