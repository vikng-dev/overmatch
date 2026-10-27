//! Real-UDP integration tests for shot transport (ADR-0021).
//!
//! Seeded, content-blind inbound loss remains outside the production protocol path. The harness
//! verifies observer exactly-once presentation, sanctioned outcomes, and owner-private damage facts.

use core::time::Duration;
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};

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
    HeldFireEvents, PendingFireEvents, PendingRecoilKicks, SeenDamage, SeenShots,
    age_sanctioned_shots, install_input_buffer_guard, install_input_send_cadence,
    receive_damage_confirms, receive_fire_events, shipping_input_delay,
};
use super::disclosure::{CombatDisclosure, NetTankStatus};
use super::hit_feel::LocalHitConfirmed;
use super::protocol::{DamageReceipt, NetCrew, NetTank, VolumeSnapshot, protocol_id};
use super::server::attach_replication_sender;
use super::test_harness::{TICK, base_app, finish, free_port, lock_real_udp_test};
use crate::ballistics::{
    BallisticVolume, ComponentHealth, FireShell, FireShellOrigin, Impact, RoundBody,
    SanctionedShots, ShellDamage, Shot, ShotSource,
};
use crate::command::TankCommand;
use crate::damage::VolumeOf;
use crate::tank::{
    Muzzle, TankRoot, TankServos, Weapon, WeaponGate, WeaponGateState, WeaponIndex, WeaponRound,
};
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
const OWNER_SNAPSHOT_HP: u32 = 73;
const OWNER_BELT: u32 = 17;

/// Shots fired by the scripted run: both delivery classes, alternating.
const SHOTS: u32 = 10;

/// DERIVED from the scale scenario: the public roots the sustained stream fires from.
const VOLLEY_COMBATANTS: u64 = 30;
const VOLLEY_WEAPONS: u8 = 2;

/// Fixture data: the harness's rounds as (muzzle speed m/s, body, wound). Every fixture weapon slot
/// lists them in this order, so a scripted shell names its round by index and each client binds
/// the same round from its own fixture weapons ([`arm_fixture_weapons`]).
const FIXTURE_ROUNDS: [(f32, RoundBody, f32); 2] = [
    (
        800.0,
        RoundBody {
            mass: 10.2,
            caliber: 0.088,
            nose_factor: 0.3,
        },
        30.0,
    ),
    (
        800.0,
        RoundBody {
            mass: 0.0118,
            caliber: 0.0079,
            nose_factor: 0.5,
        },
        10.0,
    ),
];
/// The cannon fixture's index in [`FIXTURE_ROUNDS`].
const CANNON_ROUND: u8 = 0;
/// The MG fixture's index in [`FIXTURE_ROUNDS`].
const MG_ROUND: u8 = 1;

/// A scripted authority shell firing `source`'s fixture round from its weapon slot.
fn scripted_shell(
    mechanism: crate::spec::FireMechanism,
    source: ShotSource,
    shot: ShotId,
) -> FireShell {
    let (speed, body, wound) = FIXTURE_ROUNDS[usize::from(source.round)];
    FireShell {
        origin: MUZZLE,
        direction: Dir3::NEG_Z,
        speed,
        caliber: body.caliber,
        mass: body.mass,
        nose_factor: body.nose_factor,
        penetrator_caliber: body.caliber,
        penetrator_mass: body.mass,
        wound,
        burst: None,
        mechanism,
        tracer: true,
        shot_origin: FireShellOrigin::Local,
        shooter: Some(source),
        catch_up_ticks: 0,
        shot: Some(shot),
    }
}

