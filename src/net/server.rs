//! Authoritative dedicated-server composition root.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr};

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use lightyear::prelude::input::native::{ActionState, NativeStateSequence};
use lightyear::prelude::input::server::{
    InputSystems as ServerInputSystems, InputValidationAppExt, authorize_controlled_targets,
};
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use super::disclosure::{CombatDisclosure, NetTankStatus};
use super::protocol::{
    LaunchedTurretPose, NetCrew, NetTank, ServoAngles, SetSpawnPoint, protocol_id,
};
use super::{diagnostics, harness, open_gameplay_gate, physics, spawn_map};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::TankKnockedOut;
use crate::state::GameplaySet;
// The spawn-height rule and its constants live in `terrain_grid`: ONE rule for every spawn path in
// every composition (see `terrain_grid::spawn_surface_height`), so the offline duel, the lanes, the
// bot and the spawn-map overrides cannot drift apart. Spawn points here are HORIZONTAL (XZ) — Y is
// sampled at the moment of spawn.
use crate::tank::{
    PendingTankAssets, Rig, TankContent, TankSimSource, load_tank_sim_assets, spawn_complete_tank,
};
use crate::terrain_grid::spawn_pos;
// The spawn selector reads the world's standing geometry through the SAME encoding the analytic
// track field is built from (`world::TerrainMap` blocks → `TerrainBlock`), so the two cannot drift.
use crate::track::oracle::TerrainBlock;
use crate::world::TerrainMap;
use crate::{CombatantId, SimPlugin};

const PORT: u16 = 5888;

pub fn run() {
    let mut app = App::new();
    app.add_plugins(crate::gpu_less_default_plugins(None))
        // Headless composition needs its own application runner.
        .add_plugins(ScheduleRunnerPlugin::run_loop(Duration::from_millis(2)));
    // No `CompressedImageFormatSupport` here, unlike `headless_test.rs`: every texture on the
    // server's asset allowlist (`.github/actions/build-server/action.yml`) is png, and the UASTC
    // KTX2 sits behind plugins this root does not mount — `load_tank_sim_assets` opens no scene.
    app.add_plugins(ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(super::plugin);
    super::disclosure::install_server(&mut app);
    app.add_plugins(physics::physics_plugins());
    app.add_plugins(SimPlugin);
    app.add_plugins(crate::trace::server_plugin);
    app.add_plugins(crate::cost::server_plugin);
    app.add_plugins(crate::shot_trace::server_plugin);
    super::shot_transport::install_server(&mut app);
    // Authority must reject input targets not controlled by their sending client.
    app.add_input_validator(authorize_controlled_targets::<NativeStateSequence<TankCommand>>);
    // Lightyear visibility hooks require `ReplicationSender` on each remote link.
    app.add_observer(attach_replication_sender);

    let server = app
        .world_mut()
        .spawn((
            Name::new("Server"),
            NetcodeServer::new(NetcodeConfig {
                // Must match the client's `Authentication::Manual.protocol_id`.
                protocol_id: protocol_id(),
                private_key: [0; 32], // dev only — matches the client's Authentication::Manual
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), PORT)),
            ServerUdpIo::default(),
        ))
        .id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Start { entity: server });
        info!("server: starting, listening on 0.0.0.0:{PORT}");
    });
    // The SIM load: the spec sheet only. The dedicated server opens no view artifact — no scene,
    // no textures, no rungs (ADR-0035); the walk's geometry arrives through `bake`'s extraction of
    // `<id>.sim.glb`.
    app.add_systems(Startup, load_tank_sim_assets);
    app.init_resource::<PendingClients>();
    app.init_resource::<SpawnLane>();
    app.init_resource::<SpawnOverrides>();
    app.init_resource::<CombatantIds>();
    let config = harness::PerturbConfig {
        perturb: harness::env_flag("SPIKE_PERTURB", true),
    };
    info!("server: SPIKE_PERTURB={}", config.perturb);
    app.insert_resource(config);

    app.add_systems(
        Update,
        (
            handle_new_clients,
            // Drain the spawn-map lane at frame rate: lightyear clears an undrained `MessageReceiver`
            // in `Last` every frame (the same rule `grip::answer_resync_requests` follows in `Update`).
            receive_spawn_points,
            spawn_pending_tanks,
            spawn_bot,
            schedule_bot_respawn,
            respawn_dead_bots,
            open_gameplay_gate,
            diagnostics::log_sim_evidence,
            diagnostics::log_input_arrival,
        ),
    );
    // The input arrival-margin instrument: sampled per fixed tick BEFORE lightyear consumes the
    // buffers, so the margin describes exactly the read the server is about to make. The counter
    // this adds is what certifies the client's derived sync margins (`net::sync_margin`).
    app.init_resource::<diagnostics::InputArrival>();
    app.add_systems(
        FixedPreUpdate,
        diagnostics::sample_input_arrival.before(ServerInputSystems::UpdateActionState),
    );
    app.add_systems(
        FixedUpdate,
        (
            log_tank_commands,
            harness::perturb_after_delay,
            // Command writers must run before edge consumption.
            drive_bot.in_set(GameplaySet).before(ConsumeCommandEdges),
            respawn_player_tanks
                .in_set(GameplaySet)
                .before(ConsumeCommandEdges),
        ),
    );

    app.run();
}

/// Connected clients waiting for the server's next spawn pass. The simulation blueprint is eager,
/// so this queue batches link setup and spawn work rather than waiting on view assets.
#[derive(Resource, Default)]
struct PendingClients(Vec<(Entity, PeerId)>);

/// Match-local combatant identities. `next` is never reused; player links retain their id across
/// respawn and the optional bot retains its one id across its timed respawns.
#[derive(Resource, Default)]
struct CombatantIds {
    next: u64,
    players: bevy::platform::collections::HashMap<Entity, CombatantId>,
    bot: Option<CombatantId>,
}

impl CombatantIds {
    fn allocate(&mut self) -> CombatantId {
        self.next = self.next.checked_add(1).expect(
            "combatant-id allocator exhausted u64; match cannot allocate another combatant",
        );
        CombatantId(self.next)
    }

    fn player(&mut self, link: Entity) -> CombatantId {
        if let Some(id) = self.players.get(&link) {
            return *id;
        }
        let id = self.allocate();
        self.players.insert(link, id);
        id
    }

    fn bot(&mut self) -> CombatantId {
        if let Some(id) = self.bot {
            return id;
        }
        let id = self.allocate();
        self.bot = Some(id);
        id
    }
}

/// Insert the marker required by Lightyear's per-client visibility hooks before tank spawning.
pub(super) fn attach_replication_sender(add: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(add.entity).insert(ReplicationSender);
}

/// Monotonic spawn-lane allocator; concurrent tanks must not overlap at spawn.
#[derive(Resource, Default)]
struct SpawnLane(u32);

/// Occupancy radius (m) for spawn placement: a requested spawn point with any live tank root
/// within this XZ distance counts as occupied (conservative cylinder around the tank volume).
/// Two players clicking the same map point would otherwise spawn fully overlapping dynamic
/// colliders and let the solver fling them.
const SPAWN_OCCUPIED_RADIUS_M: f32 = 6.0;

