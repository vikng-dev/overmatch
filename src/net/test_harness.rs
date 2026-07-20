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
pub(super) fn base_app() -> App {
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
    .add_plugins(PhysicsPlugins::default().build());
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

/// Grab a free loopback UDP port by binding one and dropping it. A fixed port would collide with a
/// concurrent test binary (or a stray dev server) on the same machine.
pub(super) fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("loopback UDP must be bindable")
        .local_addr()
        .expect("a bound socket has a local address")
        .port()
}
