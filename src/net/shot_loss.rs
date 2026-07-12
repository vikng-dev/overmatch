//! THE MODEL-VS-REALITY TEST for the fire-replication redundancy window (ADR-0021 piece 3).
//!
//! The existing redundancy tests (`net::client`'s `redundancy_window_delivers_every_shot_exactly_once
//! _under_loss`) prove the WINDOW LOGIC against a MODEL of the window: they hand `SeenShots` a
//! hand-rolled sequence of bursts with holes punched in it. That is a real proof of the dedup ring —
//! and it proves nothing about lightyear's actual channel, serialization, entity mapping, sequencing,
//! or the `FireRings` retention arithmetic under a lossy link. This module closes that gap: it stands
//! up a REAL server app and a REAL client app, connects them over REAL UDP through the REAL netcode
//! handshake, conditions the link with seeded packet loss, and asserts the end-to-end property the
//! whole design rests on:
//!
//!   **every shot the authority fires spawns EXACTLY ONE cosmetic shell on the observer** — none lost
//!   (the redundancy window repairs the drops), none duplicated (the `ShotId` dedup rejects the
//!   re-carried copies) — **and a ricochet the authority sanctions carries through** to the observer's
//!   shell (the keyframe survives the loss and re-seeds it).
//!
//! # What is REAL here, and what is not
//!
//! REAL: both `App`s, `ServerPlugins`/`ClientPlugins`, the shared `protocol::plugin` wire registration
//! (so the `FireChannel`'s sequenced-unreliable mode, `FireBurst`'s serialization, and `MapEntities`
//! all run for real), the netcode connect handshake with the production `PROTOCOL_FINGERPRINT`, UDP
//! sockets on loopback, `FireRings`' time-based retention, the push observers
//! (`broadcast_fire`/`on_shell_ricochet`) AND the clock-driven send site
//! (`broadcast_fire_window`, scheduled `.after(GameplaySet)` exactly as production does), a genuinely
//! REPLICATED shooter tank (so `FireEvent::shooter` is entity-MAPPED onto a real replica and passes the
//! client's `shooter_is_live` gate — see [`ShooterTank`]), the client's `receive_fire_events` dedup,
//! and the ballistics march on both ends (the server's shell genuinely ricochets off a plate; the
//! client's cosmetic shell genuinely holds at armor and re-seeds).
//!
//! That the sender is real matters more than it used to. The redundancy used to ride the events
//! themselves — one burst per fire/ricochet — so an entry's resend count was set by whatever traffic
//! happened to follow it. It is now sent by the CLOCK, every tick the window is non-empty, which is
//! what gives an isolated bounce its copies; this test drives that system under real loss, so the
//! carry-through assertion below is now a statement about the fix, not merely about the retention.
//!
//! NOT real (and deliberately so): the tank rig. Shots are fired by a test system straight into the
//! `FireShell` seam `shooting::fire` raises, from a fixed muzzle at a fixed plate — so the test is
//! about the WIRE, not about aiming. The two worlds' plates sit at the same pose, i.e. ZERO
//! interpolated-pose divergence: this test's subject is the channel under loss, not F3's
//! client-miss/server-hit class (which the `SPIKE_SHOT_TRACE` recorder measures in a live run).
//!
//! # WHICH LAYER IS CONDITIONED — and what that does and does not prove
//!
//! lightyear ships a link conditioner (`LinkConditionerConfig { incoming_loss }`, which
//! `net::client::run` already mounts for latency), and it is the natural place to induce loss — but it
//! draws from `rand::rng()`, the THREAD rng: it cannot be seeded, so a conditioner-driven test is a
//! coin-flip test, and a flaky netcode test is worse than none. So the loss is injected at OUR seam,
//! at the SAME point in the pipeline the conditioner drops from: [`drop_packets`] runs inside
//! lightyear's `LinkSystems::Receive`, after `LinkReceiveSystems::ApplyConditioner`, and drops whole
//! `RecvPayload`s out of the client's inbound `Link` buffer with a seeded LCG before any lightyear
//! system sees them.
//!
//! That means the drop is CONTENT-BLIND (it hits netcode keepalives, replication packets, and
//! `FireBurst`s alike — a real packet drop, not a message-level cheat) and lands BELOW every layer
//! under test (transport channel sequencing, message deserialization, the dedup). What it does NOT
//! exercise: the UDP socket's own loss behaviour and the kernel's buffering — irrelevant, since a
//! dropped datagram is indistinguishable from one the socket never delivered.
//!
//! # Determinism
//!
//! The drop decisions come from a seeded LCG, and both apps run on `TimeUpdateStrategy::
//! ManualDuration` (one fixed tick per `update()`), so the tick timeline is exact. The residual
//! non-determinism is the loopback socket's delivery timing — a packet may be read one update later on
//! a loaded machine, which shifts WHICH payload a given drop decision lands on. The assertions are
//! therefore properties (exactly-once, full carry-through) rather than an exact packet fate, and the
//! run is sized so the redundancy window covers many multiples of the induced loss.
//!
//! # Measured, and why the assertion has teeth
//!
//! At the [`LOSS`] this test asserts at (10%): 20 shots, 20 sanctioned ricochets, 39/407 payloads
//! dropped (9.6% observed) — every shot spawned exactly one shell, every bounce carried through.
//!
//! The teeth are at the top of the curve, and moving the send onto the CLOCK moved them a long way.
//! The same run, same seed, same fire script — only [`LOSS`] turned up:
//!
//! | induced loss | event-driven send (before)  | `broadcast_fire_window` (now)  |
//! |--------------|-----------------------------|--------------------------------|
//! | 10%          | pass                        | pass (39/407 payloads dropped) |
//! | 50%          | **FAIL** — 2/20 unspawned   | pass (187/408, 45.8% observed) |
//! | 80%          | —                           | pass (371/458, 81.0% observed) |
//! | 90%          | —                           | **FAIL** — 2/20 unspawned      |
//!
//! The property now survives ~80% packet loss and breaks by 90%: the window degrades exactly where the
//! design says it must (a scheme with NO redundancy loses ~10 of 20 shots at 50%), so the pass at 10%
//! is the window working, not the test being vacuous. The gap between those columns is what sending on
//! the clock BOUGHT — every event rides its full retain window of datagrams instead of however many
//! events happened to follow it, so only a near-total blackout drops them all. (In a live run the
//! `SPIKE_SHOT_TRACE` recorder's `send` rows count those copies per event directly; in this harness
//! they measure 21 copies per fire and per keyframe, of which 16 land inside the observer's hold.)
//!
//! # A REAL RACE this test surfaced (not fixed here — instrument first, per this slice's remit)
//!
//! `FireEvent::shooter` is entity-mapped on receipt, and lightyear's receive-side mapper falls back to
//! **`Entity::PLACEHOLDER`** when the shooter has no replica in the client's entity map yet (verified
//! live in this harness, which replicates no tank). The mapped shooter is an input to `ShotId`, so a
//! `FireEvent` that arrives BEFORE its shooter's tank replica does gets a PLACEHOLDER-keyed id, while
//! the shot's `RicochetKeyframe`/`ImpactConfirm` — arriving later, once the replica exists — keys on
//! the REAL replica entity. The two ids then disagree, the sanctioned outcome never correlates with the
//! shell, and the round holds and quietly dissolves.
//!
//! In production the tank's spawn is replicated before it can fire, so the map is normally populated
//! first — but replication and the cosmetic [`super::protocol::FireChannel`] are DIFFERENT channels
//! with no ordering between them, and the fire channel is unreliable: under loss (or right at a
//! spawn/respawn) a fire burst can legitimately land before the spawn it belongs to. The blast radius
//! is one dissolved cosmetic round (damage is server-authoritative), which is why this is filed, not
//! panicked over. The `SPIKE_SHOT_TRACE` recorder will show it as a `spawn` with no `hold`-resolution
//! and a `never_consumed` outcome in `scripts/shot/analyze.py`'s carry-through table.