/// Deterministic outward search pattern for an occupied spawn point: per-radius direction
/// tables of fixed unit vectors (exact constants — no trig at runtime, so every peer/replay
/// resolves the identical candidate), tried ring by ring through [`SPAWN_SEARCH_RINGS`]; the
/// first free candidate wins.
///
/// WHY per-radius counts: 8 fixed compass spokes leave 2π·48/8 ≈ 37.7 m arc gaps at the outer
/// ring — six whole occupancy diameters of free ground the search never sampled, so a crowded
/// click could fall back to the lane spawn with room in plain sight. Each ring's count is
/// `max(8, ceil(2πr / 8 m))` — candidates at most ~8 m of arc apart, comparable to the 6 m
/// occupancy radius — pinned by `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_8: [Vec2; 8] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(
        core::f32::consts::FRAC_1_SQRT_2,
        core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(0.0, 1.0),
    Vec2::new(
        -core::f32::consts::FRAC_1_SQRT_2,
        core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(-1.0, 0.0),
    Vec2::new(
        -core::f32::consts::FRAC_1_SQRT_2,
        -core::f32::consts::FRAC_1_SQRT_2,
    ),
    Vec2::new(0.0, -1.0),
    Vec2::new(
        core::f32::consts::FRAC_1_SQRT_2,
        -core::f32::consts::FRAC_1_SQRT_2,
    ),
];

/// 13 unit directions for the 16 m ring: cos/sin(2πk/13), CCW from +X — spacing
/// 2π·16/13 ≈ 7.73 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_13: [Vec2; 13] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.885456, 0.46472317),
    Vec2::new(0.56806475, 0.82298386),
    Vec2::new(0.12053668, 0.99270886),
    Vec2::new(-0.3546049, 0.9350162),
    Vec2::new(-0.7485108, 0.66312265),
    Vec2::new(-0.97094184, 0.23931566),
    Vec2::new(-0.97094184, -0.23931566),
    Vec2::new(-0.7485108, -0.66312265),
    Vec2::new(-0.3546049, -0.9350162),
    Vec2::new(0.12053668, -0.99270886),
    Vec2::new(0.56806475, -0.82298386),
    Vec2::new(0.885456, -0.46472317),
];
/// 19 unit directions for the 24 m ring: cos/sin(2πk/19), CCW from +X — spacing
/// 2π·24/19 ≈ 7.94 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_19: [Vec2; 19] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.94581723, 0.32469946),
    Vec2::new(0.7891405, 0.6142127),
    Vec2::new(0.54694813, 0.8371665),
    Vec2::new(0.24548548, 0.9694003),
    Vec2::new(-0.082579345, 0.9965845),
    Vec2::new(-0.40169543, 0.91577333),
    Vec2::new(-0.67728156, 0.7357239),
    Vec2::new(-0.87947375, 0.47594738),
    Vec2::new(-0.9863613, 0.16459459),
    Vec2::new(-0.9863613, -0.16459459),
    Vec2::new(-0.87947375, -0.47594738),
    Vec2::new(-0.67728156, -0.7357239),
    Vec2::new(-0.40169543, -0.91577333),
    Vec2::new(-0.082579345, -0.9965845),
    Vec2::new(0.24548548, -0.9694003),
    Vec2::new(0.54694813, -0.8371665),
    Vec2::new(0.7891405, -0.6142127),
    Vec2::new(0.94581723, -0.32469946),
];
/// 26 unit directions for the 32 m ring: cos/sin(2πk/26), CCW from +X — spacing
/// 2π·32/26 ≈ 7.73 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_26: [Vec2; 26] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.97094184, 0.23931566),
    Vec2::new(0.885456, 0.46472317),
    Vec2::new(0.7485108, 0.66312265),
    Vec2::new(0.56806475, 0.82298386),
    Vec2::new(0.3546049, 0.9350162),
    Vec2::new(0.12053668, 0.99270886),
    Vec2::new(-0.12053668, 0.99270886),
    Vec2::new(-0.3546049, 0.9350162),
    Vec2::new(-0.56806475, 0.82298386),
    Vec2::new(-0.7485108, 0.66312265),
    Vec2::new(-0.885456, 0.46472317),
    Vec2::new(-0.97094184, 0.23931566),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.97094184, -0.23931566),
    Vec2::new(-0.885456, -0.46472317),
    Vec2::new(-0.7485108, -0.66312265),
    Vec2::new(-0.56806475, -0.82298386),
    Vec2::new(-0.3546049, -0.9350162),
    Vec2::new(-0.12053668, -0.99270886),
    Vec2::new(0.12053668, -0.99270886),
    Vec2::new(0.3546049, -0.9350162),
    Vec2::new(0.56806475, -0.82298386),
    Vec2::new(0.7485108, -0.66312265),
    Vec2::new(0.885456, -0.46472317),
    Vec2::new(0.97094184, -0.23931566),
];
/// 32 unit directions for the 40 m ring: cos/sin(2πk/32), CCW from +X — spacing
/// 2π·40/32 ≈ 7.85 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_32: [Vec2; 32] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.98078525, 0.19509032),
    Vec2::new(0.9238795, 0.38268343),
    Vec2::new(0.8314696, 0.55557024),
    Vec2::new(0.70710677, 0.70710677),
    Vec2::new(0.55557024, 0.8314696),
    Vec2::new(0.38268343, 0.9238795),
    Vec2::new(0.19509032, 0.98078525),
    Vec2::new(0.0, 1.0),
    Vec2::new(-0.19509032, 0.98078525),
    Vec2::new(-0.38268343, 0.9238795),
    Vec2::new(-0.55557024, 0.8314696),
    Vec2::new(-0.70710677, 0.70710677),
    Vec2::new(-0.8314696, 0.55557024),
    Vec2::new(-0.9238795, 0.38268343),
    Vec2::new(-0.98078525, 0.19509032),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.98078525, -0.19509032),
    Vec2::new(-0.9238795, -0.38268343),
    Vec2::new(-0.8314696, -0.55557024),
    Vec2::new(-0.70710677, -0.70710677),
    Vec2::new(-0.55557024, -0.8314696),
    Vec2::new(-0.38268343, -0.9238795),
    Vec2::new(-0.19509032, -0.98078525),
    Vec2::new(0.0, -1.0),
    Vec2::new(0.19509032, -0.98078525),
    Vec2::new(0.38268343, -0.9238795),
    Vec2::new(0.55557024, -0.8314696),
    Vec2::new(0.70710677, -0.70710677),
    Vec2::new(0.8314696, -0.55557024),
    Vec2::new(0.9238795, -0.38268343),
    Vec2::new(0.98078525, -0.19509032),
];
/// 38 unit directions for the 48 m ring: cos/sin(2πk/38), CCW from +X — spacing
/// 2π·48/38 ≈ 7.94 m of arc (≤ the 8 m coverage target). Exact constants,
/// no runtime trig; near-zero components snapped to 0. Pinned by
/// `spawn_search_rings_are_unit_uniform_and_dense_enough`.
const SPAWN_DIRS_38: [Vec2; 38] = [
    Vec2::new(1.0, 0.0),
    Vec2::new(0.9863613, 0.16459459),
    Vec2::new(0.94581723, 0.32469946),
    Vec2::new(0.87947375, 0.47594738),
    Vec2::new(0.7891405, 0.6142127),
    Vec2::new(0.67728156, 0.7357239),
    Vec2::new(0.54694813, 0.8371665),
    Vec2::new(0.40169543, 0.91577333),
    Vec2::new(0.24548548, 0.9694003),
    Vec2::new(0.082579345, 0.9965845),
    Vec2::new(-0.082579345, 0.9965845),
    Vec2::new(-0.24548548, 0.9694003),
    Vec2::new(-0.40169543, 0.91577333),
    Vec2::new(-0.54694813, 0.8371665),
    Vec2::new(-0.67728156, 0.7357239),
    Vec2::new(-0.7891405, 0.6142127),
    Vec2::new(-0.87947375, 0.47594738),
    Vec2::new(-0.94581723, 0.32469946),
    Vec2::new(-0.9863613, 0.16459459),
    Vec2::new(-1.0, 0.0),
    Vec2::new(-0.9863613, -0.16459459),
    Vec2::new(-0.94581723, -0.32469946),
    Vec2::new(-0.87947375, -0.47594738),
    Vec2::new(-0.7891405, -0.6142127),
    Vec2::new(-0.67728156, -0.7357239),
    Vec2::new(-0.54694813, -0.8371665),
    Vec2::new(-0.40169543, -0.91577333),
    Vec2::new(-0.24548548, -0.9694003),
    Vec2::new(-0.082579345, -0.9965845),
    Vec2::new(0.082579345, -0.9965845),
    Vec2::new(0.24548548, -0.9694003),
    Vec2::new(0.40169543, -0.91577333),
    Vec2::new(0.54694813, -0.8371665),
    Vec2::new(0.67728156, -0.7357239),
    Vec2::new(0.7891405, -0.6142127),
    Vec2::new(0.87947375, -0.47594738),
    Vec2::new(0.94581723, -0.32469946),
    Vec2::new(0.9863613, -0.16459459),
];