/// Give each replicated root the fixture weapon slots the scripts fire from, as a production
/// client's rig does at spawn: the receive path binds every fire's round from these lists.
fn arm_fixture_weapons(add: On<Add, NetTank>, mut commands: Commands) {
    let rounds: Vec<WeaponRound> = FIXTURE_ROUNDS
        .iter()
        .map(|&(speed, body, wound)| {
            let mut fixture = WeaponRound::resolve("88mm_AP").expect("a shipped round");
            fixture.round.muzzle_speed = speed;
            fixture.round.body = body;
            fixture.round.penetrator = body;
            fixture.round.wound = wound;
            fixture
        })
        .collect();
    for slot in 0..usize::from(VOLLEY_WEAPONS) {
        commands.spawn((
            Muzzle,
            WeaponIndex(slot),
            TankRoot(add.entity),
            Weapon {
                name: format!("fixture {slot}"),
                rounds: rounds.clone(),
                fire_cycle: crate::spec::FireCycle::Single { reload_secs: 1.0 },
                recoil: None,
                barrel: None,
                fire: Vec::new(),
                load: Vec::new(),
                trigger: crate::spec::Trigger::Primary,
                report_clips: Vec::new(),
            },
        ));
    }
}
/// DERIVED from the target firing envelope and 64 Hz authority tick.
const SUSTAINED_AUTOMATIC_FACTS_PER_TICK: usize = 12;
/// DERIVED: one second of sustained traffic at 64 Hz.
const SUSTAINED_STREAM_TICKS: u32 = 64;
/// The reliable single is injected while the automatic stream is active. Its slot must not be one
/// of that tick's automatic slots, preserving the once-per-(combatant, weapon, tick) invariant.
const SUSTAINED_RELIABLE_TICK: u32 = 31;
/// DERIVED bounded repair and ACK-settlement window after a ledger fills: a late duplicate lands
/// inside it.
const SETTLEMENT_STEPS: u32 = 64;

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
    // Attribution names the replicated shooter root. Stable `ShotId` data remains independent of
    // each client's mapped display/self-exclusion entity.
    commands.trigger(scripted_shell(
        mechanism,
        ShotSource {
            tank: shooter,
            weapon: weapon as usize,
            round: CANNON_ROUND,
        },
        shot,
    ));
}

/// The owned, replicated server shooter. It is absent until the real client link connects.
#[derive(Resource, Default)]
struct ShooterTank(Option<Entity>);

/// The public replicated roots the sustained stream fires from. They are deliberately server-side
/// scripted roots rather than socket clients: this isolates public fan-out and batching from
/// client-process scheduling while retaining real server-to-client UDP delivery.
#[derive(Resource, Default)]
struct VolleyRoots(Vec<Entity>);

/// Spawns the public roots, independently from the ordinary mixed-mechanism script.
#[derive(Resource, Default)]
struct VolleyArmed(bool);

/// Enables the sustained automatic stream once the observer holds every public root.
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
    let Some((link, _)) = clients
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
                TankServos::for_count(2),
                NetTankStatus::Active,
                CombatDisclosure::owner(link),
                Replicate::to_clients(NetworkTarget::All),
                DisableReplicateHierarchy,
                InterpolationTarget::to_clients(NetworkTarget::All),
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
        commands.trigger(scripted_shell(
            crate::spec::FireMechanism::Automatic,
            ShotSource {
                tank: roots.0[root_index],
                weapon: weapon as usize,
                round: MG_ROUND,
            },
            ShotId {
                combatant: CombatantId(root_index as u64 + 1),
                weapon,
                fire_tick: timeline.tick().0,
            },
        ));
    }

    if stream_tick != SUSTAINED_RELIABLE_TICK {
        return;
    }
    // DERIVED from the configured cadence: stream tick 31 uses automatic slots 12..24 (exclusive),
    // leaving root zero / weapon zero available.
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
    commands.trigger(scripted_shell(
        crate::spec::FireMechanism::Single,
        ShotSource {
            tank: roots.0[0],
            weapon: 0,
            round: CANNON_ROUND,
        },
        reliable,
    ));
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

/// Every keyed shell's calibre at spawn, on any role: the server's from its scripted round, a
/// client's from the round its fixture weapon lists at the fire event's index.
#[derive(Resource, Default)]
struct ShellCalibers(Vec<(ShotId, f32)>);

fn collect_shell_calibers(fire: On<FireShell>, mut calibers: ResMut<ShellCalibers>) {
    if let Some(shot) = fire.shot {
        calibers.0.push((shot, fire.caliber));
    }
}