use core::time::Duration;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, PhysicsPlugins, Position, RigidBody, Rotation,
};
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
// The client and server preludes each export a DIFFERENT `NetcodeConfig`, so they are imported by
// item (with the server's aliased) rather than by glob — the ambiguity is real, not incidental.
use lightyear::link::LinkReceiveSystems;
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, InputDelayConfig, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::client::{
    PendingRecoilKicks, SeenShots, age_sanctioned_shots, publish_predicted_present,
    receive_fire_events,
};
use super::protocol::{NetTank, PROTOCOL_FINGERPRINT};
use super::server::{
    FireRings, attach_replication_sender, broadcast_fire, broadcast_fire_window, on_shell_ricochet,
};
use crate::ballistics::{BallisticVolume, FireShell, Impact, SanctionedShots, Shot, ShotSource};
use crate::{ClientReplica, Layer, ShotId};

/// The induced packet-loss rate on the client's inbound link — 10%, the top of
/// `LinkConditionerConfig::poor_condition()`'s range (lightyear's own "high-latency, lossy connection"
/// preset). Deliberately at the pessimistic end: the property must hold on a bad link, not a good one.
const LOSS: f32 = 0.10;

/// Fixed seed for the drop decisions. A netcode test that flakes is worse than no test (the whole
/// reason the loss is injected here rather than through lightyear's thread-rng conditioner).
const SEED: u64 = 0xC0FFEE_D15EA5E;