/// The outward spawn search, inside the ~50 m budget: ring radii (m) ascending, each paired
/// with its direction table (see [`SPAWN_DIRS_8`] for the density rationale). Past the last
/// ring the caller falls back to the lane spawn.
const SPAWN_SEARCH_RINGS: [(f32, &[Vec2]); 6] = [
    (8.0, &SPAWN_DIRS_8),
    (16.0, &SPAWN_DIRS_13),
    (24.0, &SPAWN_DIRS_19),
    (32.0, &SPAWN_DIRS_26),
    (40.0, &SPAWN_DIRS_32),
    (48.0, &SPAWN_DIRS_38),
];

/// Per-client spawn overrides chosen on the client's spawn map, keyed by the CLIENT LINK entity —
/// the same key [`CombatantIds`] and `ControlledBy::owner` use, so a client can only ever move its
/// own spawn (the message carries no id to forge).
///
/// Lifecycle: set/overwritten by [`receive_spawn_points`] on every accepted request, read by
/// [`respawn_player_tanks`] on that client's next respawn, and PERSISTENT — it is NOT cleared on
/// use, so every subsequent respawn keeps landing at the last point the player picked until they
/// pick another. The initial join spawn ignores it entirely (lane logic owns first placement).
#[derive(Resource, Default)]
struct SpawnOverrides(bevy::platform::collections::HashMap<Entity, Vec2>);

/// Validate a requested spawn XZ: reject non-finite components outright (a NaN would NaN the solver
/// the moment the body is inserted), otherwise clamp into the placeable square
/// ([`spawn_map::spawn_limit`], inside the terrain edge). Pure, so the bound is unit-testable
/// without a world.
fn validate_spawn_request(half_extent: f32, request: SetSpawnPoint) -> Option<Vec2> {
    if !request.x.is_finite() || !request.z.is_finite() {
        return None;
    }
    Some(spawn_map::clamp_to_spawn_limit(
        half_extent,
        Vec2::new(request.x, request.z),
    ))
}

/// A lane's spawn POINT — horizontal, like every spawn definition. Lane 0 is the base point, so
/// the single-client case is unshifted.
pub(crate) fn lane_spawn_xz(base_xz: Vec2, lane: u32) -> Vec2 {
    base_xz + lane_offset(lane)
}

/// A lane pose: the lane's XZ resolved against the live surface (`terrain_grid::spawn_pos`). On the
/// flat-slab world this is exactly the old `y = 2.0` pose.
///
/// `harness_pose` is the ONE documented exception to horizontal-only spawn data: `SPIKE_SPAWN_POSE`
/// names an exact resting CONTACT for the beached-rest repro, so its Y is honoured verbatim and
/// never re-sampled. It is a measurement instrument, not a spawn definition — an ordinary run never
/// sets it.
fn lane_spawn_pos(
    harness_pose: Option<Vec3>,
    lane: u32,
    height: Option<&crate::terrain_grid::HeightGrid>,
) -> Vec3 {
    let xz = lane_spawn_xz(harness_pose.map_or(Vec2::ZERO, Vec3::xz), lane);
    match harness_pose {
        Some(pose) => Vec3::new(xz.x, pose.y, xz.y),
        None => spawn_pos(height, xz),
    }
}

/// The XZ ground a terrain block DENIES a spawn — its world footprint expanded by
/// [`SPAWN_OCCUPIED_RADIUS_M`], the same body radius a live tank occupies — or `None` for a block
/// that IS ground rather than something standing on it.
///
/// That distinction is the vertical test, taken against the block's OWN ground — the surface under
/// its centre, where `scatter` projected it — so the margin is the building's own height. A block
/// whose top does not clear that surface is drivable ground (the flat-slab world's 1500 m ground
/// block, a buried course slab) and denies nothing. Against the CANDIDATE's ground the margin
/// would instead be the roof's clearance over `spawn_pos`'s max-over-footprint sample, which the
/// shipped map cuts to 0.08 m on one house.
///
/// The footprint is the [`TerrainBlock`] encoding the analytic track field is built from
/// (`track::terrain::build_track_field`) — the unit cube posed by the Transform — taken as its
/// world AABB: exact for an axis-aligned block, conservative for a yawed one.
fn block_footprint(
    block: &Transform,
    height: Option<&crate::terrain_grid::HeightGrid>,
) -> Option<Rect> {
    let (min, max) = TerrainBlock::new(block.translation, block.rotation, block.scale).world_aabb();
    let ground = height.map_or(0.0, |grid| {
        grid.height_at(block.translation.x, block.translation.z)
    });
    (max.y > ground).then(|| {
        Rect::from_corners(Vec2::new(min.x, min.z), Vec2::new(max.x, max.z))
            .inflate(SPAWN_OCCUPIED_RADIUS_M)
    })
}

/// Whether a [`block_footprint`] denies `candidate`. The boundary is OPEN, exactly like the tank
/// test (`>=` the occupancy radius is free), so a spawn may stand against a wall at the same
/// clearance it may stand against a hull.
fn footprint_denies(footprint: Rect, candidate: Vec2) -> bool {
    candidate.cmpgt(footprint.min).all() && candidate.cmplt(footprint.max).all()
}

