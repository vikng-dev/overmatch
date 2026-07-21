//! Real-UDP integration tests for shot transport (ADR-0021).
//!
//! Seeded, content-blind inbound loss remains outside the production protocol path. The harness
//! verifies observer exactly-once presentation, sanctioned outcomes, and owner-private damage facts.

use core::time::Duration;
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use avian3d::prelude::{Collider, CollisionLayers, LayerMask, Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::connection::client_of::ClientOf;
use lightyear::link::{LinkReceiveSystems, RecvPayload};
use lightyear::prelude::client::{
    Client, ClientPlugins, Connect, Connected, NetcodeClient, NetcodeConfig,
};
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::server::{
    NetcodeConfig as ServerNetcodeConfig, NetcodeServer, ServerPlugins, ServerUdpIo, Start,
};
use lightyear::prelude::*;

use super::client::{
    InputBufferGuardMetrics, PendingFireEvents, PendingRecoilKicks, SeenDamage, SeenShots,
    age_sanctioned_shots, install_input_buffer_guard, publish_predicted_present,
    receive_damage_confirms, receive_fire_events, shipping_input_delay,
};
use super::disclosure::{CombatDisclosure, NetTankStatus};
use super::hit_feel::LocalHitConfirmed;
use super::protocol::{DamageReceipt, NetCrew, NetTank, PROTOCOL_FINGERPRINT, VolumeSnapshot};
use super::server::attach_replication_sender;
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::ballistics::{
    BallisticVolume, ComponentHealth, FireShell, FireShellOrigin, Impact, SanctionedShots,
    ShellDamage, Shot, ShotSource,
};
use crate::command::TankCommand;
use crate::tank::{WeaponGate, WeaponGateState};
use crate::{ClientReplica, CombatantId, Layer, ShotId};

/// Configured seeded packet-loss rate on each client's inbound link.
const LOSS: f32 = 0.10;

/// Independent seeds ensure both real inbound links exercise the deterministic loss seam.
const SHOOTER_SEED: u64 = 0xC0FFEE_D15EA5E;
const OBSERVER_SEED: u64 = 0x0B5E_0B5E_5EED;

/// Distinct client identities separate the owner from the unowned observer.
const SHOOTER_CLIENT_ID: u64 = 1;
const OBSERVER_CLIENT_ID: u64 = 2;
/// Fixed, match-local test identity inserted synchronously with the server-side shooter spawn.
const SHOOTER_COMBATANT: CombatantId = CombatantId(1);
/// Non-default owner-private snapshots prove the disclosure filter, rather than mere component setup.
const OWNER_SNAPSHOT_HP: f32 = 73.0;
const OWNER_BELT: u32 = 17;

/// Shots fired by the scripted run.
const SHOTS: u32 = 20;

/// DERIVED from the scale-probe scenario: combatants firing simultaneously.
const VOLLEY_COMBATANTS: u64 = 30;
const VOLLEY_WEAPONS: u8 = 2;
/// DERIVED from the target firing envelope and 64 Hz authority tick.
const SUSTAINED_AUTOMATIC_FACTS_PER_TICK: usize = 12;
/// DERIVED: one second of sustained traffic at 64 Hz.
const SUSTAINED_STREAM_TICKS: u32 = 64;
/// The reliable single is injected while the automatic stream is active. Its slot must not be one
/// of that tick's automatic slots, preserving the once-per-(combatant, weapon, tick) invariant.
const SUSTAINED_RELIABLE_TICK: u32 = 31;
/// DERIVED from the fan-out scenario.
const FANOUT_RECEIVERS: u64 = 30;
/// DERIVED diagnostic cadence: frequent enough to locate a stalled phase without flooding CI logs.
const FANOUT_PROGRESS_INTERVAL_STEPS: u32 = 256;
/// DERIVED bounded repair and ACK-settlement window for the fan-out probe.
const FANOUT_SETTLEMENT_STEPS: u32 = 64;
/// Keep the owner-support connection separate from the observers: every measured receiver must
/// present the shooter root rather than suppressing its own local echo.
const FANOUT_CLIENT_ID_BASE: u64 = 10_000;
const FANOUT_SEED_BASE: u64 = 0x000F_A110_u64;

/// Configured spacing between scripted shots.
const FIRE_INTERVAL: u32 = 8;

/// Fixed plate distance for this harness.
const RANGE: f32 = 125.0;

/// Configured raw receive delay.
const OBSERVER_CATCH_UP_DELAY_TICKS: u32 = 12;

/// DERIVED: two-second Link-level sampling window at 64 Hz.
const MEASUREMENT_WINDOW_STEPS: u32 = 128;

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

/// Content-blind consecutive receive-window loss for correlated-burst measurements.
#[derive(Resource, Default)]
struct InboundBurst {
    remaining_updates: u32,
    dropped: u32,
}

/// Raw payloads admitted to the client link before this harness delays or drops any of them.
///
/// These are transport packets, not shot messages. They deliberately include handshake, replication,
/// acknowledgement, and game payloads, so they explain the denominator beside the impairment result
/// without pretending to be a gameplay-event count.
#[derive(Resource, Default)]
struct RawInboundPayloads {
    packets: u64,
    bytes: u64,
}

#[derive(Clone, Copy)]
struct PayloadSample {
    packets: u64,
    bytes: u64,
}

impl PayloadSample {
    fn delta_since(self, before: Self) -> Self {
        Self {
            packets: self.packets - before.packets,
            bytes: self.bytes - before.bytes,
        }
    }

    fn noisy_estimate_after_baseline(self, baseline: Self) -> (i128, i128) {
        (
            i128::from(self.packets) - i128::from(baseline.packets),
            i128::from(self.bytes) - i128::from(baseline.bytes),
        )
    }
}

fn raw_inbound_sample(app: &App) -> PayloadSample {
    let raw = app.world().resource::<RawInboundPayloads>();
    PayloadSample {
        packets: raw.packets,
        bytes: raw.bytes,
    }
}

impl SeededLoss {
    fn next_f32(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Use upper bits, avoiding the LCG's weak low-bit sequence.
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

fn drop_inbound_burst(mut links: Query<&mut Link, With<Client>>, mut burst: ResMut<InboundBurst>) {
    if burst.remaining_updates == 0 {
        return;
    }
    burst.remaining_updates -= 1;
    for mut link in &mut links {
        for _ in link.recv.drain() {
            burst.dropped += 1;
        }
    }
}

/// Count opaque datagrams at the same receive seam as the loss injector, before either impairment
/// stage touches the queue. `LinkReceiver` intentionally exposes a drain/push interface rather than
/// iteration; preserving the FIFO queue this way keeps the production decoder path unchanged.
fn count_raw_inbound_payloads(
    mut links: Query<&mut Link, With<Client>>,
    mut raw: ResMut<RawInboundPayloads>,
) {
    for mut link in &mut links {
        let payloads: Vec<_> = link.recv.drain().collect();
        for payload in payloads {
            raw.packets += 1;
            raw.bytes += payload.len() as u64;
            link.recv.push_raw(payload);
        }
    }
}

/// Observer-only fixed-tick holding of opaque payloads. This is a real-link delay (not a synthetic
/// fire event): data arrives over UDP, waits below Lightyear's decoder, then traverses the normal
/// transport/channel/message pipeline and the seeded loss injector.
#[derive(Resource, Default)]
struct ObserverPayloadDelay {
    tick: u32,
    armed: bool,
    held: VecDeque<(u32, RecvPayload)>,
}

fn delay_observer_packets(
    mut links: Query<&mut Link, With<Client>>,
    mut delay: ResMut<ObserverPayloadDelay>,
) {
    delay.tick += 1;
    if !delay.armed {
        return;
    }
    let release_tick = delay.tick + OBSERVER_CATCH_UP_DELAY_TICKS;
    for mut link in &mut links {
        for payload in link.recv.drain() {
            delay.held.push_back((release_tick, payload));
        }
        while delay
            .held
            .front()
            .is_some_and(|(release, _)| *release <= delay.tick)
        {
            let (_, payload) = delay.held.pop_front().expect("front was present");
            link.recv.push_raw(payload);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Test-only sim wiring: the shots, and the record of what each end saw.
// ---------------------------------------------------------------------------------------------

/// The server's fire script: one shot every [`FIRE_INTERVAL`] ticks, from a fixed muzzle at the plate,
/// raised into the same `FireShell` seam `shooting::fire` uses. It MUST run inside `FixedUpdate` (not
/// from the test loop): the explicit stable `ShotId` and `shot_transport::queue_fire` both read the
/// authority timeline at trigger time. Firing outside the fixed schedule would split those ticks and
/// break the correlation under test.
fn fire_script(
    armed: Res<FireArmed>,
    shooter: Res<ShooterTank>,
    timeline: Res<LocalTimeline>,
    mut fired: Local<u32>,
    mut tick: Local<u32>,
    mut modes: ResMut<ScriptedShotModes>,
    mut commands: Commands,
) {
    // Not until the observer is connected AND its timeline has synced: a shot fired into a link that
    // nobody is listening on is not a lost shot, it is a shot with no observer — and a shot that
    // arrives before the client's `LocalTimeline` has synced to the server's is rejected as absurdly
    // stale by `fire_catch_up_ticks`, which would be a harness artefact, not a netcode finding.
    if !armed.0 {
        return;
    }
    let Some(shooter) = shooter.0 else {
        return;
    };
    *tick += 1;
    if *fired >= SHOTS || !(*tick).is_multiple_of(FIRE_INTERVAL) {
        return;
    }
    *fired += 1;
    let mechanism = if (*fired).is_multiple_of(2) {
        crate::spec::FireMechanism::Automatic
    } else {
        crate::spec::FireMechanism::Single
    };
    let weapon = u8::from(mechanism == crate::spec::FireMechanism::Automatic);
    let shot = ShotId {
        combatant: SHOOTER_COMBATANT,
        weapon,
        // `shot_transport::queue_fire` reads this same authority timeline in this fixed step.
        fire_tick: timeline.tick().0,
    };
    modes.0.push((shot, mechanism));
    commands.trigger(FireShell {
        origin: MUZZLE,
        direction: Dir3::NEG_Z,
        speed: 800.0,
        caliber: 0.088,
        mass: 10.2,
        mechanism,
        tracer: true,
        shot_origin: FireShellOrigin::Local,
        // Attribution names the replicated shooter root. Stable `ShotId` data remains independent of
        // each client's mapped display/self-exclusion entity.
        shooter: Some(ShotSource {
            tank: shooter,
            weapon: weapon as usize,
        }),
        catch_up_ticks: 0,
        shot: Some(shot),
    });
}

/// The owned, replicated server shooter. It is absent until the real client link connects.
#[derive(Resource, Default)]
struct ShooterTank(Option<Entity>);

/// The public replicated roots used by the same-tick scale probe. They are deliberately server-side
/// scripted roots rather than thirty socket clients: this isolates public fan-out and batching from
/// client-process scheduling while retaining real server-to-client UDP delivery.
#[derive(Resource, Default)]
struct VolleyRoots(Vec<Entity>);

/// Enables the scale probe independently from the ordinary mixed-mechanism script.
#[derive(Resource, Default)]
struct VolleyArmed(bool);

/// Separates root replication from volley emission so the measurement window starts after every
/// observer display root is live, rather than charging setup replication to the volley.
#[derive(Resource, Default)]
struct VolleyFireArmed(bool);

/// Enables the sustained automatic stream after the one-tick scale volley has settled.
#[derive(Resource, Default)]
struct SustainedStreamArmed(bool);

/// The authority ledger for the sustained stream's reliable insertion.
#[derive(Resource, Default)]
struct SustainedStreamShots {
    ticks_emitted: u32,
    reliable: Option<ShotId>,
}

/// Spawn the shooter with the same ownership bundle as a player tank, after a real client link has a
/// [`ReplicationSender`]. Lightyear then supplies `Controlled` on that client's mapped replica.
fn spawn_owned_shooter(
    clients: Query<(Entity, &RemoteId), (With<ClientOf>, With<Connected>, With<ReplicationSender>)>,
    mut shooter: ResMut<ShooterTank>,
    mut commands: Commands,
) {
    if shooter.0.is_some() {
        return;
    }
    let Some((link, remote)) = clients
        .iter()
        .find(|(_, remote)| matches!(remote.0, PeerId::Netcode(SHOOTER_CLIENT_ID)))
    else {
        return;
    };
    shooter.0 = Some(
        commands
            .spawn((
                NetTank,
                SHOOTER_COMBATANT,
                NetCrew {
                    volumes: vec![VolumeSnapshot {
                        hp: OWNER_SNAPSHOT_HP,
                        crew: None,
                    }],
                    swap: None,
                },
                WeaponGate {
                    weapons: vec![WeaponGateState {
                        ready_tick: None,
                        paused_at_tick: None,
                        belt_remaining: OWNER_BELT,
                    }],
                },
                NetTankStatus::Active,
                CombatDisclosure::owner(link),
                Replicate::to_clients(NetworkTarget::All),
                DisableReplicateHierarchy,
                PredictionTarget::to_clients(NetworkTarget::Single(remote.0)),
                InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(remote.0)),
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ))
            .id(),
    );
}

fn spawn_volley_roots(
    armed: Res<VolleyArmed>,
    shooter: Res<ShooterTank>,
    mut roots: ResMut<VolleyRoots>,
    mut commands: Commands,
) {
    if !armed.0 || !roots.0.is_empty() {
        return;
    }
    let Some(shooter) = shooter.0 else { return };
    roots.0.push(shooter);
    for combatant in 2..=VOLLEY_COMBATANTS {
        roots.0.push(
            commands
                .spawn((
                    NetTank,
                    CombatantId(combatant),
                    NetTankStatus::Active,
                    Replicate::to_clients(NetworkTarget::All),
                    DisableReplicateHierarchy,
                    InterpolationTarget::to_clients(NetworkTarget::All),
                ))
                .id(),
        );
    }
}

/// The fire script's arming flag: set by the test once the client is connected, synced, AND holding a
/// replica of the shooter tank (see [`fire_script`] — an unmapped shooter is now dropped at the gate).
#[derive(Resource, Default)]
struct FireArmed(bool);

/// Emit the DERIVED 30 × 2 public automatic-fire volley inside one authority fixed tick.
fn fire_volley_script(
    armed: Res<VolleyFireArmed>,
    roots: Res<VolleyRoots>,
    timeline: Res<LocalTimeline>,
    mut fired: Local<bool>,
    mut commands: Commands,
) {
    if !armed.0 || *fired || roots.0.len() != VOLLEY_COMBATANTS as usize {
        return;
    }
    *fired = true;
    for (index, &tank) in roots.0.iter().enumerate() {
        let combatant = CombatantId(index as u64 + 1);
        for weapon in 0..VOLLEY_WEAPONS {
            commands.trigger(FireShell {
                origin: MUZZLE,
                direction: Dir3::NEG_Z,
                speed: 800.0,
                caliber: 0.0079,
                mass: 0.0118,
                mechanism: crate::spec::FireMechanism::Automatic,
                tracer: true,
                shot_origin: FireShellOrigin::Local,
                shooter: Some(ShotSource {
                    tank,
                    weapon: weapon as usize,
                }),
                catch_up_ticks: 0,
                shot: Some(ShotId {
                    combatant,
                    weapon,
                    fire_tick: timeline.tick().0,
                }),
            });
        }
    }
}

/// Emit one second of target-envelope automatic fire, then one reliable single-shot trajectory
/// amidst it. The scripts use the production `FireShell` seam; no message is manufactured here.
fn fire_sustained_stream(
    armed: Res<SustainedStreamArmed>,
    roots: Res<VolleyRoots>,
    timeline: Res<LocalTimeline>,
    mut stream: ResMut<SustainedStreamShots>,
    mut commands: Commands,
) {
    if !armed.0
        || stream.ticks_emitted >= SUSTAINED_STREAM_TICKS
        || roots.0.len() != VOLLEY_COMBATANTS as usize
    {
        return;
    }
    let stream_tick = stream.ticks_emitted;
    stream.ticks_emitted += 1;
    let first_slot = (stream_tick as usize * SUSTAINED_AUTOMATIC_FACTS_PER_TICK)
        % (VOLLEY_COMBATANTS as usize * VOLLEY_WEAPONS as usize);
    for offset in 0..SUSTAINED_AUTOMATIC_FACTS_PER_TICK {
        let slot = (first_slot + offset) % (VOLLEY_COMBATANTS as usize * VOLLEY_WEAPONS as usize);
        let root_index = slot / VOLLEY_WEAPONS as usize;
        let weapon = (slot % VOLLEY_WEAPONS as usize) as u8;
        commands.trigger(FireShell {
            origin: MUZZLE,
            direction: Dir3::NEG_Z,
            speed: 800.0,
            caliber: 0.0079,
            mass: 0.0118,
            mechanism: crate::spec::FireMechanism::Automatic,
            tracer: true,
            shot_origin: FireShellOrigin::Local,
            shooter: Some(ShotSource {
                tank: roots.0[root_index],
                weapon: weapon as usize,
            }),
            catch_up_ticks: 0,
            shot: Some(ShotId {
                combatant: CombatantId(root_index as u64 + 1),
                weapon,
                fire_tick: timeline.tick().0,
            }),
        });
    }

    if stream_tick != SUSTAINED_RELIABLE_TICK {
        return;
    }
    // DERIVED from the configured cadence: stream tick 31 uses automatic slots 12..24 (exclusive),
    // leaving root zero / weapon zero available. The DERIVED thirty receiver Apps remain unowned.
    debug_assert!(
        !(first_slot..first_slot + SUSTAINED_AUTOMATIC_FACTS_PER_TICK).contains(&0),
        "sustained reliable insertion must not collide with an automatic ShotId"
    );
    let reliable = ShotId {
        combatant: SHOOTER_COMBATANT,
        weapon: 0,
        fire_tick: timeline.tick().0,
    };
    stream.reliable = Some(reliable);
    commands.trigger(FireShell {
        origin: MUZZLE,
        direction: Dir3::NEG_Z,
        speed: 800.0,
        caliber: 0.088,
        mass: 10.2,
        mechanism: crate::spec::FireMechanism::Single,
        tracer: true,
        shot_origin: FireShellOrigin::Local,
        shooter: Some(ShotSource {
            tank: roots.0[0],
            weapon: 0,
        }),
        catch_up_ticks: 0,
        shot: Some(reliable),
    });
}

/// Every authoritative `ShotId` — the shots the client must each see exactly once. Collected after
/// the fire seam has spawned each complete shell.
#[derive(Resource, Default)]
struct ServerShots(Vec<ShotId>);

/// The requested mechanism for every script fire. This is recorded synchronously beside the source
/// trigger, so the harness proves that its real-link assertions covered both delivery classes.
#[derive(Resource, Default)]
struct ScriptedShotModes(Vec<(ShotId, crate::spec::FireMechanism)>);

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

/// Every distinct damaging shot the authority confirmed. `DamageReport` latches one per shell, so this
/// is the exact event count the shooter-side marker stream must preserve under loss.
#[derive(Resource, Default)]
struct ServerDamages(Vec<ShotId>);

fn collect_server_damage(damage: On<ShellDamage>, mut confirmed: ResMut<ServerDamages>) {
    confirmed.0.push(damage.shot);
}

/// Every cosmetic shell the CLIENT spawned, in order — the exactly-once ledger. Recorded off the
/// `FireShell` trigger `receive_fire_events` raises, which is the client's shell-spawn seam
/// (`on_fire_shell` spawns exactly one shell per trigger, so this counts shells).
#[derive(Resource, Default)]
struct ClientShells(Vec<(ShotId, u32)>);

fn collect_client_shells(fire: On<FireShell>, mut shells: ResMut<ClientShells>) {
    if let Some(shot) = fire.shot {
        shells.0.push((shot, fire.catch_up_ticks));
    }
}

/// A hidden keyed shell created by the real catch-up armor path. The observer records this before
/// its later sanctioned bounce re-seeds it, proving the delayed FireEvent did not skip directly to
/// an ordinary live contact.
#[derive(Resource, Default)]
struct ClientCatchUpArmorHolds(Vec<ShotId>);

fn collect_catch_up_armor_holds(
    shells: Query<(&Shot, &Visibility), Added<Shot>>,
    mut holds: ResMut<ClientCatchUpArmorHolds>,
) {
    for (shot, visibility) in &shells {
        if *visibility == Visibility::Hidden {
            holds.0.push(shot.0);
        }
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

/// Test-only receipts from the real trail consumer. Each receipt proves spacing admission followed
/// by a non-empty main-world ribbon mesh; it does not claim render-world extraction or visibility.
#[derive(Resource, Default)]
struct ClientRenderedBounces(Vec<(ShotId, u32, usize, usize)>);

fn collect_client_rendered_bounces(
    mut receipts: MessageReader<crate::vfx::TrailStationMeshEvidence>,
    mut rendered: ResMut<ClientRenderedBounces>,
) {
    for receipt in receipts.read() {
        assert!(
            receipt.vertices > 0 && receipt.indices > 0,
            "post-bounce trail evidence must come from a non-empty main-world ribbon mesh"
        );
        if !rendered
            .0
            .iter()
            .any(|(shot, sequence, _, _)| *shot == receipt.shot && *sequence == receipt.sequence)
        {
            rendered.0.push((
                receipt.shot,
                receipt.sequence,
                receipt.vertices,
                receipt.indices,
            ));
        }
    }
}

/// The shooter-side marker boundary, counted before UI (the headless harness has no camera/text).
#[derive(Resource, Default)]
struct ClientHitConfirms(Vec<DamageReceipt>);

fn collect_client_hit_confirm(
    hit: On<LocalHitConfirmed>,
    mut confirmed: ResMut<ClientHitConfirms>,
) {
    confirmed.0.push(hit.receipt);
}

#[derive(Resource, Default)]
struct PrivateCombatArrivals {
    crew: u32,
    weapon_gate: u32,
}

fn count_private_crew_arrival(_: On<Add, NetCrew>, mut arrivals: ResMut<PrivateCombatArrivals>) {
    arrivals.crew += 1;
}

fn count_private_weapon_gate_arrival(
    _: On<Add, WeaponGate>,
    mut arrivals: ResMut<PrivateCombatArrivals>,
) {
    arrivals.weapon_gate += 1;
}

/// The muzzle, and the plate [`RANGE`] metres downrange — shared by both worlds, so the client's
/// cosmetic shell contacts the same geometry the authority's shell resolved on (see the module doc on
/// what this test deliberately does NOT vary).
const MUZZLE: Vec3 = Vec3::new(0.0, 2.0, 0.0);

/// A fixed armor plate that produces the scripted ricochet path.
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
        // High enough that all scripted ricochet shocks lower HP without saturating. This makes every
        // shot generate one real authority-side `ShellDamage` while preserving the same bounce geometry.
        ComponentHealth {
            current: 10_000.0,
            max: 10_000.0,
        },
    ));
}

// ---------------------------------------------------------------------------------------------
// The two apps.
// ---------------------------------------------------------------------------------------------

fn build_server(port: u16) -> App {
    let mut app = base_app();
    app.add_plugins(ServerPlugins {
        tick_duration: TICK,
    });
    super::protocol::plugin(&mut app);
    super::disclosure::install_server(&mut app);
    app.add_plugins(crate::ballistics::plugin);
    spawn_plate(&mut app);

    super::shot_transport::install_server(&mut app);
    app.add_observer(attach_replication_sender);

    app.init_resource::<ShooterTank>();
    app.init_resource::<VolleyRoots>();
    app.init_resource::<VolleyArmed>();
    app.init_resource::<VolleyFireArmed>();
    app.init_resource::<SustainedStreamArmed>();
    app.init_resource::<SustainedStreamShots>();
    app.add_systems(Update, spawn_owned_shooter);
    app.add_systems(Update, spawn_volley_roots.after(spawn_owned_shooter));
    app.init_resource::<FireArmed>();
    app.init_resource::<ServerShots>();
    app.init_resource::<ScriptedShotModes>();
    app.init_resource::<ServerBounces>();
    app.init_resource::<ServerDamages>();
    app.add_observer(collect_server_bounces);
    app.add_observer(collect_server_damage);
    app.add_systems(
        FixedUpdate,
        (fire_script, fire_volley_script, fire_sustained_stream),
    );
    // FixedLast observes this tick's complete `Shot` spawns.
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum HarnessClient {
    Shooter,
    Observer,
    /// A public-only receiver used by the 30-client fan-out probe. It deliberately has neither the
    /// shooter's input slot nor the observer's delayed/trail instrumentation.
    FanoutReceiver,
}

fn claim_harness_input_slot(add: On<Add, Controlled>, mut commands: Commands) {
    // `net::client::claim_input_slot` is private. Reproduce its minimum production bundle here so
    // `receive_fire_events` recognizes this client's own replicated tank and suppresses its echo.
    commands.entity(add.entity).insert((
        InputMarker::<TankCommand>::default(),
        ActionState::<TankCommand>::default(),
    ));
}

fn build_client(port: u16, client_id: u64, seed: u64, role: HarnessClient) -> App {
    let mut app = base_app();
    app.add_plugins(ClientPlugins {
        tick_duration: TICK,
    });
    install_input_buffer_guard(&mut app);
    super::protocol::plugin(&mut app);
    app.add_plugins(crate::ballistics::plugin);
    if role == HarnessClient::Observer {
        crate::vfx::mount_trail_loss_harness(&mut app);
    } else {
        app.add_observer(claim_harness_input_slot);
    }
    // The SAME plate, at the same pose, in the observer's world — a client shell must have armor to
    // contact and hold at (see the module doc: zero pose divergence is deliberate here).
    spawn_plate(&mut app);

    // The replica marker: shells fly and spark, but deposit no HP — and, decisively for this test, a
    // `Shot`-carrying shell HOLDS at armor for the server's verdict instead of improvising a bounce.
    app.insert_resource(ClientReplica);

    // The production client's fire-receive wiring, verbatim (`net::client::run`).
    app.init_resource::<PendingRecoilKicks>();
    app.init_resource::<SeenShots>();
    app.init_resource::<PendingFireEvents>();
    app.init_resource::<SeenDamage>();
    app.init_resource::<SanctionedShots>();
    app.init_resource::<crate::PredictedPresent>();
    app.add_systems(Update, (receive_fire_events, receive_damage_confirms));
    app.add_systems(FixedUpdate, age_sanctioned_shots);
    app.add_systems(
        FixedUpdate,
        publish_predicted_present.before(crate::state::GameplaySet),
    );

    app.init_resource::<ClientShells>();
    app.init_resource::<ClientBounces>();
    app.init_resource::<ClientRenderedBounces>();
    app.init_resource::<ClientCatchUpArmorHolds>();
    app.init_resource::<ClientHitConfirms>();
    app.init_resource::<PrivateCombatArrivals>();
    app.add_observer(collect_client_shells);
    app.add_observer(collect_client_bounces);
    app.add_observer(collect_client_hit_confirm);
    app.add_observer(count_private_crew_arrival);
    app.add_observer(count_private_weapon_gate_arrival);
    app.add_systems(FixedFirst, collect_catch_up_armor_holds);
    if role == HarnessClient::Observer {
        app.add_systems(
            PostUpdate,
            collect_client_rendered_bounces.after(crate::vfx::TrailHarnessSet),
        );
    }

    // THE CONDITIONING SEAM (see the module doc): seeded, content-blind packet loss on the inbound
    // link, dropped exactly where lightyear's own conditioner would drop it.
    app.insert_resource(SeededLoss {
        state: seed,
        loss: LOSS,
        dropped: 0,
        passed: 0,
    });
    app.init_resource::<InboundBurst>();
    app.init_resource::<RawInboundPayloads>();
    if role == HarnessClient::Observer {
        app.init_resource::<ObserverPayloadDelay>().add_systems(
            PreUpdate,
            delay_observer_packets
                .in_set(LinkSystems::Receive)
                .after(LinkReceiveSystems::ApplyConditioner),
        );
        app.add_systems(
            PreUpdate,
            count_raw_inbound_payloads
                .in_set(LinkSystems::Receive)
                .after(delay_observer_packets)
                .before(drop_packets),
        );
        app.add_systems(
            PreUpdate,
            drop_inbound_burst
                .in_set(LinkSystems::Receive)
                .after(count_raw_inbound_payloads),
        );
        app.add_systems(
            PreUpdate,
            drop_packets
                .in_set(LinkSystems::Receive)
                .after(drop_inbound_burst),
        );
    } else {
        app.add_systems(
            PreUpdate,
            count_raw_inbound_payloads
                .in_set(LinkSystems::Receive)
                .after(LinkReceiveSystems::ApplyConditioner)
                .before(drop_packets),
        );
        app.add_systems(
            PreUpdate,
            drop_inbound_burst
                .in_set(LinkSystems::Receive)
                .after(count_raw_inbound_payloads),
        );
        app.add_systems(
            PreUpdate,
            drop_packets
                .in_set(LinkSystems::Receive)
                .after(drop_inbound_burst),
        );
    }

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
            // rejected. Use the same fixed input delay as the shipping client.
            PredictionManager::default(),
            InputTimelineConfig::new(SyncConfig::default(), shipping_input_delay()),
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id,
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

/// One update of each app, plus a breath for the loopback datagrams to land in the peer's socket
/// buffer before its next read (they are sent synchronously inside `update()`; the sleep only covers
/// the kernel's hand-off).
fn step(server: &mut App, shooter: &mut App, observer: &mut App) {
    server.update();
    std::thread::sleep(Duration::from_micros(300));
    shooter.update();
    std::thread::sleep(Duration::from_micros(300));
    observer.update();
    std::thread::sleep(Duration::from_micros(300));
}

/// Advance one server and many real client apps with two bounded kernel hand-offs. In particular,
/// this does NOT sleep per receiver: that would turn the 30-client probe into a measurement of the
/// harness scheduler rather than public fan-out.
fn step_many(server: &mut App, clients: &mut [App]) {
    server.update();
    std::thread::sleep(Duration::from_micros(500));
    for client in clients {
        client.update();
    }
    std::thread::sleep(Duration::from_micros(500));
}

/// Start the deterministic observer delay only once handshakes and ownership replication settle.
fn arm_observer_catch_up_delay(observer: &mut App) {
    observer
        .world_mut()
        .resource_mut::<ObserverPayloadDelay>()
        .armed = true;
}

fn arm_inbound_burst(client: &mut App, receive_updates: u32) {
    let mut burst = client.world_mut().resource_mut::<InboundBurst>();
    assert_eq!(
        burst.remaining_updates, 0,
        "a correlated inbound burst cannot be re-armed while active"
    );
    burst.remaining_updates = receive_updates;
    burst.dropped = 0;
}

fn clients_connected(shooter: &mut App, observer: &mut App) -> bool {
    client_connected(shooter) && client_connected(observer)
}

fn client_connected(client: &mut App) -> bool {
    client
        .world_mut()
        .query_filtered::<(), (With<Client>, With<Connected>)>()
        .iter(client.world())
        .next()
        .is_some()
}

fn client_root_count(client: &mut App) -> usize {
    client
        .world_mut()
        .query_filtered::<(), With<NetTank>>()
        .iter(client.world())
        .count()
}

fn report_fanout_progress(phase: &str, step: u32, started: Instant, clients: &[App]) {
    let guard_clears: u64 = clients
        .iter()
        .map(|client| client.world().resource::<InputBufferGuardMetrics>().cleared)
        .sum();
    println!(
        "MEASURED fan-out progress phase={phase} step={step} clients={} \
         input_buffer_guard_clears={guard_clears} elapsed={:?}",
        clients.len(),
        started.elapsed(),
    );
}

/// **THE TRIPWIRE.** Over a real Lightyear link with configured seeded loss, every authority shot
/// spawns exactly one observer shell and every sanctioned ricochet carries through.
///
/// If this fails, the split shot transport or the `ShotId` dedup (`net::client::SeenShots`) does not
/// do on a real channel what its focused tests say it does — read the failure message for which half
/// broke.
#[test]
fn every_shot_spawns_exactly_one_shell_under_ten_percent_loss() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let mut shooter = build_client(
        port,
        SHOOTER_CLIENT_ID,
        SHOOTER_SEED,
        HarnessClient::Shooter,
    );
    let mut observer = build_client(
        port,
        OBSERVER_CLIENT_ID,
        OBSERVER_SEED,
        HarnessClient::Observer,
    );
    finish(&mut server);
    finish(&mut shooter);
    finish(&mut observer);

    // Connect. The handshake is a few round trips; each `step` is one update of each app, so this is
    // generous by an order of magnitude (and the loss injector is already live, so a dropped
    // handshake packet must be retried — which is itself worth exercising).
    let mut connected = false;
    for _ in 0..900 {
        step(&mut server, &mut shooter, &mut observer);
        let shooter_connected = shooter
            .world_mut()
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(shooter.world())
            .next()
            .is_some();
        let observer_connected = observer
            .world_mut()
            .query_filtered::<(), (With<Client>, With<Connected>)>()
            .iter(observer.world())
            .next()
            .is_some();
        if shooter_connected && observer_connected {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "the client never connected over loopback UDP — the harness is broken, not the netcode"
    );

    // Production ownership split: the shooter has exactly the input slot `claim_input_slot` would
    // add, while the observer has only the interpolated replica and therefore cannot suppress it.
    let mut ownership_ready = false;
    for _ in 0..600 {
        step(&mut server, &mut shooter, &mut observer);
        let shooter_slot = shooter
            .world_mut()
            .query_filtered::<(), (
                With<NetTank>,
                With<Controlled>,
                With<ActionState<TankCommand>>,
                With<InputMarker<TankCommand>>,
            )>()
            .iter(shooter.world())
            .next()
            .is_some();
        let observer_replica = observer
            .world_mut()
            .query_filtered::<(), (With<NetTank>, Without<ActionState<TankCommand>>)>()
            .iter(observer.world())
            .next()
            .is_some();
        if shooter_slot && observer_replica {
            ownership_ready = true;
            break;
        }
    }
    assert!(
        ownership_ready,
        "the server did not produce an owned shooter slot plus an unowned observer replica"
    );
    let shooter_idle_start = raw_inbound_sample(&shooter);
    let observer_idle_start = raw_inbound_sample(&observer);
    for _ in 0..MEASUREMENT_WINDOW_STEPS {
        step(&mut server, &mut shooter, &mut observer);
    }
    let shooter_idle = raw_inbound_sample(&shooter).delta_since(shooter_idle_start);
    let observer_idle = raw_inbound_sample(&observer).delta_since(observer_idle_start);

    let shooter_active_start = raw_inbound_sample(&shooter);
    let observer_active_start = raw_inbound_sample(&observer);
    // Arm the outcome-delay seam immediately before the script. Delaying it through the idle
    // baseline changes Lightyear's synchronization steady state and makes the catch-up assertion
    // measure clock convergence rather than the shot path.
    arm_observer_catch_up_delay(&mut observer);
    server.world_mut().resource_mut::<FireArmed>().0 = true;
    for _ in 0..MEASUREMENT_WINDOW_STEPS {
        step(&mut server, &mut shooter, &mut observer);
    }
    let shooter_active = raw_inbound_sample(&shooter).delta_since(shooter_active_start);
    let observer_active = raw_inbound_sample(&observer).delta_since(observer_active_start);

    // Continue unmeasured beyond the script, flight, hold, and repair windows. This traffic is
    // deliberately excluded from the bounded Link-level estimate above.
    for _ in 0..1_200 {
        step(&mut server, &mut shooter, &mut observer);
    }

    let fired = server.world().resource::<ServerShots>().0.clone();
    let scripted_modes = server.world().resource::<ScriptedShotModes>().0.clone();
    let bounced = server.world().resource::<ServerBounces>().0.clone();
    let observer_spawned = observer.world().resource::<ClientShells>().0.clone();
    let observer_carried = observer.world().resource::<ClientBounces>().0;
    let observer_rendered = observer
        .world()
        .resource::<ClientRenderedBounces>()
        .0
        .clone();
    let observer_catch_up_holds = observer
        .world()
        .resource::<ClientCatchUpArmorHolds>()
        .0
        .clone();
    let shooter_spawned = shooter.world().resource::<ClientShells>().0.clone();
    let damaged = server.world().resource::<ServerDamages>().0.clone();
    let shooter_hit_confirms = shooter.world().resource::<ClientHitConfirms>().0.clone();
    let observer_hit_confirms = observer.world().resource::<ClientHitConfirms>().0.clone();
    let shooter_has_exact_private_combat = shooter
        .world_mut()
        .query_filtered::<(&NetCrew, &WeaponGate), (
            With<NetTank>,
            With<NetCrew>,
            With<WeaponGate>,
            With<NetTankStatus>,
        )>()
        .iter(shooter.world())
        .any(|(crew, gate)| {
            crew.volumes
                == [VolumeSnapshot {
                    hp: OWNER_SNAPSHOT_HP,
                    crew: None,
                }]
                && crew.swap.is_none()
                && gate.weapons
                    == [WeaponGateState {
                        ready_tick: None,
                        paused_at_tick: None,
                        belt_remaining: OWNER_BELT,
                    }]
        });
    let observer_has_public_status = observer
        .world_mut()
        .query_filtered::<(), (With<NetTank>, With<NetTankStatus>)>()
        .iter(observer.world())
        .next()
        .is_some();
    let observer_has_private_combat = observer
        .world_mut()
        .query_filtered::<(), (With<NetTank>, Or<(With<NetCrew>, With<WeaponGate>)>)>()
        .iter(observer.world())
        .next()
        .is_some();
    let shooter_loss = shooter.world().resource::<SeededLoss>();
    let observer_loss = observer.world().resource::<SeededLoss>();
    let (shooter_dropped, shooter_passed) = (shooter_loss.dropped, shooter_loss.passed);
    let (observer_dropped, observer_passed) = (observer_loss.dropped, observer_loss.passed);
    let shooter_raw = raw_inbound_sample(&shooter);
    let observer_raw = raw_inbound_sample(&observer);
    let shooter_noisy_estimate = shooter_active.noisy_estimate_after_baseline(shooter_idle);
    let observer_noisy_estimate = observer_active.noisy_estimate_after_baseline(observer_idle);
    let shot_metrics = server
        .world()
        .resource::<super::shot_transport::ShotTransportMetrics>();

    // The conditioning actually bit. If an upstream reorder ever moves `LinkSystems::Receive`, this
    // fails loudly rather than passing a no-loss run as a loss run.
    assert!(
        shooter_dropped >= 10 && observer_dropped >= 10,
        "the independent loss injectors did not both bite: shooter {shooter_dropped}/{}; observer \
         {observer_dropped}/{}",
        shooter_dropped + shooter_passed,
        observer_dropped + observer_passed,
    );

    assert_eq!(
        fired.len(),
        SHOTS as usize,
        "the server's fire script did not fire {SHOTS} shots (fired {}) — the harness is broken",
        fired.len()
    );
    assert_eq!(
        scripted_modes.len(),
        fired.len(),
        "the script did not record one delivery class for every authority shot"
    );
    assert!(
        scripted_modes
            .iter()
            .any(|(_, mechanism)| *mechanism == crate::spec::FireMechanism::Single)
            && scripted_modes
                .iter()
                .any(|(_, mechanism)| *mechanism == crate::spec::FireMechanism::Automatic),
        "the real-link run must exercise both Single reliable outcomes and Automatic visual batches"
    );
    assert!(
        shot_metrics.reliable_public_send_accepted_facts > 0
            && shot_metrics.visual_send_accepted_facts > 0,
        "the server did not emit both transport classes: reliable={}, visual={}",
        shot_metrics.reliable_public_send_accepted_facts,
        shot_metrics.visual_send_accepted_facts,
    );

    // EXACTLY ONE SHELL PER SHOT. A missing shell means the selected delivery class did not reach
    // presentation; a duplicate means the `ShotId` dedup admitted a second copy.
    let mut missing = Vec::new();
    let mut duplicated = Vec::new();
    for shot in &fired {
        let n = observer_spawned
            .iter()
            .filter(|(seen, _)| seen == shot)
            .count();
        match n {
            1 => {}
            0 => missing.push(*shot),
            _ => duplicated.push((*shot, n)),
        }
    }
    assert!(
        missing.is_empty(),
        "{} of {} shots NEVER spawned a cosmetic shell on the observer at {:.0}% loss \
         ({observer_dropped} payloads dropped, {observer_passed} delivered) — public shot transport did not repair the \
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
        observer_spawned.len(),
        fired.len(),
        "the observer spawned {} shells for {} shots — an unattributed shell got through",
        observer_spawned.len(),
        fired.len(),
    );
    assert!(
        shooter_spawned.is_empty(),
        "the shooter spawned {} echoed FireShell(s): its ActionState self-echo suppression is absent",
        shooter_spawned.len(),
    );

    // CARRY-THROUGH: every ricochet the authority sanctioned re-seeded the observer's shell. A
    // missing outcome would show up as a dissolved shell and no deflecting `Impact`.
    assert!(
        !bounced.is_empty(),
        "the authority resolved no ricochet at all — the plate geometry no longer produces one, so \
         the carry-through half of this test is vacuous"
    );
    assert_eq!(
        observer_carried as usize,
        bounced.len(),
        "the observer carried through {observer_carried} of the {} ricochets the authority sanctioned at \
         {:.0}% loss ({observer_dropped} payloads dropped) — a keyframe was lost and its shell dissolved \
         (or the hold expired before the keyframe arrived)",
        bounced.len(),
        LOSS * 100.0,
    );
    let missing_rendered_bounces: Vec<_> = bounced
        .iter()
        .filter(|(shot, sequence)| {
            !observer_rendered
                .iter()
                .any(|(seen, seen_sequence, _, _)| seen == shot && seen_sequence == sequence)
        })
        .map(|(shot, sequence)| (*shot, *sequence))
        .collect();
    assert!(
        missing_rendered_bounces.is_empty(),
        "{} of {} sanctioned ricochets raised an Impact but produced no spacing-filtered post-bounce \
         trail station/ribbon receipt: {missing_rendered_bounces:?}",
        missing_rendered_bounces.len(),
        bounced.len(),
    );

    let observed_catch_up_holds: Vec<_> = observer_catch_up_holds
        .iter()
        .copied()
        .filter(|held| observer_spawned.iter().any(|(shot, _)| shot == held))
        .collect();
    assert!(
        !observed_catch_up_holds.is_empty(),
        "the configured delayed observer path produced no MEASURED hidden keyed armor hold"
    );

    // DISCRETE DAMAGE CONFIRMATION: every authority-side damaging shot produces exactly one marker
    // boundary on its shooter through its owner-private reliable channel.
    // This cannot be inferred from `NetCrew`: latest-state snapshots preserve HP but coalesce event count.
    assert_eq!(
        damaged.len(),
        fired.len(),
        "the authority confirmed damage for {} of {} scripted shots — the health-bearing plate or \
         ShellDamage latch is not exercising the intended path",
        damaged.len(),
        fired.len(),
    );
    let mut missing_hit_confirms = Vec::new();
    let mut duplicate_hit_confirms = Vec::new();
    for shot in &damaged {
        let n = shooter_hit_confirms
            .iter()
            .filter(|seen| **seen == DamageReceipt::from(*shot))
            .count();
        match n {
            1 => {}
            0 => missing_hit_confirms.push(DamageReceipt::from(*shot)),
            _ => duplicate_hit_confirms.push((DamageReceipt::from(*shot), n)),
        }
    }
    assert!(
        missing_hit_confirms.is_empty(),
        "{} of {} authoritative damaging shots produced NO shooter-side hit confirm at {:.0}% loss: \
         {missing_hit_confirms:?}",
        missing_hit_confirms.len(),
        damaged.len(),
        LOSS * 100.0,
    );
    assert!(
        duplicate_hit_confirms.is_empty(),
        "duplicate reliable DamageConfirm delivery produced duplicate markers: {duplicate_hit_confirms:?}"
    );
    assert_eq!(
        shooter_hit_confirms.len(),
        damaged.len(),
        "the client emitted {} marker boundaries for {} damaging shots",
        shooter_hit_confirms.len(),
        damaged.len(),
    );
    assert!(
        observer_hit_confirms.is_empty(),
        "the unowned observer received {} owner-private damage marker(s)",
        observer_hit_confirms.len(),
    );
    assert!(
        shooter_has_exact_private_combat,
        "the owning shooter did not receive the expected private NetCrew/WeaponGate snapshot"
    );
    assert!(
        observer_has_public_status && !observer_has_private_combat,
        "combat disclosure leaked NetCrew/WeaponGate to the observer or hid its public tank status"
    );

    println!(
        "MEASURED mixed-shot E2E: {} shots, {} ricochets, {} damage confirms; DERIVED over equal \
         {MEASUREMENT_WINDOW_STEPS}-step Link windows: shooter idle {}/{} active {}/{} estimate \
         {:+}/{:+}; observer idle {}/{} active {}/{} estimate {:+}/{:+}. The estimate is active minus \
         idle opaque Link payload traffic, not shot bytes. Lifetime raw shooter {} packets/{} bytes, observer \
         {} packets/{} bytes; impairment shooter {shooter_dropped} dropped/{shooter_passed} passed, observer \
         {observer_dropped} dropped/{observer_passed} passed — observer shells/trails and shooter-only markers",
        fired.len(),
        bounced.len(),
        damaged.len(),
        shooter_idle.packets,
        shooter_idle.bytes,
        shooter_active.packets,
        shooter_active.bytes,
        shooter_noisy_estimate.0,
        shooter_noisy_estimate.1,
        observer_idle.packets,
        observer_idle.bytes,
        observer_active.packets,
        observer_active.bytes,
        observer_noisy_estimate.0,
        observer_noisy_estimate.1,
        shooter_raw.packets,
        shooter_raw.bytes,
        observer_raw.packets,
        observer_raw.bytes,
    );
}

/// **THE SCALE TRIPWIRE.** A DERIVED 30-combatant, two-weapon same-tick volley stays below the
/// application fragmentation limit and presents every observer fire exactly once over real UDP.
///
/// The rig intentionally uses thirty replicated server roots rather than thirty client processes:
/// it measures the production server's public fan-out and one observer's loss recovery without
/// adding operating-system scheduling as an unmeasured variable.
#[test]
fn thirty_combatant_two_weapon_volley_presents_each_observer_fire_once_under_loss() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let mut shooter = build_client(
        port,
        SHOOTER_CLIENT_ID,
        SHOOTER_SEED,
        HarnessClient::Shooter,
    );
    let mut observer = build_client(
        port,
        OBSERVER_CLIENT_ID,
        OBSERVER_SEED,
        HarnessClient::Observer,
    );
    finish(&mut server);
    finish(&mut shooter);
    finish(&mut observer);

    let mut connected = false;
    for _ in 0..900 {
        step(&mut server, &mut shooter, &mut observer);
        if clients_connected(&mut shooter, &mut observer) {
            connected = true;
            break;
        }
    }
    assert!(
        connected,
        "the scale probe clients never connected over loopback UDP"
    );

    server.world_mut().resource_mut::<VolleyArmed>().0 = true;
    let mut roots_ready = false;
    for _ in 0..1_200 {
        step(&mut server, &mut shooter, &mut observer);
        let observer_roots = observer
            .world_mut()
            .query_filtered::<(), With<NetTank>>()
            .iter(observer.world())
            .count();
        if observer_roots == VOLLEY_COMBATANTS as usize {
            roots_ready = true;
            break;
        }
    }
    assert!(
        roots_ready,
        "the observer did not receive all {VOLLEY_COMBATANTS} scripted replicated roots before the volley"
    );

    let observer_idle_start = raw_inbound_sample(&observer);
    for _ in 0..MEASUREMENT_WINDOW_STEPS {
        step(&mut server, &mut shooter, &mut observer);
    }
    let observer_idle = raw_inbound_sample(&observer).delta_since(observer_idle_start);

    let observer_active_start = raw_inbound_sample(&observer);
    server.world_mut().resource_mut::<VolleyFireArmed>().0 = true;
    // The volley fires on the next server fixed tick. This equal-duration active window captures
    // its first copies plus bounded repairs without counting prior root replication.
    for _ in 0..MEASUREMENT_WINDOW_STEPS {
        step(&mut server, &mut shooter, &mut observer);
    }
    let observer_active = raw_inbound_sample(&observer).delta_since(observer_active_start);

    // Continue unmeasured until all gameplay presentation assertions have settled.
    for _ in 0..900 {
        step(&mut server, &mut shooter, &mut observer);
    }

    let fired = server.world().resource::<ServerShots>().0.clone();
    let observer_shells = observer.world().resource::<ClientShells>().0.clone();
    let observer_damage = observer.world().resource::<ClientHitConfirms>().0.clone();
    let metrics = server
        .world()
        .resource::<super::shot_transport::ShotTransportMetrics>();
    let observer_loss = observer.world().resource::<SeededLoss>();
    let observer_raw = raw_inbound_sample(&observer);
    let observer_noisy_estimate = observer_active.noisy_estimate_after_baseline(observer_idle);
    let expected = (VOLLEY_COMBATANTS * u64::from(VOLLEY_WEAPONS)) as usize;

    assert_eq!(
        fired.len(),
        expected,
        "the scale script must author {VOLLEY_COMBATANTS} × {VOLLEY_WEAPONS} = {expected} shots in one authority tick"
    );
    let distinct_fired: bevy::platform::collections::HashSet<_> = fired.iter().copied().collect();
    assert_eq!(
        distinct_fired.len(),
        expected,
        "same-tick shots collided instead of remaining distinct by (CombatantId, weapon, tick)"
    );
    let distinct_presented: bevy::platform::collections::HashSet<_> =
        observer_shells.iter().map(|(shot, _)| *shot).collect();
    assert_eq!(
        observer_shells.len(),
        expected,
        "the observer presented {} shells for {expected} expected fires — a batch duplicate or loss escaped ShotId dedup/recovery",
        observer_shells.len(),
    );
    assert_eq!(
        distinct_presented.len(),
        expected,
        "the observer did not present every volley ShotId exactly once"
    );
    assert_eq!(
        distinct_presented, distinct_fired,
        "the observer presented a shot the authority did not author or missed an authored shot"
    );
    assert!(
        observer_damage.is_empty(),
        "the observer received {} owner-private damage receipts during the public volley",
        observer_damage.len(),
    );
    assert_eq!(
        metrics.reliable_public_send_accepted_facts, 0,
        "the all-Automatic volley must use only FireVisualBatch, not reliable public outcomes"
    );
    assert!(
        metrics.visual_send_accepted_facts >= expected as u64,
        "the automatic visual queue did not get every first copy accepted: accepted={}, expected={expected}",
        metrics.visual_send_accepted_facts,
    );
    assert!(
        metrics.max_batch_wire_bytes <= super::shot_transport::VISUAL_BATCH_WIRE_LIMIT,
        "a public visual batch crossed the unfragmented wire budget: {} > {}",
        metrics.max_batch_wire_bytes,
        super::shot_transport::VISUAL_BATCH_WIRE_LIMIT,
    );
    assert!(
        observer_loss.dropped > 0,
        "the observer loss injector never dropped a payload during the volley"
    );
    assert!(
        observer_active.packets > 0 && observer_active.bytes > 0,
        "the bounded active volley window contained no observer inbound transport payload"
    );

    println!(
        "MEASURED volley E2E: {expected} automatic fires; DERIVED over equal {MEASUREMENT_WINDOW_STEPS}-step \
         Link windows: observer idle {}/{} active {}/{} estimate {:+}/{:+}. The estimate is active minus \
         idle opaque Link payload traffic, not shot bytes. Lifetime observer raw {} packets/{} bytes, impairment \
         {} dropped/{} passed, max public batch {} bytes",
        observer_idle.packets,
        observer_idle.bytes,
        observer_active.packets,
        observer_active.bytes,
        observer_noisy_estimate.0,
        observer_noisy_estimate.1,
        observer_raw.packets,
        observer_raw.bytes,
        observer_loss.dropped,
        observer_loss.passed,
        metrics.max_batch_wire_bytes,
    );
}

/// **THE FAN-OUT TRIPWIRE.** A DERIVED 30-root same-tick volley and one-second target-envelope
/// automatic stream reach a DERIVED thirty independent public receivers over production UDP, while
/// a reliable cannon trajectory and owner-private damage confirmation share the flush. This stays an
/// application-level probe: opaque Link counters include control, acknowledgement, replication,
/// and game payloads rather than pretending to report per-shot or IP/UDP byte costs.
#[test]
fn thirty_combatant_volley_reaches_thirty_independent_receivers_under_loss() {
    assert!(
        !crate::env_flag("SPIKE_MG_SHORTCIRCUIT", false),
        "the fan-out contract must run production ballistics; use scripts/cost for the MG short-circuit A/B",
    );
    let _udp = lock_real_udp_test();
    let started = Instant::now();
    let port = free_port();
    let mut server = build_server(port);

    // This support connection owns the first replicated root. The DERIVED thirty-receiver scenario
    // uses different identities and no input slot, so every receiver expects the DERIVED sixty fires.
    let mut clients = Vec::with_capacity(FANOUT_RECEIVERS as usize + 1);
    clients.push(build_client(
        port,
        SHOOTER_CLIENT_ID,
        SHOOTER_SEED,
        HarnessClient::Shooter,
    ));
    for receiver in 0..FANOUT_RECEIVERS {
        clients.push(build_client(
            port,
            FANOUT_CLIENT_ID_BASE + receiver,
            FANOUT_SEED_BASE.wrapping_add(receiver.wrapping_mul(0x9E37_79B9)),
            HarnessClient::FanoutReceiver,
        ));
    }
    finish(&mut server);
    for client in &mut clients {
        finish(client);
    }
    report_fanout_progress("connect", 0, started, &clients);

    let mut connected = false;
    for step_index in 1..=1_800 {
        step_many(&mut server, &mut clients);
        if step_index % FANOUT_PROGRESS_INTERVAL_STEPS == 0 {
            report_fanout_progress("connect", step_index, started, &clients);
        }
        if clients.iter_mut().all(client_connected) {
            connected = true;
            report_fanout_progress("connected", step_index, started, &clients);
            break;
        }
    }
    assert!(
        connected,
        "the support connection and all {FANOUT_RECEIVERS} fan-out receivers never connected over loopback UDP"
    );

    server.world_mut().resource_mut::<VolleyArmed>().0 = true;
    let mut roots_ready = false;
    for step_index in 1..=1_800 {
        step_many(&mut server, &mut clients);
        if step_index % FANOUT_PROGRESS_INTERVAL_STEPS == 0 {
            report_fanout_progress("replicate-roots", step_index, started, &clients);
        }
        if clients[1..]
            .iter_mut()
            .all(|receiver| client_root_count(receiver) == VOLLEY_COMBATANTS as usize)
        {
            roots_ready = true;
            report_fanout_progress("roots-ready", step_index, started, &clients);
            break;
        }
    }
    assert!(
        roots_ready,
        "not every one of the {FANOUT_RECEIVERS} receivers had all {VOLLEY_COMBATANTS} replicated roots before firing"
    );

    let expected = (VOLLEY_COMBATANTS * u64::from(VOLLEY_WEAPONS)) as usize;
    let raw_at_arm: Vec<_> = clients[1..].iter().map(raw_inbound_sample).collect();
    let mut completed: Vec<Option<(u32, PayloadSample)>> = vec![None; FANOUT_RECEIVERS as usize];
    server.world_mut().resource_mut::<VolleyFireArmed>().0 = true;

    // The server emits in the first fixed step after arming. Complete receiver windows are captured
    // at each receiver's first exactly-once presentation, so a slow receiver cannot be hidden in an
    // aggregate average.
    let mut fired: Option<std::collections::HashSet<ShotId>> = None;
    let mut initial_volley_steps = 0;
    for step_index in 1..=1_800 {
        initial_volley_steps = step_index;
        step_many(&mut server, &mut clients);
        if step_index % FANOUT_PROGRESS_INTERVAL_STEPS == 0 {
            report_fanout_progress("initial-volley", step_index, started, &clients);
        }
        let fired_now = server.world().resource::<ServerShots>().0.clone();
        if fired.is_none() && fired_now.len() == expected {
            fired = Some(fired_now.into_iter().collect());
        }
        let Some(fired) = fired.as_ref() else {
            continue;
        };
        for (index, receiver) in clients[1..].iter().enumerate() {
            if completed[index].is_some() {
                continue;
            }
            let shells = &receiver.world().resource::<ClientShells>().0;
            let presented: std::collections::HashSet<_> =
                shells.iter().map(|(shot, _)| *shot).collect();
            if shells.len() == expected && presented == *fired {
                completed[index] = Some((step_index, raw_inbound_sample(receiver)));
            }
        }
        if completed.iter().all(Option::is_some) {
            report_fanout_progress("initial-volley-complete", step_index, started, &clients);
            break;
        }
    }

    report_fanout_progress(
        "initial-volley-terminal-check",
        initial_volley_steps,
        started,
        &clients,
    );
    let fired = fired.expect("the authoritative scale script never emitted its volley");
    assert_eq!(
        fired.len(),
        expected,
        "the fan-out scale script must author {VOLLEY_COMBATANTS} × {VOLLEY_WEAPONS} = {expected} distinct shots"
    );
    let incomplete: Vec<_> = completed
        .iter()
        .enumerate()
        .filter_map(|(index, completion)| completion.is_none().then_some(index))
        .collect();
    assert!(
        incomplete.is_empty(),
        "{} of {FANOUT_RECEIVERS} receivers did not present every volley ShotId exactly once: receiver indices {incomplete:?}",
        incomplete.len(),
    );

    // Let the bounded automatic repair horizon drain, then check the final ledger rather than only
    // the first-completion snapshot. Any later duplicate remains a product failure.
    report_fanout_progress("initial-volley-settlement-start", 0, started, &clients);
    for _ in 0..FANOUT_SETTLEMENT_STEPS {
        step_many(&mut server, &mut clients);
    }
    report_fanout_progress(
        "initial-volley-settlement-complete",
        FANOUT_SETTLEMENT_STEPS,
        started,
        &clients,
    );

    // Sustain roughly 768 RPM per weapon slot for one second: automatic public visuals are now
    // continuously active when the reliable cannon fire, bounce, and owner-private consequence
    // enter the same production transport flush.
    let sustained_raw_at_arm: Vec<_> = clients[1..].iter().map(raw_inbound_sample).collect();
    server.world_mut().resource_mut::<SustainedStreamArmed>().0 = true;
    let mut sustained_complete: Vec<Option<(u32, PayloadSample)>> =
        vec![None; FANOUT_RECEIVERS as usize];
    let mut reliable_shot = None;
    let mut sustained_stream_steps = 0;
    for step_index in 1..=1_800 {
        sustained_stream_steps = step_index;
        step_many(&mut server, &mut clients);
        if step_index % FANOUT_PROGRESS_INTERVAL_STEPS == 0 {
            report_fanout_progress("sustained-stream", step_index, started, &clients);
        }
        let stream = server.world().resource::<SustainedStreamShots>();
        if reliable_shot.is_none() {
            reliable_shot = stream.reliable;
        }
        let Some(reliable) = reliable_shot else {
            continue;
        };
        for (index, receiver) in clients[1..].iter().enumerate() {
            if sustained_complete[index].is_some() {
                continue;
            }
            let occurrences = receiver
                .world()
                .resource::<ClientShells>()
                .0
                .iter()
                .filter(|(shot, _)| *shot == reliable)
                .count();
            if occurrences == 1 {
                sustained_complete[index] = Some((step_index, raw_inbound_sample(receiver)));
            }
        }
        let owner_receipts = clients[0]
            .world()
            .resource::<ClientHitConfirms>()
            .0
            .iter()
            .filter(|receipt| **receipt == DamageReceipt::from(reliable))
            .count();
        if stream.ticks_emitted == SUSTAINED_STREAM_TICKS
            && owner_receipts == 1
            && sustained_complete.iter().all(Option::is_some)
        {
            report_fanout_progress("sustained-stream-complete", step_index, started, &clients);
            break;
        }
    }

    report_fanout_progress(
        "sustained-stream-terminal-check",
        sustained_stream_steps,
        started,
        &clients,
    );
    let reliable =
        reliable_shot.expect("the sustained stream never injected its reliable cannon shot");
    assert_eq!(
        server
            .world()
            .resource::<SustainedStreamShots>()
            .ticks_emitted,
        SUSTAINED_STREAM_TICKS,
        "the sustained automatic stream ended before its one-second target envelope"
    );
    assert!(
        server
            .world()
            .resource::<ServerBounces>()
            .0
            .iter()
            .any(|(shot, _)| *shot == reliable),
        "the injected reliable cannon shot never reached the authority ricochet seam"
    );
    let incomplete_reliable: Vec<_> = sustained_complete
        .iter()
        .enumerate()
        .filter_map(|(index, completion)| completion.is_none().then_some(index))
        .collect();
    assert!(
        incomplete_reliable.is_empty(),
        "{} receivers did not present the reliable cannon fire exactly once under sustained visual contention: {incomplete_reliable:?}",
        incomplete_reliable.len(),
    );
    let owner_reliable_receipts = clients[0]
        .world()
        .resource::<ClientHitConfirms>()
        .0
        .iter()
        .filter(|receipt| **receipt == DamageReceipt::from(reliable))
        .count();
    assert_eq!(
        owner_reliable_receipts, 1,
        "the reliable cannon's owner-private DamageConfirm must present exactly once on its owner"
    );

    // The local `Impact` observer is intentionally unkeyed (`Impact` carries no ShotId), so its
    // aggregate bounce counter cannot distinguish this cannon from the concurrent automatic hits.
    // The keyed assertion above proves receiver fire presentation; the authority bounce and the
    // reliable-send metrics below prove the remainder of this trajectory's transport route.
    report_fanout_progress("reliable-settlement-start", 0, started, &clients);
    for _ in 0..FANOUT_SETTLEMENT_STEPS {
        step_many(&mut server, &mut clients);
    }
    report_fanout_progress(
        "reliable-settlement-complete",
        FANOUT_SETTLEMENT_STEPS,
        started,
        &clients,
    );

    report_fanout_progress(
        "terminal-transport-assertions",
        FANOUT_SETTLEMENT_STEPS,
        started,
        &clients,
    );
    let metrics = server
        .world()
        .resource::<super::shot_transport::ShotTransportMetrics>();
    assert!(
        metrics.reliable_public_send_accepted_facts >= 2,
        "the reliable cannon fire and bounce were not accepted while automatic visuals were saturated: {}",
        metrics.reliable_public_send_accepted_facts,
    );
    assert!(
        metrics.max_reliable_outcome_sent_unacked_messages > 0
            && metrics.max_reliable_damage_sent_unacked_messages > 0,
        "the transport outbox probe did not observe both public outcome and private damage messages: outcome high-water={}, damage high-water={}",
        metrics.max_reliable_outcome_sent_unacked_messages,
        metrics.max_reliable_damage_sent_unacked_messages,
    );
    assert_eq!(
        (
            metrics.reliable_outcome_sent_unacked_messages,
            metrics.reliable_damage_sent_unacked_messages,
        ),
        (0, 0),
        "reliable sent-but-unacknowledged messages remained after the bounded settlement window"
    );
    assert!(
        metrics.visual_enqueued
            >= (expected + SUSTAINED_AUTOMATIC_FACTS_PER_TICK * SUSTAINED_STREAM_TICKS as usize)
                as u64,
        "the sustained target-envelope automatic stream did not enqueue every fire: enqueued={}, expected at least {}",
        metrics.visual_enqueued,
        expected + SUSTAINED_AUTOMATIC_FACTS_PER_TICK * SUSTAINED_STREAM_TICKS as usize,
    );
    assert!(
        metrics.visual_send_accepted_facts >= expected as u64,
        "the automatic visual queue did not accept the initial volley's first copies: accepted={}, expected={expected}",
        metrics.visual_send_accepted_facts,
    );
    assert!(
        metrics.max_batch_wire_bytes <= super::shot_transport::VISUAL_BATCH_WIRE_LIMIT,
        "a fan-out visual batch crossed the unfragmented wire budget: {} > {}",
        metrics.max_batch_wire_bytes,
        super::shot_transport::VISUAL_BATCH_WIRE_LIMIT,
    );

    let mut completion_packets = Vec::with_capacity(FANOUT_RECEIVERS as usize);
    let mut completion_bytes = Vec::with_capacity(FANOUT_RECEIVERS as usize);
    let mut sustained_packets = Vec::with_capacity(FANOUT_RECEIVERS as usize);
    let mut sustained_bytes = Vec::with_capacity(FANOUT_RECEIVERS as usize);
    let mut dropped_payloads = Vec::with_capacity(FANOUT_RECEIVERS as usize);
    let mut slowest_completion = 0_u32;
    let mut slowest_sustained_completion = 0_u32;
    let mut no_drops = Vec::new();
    for (index, receiver) in clients[1..].iter().enumerate() {
        let shells = &receiver.world().resource::<ClientShells>().0;
        let base_occurrences: std::collections::HashMap<_, _> = shells
            .iter()
            .filter(|(shot, _)| fired.contains(shot))
            .fold(std::collections::HashMap::new(), |mut counts, (shot, _)| {
                *counts.entry(*shot).or_insert(0_usize) += 1;
                counts
            });
        assert!(
            fired
                .iter()
                .all(|shot| base_occurrences.get(shot) == Some(&1)),
            "receiver {index} lost or duplicated a same-tick volley ShotId after sustained contention: {base_occurrences:?}",
        );
        assert_eq!(
            shells.iter().filter(|(shot, _)| *shot == reliable).count(),
            1,
            "receiver {index} lost or duplicated the reliable cannon fire after its completion window"
        );
        let damage = &receiver.world().resource::<ClientHitConfirms>().0;
        assert!(
            damage.is_empty(),
            "receiver {index} received {} owner-private damage receipt(s)",
            damage.len(),
        );
        let loss = receiver.world().resource::<SeededLoss>();
        if loss.dropped == 0 {
            no_drops.push((index, loss.passed));
        }
        dropped_payloads.push(loss.dropped);
        let (completion_step, completion_raw) =
            completed[index].expect("incomplete receivers were rejected before final accounting");
        let delta = completion_raw.delta_since(raw_at_arm[index]);
        completion_packets.push(delta.packets);
        completion_bytes.push(delta.bytes);
        slowest_completion = slowest_completion.max(completion_step);
        let (sustained_step, sustained_raw) = sustained_complete[index]
            .expect("reliable cannon incompleteness was rejected before final accounting");
        let sustained_delta = sustained_raw.delta_since(sustained_raw_at_arm[index]);
        sustained_packets.push(sustained_delta.packets);
        sustained_bytes.push(sustained_delta.bytes);
        slowest_sustained_completion = slowest_sustained_completion.max(sustained_step);
    }
    assert!(
        no_drops.is_empty(),
        "the independent 10% loss injector did not drop a payload on every receiver: {no_drops:?}"
    );

    let packet_min = *completion_packets
        .iter()
        .min()
        .expect("there are fan-out receivers");
    let packet_max = *completion_packets
        .iter()
        .max()
        .expect("there are fan-out receivers");
    let byte_min = *completion_bytes
        .iter()
        .min()
        .expect("there are fan-out receivers");
    let byte_max = *completion_bytes
        .iter()
        .max()
        .expect("there are fan-out receivers");
    let packet_total: u64 = completion_packets.iter().sum();
    let byte_total: u64 = completion_bytes.iter().sum();
    let sustained_packet_min = *sustained_packets
        .iter()
        .min()
        .expect("there are fan-out receivers");
    let sustained_packet_max = *sustained_packets
        .iter()
        .max()
        .expect("there are fan-out receivers");
    let sustained_byte_min = *sustained_bytes
        .iter()
        .min()
        .expect("there are fan-out receivers");
    let sustained_byte_max = *sustained_bytes
        .iter()
        .max()
        .expect("there are fan-out receivers");
    let sustained_packet_total: u64 = sustained_packets.iter().sum();
    let sustained_byte_total: u64 = sustained_bytes.iter().sum();
    let dropped_min = *dropped_payloads
        .iter()
        .min()
        .expect("there are fan-out receivers");
    let dropped_max = *dropped_payloads
        .iter()
        .max()
        .expect("there are fan-out receivers");

    println!(
        "MEASURED 30-receiver fan-out E2E: {} receivers × {expected} automatic fires = {} exactly-once presentations; \
         initial-volley completion opaque Link packets per client min/max {packet_min}/{packet_max}, bytes min/max \
         {byte_min}/{byte_max}, aggregate {packet_total} packets/{byte_total} bytes, slowest {slowest_completion} steps; \
         sustained 1-second target-envelope stream + reliable cannon completion packets min/max \
         {sustained_packet_min}/{sustained_packet_max}, bytes min/max {sustained_byte_min}/{sustained_byte_max}, aggregate \
         {sustained_packet_total} packets/{sustained_byte_total} bytes, slowest {slowest_sustained_completion} steps; \
         reliable outcome/damage sent-unacked high-water {}/{}, settled {}/{}; \
         seeded loss drops per receiver min/max {dropped_min}/{dropped_max}; runtime {:?}. These Link counters include control, replication, \
         acknowledgement, and game payloads — not shot bytes or IP/UDP bytes.",
        FANOUT_RECEIVERS,
        FANOUT_RECEIVERS as usize * expected,
        metrics.max_reliable_outcome_sent_unacked_messages,
        metrics.max_reliable_damage_sent_unacked_messages,
        metrics.reliable_outcome_sent_unacked_messages,
        metrics.reliable_damage_sent_unacked_messages,
        started.elapsed(),
    );
}

/// A client that joins after the replicated combat root exists receives public life state but never
/// the root's owner-private crew or weapon-gate snapshots.
#[test]
fn late_observer_receives_public_status_without_private_combat() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let mut shooter = build_client(
        port,
        SHOOTER_CLIENT_ID,
        SHOOTER_SEED,
        HarnessClient::Shooter,
    );
    shooter.world_mut().resource_mut::<SeededLoss>().loss = 0.0;
    finish(&mut server);
    finish(&mut shooter);

    let mut owner_ready = false;
    for _ in 0..1_200 {
        step_many(&mut server, std::slice::from_mut(&mut shooter));
        owner_ready = shooter
            .world_mut()
            .query_filtered::<(&NetCrew, &WeaponGate), (With<NetTank>, With<NetTankStatus>)>()
            .iter(shooter.world())
            .any(|(crew, gate)| {
                crew.volumes
                    == [VolumeSnapshot {
                        hp: OWNER_SNAPSHOT_HP,
                        crew: None,
                    }]
                    && gate.weapons
                        == [WeaponGateState {
                            ready_tick: None,
                            paused_at_tick: None,
                            belt_remaining: OWNER_BELT,
                        }]
            });
        if client_connected(&mut shooter) && owner_ready {
            break;
        }
    }
    assert!(
        owner_ready,
        "the owner did not receive its private combat state before the late join"
    );

    let mut observer = build_client(
        port,
        OBSERVER_CLIENT_ID,
        OBSERVER_SEED,
        HarnessClient::FanoutReceiver,
    );
    observer.world_mut().resource_mut::<SeededLoss>().loss = 0.0;
    finish(&mut observer);
    let mut clients = vec![shooter, observer];

    let mut late_public_ready = false;
    for _ in 0..1_200 {
        step_many(&mut server, &mut clients);
        late_public_ready = clients[1]
            .world_mut()
            .query_filtered::<(), (With<NetTank>, With<NetTankStatus>)>()
            .iter(clients[1].world())
            .next()
            .is_some();
        if client_connected(&mut clients[1]) && late_public_ready {
            break;
        }
    }
    assert!(
        late_public_ready,
        "the late observer never received the existing tank's public life state"
    );
    let leaked = clients[1]
        .world_mut()
        .query_filtered::<(), (With<NetTank>, Or<(With<NetCrew>, With<WeaponGate>)>)>()
        .iter(clients[1].world())
        .next()
        .is_some();
    let arrivals = clients[1].world().resource::<PrivateCombatArrivals>();
    println!(
        "MEASURED late-join disclosure: public_status=1, crew_arrivals={}, weapon_gate_arrivals={}",
        arrivals.crew, arrivals.weapon_gate,
    );
    assert!(
        !leaked && arrivals.crew == 0 && arrivals.weapon_gate == 0,
        "the late observer received owner-private combat state: present={leaked}, crew arrivals={}, weapon-gate arrivals={}",
        arrivals.crew,
        arrivals.weapon_gate,
    );
}

/// Measurement probe for the open automatic-fire continuity fork. A bounded correlated inbound
/// burst covers the current consecutive copy opportunities; a later automatic shot proves the
/// receiver and presentation path recover after the burst rather than merely disconnecting.
#[test]
fn correlated_burst_probe_isolates_consecutive_automatic_copies() {
    let _udp = lock_real_udp_test();
    let port = free_port();
    let mut server = build_server(port);
    let trace_path = std::env::temp_dir().join(format!(
        "overmatch-correlated-burst-{}-{}.jsonl",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos()
    ));
    server.insert_resource(crate::shot_trace::ShotTrace::for_test(&trace_path));
    let mut shooter = build_client(
        port,
        SHOOTER_CLIENT_ID,
        SHOOTER_SEED,
        HarnessClient::Shooter,
    );
    let mut observer = build_client(
        port,
        OBSERVER_CLIENT_ID,
        OBSERVER_SEED,
        HarnessClient::Observer,
    );
    shooter.world_mut().resource_mut::<SeededLoss>().loss = 0.0;
    observer.world_mut().resource_mut::<SeededLoss>().loss = 0.0;
    finish(&mut server);
    finish(&mut shooter);
    finish(&mut observer);

    let mut ready = false;
    for _ in 0..1_200 {
        step(&mut server, &mut shooter, &mut observer);
        let shooter_slot = shooter
            .world_mut()
            .query_filtered::<(), (With<NetTank>, With<ActionState<TankCommand>>)>()
            .iter(shooter.world())
            .next()
            .is_some();
        if clients_connected(&mut shooter, &mut observer)
            && shooter_slot
            && client_root_count(&mut observer) == 1
        {
            ready = true;
            break;
        }
    }
    assert!(
        ready,
        "the burst probe did not reach its replicated ready state"
    );

    server.world_mut().resource_mut::<FireArmed>().0 = true;
    // DERIVED from the scripted cadence: after thirteen armed steps, the first reliable shot has
    // fired and the first automatic shot is three authority steps away.
    for _ in 0..13 {
        step(&mut server, &mut shooter, &mut observer);
    }
    // DERIVED: six consecutive receive updates cover the current emission plus two immediate repair
    // opportunities while tolerating bounded loopback hand-off jitter.
    const BURST_RECEIVE_UPDATES: u32 = 6;
    let burst_start_tick = server.world().resource::<LocalTimeline>().tick().0;
    arm_inbound_burst(&mut observer, BURST_RECEIVE_UPDATES);
    for _ in 0..BURST_RECEIVE_UPDATES {
        step(&mut server, &mut shooter, &mut observer);
    }
    let burst_end_tick = server.world().resource::<LocalTimeline>().tick().0;
    for _ in BURST_RECEIVE_UPDATES..320 {
        step(&mut server, &mut shooter, &mut observer);
    }

    let automatic: Vec<_> = server
        .world()
        .resource::<ScriptedShotModes>()
        .0
        .iter()
        .filter_map(|(shot, mechanism)| {
            (*mechanism == crate::spec::FireMechanism::Automatic).then_some(*shot)
        })
        .collect();
    assert!(
        automatic.len() >= 2,
        "the scripted run did not author both the burst-covered and recovery automatic shots"
    );
    assert!(
        server
            .world_mut()
            .remove_resource::<crate::shot_trace::ShotTrace>()
            .is_some(),
        "the burst trace resource was installed"
    );
    let trace_rows: Vec<serde_json::Value> = std::fs::read_to_string(&trace_path)
        .expect("burst trace readable after recorder drop")
        .lines()
        .map(|line| serde_json::from_str(line).expect("burst trace row is JSON"))
        .collect();
    let covered_send_ticks: Vec<u32> = trace_rows
        .iter()
        .filter(|row| {
            row["k"] == "send"
                && row["s"] == "fire"
                && row["rel"] == false
                && row["c"] == automatic[0].combatant.0
                && row["w"] == automatic[0].weapon
                && row["ft"] == automatic[0].fire_tick
        })
        .map(|row| row["t"].as_u64().expect("send tick is an integer") as u32)
        .collect();
    assert_eq!(
        covered_send_ticks.len(),
        usize::from(super::shot_transport::VISUAL_COPIES),
        "the covered automatic fire must enter transport once per configured copy opportunity"
    );
    assert!(
        covered_send_ticks
            .windows(2)
            .all(|ticks| ticks[1] == ticks[0] + 1),
        "the current copy opportunities must remain consecutive: {covered_send_ticks:?}"
    );
    assert!(
        covered_send_ticks
            .iter()
            .all(|tick| *tick > burst_start_tick && *tick <= burst_end_tick),
        "the covered shot's send ticks {covered_send_ticks:?} escaped the armed burst interval ({burst_start_tick}, {burst_end_tick}]"
    );
    let shells = &observer.world().resource::<ClientShells>().0;
    let covered = shells
        .iter()
        .filter(|(shot, _)| *shot == automatic[0])
        .count();
    let recovered = shells
        .iter()
        .filter(|(shot, _)| *shot == automatic[1])
        .count();
    let burst = observer.world().resource::<InboundBurst>();
    println!(
        "MEASURED correlated burst: send_ticks={covered_send_ticks:?}, armed_server_interval=({burst_start_tick}, {burst_end_tick}], dropped_payloads={}, covered_shot_presentations={covered}, recovery_shot_presentations={recovered}",
        burst.dropped,
    );
    assert!(
        burst.dropped > 0,
        "the correlated burst dropped no payloads"
    );
    assert_eq!(
        covered, 0,
        "the burst-covered automatic fact escaped the configured consecutive-copy window"
    );
    assert_eq!(
        recovered, 1,
        "the first post-burst automatic fact did not recover exactly-once presentation"
    );
    std::fs::remove_file(trace_path).expect("burst trace removed");
}