/// Shots fired in the run. Sized so the assertion has statistical weight (at 10% loss, ~2 of the 20
/// fire bursts are dropped outright and must be repaired by a later burst's redundancy window) while
/// the whole test stays inside a few seconds of wall clock.
const SHOTS: u32 = 20;

/// Ticks between shots — 8 ticks (8 Hz), slow enough that each shot's flight (≈48 ticks to the plate)
/// overlaps only a handful of others, fast enough that the window keeps re-carrying recent events.
const FIRE_INTERVAL: u32 = 8;

/// Range to the armor plate (m). Sized so the shell's flight (≈0.75 s at 800 m/s) comfortably exceeds
/// the observer's catch-up skip: a client shell whose whole flight fits inside the catch-up never
/// spawns at all (it resolves as an already-landed phantom — `ballistics::on_fire_shell`), which would
/// be a correct behaviour but a useless test.
const RANGE: f32 = 600.0;

/// The 64 Hz fixed step both apps run on, one tick per `update()`.
const TICK: Duration = Duration::from_nanos(1_000_000_000 / 64);

// ---------------------------------------------------------------------------------------------
// The seeded loss injector — our conditioning seam (see the module doc).
// ---------------------------------------------------------------------------------------------

/// Seeded packet-drop state for the client's inbound link. A tiny LCG (Numerical Recipes constants)
/// rather than a dependency: the sequence must be identical on every machine and every run, and the
/// only property required of it is a uniform stream.
#[derive(Resource)]
struct SeededLoss {
    state: u64,
    loss: f32,
    /// Payloads dropped so far — asserted non-zero, so a test that silently stopped conditioning the
    /// link (an upstream reorder of `LinkSystems::Receive`, say) fails loudly instead of passing for
    /// the wrong reason.
    dropped: u32,
    /// Payloads let through — the denominator for the observed loss rate the test reports.
    passed: u32,
}

impl SeededLoss {
    fn next_f32(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Top 24 bits → [0, 1): plenty of resolution for a 10% decision, and independent of the low
        // bits an LCG makes poorly random.
        ((self.state >> 40) as f32) / ((1u32 << 24) as f32)
    }
}