/// Resolve a requested spawn XZ against live tank positions (Finding: two players clicking the
/// same point spawned overlapping dynamic colliders) and against the standing terrain `blocks`
/// (Finding: nothing stopped a click from landing a tank inside a house — `spawn_pos` takes Y from
/// the ground alone). Returns the first UNOCCUPIED point — the request itself, or the first free
/// candidate of a deterministic outward ring search ([`SPAWN_SEARCH_RINGS`], candidates clamped
/// into the placeable square) — plus whether it was nudged; `None` when everything within ~50 m is
/// occupied (the caller falls back to the lane spawn). Pure, so the policy is unit-testable without
/// a world; `blocks` is the startup-fixed [`TerrainMap`] list, walked in its own order.
fn resolve_free_spawn_xz(
    half_extent: f32,
    desired: Vec2,
    occupied: &[Vec2],
    blocks: &[Transform],
    height: Option<&crate::terrain_grid::HeightGrid>,
) -> Option<(Vec2, bool)> {
    // What STANDS on the map, resolved once in the block list's own fixed order — the ground terms
    // drop out here rather than per candidate.
    let standing: Vec<Rect> = blocks
        .iter()
        .filter_map(|block| block_footprint(block, height))
        .collect();
    let free = |candidate: Vec2| {
        !occupied.iter().any(|tank| {
            candidate.distance_squared(*tank) < SPAWN_OCCUPIED_RADIUS_M * SPAWN_OCCUPIED_RADIUS_M
        }) && !standing
            .iter()
            .any(|footprint| footprint_denies(*footprint, candidate))
    };
    if free(desired) {
        return Some((desired, false));
    }
    for (radius, dirs) in SPAWN_SEARCH_RINGS {
        for &dir in dirs {
            let candidate = spawn_map::clamp_to_spawn_limit(half_extent, desired + dir * radius);
            if free(candidate) {
                return Some((candidate, true));
            }
        }
    }
    None
}

/// Record each client's latest spawn-map choice. The key is the SENDING LINK, so authority is
/// structural: nothing in the message names a target. Reliable-and-ordered lane, and only the last
/// accepted request survives — so the surviving override matches the player's latest click, which
/// is exactly the semantics of "where I want to come back".
fn receive_spawn_points(
    mut receivers: Query<(Entity, &mut MessageReceiver<SetSpawnPoint>), With<ClientOf>>,
    mut overrides: ResMut<SpawnOverrides>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
) {
    let half_extent = spawn_map::world_half_extent(height.as_deref());
    for (link, mut receiver) in &mut receivers {
        for request in receiver.receive() {
            let Some(xz) = validate_spawn_request(half_extent, request) else {
                warn!("server: rejected non-finite spawn request from link {link}");
                continue;
            };
            overrides.0.insert(link, xz);
            info!(
                "server: link {link} set next spawn to ({:.1}, {:.1})",
                xz.x, xz.y
            );
        }
    }
}

/// Symmetric X offsets around the base spawn point. HORIZONTAL, like the spawn points it shifts:
/// a lane fans a joining client sideways, it never decides how high the tank sits.
fn lane_offset(lane: u32) -> Vec2 {
    let step = lane.div_ceil(2) as f32 * 8.0;
    let sign = if lane % 2 == 1 { 1.0 } else { -1.0 };
    Vec2::new(sign * step, 0.0)
}

/// Queues each newly connected client for [`spawn_pending_tanks`] (one predicted tank per client,
/// owned by that client; every other client interpolates it).
fn handle_new_clients(
    new: Query<(Entity, &RemoteId), (Added<Connected>, With<ClientOf>)>,
    mut pending: ResMut<PendingClients>,
) {
    for (link, remote) in &new {
        info!("server: client connected: {remote} (link {link})");
        pending.0.push((link, remote.0));
    }
}

/// Spawns a complete authoritative tank for every queued client. View handles may still be
/// loading; the simulation body comes from the eager blueprint.
fn spawn_pending_tanks(
    mut pending: ResMut<PendingClients>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    config: Res<harness::PerturbConfig>,
    mut lane: ResMut<SpawnLane>,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    if pending.0.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // Harness override (`SPIKE_SPAWN_POSE`): place the tank onto a known resting contact for the
    // beached-rest repro; unset in every normal run, so the default flat-pad spawn stands.
    let harness_pose = harness::spawn_pose();
    let spawn_rot = harness_pose.map_or(Quat::IDENTITY, |(_, rot)| rot);
    let harness_pos = harness_pose.map(|(pos, _)| pos);
    for (link, client_id) in pending.0.drain(..) {
        // Fan each client out onto its own lane (lane 0 = the base pose, so the single-client and
        // `SPIKE_SPAWN_POSE` cases are unshifted); the counter persists so reconnects don't collide.
        // The lane's XZ is unchanged; only its Y follows the ground now (`lane_spawn_pos`), so a
        // join onto a hillside no longer starts inside the terrain.
        let spawn_pos = lane_spawn_pos(harness_pos, lane.0, height.as_deref());
        lane.0 += 1;
        let root = spawn_player_tank(
            &mut commands,
            content,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
            combatants.player(link),
        );
        if config.perturb {
            commands.entity(root).insert(harness::PendingPerturbation {
                at: time.elapsed() + Duration::from_secs(2),
            });
        }
    }
}

/// Construct an authoritative player tank. Initial join and respawn share this exact ownership
/// bundle so reacquisition cannot drift from first spawn.
fn spawn_player_tank(
    commands: &mut Commands,
    content: TankContent<'_>,
    assets: &PendingTankAssets,
    link: Entity,
    client_id: PeerId,
    spawn_pos: Vec3,
    spawn_rot: Quat,
    combatant: CombatantId,
) -> Entity {
    let root = spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Tank"),
                NetTank,
                combatant,
                Transform::default(),
                // Authority body role and colliders enter the same command flush.
                RigidBody::Dynamic,
                ActionState::<TankCommand>::default(),
                Position(spawn_pos),
                // Explicit wire pose prevents Avian's required-component placeholder entering history.
                Rotation(spawn_rot),
                ServoAngles::default(),
                NetCrew::default(),
                NetTankStatus::Active,
                LaunchedTurretPose::default(),
                CombatDisclosure::owner(link),
                Replicate::to_clients(NetworkTarget::All),
            ),
            (
                // Clients build their own local skeleton; replicate only root state.
                DisableReplicateHierarchy,
                // ONE TIMELINE: every client interpolates every hull, the owner's included — the
                // own hull renders and drives from the server stream exactly like an opponent's.
                InterpolationTarget::to_clients(NetworkTarget::All),
                // The owner marker, and the only one the client's game layer keys on: it rides
                // `ControlledBy`'s owner-scoped visibility, not the prediction target.
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ),
        ),
    );
    info!("server: spawned tank {root} for client {client_id} — owner interpolates");
    root
}

/// Marker for the ownerless test-bot tank ([`spawn_bot`]) — scopes [`drive_bot`] to it, and keeps
/// it out of every other tank query the server runs.
#[derive(Component)]
struct Bot;

/// The bot's home spawn POINT — horizontal, like every spawn definition (the old flat-pad
/// `(0, 12)`). Its Y is sampled at spawn time by the shared rule; the fixed `y = 2` pose it used to
/// carry measured 113.65 m underground once the heightmap world landed.
pub(crate) const BOT_SPAWN_XZ: Vec2 = Vec2::new(0.0, 12.0);

/// The bot's spawn pose: home XZ resolved against the live surface like every other spawn.
fn bot_spawn_pos(height: Option<&crate::terrain_grid::HeightGrid>) -> Vec3 {
    spawn_pos(height, BOT_SPAWN_XZ)
}

/// Spawn the optional ownerless interpolation-test bot once.
fn spawn_bot(
    mut spawned: Local<bool>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut combatants: ResMut<CombatantIds>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    // `is_err()` = the var is unset: present (even empty, e.g. `OVERMATCH_BOT=`) counts as on.
    if *spawned || std::env::var("OVERMATCH_BOT").is_err() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    *spawned = true;
    let pos = bot_spawn_pos(height.as_deref());
    let root = spawn_bot_entity(&mut commands, &assets, content, combatants.bot(), pos);
    info!(
        "server: spawned circling bot tank {root} (OVERMATCH_BOT) at ({:.1}, {:.1}, {:.1})",
        pos.x, pos.y, pos.z
    );
}

