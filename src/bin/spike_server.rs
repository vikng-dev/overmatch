//! Networking-spike dedicated server (step 7 of the lightyear spike map's recommended order):
//! `SimPlugin` mounted for real — driving/aim/shooting/damage all run under prediction now, not
//! the increment-5 stub. Headless — the proven `headless_test.rs` recipe (full `DefaultPlugins`,
//! no GPU/window/winit), NOT `MinimalPlugins`, because the rig loads the same `.glb`/`.tank.ron`
//! assets the client does.
//! Run with `cargo run --bin spike_server --features net`.

// Same rationale as lib.rs's crate-level allow (bins don't inherit it): ordinary multi-filter
// query tuples trip this lint.
#![allow(clippy::type_complexity)]

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Forces, Position, WriteRigidBodyForces};
use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::LoadState;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use overmatch::net::{PendingTankSpec, SpikeBeacon, SpikeTank, load_tank_spec, spike_tank_rig};
use overmatch::{AppState, Rig, SimPlugin, TankCommand, on_tank_ready};

const PORT: u16 = 5888;

fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::render::RenderPlugin {
                render_creation: bevy::render::settings::WgpuSettings {
                    backends: None,
                    ..default()
                }
                .into(),
                ..default()
            })
            .set(WindowPlugin {
                primary_window: None,
                exit_condition: bevy::window::ExitCondition::DontExit,
                ..default()
            })
            .disable::<bevy::winit::WinitPlugin>(),
    )
    // No winit means no runner: drive the main schedule ourselves, unthrottled enough for a
    // 64 Hz fixed clock.
    .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)));

    // Ordering per the spike map §3: ServerPlugins, then protocol registration, then the Server
    // entity. `net::plugin` also mounts `LightyearAvianPlugin` + Position/Rotation/velocity
    // registration (map §5).
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(overmatch::net::plugin);
    app.add_plugins(overmatch::net::physics_plugins());
    // Step 7: the real sim, for real — driving/aim/shooting/damage all run under prediction now.
    // `SimPlugin` no longer bundles physics (that split already happened in Milestone A), so it
    // composes cleanly alongside `physics_plugins()` above. Not `tank::sp_spawn_plugin` (the
    // single-player two-tank duel scenario) — the server keeps its own per-client spawn below.
    app.add_plugins(SimPlugin);

    let server = app
        .world_mut()
        .spawn((
            Name::new("SpikeServer"),
            NetcodeServer::new(NetcodeConfig {
                protocol_id: 0,
                private_key: [0; 32], // dev only — matches the client's Authentication::Manual
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), PORT)),
            ServerUdpIo::default(),
        ))
        .id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Start { entity: server });
        commands.spawn((
            Name::new("SpikeBeacon"),
            SpikeBeacon,
            Replicate::to_clients(NetworkTarget::All),
        ));
        info!("spike_server: starting, listening on 0.0.0.0:{PORT}");
    });
    app.add_systems(Startup, load_tank_spec);
    app.init_resource::<PendingClients>();

    app.add_systems(
        Update,
        (
            handle_new_clients,
            spawn_pending_tanks,
            open_gameplay_gate,
            log_positions,
            count_rig_binds,
            overmatch::net::log_sim_evidence,
        ),
    );
    app.add_systems(FixedUpdate, (log_tank_commands, perturb_after_delay));

    app.run();
}

/// `SimPlugin` mounts `state::sim_plugin` (`AppState`, `GameplaySet` gated on `Playing`), but the
/// bins have no menu/loading flow to drive that transition themselves (step 7 task: "the bins
/// never enter Playing on their own now"). The server already gates spawning on the spec load
/// (`spawn_pending_tanks`); this just opens the `GameplaySet` gate once, the same load dependency.
fn open_gameplay_gate(
    spec: Option<Res<PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    state: Res<State<AppState>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if *state.get() != AppState::Loading {
        return;
    }
    let Some(spec) = spec else { return };
    if matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        next.set(AppState::Playing);
    }
}

/// Per-client one-shot: fires ~2 s after connect, applying a large lateral impulse the client
/// cannot have predicted (server-only side effect) — guarantees a misprediction and thus a
/// rollback (increment 5 success criterion).
#[derive(Component)]
struct PendingPerturbation {
    at: Duration,
}