/// Drop whole inbound payloads with a seeded probability — the conditioning seam. Registered
/// `.in_set(LinkSystems::Receive).after(LinkReceiveSystems::ApplyConditioner)`: exactly where
/// lightyear's own `LinkConditioner` releases packets into the receive buffer, and BEFORE netcode,
/// the transport channels, or the message layer read a single byte. Content-blind by construction
/// (these are opaque `RecvPayload` byte buffers).
fn drop_packets(mut links: Query<&mut Link, With<Client>>, mut loss: ResMut<SeededLoss>) {
    for mut link in &mut links {
        let payloads: Vec<_> = link.recv.drain().collect();
        for payload in payloads {
            if loss.next_f32() < loss.loss {
                loss.dropped += 1;
            } else {
                loss.passed += 1;
                link.recv.push_raw(payload);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Test-only sim wiring: the shots, and the record of what each end saw.
// ---------------------------------------------------------------------------------------------

/// The server's fire script: one shot every [`FIRE_INTERVAL`] ticks, from a fixed muzzle at the plate,
/// raised into the same `FireShell` seam `shooting::fire` uses. It MUST run inside `FixedUpdate` (not
/// from the test loop): `broadcast_fire` stamps the `FireEvent`'s `fire_tick` from the timeline at
/// trigger time and `protocol::stamp_shot_ids` stamps the shell's `Shot` in `FixedPostUpdate` of the
/// SAME tick — firing from outside the fixed schedule would split those two ticks and break the very
/// correlation under test.
fn fire_script(
    armed: Res<FireArmed>,
    shooter: Res<ShooterTank>,
    mut fired: Local<u32>,
    mut tick: Local<u32>,
    mut commands: Commands,
) {
    // Not until the observer is connected AND its timeline has synced: a shot fired into a link that
    // nobody is listening on is not a lost shot, it is a shot with no observer — and a shot that
    // arrives before the client's `LocalTimeline` has synced to the server's is rejected as absurdly
    // stale by `fire_catch_up_ticks`, which would be a harness artefact, not a netcode finding.
    if !armed.0 {
        return;
    }
    *tick += 1;
    if *fired >= SHOTS || !(*tick).is_multiple_of(FIRE_INTERVAL) {
        return;
    }
    *fired += 1;
    commands.trigger(FireShell {
        origin: MUZZLE,
        direction: Dir3::NEG_Z,
        speed: 800.0,
        caliber: 0.088,
        mass: 10.2,
        tracer: true,
        // An attributed shot naming the REPLICATED shooter tank ([`ShooterTank`]). `broadcast_fire`
        // only broadcasts a shot that names a tank, and `stamp_shot_ids` only completes a `ShotId` for
        // a shell that carries a `ShotSource` — but the receiver now demands more than a well-formed
        // id: `net::client::shooter_is_live` DROPS any fire/keyframe/confirm whose shooter does not
        // resolve to a live replicated tank on this client. So the shooter must be a genuinely
        // replicated entity (it is: `build_server` spawns it with `NetTank` + `Replicate`), and the
        // ids the client keys on are the entity-MAPPED replicas of it — the real production path, and
        // the one the mis-keying bug this module documents lives on.
        shooter: Some(ShotSource {
            tank: shooter.0,
            weapon: 0,
        }),
        catch_up_ticks: 0,
        shot: None,
    });
}

/// The server's shooter tank — a REPLICATED entity, not a synthetic id.
///
/// It carries the minimum the wire path demands and nothing more: [`NetTank`] (the tank-identity
/// marker the client's `shooter_is_live` gate resolves a shot's shooter against) and `Replicate`, so a
/// replica of it exists in the observer's world and lightyear's receive-side entity mapper can map
/// `FireEvent::shooter` onto it. No rig, no hull, no crew — this test is about the wire.
///
/// It is why the harness needs `attach_replication_sender` (each client link needs a
/// `ReplicationSender` or nothing replicates at all) and `DisableReplicateHierarchy` (replicate the
/// ROOT alone, as `net::server` does).
#[derive(Resource)]
struct ShooterTank(Entity);

/// The fire script's arming flag: set by the test once the client is connected, synced, AND holding a
/// replica of the shooter tank (see [`fire_script`] — an unmapped shooter is now dropped at the gate).
#[derive(Resource, Default)]
struct FireArmed(bool);

/// The CROSS-WORLD shot key: `(weapon slot, fire tick)`.
///
/// A [`ShotId`]'s `shooter` is an `Entity`, and an entity id is world-local — the client's is its own
/// replica of the server's tank (and, where the shooter has no replica yet, lightyear's receive-side
/// mapper falls back to `Entity::PLACEHOLDER`; see the module doc's finding). So the two ends' ids for
/// ONE shot never compare equal, and any cross-process ledger must key on the world-independent part.
/// This is the same join `scripts/shot/analyze.py` makes, for the same reason.
fn key(shot: &ShotId) -> (u8, u32) {
    (shot.weapon, shot.fire_tick)
}

/// Every `ShotId` the authority stamped — the shots the client must each see exactly once. Collected
/// as the shells are stamped (`protocol::stamp_shot_ids`, `FixedPostUpdate`), which is the moment a
/// shot's identity exists.
#[derive(Resource, Default)]
struct ServerShots(Vec<ShotId>);

fn collect_server_shots(stamped: Query<&Shot, Added<Shot>>, mut shots: ResMut<ServerShots>) {
    for shot in &stamped {
        shots.0.push(shot.0);
    }
}

/// Every ricochet the authority sanctioned (and therefore put on the wire as a `RicochetKeyframe`).
#[derive(Resource, Default)]
struct ServerBounces(Vec<(ShotId, u32)>);

fn collect_server_bounces(
    ricochet: On<crate::ballistics::ShellRicochet>,
    mut bounces: ResMut<ServerBounces>,
) {
    bounces.0.push((ricochet.shot, ricochet.sequence));
}

/// Every cosmetic shell the CLIENT spawned, in order — the exactly-once ledger. Recorded off the
/// `FireShell` trigger `receive_fire_events` raises, which is the client's shell-spawn seam
/// (`on_fire_shell` spawns exactly one shell per trigger, so this counts shells).
#[derive(Resource, Default)]
struct ClientShells(Vec<ShotId>);

fn collect_client_shells(fire: On<FireShell>, mut shells: ResMut<ClientShells>) {
    if let Some(shot) = fire.shot {
        shells.0.push(shot);
    }
}

/// Every carried-through bounce the client RENDERED: an `Impact` carrying a deflection is, by
/// construction, a server-sanctioned ricochet re-seeded onto the cosmetic shell (the pre-armed, held,
/// and overdue paths all fire one; a dissolved shell fires none). The count is the carry-through.
#[derive(Resource, Default)]
struct ClientBounces(u32);

fn collect_client_bounces(impact: On<Impact>, mut bounces: ResMut<ClientBounces>) {
    if impact.deflection.is_some() {
        bounces.0 += 1;
    }
}

/// The muzzle, and the plate [`RANGE`] metres downrange — shared by both worlds, so the client's
/// cosmetic shell contacts the same geometry the authority's shell resolved on (see the module doc on
/// what this test deliberately does NOT vary).
const MUZZLE: Vec3 = Vec3::new(0.0, 2.0, 0.0);

/// A 20 m × 20 m, 100 mm steel plate, yawed 75° so a round fired straight down −Z strikes it at 75°
/// from its normal — past `RICOCHET_ANGLE` (~70°) and not overmatched (0.088 m < 3 × 0.1 m), which is
/// the authority's ricochet condition. The generous face absorbs the round's ~2.8 m of gravity drop
/// over the flight, so every shot bounces.
fn spawn_plate(app: &mut App) {
    // Steel: reference-mm of armor per metre of material (matches `sandbox::spawn_targets` and the
    // ballistics tests) — the plate's cost is then ≈ its thickness in mm.
    const STEEL: f32 = 1000.0;
    app.world_mut().spawn((
        // `Position`/`Rotation`, NOT `Transform`: the netcode physics configuration disables
        // avian's `PhysicsTransformPlugin` (`net::physics::physics_plugins`), so a `Transform`
        // alone would never reach the collider.
        Position(Vec3::new(0.0, 2.0, -RANGE)),
        Rotation(Quat::from_rotation_y(75.0_f32.to_radians())),
        RigidBody::Static,
        Collider::cuboid(20.0, 20.0, 0.1),
        CollisionLayers::new([Layer::Armor], LayerMask::ALL),
        BallisticVolume {
            material_factor: STEEL,
        },
    ));
}

// ---------------------------------------------------------------------------------------------
// The two apps.
// ---------------------------------------------------------------------------------------------

/// The plugin floor both apps share: the sim pieces the wire path actually touches (ballistics + the
/// physics the march raycasts against + the assets `setup_assets` preloads), with no rig, no tank, and
/// no render. `DefaultPlugins` is deliberately NOT used (the production bins need it for the `.glb`
/// rig; this test fires into the `FireShell` seam directly), which keeps the run to a few seconds.
fn base_app() -> App {
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

fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.add_plugins(crate::ballistics::plugin);
    spawn_plate(&mut app);

    // The production server's fire/ricochet broadcast wiring, verbatim (`net::server::run`): the two
    // observers PUSH onto the redundancy window, and `broadcast_fire_window` — `.after(GameplaySet)`,
    // exactly as production schedules it — is the ONE thing that sends. Registering the observers
    // without it would broadcast NOTHING (the send no longer rides the events), so this test now
    // exercises the clock-driven window sender itself: the very code the carry-through fix added, under
    // real packet loss. (`on_shell_terminal` is deliberately absent — every shot here bounces off the
    // plate and ends in open air, so the authority emits no `ImpactConfirm`.)
    app.init_resource::<FireRings>();
    app.add_observer(broadcast_fire);
    app.add_observer(on_shell_ricochet);
    app.add_observer(attach_replication_sender);
    app.add_systems(
        FixedUpdate,
        broadcast_fire_window.after(crate::state::GameplaySet),
    );

    // THE SHOOTER — a replicated tank, because the client now refuses a shot whose shooter it cannot
    // resolve (`net::client::shooter_is_live`). See [`ShooterTank`].
    let shooter = app
        .world_mut()
        .spawn((
            NetTank,
            Replicate::to_clients(NetworkTarget::All),
            DisableReplicateHierarchy,
        ))
        .id();
    app.insert_resource(ShooterTank(shooter));

    app.init_resource::<FireArmed>();
    app.init_resource::<ServerShots>();
    app.init_resource::<ServerBounces>();
    app.add_observer(collect_server_bounces);
    app.add_systems(FixedUpdate, fire_script);
    // After `stamp_shot_ids` (FixedPostUpdate), so `Added<Shot>` sees this tick's shells.
    app.add_systems(FixedLast, collect_server_shots);

    let server = app
        .world_mut()
        .spawn((
            NetcodeServer::new(ServerNetcodeConfig {
                protocol_id: PROTOCOL_FINGERPRINT,
                private_key: [0; 32],
                ..default()
            }),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port)),
            ServerUdpIo::default(),
        ))
        .id();
    app.world_mut().commands().trigger(Start { entity: server });
    app
}

fn build_client(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    app.add_plugins(crate::ballistics::plugin);
    // The SAME plate, at the same pose, in the observer's world — a client shell must have armor to
    // contact and hold at (see the module doc: zero pose divergence is deliberate here).
    spawn_plate(&mut app);

    // The replica marker: shells fly and spark, but deposit no HP — and, decisively for this test, a
    // `Shot`-carrying shell HOLDS at armor for the server's verdict instead of improvising a bounce.
    app.insert_resource(ClientReplica);

    // The production client's fire-receive wiring, verbatim (`net::client::run`).
    app.init_resource::<PendingRecoilKicks>();
    app.init_resource::<SeenShots>();
    app.init_resource::<SanctionedShots>();
    app.init_resource::<crate::PredictedPresent>();
    app.add_systems(Update, receive_fire_events);
    app.add_systems(FixedUpdate, age_sanctioned_shots);
    app.add_systems(
        FixedUpdate,
        publish_predicted_present.before(crate::state::GameplaySet),
    );

    app.init_resource::<ClientShells>();
    app.init_resource::<ClientBounces>();
    app.add_observer(collect_client_shells);
    app.add_observer(collect_client_bounces);

    // THE CONDITIONING SEAM (see the module doc): seeded, content-blind packet loss on the inbound
    // link, dropped exactly where lightyear's own conditioner would drop it.
    app.insert_resource(SeededLoss {
        state: SEED,
        loss: LOSS,
        dropped: 0,
        passed: 0,
    });
    app.add_systems(
        PreUpdate,
        drop_packets
            .in_set(LinkSystems::Receive)
            .after(LinkReceiveSystems::ApplyConditioner),
    );

    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let client = app
        .world_mut()
        .spawn((
            Client::default(),
            Link::new(None),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
            PeerAddr(server_addr),
            // The prediction stack is what SYNCS `LocalTimeline` to the server's tick — and the
            // client's predicted present `P` is what `fire_catch_up_ticks` measures a shot's age
            // against, so without it every arriving `FireEvent` would read as absurdly stale and be
            // rejected. Same `balanced()` input delay the shipping client runs.
            PredictionManager::default(),
            InputTimelineConfig::new(SyncConfig::default(), InputDelayConfig::balanced()),
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id: 1,
                    private_key: [0; 32],
                    protocol_id: PROTOCOL_FINGERPRINT,
                },
                NetcodeConfig::default(),
            )
            .expect("manual dev token should always build"),
            UdpIo::default(),
        ))
        .id();
    app.world_mut()
        .commands()
        .trigger(Connect { entity: client });
    app
}