/// Construct the ownerless bot used by both initial spawn and respawn. `spawn_pos` comes from
/// [`bot_spawn_pos`] at both call sites — footprint-lifted onto the terrain, never a constant.
fn spawn_bot_entity(
    commands: &mut Commands,
    assets: &PendingTankAssets,
    content: TankContent<'_>,
    combatant: CombatantId,
    spawn_pos: Vec3,
) -> Entity {
    spawn_complete_tank(
        commands,
        content,
        assets.presentation(),
        (
            (
                Name::new("Bot"),
                Bot,
                // Name is not replicated; NetBot identifies it to the client HUD.
                super::protocol::NetBot,
                NetTank,
                combatant,
                Transform::default(),
                RigidBody::Dynamic,
                Position(spawn_pos),
                Rotation(Quat::IDENTITY),
                ServoAngles::default(),
                NetCrew::default(),
            ),
            (
                NetTankStatus::Active,
                LaunchedTurretPose::default(),
                CombatDisclosure::hidden(),
                Replicate::to_clients(NetworkTarget::All),
            ),
            (
                DisableReplicateHierarchy,
                // No owner or prediction target: every client interpolates this body.
                InterpolationTarget::to_clients(NetworkTarget::All),
            ),
        ),
    )
}

/// Schedules a bot's respawn the tick its root gains [`TankKnockedOut`] — the emergent death label
/// (`damage::mark_dead_tanks` at 0 living crew, `damage::process_cookoffs` on cookoff). Stamps the
/// virtual-clock instant 5 s out onto [`BotRespawnAt`]; the `Without<BotRespawnAt>` filter keeps a
/// second death label (e.g. crew-loss after a cookoff) from rescheduling. Reads the same
/// `Time<Virtual>` clock the respawn consumer and `spawn_pending_tanks` do.
fn schedule_bot_respawn(
    dead: Query<Entity, (With<Bot>, Added<TankKnockedOut>, Without<BotRespawnAt>)>,
    time: Res<Time<Virtual>>,
    mut commands: Commands,
) {
    for bot in &dead {
        commands
            .entity(bot)
            .insert(BotRespawnAt(time.elapsed_secs() + 5.0));
        info!("server: bot {bot} knocked out; respawning in 5s");
    }
}

/// When a scheduled [`BotRespawnAt`] comes due, sweep the dead bot and spawn a fresh one at the same
/// pose through the same blueprint-backed constructor.
fn respawn_dead_bots(
    dead: Query<(Entity, &BotRespawnAt, &Rig, &CombatantId), With<Bot>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    time: Res<Time<Virtual>>,
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    // (root, its Rig.turret) for every bot now due. Capture the turret handle BEFORE despawning the
    // root: if the bot cooked off, `damage::launch_turrets_on_cookoff` stripped the turret's
    // `ChildOf` and made it a free body, so it is NOT a descendant of the root and the recursive
    // root despawn below would miss it — leaking one launched turret per respawn.
    let due: Vec<(Entity, Entity, CombatantId)> = dead
        .iter()
        .filter(|(_, at, _, _)| now >= at.0)
        .map(|(root, _, rig, combatant)| (root, rig.turret, *combatant))
        .collect();
    if due.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    for (root, turret, combatant) in due {
        // Recursive despawn sweeps the root and its attached rig (children + relationship targets).
        commands.entity(root).despawn();
        // The launched turret, if it detached on cookoff. `try_despawn` is a silent no-op when the
        // turret is still an attached child (already swept above) or otherwise gone — no panic, no
        // double-free, so the one branch covers both the cookoff and crew-loss deaths.
        commands.entity(turret).try_despawn();
        let fresh = spawn_bot_entity(
            &mut commands,
            &assets,
            content,
            combatant,
            bot_spawn_pos(height.as_deref()),
        );
        info!("server: respawned bot as {fresh} (was {root})");
    }
}

/// Marker scheduling a bot respawn: the `Time<Virtual>` timestamp (secs) at which the dead bot is
/// swept and a fresh one spawned. Inserted by [`schedule_bot_respawn`], consumed by
/// [`respawn_dead_bots`].
#[derive(Component)]
struct BotRespawnAt(f32);