/// Connected clients waiting on the Tiger spec load (§2 of the map: the spec is a load
/// dependency of spawning, same as `tank.rs`/`sandbox.rs` — `on_tank_ready` asserts it's already
/// loaded). A client can connect before the tiny `.tank.ron` parse finishes; queueing avoids
/// spawning a tank root the binder would immediately panic on.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

/// Queues each newly connected client — spawning is deferred to [`spawn_pending_tanks`] once the
/// Tiger spec has loaded (spike map §6/§7 spawn pattern: one predicted tank per client, owned by
/// that client, everyone else would interpolate it — no second client yet).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut pending: ResMut<PendingClients>,
) {
    for (link, remote) in &new {
        info!("spike_server: client connected: {remote} (link {link})");
        pending.0.push((link, remote.0));
    }
}

/// Spawns the real Tiger rig for every queued client once the spec has loaded — the increment-6
/// swap for increment 5's primitive cuboid spawn. `on_tank_ready` (observed below) builds the
/// colliders/armor volumes from the same spec once the glb scene arrives (async, per tank).
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    spec: Option<Res<PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(spec) = spec else { return };
    if !matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        return;
    }
    for (link, client_id) in pending.0.drain(..) {
        commands
            .spawn((
                Name::new("SpikeTank"),
                spike_tank_rig(&asset_server, &spec.0),
                ActionState::<TankCommand>::default(),
                Position(Vec3::new(0.0, 2.0, 0.0)),
                Replicate::to_clients(NetworkTarget::All),
                PredictionTarget::to_clients(NetworkTarget::Single(client_id)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(client_id)),
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
                PendingPerturbation {
                    at: time.elapsed() + Duration::from_secs(2),
                },
            ))
            .observe(on_tank_ready);
    }
}

/// Verdict 1 (increment 6): the binder must fire exactly once per tank despite rollback replays —
/// rollback only re-runs `FixedMain` (map §8), and this observer fires from `WorldInstanceReady`
/// (outside `FixedMain`), so a count > 1 per tank would mean that assumption was wrong. `Rig` is
/// the observer's own terminal insert, so counting `Added<Rig>` is an external, non-invasive proxy
/// for "the binder ran" without touching `tank.rs`.
fn count_rig_binds(binds: Query<Entity, Added<Rig>>) {
    for entity in &binds {
        info!("spike_server: {entity} Rig bound (on_tank_ready fired)");
    }
}

/// Applies the forced-rollback perturbation once, ~2 s after spawn — a lateral impulse only the
/// server applies, so the client's prediction (which never saw it coming) mispredicts and must
/// roll back when the replicated `Position` disagrees.
fn perturb_after_delay(
    mut tanks: Query<(Entity, &PendingPerturbation, Forces)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for (entity, pending, mut forces) in &mut tanks {
        if time.elapsed() < pending.at {
            continue;
        }
        // Sized for ~3 m/s of lateral delta-v on the 57 t tank (net.rs::TANK_MASS) — comfortably
        // above the 0.01 m/s-equivalent rollback threshold (forces exactly one misprediction) but
        // small next to the ~4-15 m/s cruise speed, so the resulting one-tick displacement stays
        // under the ROLLBACK-SNAP detector's 0.5 m bar. The previous 4,000,000 N*s value injected
        // ~70 m/s instantly — legitimate per-tick motion at that speed (~1.1 m/tick) was tripping
        // the snap detector on its own, misread as rollback oscillation (see spike log).
        const IMPULSE: f32 = 171_000.0;
        forces.apply_linear_impulse(Vec3::X * IMPULSE);
        info!("spike_server: {entity} perturbation impulse applied (forced rollback trigger)");
        commands.entity(entity).remove::<PendingPerturbation>();
    }
}

/// Periodic authoritative position log (every ~2 s), so the client's converged position can be
/// diffed against this end for the increment-5/6 convergence success criterion.
fn log_positions(
    tanks: Query<(Entity, &Position), With<SpikeTank>>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    for (entity, position) in &tanks {
        info!("spike_server: {entity} position={:?}", position.0);
    }
}

/// Step-4 success signal (carried over): the client's `TankCommand` arriving through lightyear's
/// input buffer.
fn log_tank_commands(states: Query<(Entity, &ActionState<TankCommand>)>) {
    for (entity, state) in &states {
        let cmd = &state.0;
        if cmd.throttle != 0.0 || cmd.fire_primary {
            info!(
                "spike_server: {entity} command: throttle={} steer={} fire_primary={}",
                cmd.throttle, cmd.steer, cmd.fire_primary
            );
        }
    }
}