/// Drive plugin finish/cleanup by hand — a bare `update()` loop skips it, and avian registers its
/// diagnostics resources (which the spatial-query systems require) in `Plugin::finish`.
fn finish(app: &mut App) {
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
}

/// Grab a free loopback UDP port by binding one and dropping it. A fixed port would collide with a
/// concurrent test binary (or a stray dev server) on the same machine.
fn free_port() -> u16 {
    UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("loopback UDP must be bindable")
        .local_addr()
        .expect("a bound socket has a local address")
        .port()
}

/// One update of each app, plus a breath for the loopback datagrams to land in the peer's socket
/// buffer before its next read (they are sent synchronously inside `update()`; the sleep only covers
/// the kernel's hand-off).
fn step(server: &mut App, client: &mut App) {
    server.update();
    std::thread::sleep(Duration::from_micros(300));
    client.update();
    std::thread::sleep(Duration::from_micros(300));
}

/// **THE TRIPWIRE.** Over a real lightyear link with 10% seeded packet loss: every shot the authority
/// fires spawns exactly one cosmetic shell on the observer, and every ricochet the authority sanctions
/// carries through to that shell.
///
/// If this fails, the redundancy window (`net::server::FireRings`, retention `FIRE_RETAIN_TICKS`) or
/// the `ShotId` dedup (`net::client::SeenShots`) does not do on a real channel what the model tests
/// say it does — read the failure message for which half broke.
#[test]
fn every_shot_spawns_exactly_one_shell_under_ten_percent_loss() {
    let port = free_port();
    let mut server = build_server(port);
    let mut client = build_client(port);
    finish(&mut server);
    finish(&mut client);

    // Connect. The handshake is a few round trips; each `step` is one update of each app, so this is
    // generous by an order of magnitude (and the loss injector is already live, so a dropped
    // handshake packet must be retried — which is itself worth exercising).
    let mut connected = false;
    for _ in 0..600 {
        step(&mut server, &mut client);
        if client
            .world_mut()
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(client.world())
            .next()
            .is_some()
        {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "the client never connected over loopback UDP — the harness is broken, not the netcode"
    );

    // Let the client's timeline SYNC to the server's before arming the guns: until it has, the
    // predicted present `P` is meaningless and every arriving `FireEvent` reads as absurdly stale.
    // AND wait for the shooter tank's REPLICA to land: `net::client::shooter_is_live` drops any shot
    // whose shooter does not resolve to a live replicated tank, so firing before the replica arrives
    // would measure that guard (correctly!) refusing every round — a harness artefact, not a netcode
    // finding. The race it guards is real and worth its own test; THIS test's subject is the
    // redundancy window, so it starts from the state a duel is actually in: both tanks known.
    let mut shooter_replicated = false;
    for _ in 0..300 {
        step(&mut server, &mut client);
        if client
            .world_mut()
            .query_filtered::<(), With<NetTank>>()
            .iter(client.world())
            .next()
            .is_some()
        {
            shooter_replicated = true;
            break;
        }
    }
    assert!(
        shooter_replicated,
        "the shooter tank never replicated to the observer — every shot would be dropped at the \
         `shooter_is_live` gate, so the run would prove nothing about the redundancy window"
    );
    server.world_mut().resource_mut::<FireArmed>().0 = true;

    // The run: 20 shots at 8-tick spacing (160 ticks), plus the last shot's ≈48-tick flight to the
    // plate, its hold at armor, and the keyframe's return trip — with a wide margin so a late repair
    // still lands inside the window.
    for _ in 0..400 {
        step(&mut server, &mut client);
    }

    let fired = server.world().resource::<ServerShots>().0.clone();
    let bounced = server.world().resource::<ServerBounces>().0.clone();
    let spawned = client.world().resource::<ClientShells>().0.clone();
    let carried = client.world().resource::<ClientBounces>().0;
    let loss = client.world().resource::<SeededLoss>();
    let (dropped, passed) = (loss.dropped, loss.passed);

    // The conditioning actually bit. If an upstream reorder ever moves `LinkSystems::Receive`, this
    // fails loudly rather than passing a no-loss run as a loss run.
    assert!(
        dropped >= 10,
        "the loss injector dropped only {dropped} payloads (of {} seen) — the link was not \
         meaningfully conditioned, so this run proves nothing about redundancy",
        dropped + passed
    );

    assert_eq!(
        fired.len(),
        SHOTS as usize,
        "the server's fire script did not fire {SHOTS} shots (fired {}) — the harness is broken",
        fired.len()
    );

    // EXACTLY ONE SHELL PER SHOT. Two failures live here: a shot that never spawned a shell (the
    // redundancy window failed to repair a dropped burst) and a shot that spawned twice (the `ShotId`
    // dedup failed to reject a re-carried copy).
    let mut missing = Vec::new();
    let mut duplicated = Vec::new();
    for shot in &fired {
        let n = spawned.iter().filter(|s| key(s) == key(shot)).count();
        match n {
            1 => {}
            0 => missing.push(key(shot)),
            _ => duplicated.push((key(shot), n)),
        }
    }
    assert!(
        missing.is_empty(),
        "{} of {} shots NEVER spawned a cosmetic shell on the observer at {:.0}% loss \
         ({dropped} payloads dropped, {passed} delivered) — the redundancy window did not repair the \
         drops: {missing:?}",
        missing.len(),
        fired.len(),
        LOSS * 100.0,
    );
    assert!(
        duplicated.is_empty(),
        "{} shot(s) spawned MORE THAN ONE cosmetic shell — the ShotId dedup (SeenShots) failed to \
         reject a redundancy-window duplicate: {duplicated:?}",
        duplicated.len(),
    );
    // No shell for a shot the server never fired (a mangled ShotId off the wire would show up here).
    assert_eq!(
        spawned.len(),
        fired.len(),
        "the observer spawned {} shells for {} shots — an unattributed shell got through",
        spawned.len(),
        fired.len(),
    );

    // CARRY-THROUGH: every ricochet the authority sanctioned re-seeded the observer's shell. A lost
    // keyframe (all of its redundant copies dropped) would show up as a dissolved shell and no
    // deflecting `Impact`.
    assert!(
        !bounced.is_empty(),
        "the authority resolved no ricochet at all — the plate geometry no longer produces one, so \
         the carry-through half of this test is vacuous"
    );
    assert_eq!(
        carried as usize,
        bounced.len(),
        "the observer carried through {carried} of the {} ricochets the authority sanctioned at \
         {:.0}% loss ({dropped} payloads dropped) — a keyframe was lost and its shell dissolved \
         (or the hold expired before the keyframe arrived)",
        bounced.len(),
        LOSS * 100.0,
    );

    println!(
        "loss-injected E2E: {} shots, {} ricochets, {dropped}/{} payloads dropped ({:.1}% observed \
         loss) — every shot spawned exactly one shell, every bounce carried through",
        fired.len(),
        bounced.len(),
        dropped + passed,
        100.0 * dropped as f32 / (dropped + passed) as f32,
    );
}