/// Server-authoritative PLAYER respawn (the friend-fight counterpart to the bot loop above): when a
/// client's own tank is knocked out and the client latches a [`TankCommand::respawn`] edge, sweep the
/// dead tank and spawn that client a fresh one through the same ownership path connect uses
/// ([`spawn_player_tank`]).
///
/// **The death is VALIDATED on the authority, never trusted from the client.** The query is gated
/// `With<TankKnockedOut>` — the emergent death label the server itself latched (`damage::mark_dead_tanks`
/// at 0 living crew, `damage::process_cookoffs` on cookoff) off its OWN authoritative sim. A client
/// that forges `respawn: true` while alive names a tank that carries no `TankKnockedOut`, so it never
/// matches and nothing happens. `With<ControlledBy>` (read as `&ControlledBy`) scopes this to owned
/// player tanks and excludes the ownerless bot, whose death→respawn is the separate timed
/// [`respawn_dead_bots`] loop — the two never overlap.
///
/// Runs on the fixed clock in `GameplaySet`, `.before(ConsumeCommandEdges)`, exactly like [`drive_bot`]
/// and every other command reader: `bridge_action_state_to_tank_command` has already written this
/// tick's `respawn` edge (and, under input starvation, already CLEARED it via `TankCommand::clear_edges`,
/// so a stale held-last input can't re-trigger a respawn), and `consume_edges` clears it at the tick's
/// end — so a single latched edge respawns exactly once. The recursive root despawn drops the dead rig;
/// `tank::sweep_launched_turret_on_root_despawn` (`On<Remove, Rig>`, mounted in `SimPlugin` on both
/// ends) sweeps a cooked-off turret that detached from the root, so no launched turret leaks — the same
/// guarantee `respawn_dead_bots` relies on.
fn respawn_player_tanks(
    dead: Query<(Entity, &TankCommand, &ControlledBy, &CombatantId), With<TankKnockedOut>>,
    remotes: Query<&RemoteId>,
    // Every tank root's live position — the occupancy set an override spawn checks against
    // (includes dead husks still on the field and the bot; excludes roots swept this pass).
    tanks: Query<(Entity, &Position), With<NetTank>>,
    assets: Res<PendingTankAssets>,
    source: TankSimSource,
    mut lane: ResMut<SpawnLane>,
    overrides: Res<SpawnOverrides>,
    // The authority's own ground truth for spawn-map placement; absent in the flat-slab world.
    height: Option<Res<crate::terrain_grid::HeightGrid>>,
    // The world's standing geometry (the scatter's buildings): occupancy an override spawn checks
    // against exactly like a live tank. Absent in bare test worlds.
    map: Option<Res<TerrainMap>>,
    mut commands: Commands,
) {
    // (dead root, owner link, owner client id) for every owned tank that both IS dead and asked to
    // respawn this tick. Resolve the owner's `RemoteId` up front (the `PeerId` the fresh tank must be
    // predicted/owned by); an owner link mid-disconnect with no `RemoteId` is skipped rather than
    // respawned to a client that is leaving.
    let requests: Vec<(Entity, Entity, PeerId, CombatantId)> = dead
        .iter()
        .filter(|(_, command, _, _)| command.respawn)
        .filter_map(|(root, _, controlled, combatant)| {
            remotes
                .get(controlled.owner)
                .ok()
                .map(|remote| (root, controlled.owner, remote.0, *combatant))
        })
        .collect();
    if requests.is_empty() {
        return;
    }
    let Some(content) = source.get() else {
        return;
    };
    // A respawn takes the NEXT free lane (never reset — same rule a reconnecting client follows), so a
    // fresh tank never lands on top of another body and NaNs the solver. The base pose honors the
    // `SPIKE_SPAWN_POSE` harness override exactly as the connect path does.
    let harness_pose = harness::spawn_pose();
    let spawn_rot = harness_pose.map_or(Quat::IDENTITY, |(_, rot)| rot);
    let harness_pos = harness_pose.map(|(pos, _)| pos);
    // Roots being swept this pass are NOT occupancy: their colliders despawn in the same command
    // flush the fresh tanks spawn in.
    let sweeping: Vec<Entity> = requests.iter().map(|(root, ..)| *root).collect();
    // Occupancy also includes the spots ALREADY HANDED OUT this pass: two clients respawning onto
    // the same click in the same tick would otherwise both read the pre-pass world and overlap.
    let mut placed_this_pass: Vec<Vec2> = Vec::new();
    for (root, link, client_id, combatant) in requests {
        // A spawn-map choice REPLACES the lane pose for this respawn (and stays chosen for the next
        // one — see `SpawnOverrides`); the lane counter is left alone so the fallback placement is
        // unchanged for everyone still using it. The Y is resolved on the authority.
        let spawn_pos = match overrides.0.get(&link) {
            Some(&xz) => {
                let occupied: Vec<Vec2> = tanks
                    .iter()
                    .filter(|(tank, _)| !sweeping.contains(tank))
                    .map(|(_, position)| position.0.xz())
                    .chain(placed_this_pass.iter().copied())
                    .collect();
                match resolve_free_spawn_xz(
                    spawn_map::world_half_extent(height.as_deref()),
                    xz,
                    &occupied,
                    map.as_ref().map_or(&[][..], |map| &map.blocks),
                    height.as_deref(),
                ) {
                    Some((spot, nudged)) => {
                        if nudged {
                            info!(
                                "server: spawn override ({:.1}, {:.1}) occupied — nudged to ({:.1}, {:.1})",
                                xz.x, xz.y, spot.x, spot.y
                            );
                        }
                        Some(spawn_pos(height.as_deref(), spot))
                    }
                    None => {
                        info!(
                            "server: spawn override ({:.1}, {:.1}) and every candidate within \
                             {} m occupied — falling back to the lane spawn",
                            xz.x,
                            xz.y,
                            SPAWN_SEARCH_RINGS[SPAWN_SEARCH_RINGS.len() - 1].0
                        );
                        None
                    }
                }
            }
            None => None,
        }
        .unwrap_or_else(|| {
            let pos = lane_spawn_pos(harness_pos, lane.0, height.as_deref());
            lane.0 += 1;
            pos
        });
        placed_this_pass.push(spawn_pos.xz());
        // Recursive despawn sweeps the dead root and its attached rig; the `On<Remove, Rig>` observer
        // handles any cooked-off turret that had detached (see the system doc).
        commands.entity(root).despawn();
        let fresh = spawn_player_tank(
            &mut commands,
            content,
            &assets,
            link,
            client_id,
            spawn_pos,
            spawn_rot,
            combatant,
        );
        info!("server: player {client_id} respawn requested — swept {root}, spawned {fresh}");
    }
}

/// Drive the bot in a steady circle AND hold its main gun's trigger: constants written straight into
/// its own `TankCommand` (a required component of `Tank`). The bot carries no `ActionState`, so
/// `bridge_action_state_to_tank_command` (protocol.rs)
/// never touches it — this is the sole writer. Ordered in `GameplaySet` before the edge-consumer,
/// with the other command writers; the fields are levels (never cleared), so it circles and fires for
/// good. Firing makes the bot a self-firing target that exercises remote shot presentation, recoil,
/// and hit-reaction paths without a second client.
fn drive_bot(mut bots: Query<&mut TankCommand, With<Bot>>) {
    for mut command in &mut bots {
        // Gentle constants: enough drive + differential yaw to circle on the flat pad without
        // leaving it or flipping. Everything else stays at `TankCommand::default()`.
        command.throttle = 0.5;
        command.steer = 0.5;
        // Hold primary fire: the gun fires each time its reload completes (so ~one main-gun shot per
        // reload), forward/unaimed as it circles. Aiming at a player is later bot-AI work; for now
        // this is purely the solo test-fire source described above.
        command.fire_primary = true;
    }
}