/// A hidden keyed shell created by the catch-up armor path (a spawn fast-forwarded into a
/// contact). Under the cursor clock the steady delayed-observer link must record NONE: the round
/// waits in the `CursorQueue` and presents at its own tick, age ~0, at the muzzle — a hold here
/// means a presentation site aged the shell on the arrival clock again.
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
    if impact.outcome.deflection().is_some() {
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
    // The plate stands in for a tank's armour, which always names its tank (`VolumeOf`): the
    // authority confirms damage only to a pool a tank owns.
    let owner = app.world_mut().spawn_empty().id();
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
            material: crate::ballistics::VolumeMaterial::fixture(STEEL),
            substance: "RHA".to_string(),
        },
        VolumeOf(owner),
        // High enough that all scripted ricochet shocks lower HP without saturating. This makes every
        // shot generate one real authority-side `ShellDamage` while preserving the same bounce geometry.
        ComponentHealth {
            current: 100_000,
            max: 100_000,
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
    // The authority's composition: the SIM half alone, exactly as `net::server::run` composes it.
    app.add_plugins(crate::ballistics::sim_plugin);
    spawn_plate(&mut app);

    super::shot_transport::install_server(&mut app);
    app.add_observer(attach_replication_sender);

    app.init_resource::<ShooterTank>();
    app.init_resource::<VolleyRoots>();
    app.init_resource::<VolleyArmed>();
    app.init_resource::<SustainedStreamArmed>();
    app.init_resource::<SustainedStreamShots>();
    app.add_systems(Update, spawn_owned_shooter);
    app.add_systems(Update, spawn_volley_roots.after(spawn_owned_shooter));
    app.init_resource::<FireArmed>();
    app.init_resource::<ServerShots>();
    app.init_resource::<ScriptedShotModes>();
    app.init_resource::<ShellCalibers>();
    app.add_observer(collect_shell_calibers);
    app.init_resource::<ServerBounces>();
    app.init_resource::<ServerDamages>();
    app.add_observer(collect_server_bounces);
    app.add_observer(collect_server_damage);
    app.add_systems(FixedUpdate, (fire_script, fire_sustained_stream));
    // FixedLast observes this tick's complete `Shot` spawns.
    app.add_systems(FixedLast, collect_server_shots);

    let server = app
        .world_mut()
        .spawn((
            NetcodeServer::new(ServerNetcodeConfig {
                protocol_id: protocol_id(),
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
    /// A public-only receiver. It deliberately has neither the shooter's input slot nor the
    /// observer's delayed/trail instrumentation.
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
    install_input_send_cadence(&mut app);
    super::protocol::plugin(&mut app);
    // A CLIENT's composition: both halves. The view half is what dresses a shell with `ShellVisual`,
    // which is the signature the trail harness below attaches its ribbon on.
    app.add_plugins((
        crate::ballistics::sim_plugin,
        crate::ballistics::view_plugin,
    ));
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
    app.init_resource::<HeldFireEvents>();
    app.init_resource::<PendingFireEvents>();
    app.init_resource::<SeenDamage>();
    app.init_resource::<SanctionedShots>();
    app.init_resource::<super::extrapolate::ImpulseTicks>();
    app.init_resource::<super::fire_presentation::OwnFireDiag>();
    app.add_systems(Update, (receive_fire_events, receive_damage_confirms));
    app.add_systems(FixedUpdate, age_sanctioned_shots);
    app.add_observer(arm_fixture_weapons);

    app.init_resource::<ClientShells>();
    app.init_resource::<ShellCalibers>();
    app.add_observer(collect_shell_calibers);
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
    } else {
        app.add_systems(
            PreUpdate,
            count_raw_inbound_payloads
                .in_set(LinkSystems::Receive)
                .after(LinkReceiveSystems::ApplyConditioner)
                .before(drop_packets),
        );
    }
    app.add_systems(
        PreUpdate,
        drop_packets
            .in_set(LinkSystems::Receive)
            .after(count_raw_inbound_payloads),
    );

    let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    // The input timeline is what SYNCS `LocalTimeline` to the server's tick — the present `P` that
    // `fire_catch_up_ticks` measures a shot's age against. Same fixed input delay as the shipping
    // client, and inserted the same way: an app-global resource, after `ClientPlugins`.
    app.insert_resource(InputTimelineConfig::new(
        SyncConfig::default(),
        shipping_input_delay(),
    ));
    let client = app
        .world_mut()
        .spawn((
            Client,
            Link::default(),
            LocalAddr(SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)),
            PeerAddr(server_addr),
            // As on the shipping client: lightyear's client replication filters on it.
            ReplicationReceiver,
            NetcodeClient::new(
                Authentication::Manual {
                    server_addr,
                    client_id,
                    private_key: [0; 32],
                    protocol_id: protocol_id(),
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

/// Advance one server and many real client apps with two bounded kernel hand-offs. It does NOT
/// sleep per client: the hand-off cost stays independent of the client count.
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

/// Every scripted shot has resolved on the authority and each ledger holds at least its count: a
/// shell per shot on both clients, every sanctioned ricochet carried and rendered on the observer,
/// a hit confirm per damaging shot on the shooter. The exact counts are asserted after settlement.
fn mixed_ledgers_filled(server: &App, shooter: &App, observer: &App) -> bool {
    let fired = server.world().resource::<ServerShots>().0.len();
    let bounced = server.world().resource::<ServerBounces>().0.len();
    let damaged = server.world().resource::<ServerDamages>().0.len();
    fired == SHOTS as usize
        && damaged == fired
        && observer.world().resource::<ClientShells>().0.len() >= fired
        && shooter.world().resource::<ClientShells>().0.len() >= fired
        && observer.world().resource::<ClientBounces>().0 as usize >= bounced
        && observer.world().resource::<ClientRenderedBounces>().0.len() >= bounced
        && shooter.world().resource::<ClientHitConfirms>().0.len() >= damaged
}

/// **THE TRIPWIRE.** Over a real Lightyear link with configured seeded loss, every authority shot
/// spawns exactly one observer shell and every sanctioned ricochet carries through. Then, with one
/// second of target-envelope automatic fire saturating the visual queue, a reliable cannon shot
/// presents exactly once on the observer and its owner-private `DamageConfirm` exactly once on its
/// owner.
///
/// If this fails, the split shot transport or the `ShotId` dedup (`net::client::SeenShots`) does not
/// do on a real channel what its focused tests say it does — read the failure message for which half
/// broke.
#[expect(
    clippy::too_many_lines,
    reason = "one real-UDP scenario whose phases (a volley under seeded loss, a saturating automatic \
              stream around one reliable cannon shot, the owner-private receipt) each assert on the \
              live link the previous phase built, so splitting them would rebuild that link per phase"
)]
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

    // Continue unmeasured until every ledger has its count, then through the settlement window so a
    // late duplicate still lands before the exact counts below. This traffic is deliberately
    // excluded from the bounded Link-level estimate above.
    for _ in 0..1_200 {
        step(&mut server, &mut shooter, &mut observer);
        if mixed_ledgers_filled(&server, &shooter, &observer) {
            break;
        }
    }
    for _ in 0..SETTLEMENT_STEPS {
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
    // FUSED OWN FIRE: the shooter presents its own rounds from the server echo — exactly once per
    // shot, over the same loss-repaired transport the observer rides.
    let mut own_missing = Vec::new();
    let mut own_duplicated = Vec::new();
    for shot in &fired {
        let n = shooter_spawned
            .iter()
            .filter(|(seen, _)| seen == shot)
            .count();
        match n {
            1 => {}
            0 => own_missing.push(*shot),
            _ => own_duplicated.push((*shot, n)),
        }
    }
    assert!(
        own_missing.is_empty(),
        "{} of {} shots never presented on the SHOOTER at {:.0}% loss ({shooter_dropped} payloads \
         dropped, {shooter_passed} delivered) — under fused own fire the echo is the shooter's only \
         presentation path: {own_missing:?}",
        own_missing.len(),
        fired.len(),
        LOSS * 100.0,
    );
    assert!(
        own_duplicated.is_empty(),
        "{} shooter shot(s) presented MORE THAN ONCE — the ShotId dedup (SeenShots) admitted a \
         redundancy-window duplicate of the own echo: {own_duplicated:?}",
        own_duplicated.len(),
    );
    assert_eq!(
        shooter_spawned.len(),
        fired.len(),
        "the shooter spawned {} shells for {} shots — an unattributed shell got through",
        shooter_spawned.len(),
        fired.len(),
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

    // THE CURSOR ABSORBS A STEADY TRANSPORT DELAY. The configured 12-tick raw receive delay used
    // to force local-clock catch-up spawns (hidden keyed armor holds at the plate); under the
    // cursor clock the delayed round presents at its own tick, age ~0, at the muzzle — so the
    // delayed observer must record NO hidden hold. Reverting a presentation site to arrival-clock
    // aging fast-forwards these shells ~12 ticks (~150 m, past the 125 m plate) and reds this.
    // Catch-up survives only for a round arriving after its cursor crossing (a loss-repaired
    // resend), whose seeded-deterministic age on this link stays under the shell's flight to the
    // plate (RANGE / one tick of flight).
    let observed_catch_up_holds: Vec<_> = observer_catch_up_holds
        .iter()
        .copied()
        .filter(|held| observer_spawned.iter().any(|(shot, _)| shot == held))
        .collect();
    assert!(
        observed_catch_up_holds.is_empty(),
        "hidden keyed armor hold(s) under a steady delay — a presentation site is aging shells \
         on the arrival clock again: {observed_catch_up_holds:?}"
    );
    let max_spawn_age = observer_spawned
        .iter()
        .map(|(_, catch_up)| *catch_up)
        .max()
        .unwrap_or(0);
    println!(
        "MEASURED delayed-observer spawn ages: {} shells, max catch_up {max_spawn_age} tick(s)",
        observer_spawned.len(),
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

    // RELIABLE UNDER CONTENTION: one second of target-envelope automatic fire from every public
    // root keeps the visual queue saturated while a reliable cannon shot and its owner-private
    // damage confirmation enter the same transport flush.
    let presentations = |client: &App, shot: ShotId| {
        client
            .world()
            .resource::<ClientShells>()
            .0
            .iter()
            .filter(|(seen, _)| *seen == shot)
            .count()
    };
    let receipts = |client: &App, shot: ShotId| {
        client
            .world()
            .resource::<ClientHitConfirms>()
            .0
            .iter()
            .filter(|receipt| **receipt == DamageReceipt::from(shot))
            .count()
    };
    server.world_mut().resource_mut::<VolleyArmed>().0 = true;
    let mut roots_ready = false;
    for _ in 0..1_200 {
        step(&mut server, &mut shooter, &mut observer);
        if client_root_count(&mut observer) == VOLLEY_COMBATANTS as usize {
            roots_ready = true;
            break;
        }
    }
    assert!(
        roots_ready,
        "the observer did not receive all {VOLLEY_COMBATANTS} public roots before the stream"
    );
    server.world_mut().resource_mut::<SustainedStreamArmed>().0 = true;
    let mut reliable = None;
    for _ in 0..1_800 {
        step(&mut server, &mut shooter, &mut observer);
        let stream = server.world().resource::<SustainedStreamShots>();
        reliable = reliable.or(stream.reliable);
        if let Some(shot) = reliable
            && stream.ticks_emitted == SUSTAINED_STREAM_TICKS
            && presentations(&observer, shot) == 1
            && receipts(&shooter, shot) == 1
        {
            break;
        }
    }
    for _ in 0..SETTLEMENT_STEPS {
        step(&mut server, &mut shooter, &mut observer);
    }
    let reliable = reliable.expect("the sustained stream never injected its reliable cannon shot");
    assert_eq!(
        server
            .world()
            .resource::<SustainedStreamShots>()
            .ticks_emitted,
        SUSTAINED_STREAM_TICKS,
        "the sustained automatic stream ended before its one-second target envelope"
    );
    assert_eq!(
        presentations(&observer, reliable),
        1,
        "the observer did not present the reliable cannon fire exactly once under sustained visual \
         contention"
    );
    assert_eq!(
        receipts(&shooter, reliable),
        1,
        "the reliable cannon's owner-private DamageConfirm must present exactly once on its owner"
    );

    // ROUND BINDING over the real wire: every shell a client presented flies the round the
    // authority fired. The stream fires the MG fixture round and the scripts the cannon round, so a
    // fire event that lost its round index binds the wrong calibre here.
    let fired_calibers: std::collections::HashMap<ShotId, f32> = server
        .world()
        .resource::<ShellCalibers>()
        .0
        .iter()
        .copied()
        .collect();
    for (role, client) in [("observer", &observer), ("shooter", &shooter)] {
        let presented = &client.world().resource::<ShellCalibers>().0;
        for round in [CANNON_ROUND, MG_ROUND] {
            let caliber = FIXTURE_ROUNDS[usize::from(round)].1.caliber;
            assert!(
                presented.iter().any(|&(_, presented)| presented == caliber),
                "the {role} presented no shell of fixture round {round}"
            );
        }
        for &(shot, caliber) in presented {
            assert_eq!(
                fired_calibers.get(&shot),
                Some(&caliber),
                "the {role} bound a round for {shot:?} the authority did not fire"
            );
        }
    }
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

    for _ in 0..1_200 {
        step_many(&mut server, std::slice::from_mut(&mut shooter));
        if client_connected(&mut shooter) {
            break;
        }
    }

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