/// Log commands received through Lightyear's input buffer.
fn log_tank_commands(states: Query<(Entity, &ActionState<TankCommand>)>) {
    for (entity, state) in &states {
        let cmd = &state.0;
        if cmd.throttle != 0.0 || cmd.fire_primary {
            info!(
                "server: {entity} command: throttle={} steer={} fire_primary={}",
                cmd.throttle, cmd.steer, cmd.fire_primary
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The shared spawn rule these tests assert the server composes correctly.
    use crate::terrain_grid::spawn_surface_height;

    #[test]
    fn combatant_ids_are_nonzero_unique_and_retained_for_respawn() {
        let mut world = World::new();
        let player_a = world.spawn_empty().id();
        let player_b = world.spawn_empty().id();
        let mut ids = CombatantIds::default();

        let first_life = ids.player(player_a);
        let respawn = ids.player(player_a);
        let other_player = ids.player(player_b);
        let bot_first_life = ids.bot();
        let bot_respawn = ids.bot();

        assert_ne!(first_life, CombatantId(0));
        assert_eq!(
            first_life, respawn,
            "a player keeps its match identity on respawn"
        );
        assert_eq!(
            bot_first_life, bot_respawn,
            "the bot keeps its match identity on respawn"
        );
        assert_ne!(first_life, other_player);
        assert_ne!(first_life, bot_first_life);
        assert_ne!(other_player, bot_first_life);
    }

    /// An in-bounds request passes through untouched; an out-of-bounds one is CLAMPED into the
    /// playable square rather than refused, so a click near the map edge still places you.
    #[test]
    fn spawn_requests_are_clamped_into_the_world() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let limit = spawn_map::spawn_limit(half);
        assert_eq!(
            validate_spawn_request(half, SetSpawnPoint { x: 40.0, z: -90.0 }),
            Some(Vec2::new(40.0, -90.0)),
        );
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: 99_000.0,
                    z: -99_000.0,
                }
            ),
            Some(Vec2::new(limit, -limit)),
        );
    }

    /// A non-finite request is REFUSED outright: a NaN spawn would NaN the solver the moment the
    /// body enters the world, and no clamp can repair it.
    #[test]
    fn non_finite_spawn_requests_are_refused() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: f32::NAN,
                    z: 0.0
                }
            ),
            None,
        );
        assert_eq!(
            validate_spawn_request(
                half,
                SetSpawnPoint {
                    x: 0.0,
                    z: f32::INFINITY,
                }
            ),
            None,
        );
    }

    /// THE regression guard for "tanks spawn underground", netcode half: every spawn point this
    /// layer owns — the lanes a joining client fans out onto, the interpolation bot, and the
    /// extreme corners a spawn-map click can be clamped to — resolved through the shared rule
    /// against the REAL heightmap.
    ///
    /// The sim half lives in `terrain_grid` and covers the offline duel; it cannot cover these,
    /// because `tests/net_boundary.rs` forbids sim code from naming `crate::net`. Same rule, same
    /// assertion helper, asserted from the layer that owns the points.
    #[test]
    fn every_netcode_spawn_point_lands_above_the_shipped_terrain() {
        use crate::terrain_grid::tests::{assert_spawn_clears_terrain, shipped_grid};
        let grid = shipped_grid();
        assert_spawn_clears_terrain(&grid, "interpolation bot", BOT_SPAWN_XZ);
        // Lanes fan out from the origin; 8 covers far more clients than a match holds.
        for lane in 0..8 {
            assert_spawn_clears_terrain(
                &grid,
                &format!("server lane {lane}"),
                lane_spawn_xz(Vec2::ZERO, lane),
            );
        }
        // The extremes a spawn-map click can reach, which the authority clamps to before resolving.
        let limit = spawn_map::spawn_limit(grid.half_extent());
        for (name, xz) in [
            ("spawn-map corner -,-", Vec2::new(-limit, -limit)),
            ("spawn-map corner +,+", Vec2::new(limit, limit)),
            ("spawn-map corner -,+", Vec2::new(-limit, limit)),
            ("spawn-map corner +,-", Vec2::new(limit, -limit)),
        ] {
            assert_spawn_clears_terrain(&grid, name, xz);
        }
    }

    /// The override reproduces the flat-pad spawn's clearance: XZ from the client, Y SAMPLED by the
    /// authority from the ground it owns, plus the same 2 m every other spawn uses.
    #[test]
    fn override_pose_keeps_the_lane_spawn_clearance() {
        use crate::terrain_grid::{GRID_RESOLUTION, HeightGrid};
        assert_eq!(
            spawn_pos(None, Vec2::new(10.0, -20.0)),
            Vec3::new(10.0, 2.0, -20.0),
            "over flat ground this is exactly the default lane spawn's y",
        );
        // A world sitting 37.5 m up: the client still only names XZ.
        let n = GRID_RESOLUTION as usize;
        let hill = HeightGrid::new(
            vec![37.5f32; n * n].into(),
            GRID_RESOLUTION,
            crate::terrain_grid::FIXTURE_EXTENT,
        );
        assert_eq!(
            spawn_pos(Some(&hill), Vec2::new(10.0, -20.0)),
            Vec3::new(10.0, 39.5, -20.0),
        );
    }

    /// The lane spawn keeps its XZ and its 2 m clearance; with no height grid (the flat-slab world)
    /// it is byte-for-byte the pose the server used before terrain. A `SPIKE_SPAWN_POSE` override is
    /// passed through untouched — it names an exact resting contact, so re-sampling it would break
    /// the beached-rest repro (the one documented exception to horizontal-only spawn data).
    #[test]
    fn lane_spawn_keeps_its_xz_and_clearance() {
        assert_eq!(lane_spawn_pos(None, 0, None), Vec3::new(0.0, 2.0, 0.0));
        let fanned = lane_spawn_pos(None, 1, None);
        let offset = lane_offset(1);
        assert_eq!(
            fanned,
            Vec3::new(offset.x, 2.0, offset.y),
            "lane fan-out is unchanged over flat ground",
        );
        let harness = Vec3::new(3.0, 0.42, -7.0);
        assert_eq!(
            lane_spawn_pos(Some(harness), 0, None),
            harness,
            "a harness spawn pose is never re-sampled",
        );
    }

    #[test]
    #[should_panic(expected = "combatant-id allocator exhausted u64")]
    fn combatant_id_exhaustion_is_not_silently_reused() {
        let mut ids = CombatantIds {
            next: u64::MAX,
            ..default()
        };
        let _ = ids.allocate();
    }

    /// The occupancy policy: a free request passes through untouched; a request on top of a live
    /// tank is nudged to the FIRST free candidate of the deterministic ring search; a fully
    /// crowded neighbourhood returns `None` (lane-spawn fallback).
    #[test]
    fn occupied_spawn_requests_are_nudged_deterministically() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        // Free field: untouched, not nudged.
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &[], &[], None),
            Some((desired, false))
        );
        // A tank exactly at the radius is NOT blocking (>= is free)…
        let at_radius = desired + Vec2::new(SPAWN_OCCUPIED_RADIUS_M, 0.0);
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &[at_radius], &[], None),
            Some((desired, false)),
        );
        // …but one inside it is, and the search takes candidates in fixed ring order: the
        // occupant at +5.9 m X blocks the request, blocks +X@8 (2.1 m away) and NE@8 (5.66 m
        // away), so the first FREE candidate is +Z@8 — deterministically.
        let blocking = desired + Vec2::new(SPAWN_OCCUPIED_RADIUS_M - 0.1, 0.0);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[blocking], &[], None).expect("a ring is free");
        assert!(nudged);
        assert_eq!(
            spot,
            desired + SPAWN_DIRS_8[2] * SPAWN_SEARCH_RINGS[0].0,
            "first free candidate in fixed ring order"
        );
        // A tank ON the point (opponent parked there): first candidate +X@8 is 8 m away — free.
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[desired], &[], None).expect("a ring is free");
        assert!(nudged);
        assert_eq!(spot, desired + Vec2::new(SPAWN_SEARCH_RINGS[0].0, 0.0));
        // Every candidate occupied (a tank parked on each): lane fallback.
        let mut crowd = vec![desired];
        for (radius, dirs) in SPAWN_SEARCH_RINGS {
            for &dir in dirs {
                crowd.push(desired + dir * radius);
            }
        }
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &crowd, &[], None),
            None
        );
        // Candidates near the map edge clamp into the placeable square.
        let limit = spawn_map::spawn_limit(half);
        let corner = Vec2::new(limit, limit);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, corner, &[corner], &[], None).expect("a ring is free");
        assert!(nudged);
        assert!(spot.x.abs() <= limit && spot.y.abs() <= limit);
    }

    /// A building block in the [`TerrainMap`] encoding (`world::spawn_block` / `scatter`): a
    /// `size`-metre box standing on a surface at y = 0, centred at `center` in XZ, its bottom
    /// `sink` metres under that surface (the house proxy's skirt).
    fn building(center: Vec2, size: Vec3, sink: f32) -> Transform {
        Transform::from_xyz(center.x, size.y / 2.0 - sink, center.y).with_scale(size)
    }

    /// The building rule: a spawn inside a house is refused and the ring search lands the tank
    /// outside its walls, while the flat-slab world's GROUND block — a block whose top IS the
    /// surface it defines — denies nothing (it would otherwise swallow the whole placeable square).
    #[test]
    fn spawns_inside_a_building_are_refused_and_the_ground_block_is_not() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        // A hall 10 m × 14 m in XZ around the request: with the 6 m clearance its footprint
        // covers every ring-8 candidate (max |dx| = 8 < 11, |dz| = 8 < 13), so the first free
        // candidate is ring 16's +X — deterministically, in fixed ring order.
        let hall = building(desired, Vec3::new(10.0, 6.0, 14.0), 0.5);
        let (spot, nudged) =
            resolve_free_spawn_xz(half, desired, &[], &[hall], None).expect("a ring is free");
        assert!(nudged);
        assert_eq!(spot, desired + SPAWN_DIRS_13[0] * SPAWN_SEARCH_RINGS[1].0);
        let walls = block_footprint(&hall, None).expect("a building stands on the map");
        assert!(
            !footprint_denies(walls, spot),
            "the resolved spot must clear the building's walls"
        );
        // The flat-slab world's ground: a 1500 m block whose top face IS the surface every spawn
        // stands on. Under a footprint-only rule it would occupy the entire world.
        let ground = Transform::from_xyz(0.0, -0.5, 0.0).with_scale(Vec3::new(1500.0, 1.0, 1500.0));
        assert_eq!(
            block_footprint(&ground, None),
            None,
            "the ground is not a wall"
        );
        assert_eq!(
            resolve_free_spawn_xz(half, desired, &[], &[ground], None),
            Some((desired, false)),
        );
        // A house standing ON that ground is still occupancy — the ground term never masks it.
        let house = building(desired, Vec3::new(4.0, 3.0, 6.0), 0.5);
        let (spot, nudged) = resolve_free_spawn_xz(half, desired, &[], &[ground, house], None)
            .expect("a ring is free");
        assert!(nudged);
        assert!(!footprint_denies(
            block_footprint(&house, None).expect("a house stands on the map"),
            spot,
        ));
    }

    /// A click on a house behaves EXACTLY like a click on an occupied spot — the clicked point is
    /// validated the same way (clamped into the placeable square) and resolved through the same
    /// ring search, so the tank is nudged to free ground rather than dropped inside the walls.
    #[test]
    fn a_click_on_a_house_resolves_like_a_click_on_an_occupied_spot() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let click = SetSpawnPoint { x: 60.0, z: 20.0 };
        let xz = validate_spawn_request(half, click).expect("a finite click is accepted");
        // The house proxy's own footprint (4 m × 6 m), centred on the click.
        let house = building(xz, Vec3::new(4.0, 3.0, 6.0), 0.5);
        let on_house =
            resolve_free_spawn_xz(half, xz, &[], &[house], None).expect("a ring is free");
        let on_tank = resolve_free_spawn_xz(half, xz, &[xz], &[], None).expect("a ring is free");
        assert_eq!(on_house, on_tank, "a house is occupancy like a tank is");
        assert_eq!(
            on_house,
            (xz + Vec2::new(SPAWN_SEARCH_RINGS[0].0, 0.0), true)
        );
    }

    /// The ring tables are what their doc claims: per-ring count `max(8, ceil(2πr / 8 m))` (so
    /// candidate spacing never exceeds ~8 m of arc — the 8-spoke pattern left 37.7 m gaps at
    /// 48 m), radii ascending, and every entry the exact unit vector `(cos, sin)(2πk/n)` CCW
    /// from +X. Test-side trig only — the tables themselves are constants.
    #[test]
    fn spawn_search_rings_are_unit_uniform_and_dense_enough() {
        let mut previous = 0.0_f32;
        for (radius, dirs) in SPAWN_SEARCH_RINGS {
            assert!(radius > previous, "ring radii must ascend");
            previous = radius;
            let n = dirs.len();
            let want = ((core::f32::consts::TAU * radius / 8.0).ceil() as usize).max(8);
            assert_eq!(n, want, "ring {radius} m needs {want} directions, has {n}");
            for (k, dir) in dirs.iter().enumerate() {
                let angle = core::f32::consts::TAU * k as f32 / n as f32;
                let exact = Vec2::new(angle.cos(), angle.sin());
                assert!(
                    (*dir - exact).length() < 1e-6,
                    "ring {radius} m dir {k} is {dir:?}, wants {exact:?}"
                );
            }
        }
        assert_eq!(
            SPAWN_SEARCH_RINGS[SPAWN_SEARCH_RINGS.len() - 1].0,
            48.0,
            "the ~50 m search budget is unchanged"
        );
    }

    /// The between-spokes case the 8-spoke pattern MISSED: park tanks on every candidate the
    /// old search would have tried (all inner-ring candidates plus the 8 compass points at
    /// 48 m) — the old search returned `None` (lane fallback) with whole occupancy-diameters of
    /// free ground between the outer spokes; the dense rings find the spoke neighbour ~7.9 m of
    /// arc away, deterministically.
    #[test]
    fn outer_ring_between_spokes_gap_is_searched() {
        let half = crate::terrain_grid::FIXTURE_EXTENT.half_extent();
        let desired = Vec2::new(100.0, -50.0);
        let mut crowd = vec![desired];
        // Every candidate of every ring below 48 m…
        for (radius, dirs) in &SPAWN_SEARCH_RINGS[..SPAWN_SEARCH_RINGS.len() - 1] {
            for &dir in *dirs {
                crowd.push(desired + dir * *radius);
            }
        }
        // …plus exactly the old 8 compass spokes at 48 m.
        for dir in SPAWN_DIRS_8 {
            crowd.push(desired + dir * 48.0);
        }
        let (spot, nudged) = resolve_free_spawn_xz(half, desired, &crowd, &[], None)
            .expect("a between-spokes candidate is free");
        assert!(nudged);
        // The first free candidate in fixed order: ring 48's direction 1 (9.47° — 7.93 m of arc
        // from the occupied +X spoke, outside the 6 m occupancy radius).
        assert_eq!(spot, desired + SPAWN_DIRS_38[1] * 48.0);
        assert!(
            (spot - desired).length() > 47.9,
            "the free spot sits on the outer ring"
        );
        for tank in &crowd {
            assert!(
                spot.distance(*tank) >= SPAWN_OCCUPIED_RADIUS_M,
                "the found spot must clear every occupant"
            );
        }
    }

    /// The footprint ground query (Finding: sampling only the tank-root point buried the uphill
    /// axles 1.65 m on a legal slope click): spawn Y reads the MAXIMUM grid height over the
    /// 10 m × 10 m footprint, so a bump 2.5 m off-center lifts the spawn even though the
    /// center-point height is 0.
    #[test]
    fn spawn_height_samples_the_tank_footprint_not_the_point() {
        use crate::terrain_grid::{GRID_RESOLUTION, HeightGrid};
        // The production lattice (~0.977 m spacing), flat except one 10 m node one step off the
        // origin — inside the footprint square of a spawn at the origin, outside its centre point.
        let n = GRID_RESOLUTION as usize;
        let mut samples = vec![0.0f32; n * n];
        let center = n / 2; // node at world 0
        samples[center * n + (center + 1)] = 10.0;
        let grid = HeightGrid::new(
            samples.into(),
            GRID_RESOLUTION,
            crate::terrain_grid::FIXTURE_EXTENT,
        );
        assert_eq!(grid.height_at(0.0, 0.0), 0.0, "center-point height is flat");
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::ZERO),
            10.0,
            "the footprint max sees the off-centre bump"
        );
        // Far away the footprint reads the flat ground.
        assert_eq!(
            spawn_surface_height(Some(&grid), Vec2::new(500.0, 500.0)),
            0.0
        );
        // And the pose composes it with the standard clearance.
        assert_eq!(
            spawn_pos(Some(&grid), Vec2::ZERO),
            Vec3::new(0.0, 12.0, 0.0)
        );
        // No grid = the flat-slab world: y = 2 exactly, for players and the bot alike.
        assert_eq!(spawn_surface_height(None, Vec2::ZERO), 0.0);
        assert_eq!(bot_spawn_pos(None), Vec3::new(0.0, 2.0, 12.0));
    }
}
