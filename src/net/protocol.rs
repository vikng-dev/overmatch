//! The wire contract: everything both sides must register identically. lightyear requires the same
//! protocol registration on client and server (replicated components, the input protocol, the avian
//! prediction/rollback registration) — mismatch here desyncs or fails the connection. If a component
//! or input rides the wire, its registration lives here and nowhere else.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, RigidBody, Rotation};
use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use lightyear::avian3d::plugin::{AvianReplicationMode, LightyearAvianPlugin};
// `Remote` (bevy_replicon's "this entity arrived by replication", re-exported): the honest
// authority-vs-replica discriminator — see `upgrade_predicted_to_dynamic` on why
// `Predicted`/`Interpolated` are not (the server entity carries both markers itself).
use lightyear::core::confirmed_history::ConfirmedHistory;
use lightyear::prelude::client::Remote;
use lightyear::prelude::input::native::{ActionState, NativeBuffer};
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ballistics::ComponentHealth;
use crate::command::TankCommand;
use crate::damage::{
    Ammo, CrewStation, Crewman, DamageConsequences, Dead, LaunchedTurret, PendingSwap,
    TankKnockedOut, TankVolumes, knockout_from_counts,
};
use crate::driving::DriveState;
use crate::state::GameplaySet;
use crate::tank::{Rig, ServoCommand, ServoIndex, ServoSpec, TankSim};

// ---------------------------------------------------------------------------
// Protocol compatibility guard
// ---------------------------------------------------------------------------
//
// WHY THIS EXISTS. bevy_replicon addresses replicated components by their REGISTRATION INDEX, not
// by name, so a client and server built from different revisions of `plugin` (below) silently
// misapply each other's messages: the deployed alpha.4 server replicated `NetHealth` at the index a
// main-built client had since re-registered as `NetCrew`, and the client spammed per-tick
// `unable to apply mutate message ... missing history component` forever, with no hint of the cause
// (2026-07-11 incident). The netcode handshake already carries the mechanism to refuse this cleanly,
// BEFORE replication ever starts: netcode.io's `protocol_id` is folded into the connect-token AEAD,
// so a client whose `protocol_id` differs from the server's produces a token the server cannot
// decrypt — it drops the request and the client times out, exactly as if the server were down (a
// mismatch is transport-indistinguishable from a timeout; see `net::client`'s connect overlay). We
// therefore bake [`PROTOCOL_FINGERPRINT`] into BOTH ends' `protocol_id` (`net::client`/`net::server`),
// turning a version/wire skew into a clean refusal instead of a corrupt-forever connection.

/// The pinned protocol revision. It rides [`PROTOCOL_FINGERPRINT`] alongside the crate version, so
/// two builds that share a crate version but differ on the WIRE SURFACE (the replicated
/// component/message/channel set `plugin` registers) still refuse each other once this is bumped.
///
/// **Bump this — and re-pin [`WIRE_SURFACE_HASH`] — in the SAME diff whenever the wire surface
/// changes** (a replicated component added/removed/reordered/renamed, a message or channel changed,
/// the input type changed). The `wire_surface_is_pinned` tripwire fails until you do, which
/// is the point: it makes a silent wire-breaking change impossible.
pub const PROTOCOL_REV: u32 = 2;

/// The protocol fingerprint both ends bake into their netcode `protocol_id` (`Authentication::Manual`
/// on the client, `NetcodeConfig` on the server). Derived at COMPILE TIME from [`PROTOCOL_REV`] + the
/// crate version, so it is a pure function of the build — the SAME build always yields the SAME value
/// (the two-app integration tests build both ends from this crate, so they always agree and still
/// connect), and any version bump OR [`PROTOCOL_REV`] bump changes it, refusing a skewed peer.
pub const PROTOCOL_FINGERPRINT: u64 = protocol_fingerprint();

/// FNV-1a over `bytes`, continuing from `seed` — a tiny, dependency-free, `const`-evaluable hash so
/// [`PROTOCOL_FINGERPRINT`] is a compile-time constant with no build script or proc macro. Not
/// cryptographic (it doesn't need to be — `protocol_id` is a compatibility tag, not a secret; the
/// dev private key is a separate `[0; 32]`), just a stable well-mixed fold of the inputs.
const fn fnv1a_64(seed: u64, bytes: &[u8]) -> u64 {
    let mut hash = seed;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    hash
}

/// Fold [`PROTOCOL_REV`] then the crate version into one `u64` (FNV-1a offset basis as the seed).
const fn protocol_fingerprint() -> u64 {
    let rev_bytes = PROTOCOL_REV.to_le_bytes();
    let after_rev = fnv1a_64(0xcbf2_9ce4_8422_2325, &rev_bytes);
    fnv1a_64(after_rev, env!("CARGO_PKG_VERSION").as_bytes())
}

/// Replicated tank-identity marker — how the client recognizes a replicated entity as a tank
/// (before its local sim body exists) without replicating the sim's own `Tank` marker. Deliberately
/// NOT `Tank`: replicating `Tank` fires its `On<Add, Tank>` observers at replication-receive time,
/// ahead of the client's sim-body build, and that ordering deterministically NaN'd the tank at
/// `Dynamic` activation (4/4 crash, 2026-07-05 restructure regression — root pos/rot/velocities all
/// NaN within a frame). The sim's `Tank` stays a local component that arrives with the sim body,
/// exactly like every other rig component (`spawn_tank_sim`).
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct NetTank;

/// Replicated bot marker — `Name` is not replicated, so the client can't read the server's
/// `Name::new("Bot")`; this rides the wire so the client's HUD can prefix the bot's nameplate with
/// `[BOT]`. Plain replication like [`NetTank`] (no prediction/interpolation).
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct NetBot;

/// Authoritative turret/gun angles (radians, parent-local — `ServoState::current`'s own frame),
/// published on the tank root by the authority and replicated. Remote (interpolated) tanks —
/// other players' tanks, from step 9 — have no local servo sim; this is how their rigs lay.
///
/// Applied as `ServoCommand` *targets*, not written into `ServoState`: the local servo mechanism
/// (`drive_servos`) chases the authoritative angle under its real speed/accel profile, which
/// smooths replication-rate steps for free — no interpolation registration, no transform fights
/// with `interpolate_servos`. The hull MG's servos are deliberately not covered yet (per-weapon
/// laying is its own slice); a remote hull MG rests until then.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct ServoAngles {
    pub turret: f32,
    pub gun: f32,
}

/// Authoritative per-crew-seat occupancy, published on the tank root by the authority and replicated
/// so the client renders swap progress and a foreign backfill (`Crewman.home != seat`) without
/// running the swap flip locally. Travels INSIDE [`NetCrew`] so occupancy, HP, and aliveness are one
/// atomic snapshot (see the type doc). `home` is the occupant's native station; `dead` is the
/// server's authoritative aliveness (its monotonic `Dead` latch, fed only by its own sim).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct CrewSnapshot {
    /// The occupant's native station (specialty) — after a backfill swap this differs from the seat's
    /// own [`CrewStation`], which is a fixed local fact both ends spawn from the RON.
    pub home: CrewStation,
    /// Whether the occupant is dead on the authority. The client DERIVES its local `Dead` marker from
    /// this each tick (idempotent), never latching it from re-assertable HP.
    pub dead: bool,
}

/// One health-bearing volume's authoritative snapshot within [`NetCrew`]: its HP, plus — for a crew
/// seat — the occupant facts. `crew` is `None` for module/ammo volumes (they have HP but no occupant).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    /// The volume's live `ComponentHealth.current`.
    pub hp: f32,
    /// The occupant facts for a crew seat; `None` for a module or ammo volume.
    pub crew: Option<CrewSnapshot>,
}

/// The authoritative combat snapshot of a tank, published on the root by the authority and replicated
/// so the client's death / HUD / crew bar emerge from server-owned state (server-authoritative combat).
///
/// **One atomic snapshot, so no frame is internally inconsistent.** It SUBSUMES the former `NetHealth`:
/// every health-bearing volume's HP travels here in `TankVolumes` iteration order (the SAME order both
/// ends derive, since both build the rig from one RON spec via `spawn_tank_sim` — sorted-by-name volume
/// spawn), and each crew seat's occupancy (`Crewman.home`) and aliveness (`Dead`) ride the SAME entry.
/// A crew swap moves a live occupant between seats; replicating HP alone (as `NetHealth` did) let the
/// server's still-pre-swap HP re-assert onto the seat a client-side flip had just moved the live man
/// into — the corruption this component exists to end (the client no longer flips; it reads this).
/// Index `i` maps to the same volume on both ends; a length mismatch at apply time skips the tank.
///
/// `swap` carries the in-flight backfill (`(source seat, target seat, seconds remaining)`) so the crew
/// bar's countdown is a cosmetic reading of replicated state (ADR-0014), not a client-predicted timer.
///
/// Plain replication (no prediction/interpolation), same idiom as [`ServoAngles`]; `set_if_neq` on
/// publish so a resting tank stops churning change-detection. During a swap the `remaining` countdown
/// changes every tick, so the snapshot DOES resend each tick then — deliberate: a swap is a rare 4 s
/// event and replicating its progress is the point; at rest (`swap == None`, HP and crew stable) the
/// snapshot is stable and `set_if_neq` suppresses idle churn exactly as `NetHealth` did.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetCrew {
    /// Every health-bearing volume in [`health_bearing_volumes`] order: HP + (for seats) occupancy.
    pub volumes: Vec<VolumeSnapshot>,
    /// The in-flight backfill swap, if any: `(source seat, target seat, seconds remaining)`.
    pub swap: Option<(CrewStation, CrewStation, f32)>,
}

/// Authoritative world pose of the launched (cooked-off) turret, published on the tank root by the
/// authority and replicated so the client can SHOW the toss it does NOT simulate locally
/// (`damage::launch_turrets_on_cookoff` early-returns on the `ClientReplica` gate — a client-local
/// launch pops to an unsynced origin and re-fires on reconnect). `None` until the turret launches,
/// then `Some((world position, world rotation))` — the "Approach A" design: keep the turret on the
/// client's locally-built rig (the `Rig.turret` join key) and drive it KINEMATICALLY from this
/// datum instead of promoting it to its own replicated entity. Plain replication (no
/// prediction/interpolation), same idiom as [`ServoAngles`]/[`NetHealth`]; `set_if_neq` on publish
/// so a resting turret stops churning change-detection (and replication resends).
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct LaunchedTurretPose(pub Option<(Vec3, Quat)>);

/// Cosmetic opponent-fire tracer ("FireEvent" seam): a replicated MESSAGE (not a component), one
/// broadcast per authoritative shot, so every OTHER client spawns a LOCAL cosmetic shell for a tank
/// it only interpolates. A remote (interpolated) tank runs no local `fire` — it has no
/// `ActionState`/`TankCommand` — so without this its shots are invisible; a client sees only its
/// OWN predicted tank's shells. Loss-tolerant BY CONSTRUCTION: damage is server-authoritative
/// (`NetHealth`), so a dropped `FireEvent` costs a missing tracer, never a missing hit — which is
/// exactly why [`FireChannel`] is unreliable + unordered.
///
/// Geometry mirrors [`crate::ballistics::FireShell`] (origin / bore / speed / caliber / mass) so the
/// receiver can re-raise that same event and let the existing `integrate_projectiles` fly it (already
/// damage-gated off under `ClientReplica` — cosmetic with no new gating). The bore rides as a `Vec3`,
/// NOT a `Dir3`: a corrupt/zero direction off the wire must be REJECTED on receipt (hold the tracer)
/// rather than trip a `Dir3` non-zero invariant, so the client reconstructs `Dir3` behind the same
/// bore guard `fire` itself uses.
#[derive(Clone, Serialize, Deserialize)]
pub struct FireEvent {
    pub origin: Vec3,
    /// Bore direction as a raw `Vec3` — see the type doc on why it is not a `Dir3`.
    pub direction: Vec3,
    pub speed: f32,
    pub caliber: f32,
    pub mass: f32,
    /// Whether this round is a tracer, as the shooter's belt decided it (mirrors
    /// [`crate::ballistics::FireShell::tracer`]). Carried so every remote client dresses the shell the
    /// SAME way the shooter and server do — the emissive streak for a tracer, no visual for a
    /// non-tracer MG round — rather than re-deriving it (a remote has no belt counter for that tank).
    pub tracer: bool,
    /// The firing tank root, ENTITY-MAPPED (`MapEntities` below) so the server entity resolves to the
    /// receiver's local replica. The client resolves it to kick that tank's barrel recoil spring
    /// (`net::client::apply_pending_recoil_kicks`) — the "replicate the cause" half of remote recoil.
    pub shooter: Entity,
    /// Which weapon fired — its slot in the shooter's `TankSim::weapons` (its `WeaponIndex`). Plain
    /// data, NOT entity-mapped: the receiver reads it against its OWN local rig to find the firing
    /// weapon's `Weapon.recoil.kick`, so nothing about the recoil impulse rides the wire — only which
    /// weapon fired. A `u8` is ample (a tank carries a handful of weapons; 256 slots is unreachable),
    /// and the receiver bounds-checks it against the local `TankSim`/muzzles and SKIPS a slot it
    /// can't resolve — the same "reject off the wire, never panic or index out of bounds" discipline
    /// [`direction`](Self::direction) uses.
    pub weapon: u8,
    /// The server `Tick` the shot was fired on (`broadcast_fire` stamps the server's `LocalTimeline`).
    /// Lets the receiver AGE the shell to where it already is rather than start its flight at the muzzle
    /// when the message arrives: `net::client::receive_fire_events` fast-forwards it by the ticks
    /// elapsed since this one (`fast_forward_shell`), using the same per-tick integrator the live march
    /// steps.
    ///
    /// # Which tick the receiver ages the shell to — the crux
    ///
    /// A projectile has no free will: its entire future is `(origin, direction, speed, fire_tick) +
    /// physics`, so placing it at ANY tick is exact arithmetic, not a guess — unlike a remote tank,
    /// whose future needs a human's next input (which is why a remote tank is interpolated in the past
    /// and a shell honestly can be advanced). At one instant the client holds four tick indices,
    /// ordered `I` (interpolation) and `C` (confirmed) both behind `S` (server now) behind `P`
    /// (predicted present):
    /// - `P = LocalTimeline::tick()` — the tick this client's OWN tank is simulated at. Runs `S + RTT/2
    ///   + jitter_margin + 1 + error_margin − input_delay` (verified: `lightyear_sync`
    ///   `timeline/input.rs` `InputTimeline::sync_objective`).
    /// - `S ≈` `RemoteTimeline::current_estimate` `= last_received_tick + RTT/2` (`timeline/remote.rs`).
    /// - `C = ReplicationCheckpointMap::last_confirmed_tick()`, the newest fully-confirmed server tick.
    /// - `I = S − (interp_delay + jitter)`, where `interp_delay = max(send_interval·1.7, min_delay)`
    ///   (`lightyear_interpolation` `timeline.rs` `to_duration` / `InterpolationTimeline::sync_objective`).
    ///   Our per-tick sender advertises `send_interval = 0`, so `min_delay` is the whole delay — pinned
    ///   to 100 ms by the explicit `InterpolationConfig` in `net::client` (the remote-tank teleport fix;
    ///   the 5 ms lightyear default put `I` AHEAD of the newest received keyframe at WAN RTT).
    ///
    /// **We age the shell to `P`. CHOSEN.** Elapsed = `P − fire_tick`. Reasoning:
    /// 1. **The interaction that matters is shell-vs-our-own-predicted-hull, and our hull lives at `P`.**
    ///    This client only ever predicts and *feels* hits on its OWN tank (everyone else's damage is
    ///    server-authoritative via [`NetHealth`]). Co-indexing the shell with `P` puts both objects at
    ///    the same tick in the same physics world, so their collision is well-posed — the property a
    ///    future predicted hit-impulse needs.
    /// 2. **Tick-indexing, not wall-clock, is what makes client and server agree.** Both integrate the
    ///    shell from `fire_tick`; aged to `P` the shell is at `pos(P)` while our hull is our prediction
    ///    for tick `P`, so a local hit falls on the SAME tick number the server's does — same tick,
    ///    same result, no rollback.
    ///
    /// **Rejected — `C` (confirmed frame).** An earlier revision chose this "so the tracer reaches the
    /// victim as its replicated health drops." But our own hull is RENDERED at `P`, ahead of `C`, so a
    /// `C`-indexed shell lags our rendered hull by `P − C` (the full prediction offset) for its whole
    /// flight — it is visibly short of us exactly when we are hit. That IS the bug this change exists to
    /// fix, re-introduced. `S` (server-now) is likewise rejected: it is a future the client cannot
    /// confirm, arriving one-way-latency early.
    ///
    /// **Rejected — `I` (interpolation frame).** Aging to `I` would make the shell exit the interpolated
    /// *shooter's* rendered barrel cleanly. That is a cosmetic win paid for with correctness: the
    /// shell-vs-own-hull physics goes ill-posed. You can be coherent with the server / your own hull, or
    /// with the interpolated shooter's barrel, NOT both; we choose the former.
    ///
    /// # The cost, named not buried
    ///
    /// The shell appears ALREADY DOWNRANGE: `P − fire_tick ≈ (P − S) + RTT/2`. MEASURED at 80 ms/10 ms
    /// (RTT ≈ 91 ms): `P − S` ≈ +1.0 tick, `S − C` ≈ +2.96 ticks, so the skip is **≈4 ticks ≈ 61 ms ≈
    /// ~49 m at 800 m/s**, growing with RTT (`design/timelines-and-shear.md` §2). An earlier revision
    /// of this comment derived ~10 ticks / ~125 m by assuming a prediction margin of ~RTT/2; the
    /// margin is far smaller, because `InputDelayConfig::balanced()` absorbs most of the round trip
    /// into input delay rather than prediction. That is not a bug — it is information the client did not have
    /// (it learned of the shot late). Mitigated by populating [`crate::ballistics::ShellPath`] across
    /// the skipped flight from the same integrator, so the tracer reads as a round already in the air
    /// rather than one teleporting into existence. (The `FireEvent` channel being unreliable/unordered
    /// — see [`FireChannel`] — only ever adds to that lateness, never removes it.)
    ///
    /// NOT entity-mapped and NOT a `Dir3`-style guarded value: a raw tick is meaningless to remap, and
    /// the receiver clamps/rejects an absurd value itself (`net::client::fire_catch_up_ticks`).
    pub fire_tick: Tick,
}

impl MapEntities for FireEvent {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        // Only `shooter` is an entity; every other field is plain geometry or data (the `weapon`
        // slot is read against the receiver's own local rig, so it is NOT mapped).
        self.shooter = mapper.get_mapped(self.shooter);
    }
}

/// The dedicated channel [`FireEvent`] rides: unreliable + unordered, matching the message's
/// loss-tolerance (a missing tracer is cosmetic; there is nothing to retry or re-sequence). A
/// zero-sized marker type — `Channel` is blanket-implemented for any `Send + Sync + 'static` type
/// (lightyear_transport channel/mod.rs), so the type IS the channel; its settings are registered in
/// [`plugin`].
pub struct FireChannel;

/// The tank's health-bearing volumes in `TankVolumes` order — the SINGLE definition of which volumes
/// (and in what order) [`NetHealth`] snapshots, so publish and apply can never drift out of alignment
/// (index `i` addresses the same volume on both ends). `has_health` is the caller's query membership
/// test, so this serves both the immutable (publish) and mutable (apply) `ComponentHealth` query.
///
/// `pub(crate)` so the view-layer hit-feel cue (`net::hit_feel`) can map a `NetHealth` index back to
/// its volume entity through the SAME ordered filter both wire ends use — a bearing for the hit
/// direction reads off that volume's world pose. One filter, one order, no drift.
pub(crate) fn health_bearing_volumes(
    volumes: &TankVolumes,
    has_health: impl Fn(Entity) -> bool,
) -> Vec<Entity> {
    volumes.iter().filter(|&v| has_health(v)).collect()
}

/// The read side of [`publish_net_crew`]: one health-bearing volume's live facts. `crew`/`crewman`/
/// `dead` are the crew-seat occupancy the snapshot carries (ammo-ness is NOT published — the client
/// reads its own local `Ammo` marker against the replicated HP to spot a cooked-off bin).
#[derive(QueryData)]
struct VolumeSnapshotSource {
    health: &'static ComponentHealth,
    crew: Option<&'static CrewStation>,
    crewman: Option<&'static Crewman>,
    dead: Option<&'static Dead>,
}

/// Authority side: collect the tank's atomic combat snapshot — every health-bearing volume's HP plus
/// each crew seat's occupancy/aliveness, in the shared [`health_bearing_volumes`] order — into the
/// replicated [`NetCrew`], and stamp any in-flight [`PendingSwap`] as station pairs.
/// `FixedPostUpdate` (after the damage chain has run this tick), `Without<Remote>` = authority-only in
/// shared code (every client tank carries `Remote` — see `publish_servo_angles`). The collect order
/// is exactly the apply order in [`apply_net_crew`].
fn publish_net_crew(
    mut tanks: Query<(&TankVolumes, Option<&PendingSwap>, &mut NetCrew), Without<Remote>>,
    sources: Query<VolumeSnapshotSource>,
    stations: Query<&CrewStation>,
) {
    for (volumes, pending, mut net) in &mut tanks {
        let snapshot: Vec<VolumeSnapshot> =
            health_bearing_volumes(volumes, |v| sources.contains(v))
                .iter()
                .map(|&v| {
                    let Ok(src) = sources.get(v) else {
                        return VolumeSnapshot {
                            hp: 0.0,
                            crew: None,
                        };
                    };
                    VolumeSnapshot {
                        hp: src.health.current,
                        // A crew seat carries its occupant's home (defaulting to the seat before any
                        // backfill) and the authority's monotonic aliveness.
                        crew: src.crew.map(|seat| CrewSnapshot {
                            home: src.crewman.map_or(*seat, |c| c.home),
                            dead: src.dead.is_some(),
                        }),
                    }
                })
                .collect();
        // The in-flight swap as STATIONS (stable on the wire, unlike the seat entity ids in
        // `PendingSwap`); dropped if a seat vanished mid-flight, matching `tick_swaps`.
        let swap = pending.and_then(|ps| {
            let (Ok(&a), Ok(&b)) = (stations.get(ps.a), stations.get(ps.b)) else {
                return None;
            };
            Some((a, b, ps.remaining))
        });
        // `set_if_neq`: no change-detection churn (nor replication resends) while at rest.
        net.set_if_neq(NetCrew {
            volumes: snapshot,
            swap,
        });
    }
}

/// The write side of [`apply_net_crew`]: mutate one volume's HP + occupant `home`, read its
/// aliveness/ammo membership. The tank ROOT (`NetCrew`/`TankVolumes`) and its VOLUMES are disjoint
/// entities, so this and the root query never alias.
#[derive(QueryData)]
#[query_data(mutable)]
struct VolumeSink {
    health: &'static mut ComponentHealth,
    crewman: Option<&'static mut Crewman>,
    ammo: Has<Ammo>,
    dead: Has<Dead>,
}

/// Client side: realize the replicated [`NetCrew`] onto each `Remote` tank — write authoritative HP
/// and occupant `home` onto its local volumes, DERIVE each seat's `Dead` marker idempotently, and
/// DERIVE the tank's `TankKnockedOut` label from the same snapshot. This replaces the former
/// `apply_net_health` and the client's run of the monotonic-latching damage chain
/// (`kill_crew`/`mark_dead_tanks`/`process_cookoffs`, now authority-only): the client no longer
/// *decides* death from re-assertable HP, it *reads* it (ADR-0016 — replicate the cause, derive the
/// consequence; death is absorbing, so the server's latch needs no history and the client's derive
/// needs no latch).
///
/// **This system is TICK-AGNOSTIC, and that is only safe because state rollback runs in
/// `RollbackMode::Check`.** It applies the newest confirmed snapshot to whatever tick is being
/// simulated — forward tick or replayed tick alike. [`NetCrew`] is plain-replicated (no `.predict()`,
/// hence no `ConfirmedHistory`), so it holds exactly one value, and a rollback replay of ticks
/// `T..present` writes that one value onto every replayed tick.
///
/// Applying it FORWARD is correct: the newest-confirmed snapshot is the best estimate for a predicted
/// tick, which is just prediction. Applying it BACKWARD — a post-death snapshot onto a genuinely
/// pre-death tick — would suppress thrust the forward sim applied (the drive/reload/fire capability
/// gate rides `Dead` via `damage::part_qualities`). Unlike the old `NetHealth` design this no longer
/// depends on `Dead` being a *never-rolled-back* latch: here `Dead` is RE-DERIVED from the confirmed
/// snapshot on every (forward or replayed) tick, so a replayed tick simply gets the same
/// newest-confirmed aliveness — consistent by construction rather than by the latch's immunity.
///
/// It stays unreachable for the same structural reason `apply_net_health` was safe: **every STATE
/// rollback starts at `server_confirmed_tick`** (`lightyear_prediction` rollback.rs — `Always`
/// :494-509, `Check` :534-556/:613-635), so a state replay window never begins before the tick whose
/// confirmed snapshot killed the crewman. The residual (a newer snapshot live while a rollback targets
/// an older tick, because `last_confirmed_tick` is a GLOBAL Replicon value applied per-message) is the
/// cross-entity replication lag (1-3 ticks, sub-centimetre), transient and self-healing.
///
/// **Enabling INPUT-side rollback is still the flag to watch** — it targets ticks OLDER than
/// `server_confirmed_tick` (rollback.rs:669/:694) and could land before a death tick — but note this
/// design already removes the old sharp edge there: the capability gate now reads a `Dead` that tracks
/// the confirmed snapshot per tick rather than a never-rolled-back latch, so input rollback no longer
/// needs a *separate* fix for the aliveness gate on top of a tick-correct health representation.
///
/// Ordered `.before(DamageConsequences)` for parity with the authority (whose chain is gated off on
/// the client — `Without` `ClientReplica`); a length mismatch (rig still spawning) skips the tank.
fn apply_net_crew(
    tanks: Query<(Entity, &TankVolumes, &NetCrew, Option<&TankKnockedOut>), With<Remote>>,
    mut volumes: Query<VolumeSink>,
    mut commands: Commands,
) {
    for (tank, tank_volumes, net, knocked_out) in &tanks {
        // The health-bearing volumes in publish order (the SAME shared filter the server used).
        let bearers = health_bearing_volumes(tank_volumes, |v| volumes.contains(v));
        // A length mismatch is expected transiently while the client's rig is still spawning and
        // self-heals once it's fully built; a persistent mismatch means client/server spec skew
        // (a distribution concern — matched builds never skew). Skip rather than write misaligned.
        if bearers.len() != net.volumes.len() {
            continue;
        }
        // Recompute the tank's aggregate crew/ammo state from scratch every tick — no monotonic
        // accumulation — so the knockout label below is a pure function of this snapshot.
        let mut crew_total = 0;
        let mut crew_living = 0;
        let mut ammo_cooked = false;
        for (volume, snap) in bearers.into_iter().zip(&net.volumes) {
            let Ok(mut sink) = volumes.get_mut(volume) else {
                continue;
            };
            sink.health.current = snap.hp;
            if let Some(crew) = snap.crew {
                if let Some(mut crewman) = sink.crewman
                    && crewman.home != crew.home
                {
                    crewman.home = crew.home;
                }
                // Idempotent `Dead` derivation: track the authoritative aliveness, never latch it.
                match (crew.dead, sink.dead) {
                    (true, false) => {
                        commands.entity(volume).insert(Dead);
                    }
                    (false, true) => {
                        commands.entity(volume).remove::<Dead>();
                    }
                    _ => {}
                }
                crew_total += 1;
                if !crew.dead {
                    crew_living += 1;
                }
            }
            if sink.ammo && snap.hp <= 0.0 {
                ammo_cooked = true;
            }
        }
        // Idempotent knockout derivation via the ONE shared rule (`damage::knockout_from_counts`),
        // the same threshold the authority's `mark_dead_tanks` latches from — so "knocked out" means
        // the same thing on both ends, computed once.
        match (
            knockout_from_counts(crew_total, crew_living, ammo_cooked),
            knocked_out,
        ) {
            (Some(reason), None) => {
                commands.entity(tank).insert(TankKnockedOut { reason });
            }
            (None, Some(_)) => {
                commands.entity(tank).remove::<TankKnockedOut>();
            }
            _ => {}
        }
    }
}

/// Client side: mirror the replicated [`NetCrew::swap`] into a LOCAL, cosmetic [`PendingSwap`] on each
/// `Remote` tank, resolving the wire's station pair back to this tank's seat entities. This is the net
/// half of ADR-0016 for the swap timer — replicate the cause (the server's in-flight swap), derive the
/// view state — and it keeps the crew bar (`crew_ui`) a pure SIM-layer view: it reads `PendingSwap`
/// exactly as in single-player, never naming the netcode (the `tests/net_boundary.rs` contract).
///
/// Safe precisely because the deciding swap systems are authority-only: on a net client
/// `damage::apply_crew_swap_commands` and `damage::tick_swaps` are gated off (`Without` `ClientReplica`),
/// so this synthesized `PendingSwap` is never ticked or completed locally — it is a display datum only,
/// re-derived from the authoritative snapshot every tick (so a cancel/complete on the server clears it).
fn mirror_swap_from_net_crew(
    tanks: Query<(Entity, &NetCrew, Option<&PendingSwap>), With<Remote>>,
    seats: Query<(Entity, &CrewStation, &crate::damage::VolumeOf)>,
    mut commands: Commands,
) {
    for (tank, net, pending) in &tanks {
        match net.swap {
            Some((a_station, b_station, remaining)) => {
                let seat_of = |station: CrewStation| {
                    seats
                        .iter()
                        .find(|(_, s, owner)| **s == station && owner.tank() == tank)
                        .map(|(e, ..)| e)
                };
                if let (Some(a), Some(b)) = (seat_of(a_station), seat_of(b_station)) {
                    commands
                        .entity(tank)
                        .insert(PendingSwap { a, b, remaining });
                }
            }
            // No swap in flight on the authority: drop any leftover display datum.
            None => {
                if pending.is_some() {
                    commands.entity(tank).remove::<PendingSwap>();
                }
            }
        }
    }
}

/// Authority side: mirror the live `ServoState` angles onto the replicated root component.
/// `FixedPostUpdate`, so it reads what `drive_servos` (FixedUpdate, after `GameplaySet`) just
/// stepped. `Without<Remote>` makes it authority-only in shared code: every client-side tank
/// arrived by replication and carries `Remote` (see `upgrade_predicted_to_dynamic` on why the
/// `Predicted`/`Interpolated` markers can NOT discriminate here — the server carries both).
fn publish_servo_angles(
    mut tanks: Query<(&Rig, &TankSim, &mut ServoAngles), Without<Remote>>,
    servo_slots: Query<&ServoIndex>,
) {
    for (rig, sim, mut angles) in &mut tanks {
        let angle = |servo| {
            servo_slots
                .get(servo)
                .ok()
                .and_then(|slot| sim.servos.get(slot.0))
                .map(crate::tank::ServoState::current)
        };
        let (Some(turret), Some(gun)) = (angle(rig.turret), angle(rig.gun)) else {
            continue;
        };
        // `set_if_neq`: no change-detection churn (and no replication resends) while at rest.
        angles.set_if_neq(ServoAngles { turret, gun });
    }
}

/// Client side, remote (interpolated) tanks: feed the replicated angles to the local servos as
/// targets — the mechanism does the rest (see [`ServoAngles`]). In `GameplaySet` so it shares the
/// Playing gate with the rest of the sim; `drive_servos` orders itself after the whole set, so the
/// targets land before the mechanism steps. No write conflict with `drive_aim_servos` (also in the
/// set): a remote tank's `TankCommand` stays default (no input slot, and the bridge below skips
/// non-simulated tanks), so `aim` is `None` and that system never touches these tanks' servos.
fn apply_servo_angles(
    tanks: Query<(&ServoAngles, &Rig), (With<Remote>, Without<Predicted>)>,
    mut servos: Query<&mut ServoCommand>,
) {
    for (angles, rig) in &tanks {
        if let Ok(mut turret) = servos.get_mut(rig.turret) {
            turret.target = angles.turret;
        }
        if let Ok(mut gun) = servos.get_mut(rig.gun) {
            gun.target = angles.gun;
        }
    }
}

/// Authority side: mirror the launched turret's live avian world pose onto the replicated root
/// datum. `FixedPostUpdate` (after `launch_turrets_on_cookoff` has made the turret a free body and
/// the solver has stepped it), `Without<Remote>` = authority-only (every client tank carries
/// `Remote`). The join key is `Rig.turret` — the same turret `Entity` the authority launched; once
/// it carries `LaunchedTurret` the second query resolves it, and we publish `Some(pose)`. Before
/// launch the turret isn't a free body (no launched match) so the datum stays its `None` default.
/// `set_if_neq` so a resting turret stops churning change-detection.
fn publish_launched_turret_pose(
    mut tanks: Query<(&Rig, &mut LaunchedTurretPose), Without<Remote>>,
    launched: Query<(&Position, &Rotation), With<LaunchedTurret>>,
) {
    for (rig, mut pose) in &mut tanks {
        if let Ok((position, rotation)) = launched.get(rig.turret) {
            pose.set_if_neq(LaunchedTurretPose(Some((position.0, rotation.0))));
        }
    }
}

/// Client side, remote tanks: realize the authoritative launched-turret pose the client does NOT
/// simulate (`launch_turrets_on_cookoff` is gated off on the replica). `With<Remote>` = replica-only
/// in shared code, exactly like [`apply_servo_angles`]. Two phases keyed off whether the client's
/// `Rig.turret` has been detached yet (`LaunchedTurret` presence — the one-time guard):
///   - Not yet detached: strip the servo attachment (`ChildOf` + the three servo components) and
///     re-spawn the turret as a free `Kinematic` body seeded AT the authoritative pose. `Position`/
///     `Rotation` go in the tuple BEFORE `RigidBody` (the placeholder-NaN discipline — a body must
///     never flush with a `PLACEHOLDER` pose). Inserting `LaunchedTurret` fires the existing
///     `detach_view_on_turret_launch` observer, which reparents the glb turret subtree for free.
///   - Already detached: write the pose straight onto the (kinematic) turret's `Position`/`Rotation`
///     — avian never overrides a kinematic body's pose, so this direct write IS the kinematic drive
///     that tracks the server's flying-then-resting turret. `set_if_neq` to avoid at-rest churn.
///
/// Borrow structure: `tanks` reads the tank ROOT (`Rig`/`LaunchedTurretPose`), `launched` is the
/// read-only detach guard, and `poses` mutates the TURRET's `Position`/`Rotation` — three disjoint
/// component sets over (root vs turret) entities, so no aliasing and no query conflict.
fn apply_launched_turret_pose(
    tanks: Query<(&Rig, &LaunchedTurretPose), With<Remote>>,
    launched: Query<(), With<LaunchedTurret>>,
    mut poses: Query<(&mut Position, &mut Rotation), With<LaunchedTurret>>,
    mut commands: Commands,
) {
    for (rig, pose) in &tanks {
        let Some((position, rotation)) = pose.0 else {
            continue;
        };
        if launched.contains(rig.turret) {
            // Already detached: kinematically drive the free turret to the authoritative pose.
            if let Ok((mut p, mut r)) = poses.get_mut(rig.turret) {
                p.set_if_neq(Position(position));
                r.set_if_neq(Rotation(rotation));
            }
        } else {
            // One-time detach: fires exactly once because the `LaunchedTurret` insert (below) makes
            // the `launched.contains` guard true for every subsequent tick.
            commands
                .entity(rig.turret)
                .remove::<(ChildOf, ServoCommand, ServoIndex, ServoSpec)>()
                .insert((
                    Position(position),
                    Rotation(rotation),
                    RigidBody::Kinematic,
                    LaunchedTurret,
                ));
        }
    }
}

/// Coarsened rollback thresholds for the tank root (map §1): the reference examples' 1 cm / 0.01
/// rad bar is tuned for a single-collider capsule character, not a 16-contact 57 t rig — solver
/// noise on a body this complex trips that bar far more often than genuine misprediction (measured:
/// ~430 rollbacks/15s at 100ms latency vs 13 for the increment-5 primitive, all invisible/converging
/// per the increment-6 log). Correction smoothing (`add_linear_correction_fn`, already wired) hides
/// a ≤5 cm snap; coarsening to 0.05 trades some correctness-under-genuine-desync for a large CPU
/// win on the honest-noise case. Position in metres, Rotation in radians, velocities in m/s or
/// rad/s-equivalent — same shape as the map §1(b) reference thresholds, five times coarser.
///
/// Velocity is deliberately DESYNC-ONLY (1.0), not a noise tripwire — the jitter investigation's
/// reconciliation-amplification finding. A rollback is not free reconciliation: it restores a
/// ~12-tick-old confirmed state and RE-SIMULATES to the present, and the replay is chaotic through
/// friction/contact (stick-slip brush anchors, contact transients), so the corrected present lands
/// farther from the old present than the triggering error ever was — measured on a windowed feel
/// capture: applied visual correction = 5.6× the same-tick sim divergence at median, 43× at p90,
/// corrections active on 41% of frames with |error| p50 0.35 m, from triggers barely over the old
/// 0.20 m/s bar while true positions agreed to 0.5–4 cm. Velocity-triggered rollbacks were
/// INJECTING visible motion, not removing desync. Velocity errors self-damp through the suspension;
/// the position/rotation bars — which actually fire since the Interpolated-marker fix — are the
/// honest desync backstops, so drift is caught at 5 cm regardless. 1.0 m/s keeps the velocity
/// condition only for gross desync (teleports, missed impacts), where a replay is genuinely
/// cheaper than waiting for the position bar. The conditions must stay: without one, lightyear
/// falls back to `PartialEq::ne` — bit-equality that f32 solver output never satisfies.
/// `pub(crate)` because `net::watchdog` re-runs the same comparisons with the same bars — one
/// definition of "desynced enough to roll back", two detectors (receive-time + backstop).
///
/// Two notes from the 2026-07-06 review (ADR-0015): the rollback-count evidence above predates
/// the watchdog — pre-watchdog lat0 rollback COUNTS measured check starvation (the receive-time
/// check silently dead at zero prediction margin, see `net::watchdog`), not convergence, and are
/// invalid as an A/B metric. And these coarsened bars are Layer-2 scaffolding, a ratchet rather
/// than a setting: as the divergence they absorb collapses (contact-restore fix, upstream
/// constraint ordering), tighten them toward the 1 cm / 0.01 rad reference values.
pub(crate) const ROLLBACK_POSITION_M: f32 = 0.05;
pub(crate) const ROLLBACK_ROTATION_RAD: f32 = 0.05;
pub(crate) const ROLLBACK_VELOCITY: f32 = 1.0;

// The mismatch METRICS those thresholds are measured against — like the bars above, defined once
// and shared by both detectors (the registered rollback conditions below and `net::watchdog`'s
// re-run of the same comparison), so "desynced enough to roll back" has exactly one definition:
// one metric, one bar, two call sites.

/// Confirmed-vs-predicted `Position` divergence: straight-line distance (m).
pub(crate) fn position_error(a: &Position, b: &Position) -> f32 {
    (a.0 - b.0).length()
}

/// Confirmed-vs-predicted `Rotation` divergence: shortest rotation angle between the two (rad).
pub(crate) fn rotation_error(a: &Rotation, b: &Rotation) -> f32 {
    a.angle_between(*b)
}

/// Confirmed-vs-predicted `LinearVelocity` divergence: vector difference magnitude (m/s).
pub(crate) fn linear_velocity_error(a: &LinearVelocity, b: &LinearVelocity) -> f32 {
    (a.0 - b.0).length()
}

/// Confirmed-vs-predicted `AngularVelocity` divergence: vector difference magnitude (rad/s).
pub(crate) fn angular_velocity_error(a: &AngularVelocity, b: &AngularVelocity) -> f32 {
    (a.0 - b.0).length()
}

/// The ordered WIRE SURFACE: every replicated component, replicated message, channel, and input type
/// that rides the wire, in the EXACT order [`plugin`] registers them below. This is the surface
/// bevy_replicon addresses by registration index — the thing a version skew corrupts (the motivating
/// `NetHealth`-vs-`NetCrew`-at-the-same-index incident).
///
/// It is HAND-MAINTAINED and bound to `plugin` by the `wire_surface_is_pinned` tripwire (in the test
/// module): this list must mirror the registration block one-for-one, in order. Enumerating
/// lightyear's `ComponentRegistry` at runtime was considered and rejected as disproportionate (it
/// keys on `TypeId`, mixes in lightyear-internal registrations, and its `finish()` poisons the
/// registry) — the sanctioned "ordered list adjacent to the registration block with a comment binding
/// them" is the proportionate guard. Changing the wire (rename/add/remove/reorder a replicated type,
/// message, channel, or the input) must edit this list too, which changes [`WIRE_SURFACE_HASH`] and
/// fails the tripwire — forcing a [`PROTOCOL_REV`] bump + re-pin in the same diff.
///
/// `#[cfg(test)]`: this and [`WIRE_SURFACE_HASH`] exist only to drive that tripwire, so they compile
/// only under test — but they live HERE, adjacent to `plugin`, so an editor of the registration block
/// sees the list they must keep in step.
#[cfg(test)]
const WIRE_SURFACE: &[&str] = &[
    // Plain-replicated markers/snapshots — `app.component::<_>().replicate()`, in order:
    "NetTank",
    "NetBot",
    "ServoAngles",
    "NetCrew",
    "LaunchedTurretPose",
    // The cosmetic-fire channel then message — `app.add_channel` / `app.register_message`:
    "FireChannel",
    "FireEvent",
    // The input protocol — `InputPlugin::<TankCommand>`:
    "TankCommand",
    // Predicted+rollback avian components — `app.component::<_>().replicate().predict()`, in order:
    "Position",
    "Rotation",
    "LinearVelocity",
    "AngularVelocity",
];

/// The pinned structural hash of [`WIRE_SURFACE`]. Re-pin this (and bump [`PROTOCOL_REV`]) in the
/// same diff whenever the wire surface changes; the `wire_surface_is_pinned` tripwire prints the new
/// value in its failure message. `#[cfg(test)]` for the same reason as `WIRE_SURFACE`.
#[cfg(test)]
const WIRE_SURFACE_HASH: u64 = 0x3291_7748_6c3b_98f4;

// ---------------------------------------------------------------------------
// Deep wire-surface coverage (field-level + external-dep skew)
// ---------------------------------------------------------------------------
//
// [`WIRE_SURFACE`] pins the ordered SET OF TYPES that ride the wire; the `plugin_registrations_match_
// wire_surface` tripwire binds that list to the actual `plugin` registration block, and the
// `wire_surface_is_pinned` tripwire pins the list's hash. Together those catch a type ADDED, REMOVED,
// RENAMED, or REORDERED. They do NOT catch a change to what a type SERIALIZES: adding a field to
// `VolumeSnapshot`/`CrewSnapshot`/`NetCrew` renames no registered type, so the fingerprint stays put
// while the two ends misdeserialize each other — the exact silent skew the guard exists to refuse.
//
// COVERAGE MODEL, in two halves that together cover every byte on the wire:
//   * OWN types (defined in this crate) — covered by [`WIRE_TYPES_HASH`]: the `wire_types_are_pinned`
//     tripwire source-scans each wire-facing struct/enum's DEFINITION TEXT (comments/whitespace
//     stripped, so a doc or reformat edit is invisible; a field/variant/type change is not) and hashes
//     the lot. This is the whole `WIRE_SURFACE` own-type graph, followed through embeds: `NetCrew`
//     carries `VolumeSnapshot` which carries `CrewSnapshot`, `TankCommand` (src/command.rs) carries
//     `CrewSwap`, and both `CrewSnapshot` and `CrewSwap` carry `CrewStation` (src/damage.rs).
//   * EXTERNAL types (avian `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity`, plus lightyear's
//     own wire framing) — their source is not in this tree to scan, so they are covered by DEP VERSION:
//     [`WIRE_DEP_AVIAN3D`]/[`WIRE_DEP_LIGHTYEAR`] pin the resolved `Cargo.lock` versions, so a bump of
//     either dep (which can silently change how those types serialize or how lightyear frames them)
//     also trips a tripwire and demands a [`PROTOCOL_REV`] bump.
//
// Every one of these tripwires fails with the SAME instruction — bump `PROTOCOL_REV` and re-pin — so a
// wire change on ANY axis (type set, field layout, or dep version) forces the fingerprint to move.

/// The pinned hash of the OWN wire-facing type DEFINITIONS (field layout, not just names). Re-pin this
/// (and bump [`PROTOCOL_REV`]) in the same diff whenever a wire-facing struct/enum's definition
/// changes; the `wire_types_are_pinned` tripwire prints the new value. See the block above for the
/// coverage model. `#[cfg(test)]` for the same reason as `WIRE_SURFACE`.
#[cfg(test)]
const WIRE_TYPES_HASH: u64 = 0xa913_18e8_4ceb_4ba5;

/// The pinned `Cargo.lock` versions of the external crates whose types ride the wire (avian's
/// replicated physics components; lightyear's wire framing / input protocol). A bump of either can
/// change the on-wire bytes without touching any source in this tree, so it must also bump
/// [`PROTOCOL_REV`]; the `wire_types_are_pinned` tripwire enforces it. `#[cfg(test)]` for the same
/// reason as `WIRE_SURFACE`.
#[cfg(test)]
const WIRE_DEP_AVIAN3D: &str = "0.7.0";
#[cfg(test)]
const WIRE_DEP_LIGHTYEAR: &str = "0.28.0";

/// Registers everything both sides of the wire must agree on: replicated components, the cosmetic
/// fire message/channel, and the `TankCommand` input protocol — the exact surface enumerated in
/// `WIRE_SURFACE` above (keep the two in lockstep; the `wire_surface_is_pinned` tripwire enforces
/// it). Grows as later increments add more (§5/§7 of the spike map).
pub(crate) fn plugin(app: &mut App) {
    app.component::<NetTank>().replicate();
    app.component::<NetBot>().replicate();
    // Plain replication, no `.predict()`/interpolation: predicted tanks simulate their own servos,
    // and non-predicted consumers chase the raw angle through the servo mechanism (see the type).
    app.component::<ServoAngles>().replicate();
    // Server-authoritative atomic combat snapshot (same plain-replication shape as `ServoAngles`):
    // per-volume HP + per-seat occupancy/aliveness + in-flight swap, all in one component so no
    // frame is internally inconsistent. Subsumes the former `NetHealth`. The client's damage/death
    // emerge from this, not a divergent local kill.
    app.component::<NetCrew>().replicate();
    // Authoritative launched-turret world pose (same plain-replication shape): the client shows the
    // cooked-off toss it does NOT simulate locally, driving its own rig turret kinematically.
    app.component::<LaunchedTurretPose>().replicate();

    // The cosmetic opponent-fire tracer (`FireEvent`) and its dedicated loss-tolerant channel. A
    // MESSAGE, not a replicated component: it is a one-shot fire-and-forget event, not a piece of
    // ongoing state. `ServerToClient` (the server is the sole broadcaster — see `net::server`);
    // `add_map_entities` registers `FireEvent`'s `MapEntities` so the `shooter` entity resolves to
    // the receiver's local replica on deserialize. Registered in this SHARED plugin so both ends
    // agree on the message id, direction, and channel — exactly like the `.replicate()` block above.
    app.add_channel::<FireChannel>(ChannelSettings {
        // Unreliable + unordered: a dropped or reordered tracer is cosmetically harmless (damage is
        // server-authoritative via `NetHealth`), so paying for acks/retries/sequencing would be pure
        // overhead on a high-frequency event.
        mode: ChannelMode::UnorderedUnreliable,
        ..default()
    })
    // The CHANNEL's own direction — NOT just the message's. This installs the per-link
    // sender/receiver observers (`add_sender_channel`/`add_receiver_channel` in lightyear_transport)
    // that populate each new `Transport`'s channel senders from the registry; without it the channel
    // exists in the `ChannelRegistry` but no link ever gets a `FireChannel` sender, so every send
    // fails `ChannelNotFound` at runtime (compiles fine — the bug only shows live). Same idiom as
    // lightyear's own `InputChannel`/`RepliconUpdatesChannel` registrations.
    .add_direction(NetworkDirection::ServerToClient);
    app.register_message::<FireEvent>()
        .add_map_entities()
        .add_direction(NetworkDirection::ServerToClient);

    app.add_plugins(input::native::InputPlugin::<TankCommand>::default());

    // Avian replication (map §5): mount lightyear_avian3d's ordering fixes, then register the
    // root's Position/Rotation/velocities as predicted+rollback-eligible. Verbatim rollback
    // conditions/correction/interpolation fns from `avian_3d_character`'s `protocol.rs` — the only
    // real 3D reference in the lightyear repo for this registration shape, except the thresholds
    // (see `ROLLBACK_POSITION_M` etc. above — coarsened for step 7).
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position,
        // Roll back avian's non-replicated SOLVER state across rollback replay, not just the
        // replicated Position/Rotation/velocities. Defaults to `false` (the `..default()` we
        // shipped through step 8) — and with it off, every rollback re-steps the solver against a
        // STALE `ContactGraph`/`ConstraintGraph` and a stale collider BVH (`ColliderTrees`), left
        // wherever the abandoned misprediction ran. The whole block that fixes this is gated on the
        // flag (lightyear_avian3d 0.28 plugin.rs:355-399): `ContactGraph`/`ConstraintGraph`/
        // `PhysicsIslands` `local_rollback`, `RollbackMovedProxies`, and the two PreUpdate repair
        // systems — `restore_collider_tree_from_enlarged_aabbs` (rebuilds the BVH leaves from the
        // rolled-back `EnlargedAabb`s) and `repair_missing_contact_pairs_from_restored_aabbs`. That
        // last one exists precisely because, in avian's own words, "a stale tree can miss contacts
        // even when Position/Velocity were rolled back correctly" — the exact failure the beached-
        // rest repro caught: a tank resting on the §2 side-slope slab edge (hull contact, wheels
        // off terrain) drove a sustained ~12 rollbacks/s storm, every rollback attributed to
        // `LinearVelocity`, because replaying the settled contact against stale solver state
        // produced push-out velocities the honest server rest never had.
        // Mounted in shared `net::plugin`, so both ends register it — but only the client rolls
        // back; the server pays only the per-tick `ContactGraph`/`ColliderAabb` snapshot cost.
        rollback_resources: true,
        ..default()
    });
    // Each condition also feeds the jitter-trace recorder (`crate::trace`) its measured magnitude
    // WHEN it trips — the `trg` attribution on every `rollback` row, so analysis can see which
    // component (and how far out) forced each rollback. `note_if_tripped` measures-compares-notes in
    // one call and returns the trip verdict; it is a no-op unless `SPIKE_TRACE` is set (a single
    // relaxed atomic load), so the untraced hot path is unchanged.
    app.component::<Position>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Position, b: &Position| {
            crate::trace::note_if_tripped("Position", position_error(a, b), ROLLBACK_POSITION_M)
        })
        .add_linear_correction_fn()
        .add_linear_interpolation();
    app.component::<Rotation>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Rotation, b: &Rotation| {
            crate::trace::note_if_tripped("Rotation", rotation_error(a, b), ROLLBACK_ROTATION_RAD)
        })
        .add_linear_correction_fn()
        .add_linear_interpolation();
    // Without an explicit condition these default to `PartialEq::ne` (exact bit equality), which
    // f32 solver output essentially never satisfies between client and server — see the Position
    // comment above for the coarsening rationale (same thresholds, applied uniformly).
    app.component::<LinearVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &LinearVelocity, b: &LinearVelocity| {
            crate::trace::note_if_tripped(
                "LinearVelocity",
                linear_velocity_error(a, b),
                ROLLBACK_VELOCITY,
            )
        });
    app.component::<AngularVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &AngularVelocity, b: &AngularVelocity| {
            crate::trace::note_if_tripped(
                "AngularVelocity",
                angular_velocity_error(a, b),
                ROLLBACK_VELOCITY,
            )
        });

    // Non-replicated rollback state — ROOT-RESIDENT ONLY, by design: the root is the predicted
    // entity, so plain `local_rollback` attaches history with no child decoration machinery
    // (`TankSim` centralizes what used to live on turret/gun/muzzle/wheel children — see its doc
    // for the hazard cluster that design retired).
    app.local_rollback::<DriveState>();
    app.local_rollback::<TankSim>();
    app.add_observer(strip_confirmed_history::<DriveState>);
    app.add_observer(strip_confirmed_history::<TankSim>);

    app.add_systems(
        FixedPostUpdate,
        (
            publish_servo_angles,
            publish_net_crew,
            publish_launched_turret_pose,
        ),
    );
    app.add_systems(
        FixedUpdate,
        (apply_servo_angles, apply_launched_turret_pose).in_set(GameplaySet),
    );
    // Client: realize the authoritative combat snapshot (HP + occupancy + derived Dead/knockout) on
    // every replica tank. `.before(DamageConsequences)` for parity with the authority — whose chain
    // is gated off on the client (`damage::plugin`), where this derivation stands in for it.
    app.add_systems(
        FixedUpdate,
        apply_net_crew
            .in_set(GameplaySet)
            .before(DamageConsequences),
    );
    // Client: keep the crew bar's view-only `PendingSwap` in step with the replicated swap, so the
    // sim-layer `crew_ui` reads it exactly as in single-player (never naming the netcode).
    app.add_systems(FixedUpdate, mirror_swap_from_net_crew.in_set(GameplaySet));
    // Bridge lightyear's input buffer into the sim's own `TankCommand` (command.rs's contract):
    // sim systems (`ramp_drive`, `fire`, `drive_aim_servos`) read `TankCommand`, never
    // `ActionState` directly, so this is the one seam translating net input into sim input.
    // `.before(GameplaySet)`, NOT merely `.before(ConsumeCommandEdges)`: every consumer — the
    // readers (`fire`, `ramp_drive`, `drive_aim_servos`) AND the edge-clearer (`consume_edges`)
    // — lives in `GameplaySet`, and ordering only against `ConsumeCommandEdges` leaves the bridge
    // unordered vs `fire`. Measured failure with the weaker constraint: `fire` ran first, read
    // the pre-bridge command, then `consume_edges` cleared the edge the bridge had just written —
    // the click vanished without any tick consuming it (reload never left 0.0).
    // Not gated `.run_if(not(is_in_rollback))`: replay must re-feed the same historical
    // `ActionState` lightyear itself restores per tick (map §3.4's "no gate needed" class — this
    // is a pure copy from already-correctly-restored state, not an externality).
    app.add_systems(
        FixedUpdate,
        bridge_action_state_to_tank_command.before(GameplaySet),
    );
}

/// Kill lightyear's stale-confirmed poisoning of local-only rollback state: `add_prediction_history`
/// (lightyear_prediction `predicted_history.rs`) fires when a `local_rollback` component is added to
/// an entity that is `Predicted` + carries `ConfirmHistory` — our replicated tank root — and seeds
/// `ConfirmedHistory<C>` with the component's ADD-TIME value, treating it as an authoritative
/// init-message write. For a component the server never replicates that seed is the buffer's only
/// entry forever, and `prepare_rollback` prefers confirmed history over predicted whenever it merely
/// EXISTS — so every state rollback restored `TankSim`/`DriveState` to their add-time defaults
/// instead of the rollback tick's predicted value. Measured symptom chain (2026-07-05): restored
/// `captured=false` made `drive_servos` re-capture servo rest quats from the live (already-slewed)
/// node transform, permanently baking the current lay into the servo zero — turret resolving away
/// from the aim point, gun visibly outside its travel limits — plus per-rollback resets of turret
/// angle, reload timers, and wheel anchors. Stripping the component on add makes `prepare_rollback`
/// fall through to predicted history, which is the correct source for never-replicated state. The
/// seed path is designed for replicated components arriving in init messages; a local-only component
/// added later is outside its intent (upstream report candidate).
fn strip_confirmed_history<C: Component + Clone>(
    add: On<Add, ConfirmedHistory<C>>,
    mut commands: Commands,
) {
    commands
        .entity(add.entity)
        .try_remove::<ConfirmedHistory<C>>();
}

/// Copy this tick's `ActionState<TankCommand>` (lightyear's input-buffer-backed component) into the
/// entity's own `TankCommand` (the sim's actual read contract, `command.rs`) — the seam between
/// networked input and every sim system. Only entities carrying both, which are exactly the
/// locally-simulated tanks: the server's tanks get `ActionState` at spawn, the client's own
/// predicted tank gets it when `InputMarker<TankCommand>` claims the slot (`claim_input_slot`,
/// client module); remote (interpolated) tanks never carry one. `TankCommand` itself comes from
/// `command::core_plugin`'s `attach_command` observer (`On<Add, Tank>`).
///
/// **Edges are only valid on a tick a real input actually arrived for.** lightyear extrapolates a
/// starved input stream by holding the last `ActionState` forever: the server's
/// `update_action_state` calls `InputBuffer::get_predict(tick)`, which returns `get_last()` once
/// `tick` is past the buffered range (`lightyear_inputs` `input_buffer.rs:316` / `server.rs:707`,
/// "equivalent to considering that the player will keep playing the last action"). Hold-last is
/// CORRECT for `TankCommand`'s levels (`throttle`/`steer`/`fire_secondary`) and absolutes
/// (`aim`/`range`), but WRONG for its edges (`fire_primary`/`crew_swap`): a held edge re-latches
/// every tick, so `consume_edges` (`command.rs`) can never win. Left unfixed that is one unrequested
/// shot per reload cycle (`shooting::fire` is reload-gated) and a crew swap that re-arms itself
/// forever — `damage::tick_swaps` drops `PendingSwap` on completion, and the still-held `Start` edge
/// makes `apply_crew_swap_commands` insert a fresh one every ~`SWAP_SECONDS`.
///
/// So consult the entity's own `InputBuffer` for the tick `FixedUpdate` is stepping
/// (`LocalTimeline::tick()`). The `ActionState` is a hold-last extrapolation — and its edges must be
/// dropped — precisely when the buffer HAS data but none for this tick: `get(tick).is_none()` AND
/// `get_last().is_some()`. `get` is the EXACT, non-extrapolating lookup (`None` past the buffered
/// range, resolving `Compressed::SameAsPrecedent` back to the value the client actually sent);
/// `get_last` is `Some` iff the buffer holds ANY entry (vendored `lightyear_inputs`
/// input_buffer.rs:339). Only when both hold is the server's `update_action_state` holding the last
/// input forever. Copy the whole command otherwise, edges included. The four cases:
/// - **Client own tank, forward tick:** buffer non-empty, `get(tick)` `Some` → not held-last → edge
///   passes. In the PRE-SYNC window (before `InputTimeline` syncs) the buffer is absent/empty, so
///   `held_last` is `false` and a genuine click passes — the case the coarser `get(tick).is_some()`
///   rule wrongly dropped.
/// - **Client own tank, rollback replay:** lightyear restores the historical `ActionState` per
///   replayed tick, and the buffer retains `max_rollback_ticks + 1` of history (`lightyear_inputs`
///   `client.rs`), so `get(replayed_tick)` is `Some` → the own fire edge re-fires during replay.
/// - **Server tank, no input message yet:** buffer absent/empty → not held-last → passes, and the
///   `ActionState` is `default()` anyway, so there is no edge to carry. Harmless.
/// - **Server tank, starved:** buffer non-empty, `get(tick)` `None`, `get_last` `Some` → held-last →
///   edges cleared. The original starvation re-latch (`701d0a7`) stays fixed.
///
/// This is shared code mounted on BOTH ends. On the server it fixes the starvation above; on the
/// client `LocalTimeline::tick()` is the REPLAYED tick during rollback resim (incremented in
/// `FixedFirst` even inside rollback).
///
/// **Loss trade (deliberate, NON-FIX).** A fire edge whose input arrives AFTER its tick was
/// simulated is dropped, not fired late — firing an edge on a tick it was not issued for is the bug
/// (the shot leaves at the wrong muzzle pose and diverges from what the client predicted), so past
/// ticks are dropped in every netcode. lightyear's per-message input redundancy normally prevents
/// it: an `InputMessage` carries "the inputs for the last N ticks before T" (vendored
/// `lightyear_inputs` client.rs module doc + `num_ticks *= packet_redundancy`, client.rs:686), so an
/// isolated packet loss does NOT lose the edge — a later message re-carries it and it can still land
/// before the server simulates that tick. Only under loss deep enough to outlast that redundancy
/// window is a fire edge dropped rather than fired late; the client may then have predicted a shot
/// the server never fires, leaving its `reload_remaining` (root-resident in `TankSim`, NOT
/// replicated) disagreeing with the server's until the next shot reconciles it. That is inherent to
/// predicting fire on a lossy input stream, not introduced by the edge-clearing rule.
///
/// **Load-bearing invariant.** `get` resolving `SameAsPrecedent` means two consecutive buffered
/// `fire_primary: true`s would BOTH bridge as edges and fire twice. That is fine because it can only
/// happen for two DISTINCT clicks on back-to-back ticks (two intended shots): `gather_commands`
/// latches the click from `just_pressed` (true for one frame per physical press) and `consume_edges`
/// clears it before the next `feed_action_state`, so a single held mouse button produces exactly ONE
/// buffered `true`. If `gather_commands` is ever changed to latch from `pressed`, a hold would put a
/// run of `true`s in the buffer and this bridge would have to dedupe consecutive edges as well.
fn bridge_action_state_to_tank_command(
    timeline: Res<LocalTimeline>,
    mut tanks: Query<(
        &ActionState<TankCommand>,
        Option<&NativeBuffer<TankCommand>>,
        &mut TankCommand,
    )>,
) {
    let tick = timeline.tick();
    for (action, buffer, mut command) in &mut tanks {
        // Whole-struct copy (matches `ActionState`'s "absolute snapshot per tick" contract) …
        let mut next = action.0;
        // … but a HOLD-LAST EXTRAPOLATION must not carry an edge. The `ActionState` is extrapolated
        // exactly when the buffer HAS data but none for this tick (`get(tick).is_none()` while
        // `get_last().is_some()`) — that is when the server's `update_action_state` holds the last
        // input forever (`get_predict` → `get_last`). An ABSENT or EMPTY buffer is NOT extrapolating
        // (nothing is being held): on the client's own tank the `ActionState` was authored THIS tick
        // by `feed_action_state`, so a genuine click in the pre-sync join/spawn window must pass.
        // `get`, not `get_predict`, is the exact non-extrapolating lookup. See the doc for the full
        // per-case argument (`get_last` semantics: vendored `lightyear_inputs` input_buffer.rs:339).
        let held_last = buffer.is_some_and(|b| b.get(tick).is_none() && b.get_last().is_some());
        if held_last {
            next.clear_edges();
        }
        *command = next;
    }
}

/// The starvation bug the fix above targets, driven over real ECS state. `TankCommand`'s private
/// types are unreachable from an external `tests/` crate, so the honest repro lives in-crate.
#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::Tick;

    use super::*;
    use crate::command::CrewSwap;
    use crate::damage::CrewStation;

    /// FNV-1a over the ordered [`WIRE_SURFACE`] names, each terminated by a `\n` separator so no two
    /// distinct lists can collide by concatenation (`["ab","c"]` must not hash as `["a","bc"]`). The
    /// same const the tripwire pins; recomputed here so a failure can print the value to re-pin.
    fn hash_wire_surface() -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for name in WIRE_SURFACE {
            for byte in name.bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= u64::from(b'\n');
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// THE TRIPWIRE. The wire surface must equal its pinned snapshot. When this fails, the replicated
    /// component/message/channel/input set changed — bump [`PROTOCOL_REV`] and re-pin
    /// [`WIRE_SURFACE_HASH`] to the value this prints, in the SAME diff. That is what makes a silent
    /// wire-breaking change impossible: mismatched builds already refuse at the netcode handshake
    /// (both ends' `protocol_id` = [`PROTOCOL_FINGERPRINT`]), and bumping `PROTOCOL_REV` is what
    /// actually moves that fingerprint so the refusal fires.
    #[test]
    fn wire_surface_is_pinned() {
        let actual = hash_wire_surface();
        assert_eq!(
            actual, WIRE_SURFACE_HASH,
            "wire surface changed: bump PROTOCOL_REV and re-pin WIRE_SURFACE_HASH to {actual:#018x}",
        );
    }

    // --- Source-scan tripwires --------------------------------------------------------------------
    //
    // These read this crate's own source (via `CARGO_MANIFEST_DIR`) and grep it, the same SOURCE-SCAN
    // pattern `tests/net_boundary.rs` uses to enforce an architectural contract. They are honest-but-
    // simple line/substring scanners, NOT a Rust parser — resilient to reformatting, and biased toward
    // FALSE TRIPS (which merely force a harmless re-pin) over false passes (which would let a wire skew
    // through). Comments/whitespace are stripped first so prose and layout edits never trip them.

    /// Read a repo-relative source file (or `Cargo.lock`) for a scan.
    fn read_source(rel: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {rel}: {e}"))
    }

    /// Blank `//` line- and `/* */` (nesting) block-comments to spaces, preserving newlines so line
    /// indices survive. Same intent as `net_boundary`'s `strip_comments`: only CODE is scanned, so a
    /// wire type named in prose can't trip a tripwire and a doc edit can't move a hash.
    fn strip_comments(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut rest = src;
        let mut block_depth = 0usize;
        while let Some(c) = rest.chars().next() {
            if block_depth > 0 {
                if rest.starts_with("/*") {
                    block_depth += 1;
                    out.push_str("  ");
                    rest = &rest[2..];
                } else if rest.starts_with("*/") {
                    block_depth -= 1;
                    out.push_str("  ");
                    rest = &rest[2..];
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                    rest = &rest[c.len_utf8()..];
                }
            } else if rest.starts_with("/*") {
                block_depth = 1;
                out.push_str("  ");
                rest = &rest[2..];
            } else if rest.starts_with("//") {
                // Blank to end of line (keep the newline).
                let end = rest.find('\n').unwrap_or(rest.len());
                for _ in 0..end {
                    out.push(' ');
                }
                rest = &rest[end..];
            } else {
                out.push(c);
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }

    /// The `plugin` fn body (between its outermost braces), comment-stripped. Brace-matched rather than
    /// line-delimited so the closure/config braces inside it (e.g. `ChannelSettings { .. }`, the
    /// `LightyearAvianPlugin { .. }` block) don't fool the scan.
    fn plugin_body(stripped: &str) -> &str {
        let sig = stripped
            .find("fn plugin(app: &mut App) {")
            .expect("plugin() signature present");
        let open = sig + stripped[sig..].find('{').expect("plugin() opening brace");
        let bytes = stripped.as_bytes();
        let mut depth = 0i32;
        for (i, &b) in bytes.iter().enumerate().skip(open) {
            match b {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return &stripped[open + 1..i];
                    }
                }
                _ => {}
            }
        }
        panic!("unbalanced braces in plugin()");
    }

    /// The registration forms that put a type on the wire, each identified by the exact call `plugin`
    /// makes. NOT `local_rollback::<T>` / `add_observer(..::<T>)` — those register ROOT-LOCAL rollback
    /// state that never crosses the wire, so they are deliberately absent.
    const REG_MARKERS: &[&str] = &[
        ".component::<",        // `app.component::<T>().replicate()` (+ `.predict()` chains)
        ".add_channel::<",      // `app.add_channel::<T>(..)`
        ".register_message::<", // `app.register_message::<T>()`
        "InputPlugin::<",       // `InputPlugin::<T>::default()` — the input protocol
    ];

    /// Derive the ordered wire-type list straight from `plugin`'s registration calls, left-to-right,
    /// top-to-bottom. Each marker is immediately followed by `<Type>`; the last `::`-segment is the
    /// bare type name `WIRE_SURFACE` lists.
    fn registered_types(body: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in body.lines() {
            let mut rest = line;
            while let Some((idx, marker)) = REG_MARKERS
                .iter()
                .filter_map(|m| rest.find(m).map(|i| (i, *m)))
                .min_by_key(|(i, _)| *i)
            {
                let after = &rest[idx + marker.len()..];
                let Some(end) = after.find('>') else { break };
                let name = after[..end]
                    .trim()
                    .rsplit("::")
                    .next()
                    .expect("split yields at least one segment")
                    .trim()
                    .to_string();
                out.push(name);
                rest = &after[end + 1..];
            }
        }
        out
    }

    /// FINDING 1 TRIPWIRE. The types `plugin` actually registers must equal [`WIRE_SURFACE`], in order.
    /// This is what stops the hand list from silently diverging from the code: add / remove / reorder a
    /// registration in `plugin` (e.g. `app.component::<NetSmoke>().replicate()`) WITHOUT editing
    /// `WIRE_SURFACE` and this fails — which is the whole point, since forgetting the `PROTOCOL_REV`
    /// bump means forgetting the adjacent list too. Fixing it (updating `WIRE_SURFACE`) then trips
    /// `wire_surface_is_pinned`, which forces the `PROTOCOL_REV` bump + re-pin. One edit, two gates, no
    /// silent wire skew.
    #[test]
    fn plugin_registrations_match_wire_surface() {
        let src = read_source("src/net/protocol.rs");
        let derived = registered_types(plugin_body(&strip_comments(&src)));
        let derived: Vec<&str> = derived.iter().map(String::as_str).collect();
        assert_eq!(
            derived.as_slice(),
            WIRE_SURFACE,
            "plugin()'s registration block no longer matches WIRE_SURFACE. A replicated component, \
             channel, message, or input was added / removed / reordered in plugin() without updating \
             the hand-maintained WIRE_SURFACE list beside it. Update WIRE_SURFACE to match plugin() \
             (which then fails wire_surface_is_pinned), then bump PROTOCOL_REV and re-pin \
             WIRE_SURFACE_HASH — all in the same diff.",
        );
    }

    /// The own wire-facing types whose DEFINITION TEXT rides [`WIRE_TYPES_HASH`], each as
    /// `(source file, type name)`. This is the `WIRE_SURFACE` own-type graph followed through its
    /// embeds (see the coverage-model block by the const): the five snapshot/marker components and the
    /// fire message from this file, `TankCommand`/`CrewSwap` from `command.rs`, and the `CrewStation`
    /// both crew types embed from `damage.rs`. External wire types (avian/lightyear) are covered by dep
    /// version instead — they have no source here to scan.
    const WIRE_TYPE_DEFS: &[(&str, &str)] = &[
        ("src/net/protocol.rs", "NetTank"),
        ("src/net/protocol.rs", "NetBot"),
        ("src/net/protocol.rs", "ServoAngles"),
        ("src/net/protocol.rs", "NetCrew"),
        ("src/net/protocol.rs", "VolumeSnapshot"),
        ("src/net/protocol.rs", "CrewSnapshot"),
        ("src/net/protocol.rs", "LaunchedTurretPose"),
        ("src/net/protocol.rs", "FireChannel"),
        ("src/net/protocol.rs", "FireEvent"),
        ("src/command.rs", "TankCommand"),
        ("src/command.rs", "CrewSwap"),
        ("src/damage.rs", "CrewStation"),
    ];

    /// Whether `line` is the `struct NAME`/`enum NAME` DEFINITION line — the keyword immediately
    /// followed by the exact name and a non-identifier boundary, so `VolumeSnapshot` never matches
    /// `VolumeSnapshotSource`.
    fn is_def_line(line: &str, name: &str) -> bool {
        ["struct ", "enum "].iter().any(|kw| {
            let needle = format!("{kw}{name}");
            line.find(&needle).is_some_and(|idx| {
                line[idx + needle.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_')
            })
        })
    }

    /// Extract a type's DEFINITION from comment-stripped source `stripped`: its preceding attribute
    /// lines (the `#[derive(..)]` that governs its serde), the `struct`/`enum` header, and the body up
    /// to the closing `}` (or the `;` of a unit struct). Whitespace is then removed entirely, so only a
    /// real token change (a field, a variant, a type, a derive) moves the result — a reformat or a doc
    /// edit does not. Walking back over the contiguous non-blank attribute lines stops at the blank the
    /// stripped doc-comment leaves above every wire type (house style: every wire type is documented),
    /// so it is conservative — an over-grab only forces a harmless re-pin, never a missed field.
    fn normalized_type_def(stripped: &str, name: &str) -> String {
        let lines: Vec<&str> = stripped.lines().collect();
        let kw = lines
            .iter()
            .position(|l| is_def_line(l, name))
            .unwrap_or_else(|| panic!("definition of `{name}` not found"));
        // Walk back over contiguous non-blank lines (the attribute block), stop at the blank the
        // doc-comment strips to.
        let mut start = kw;
        while start > 0 && !lines[start - 1].trim().is_empty() {
            start -= 1;
        }
        // Forward from the header: a unit struct ends at the first `;`, else brace-match the body.
        let region: String = lines[kw..].join("\n");
        let semi = region.find(';');
        let brace = region.find('{');
        let end = match (brace, semi) {
            (Some(b), Some(s)) if s < b => s + 1,
            (None, Some(s)) => s + 1,
            (Some(b), _) => {
                let bytes = region.as_bytes();
                let mut depth = 0i32;
                let mut close = None;
                for (i, &by) in bytes.iter().enumerate().skip(b) {
                    match by {
                        b'{' => depth += 1,
                        b'}' => {
                            depth -= 1;
                            if depth == 0 {
                                close = Some(i + 1);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                close.unwrap_or_else(|| panic!("unbalanced braces in `{name}` definition"))
            }
            _ => panic!("no terminator (`;` or `{{`) for `{name}` definition"),
        };
        let attrs = lines[start..kw].join("\n");
        let def = format!("{attrs}\n{}", &region[..end]);
        def.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// FNV-1a over every own wire type's normalized definition, each terminated by `\n` (so no two
    /// distinct definition sets collide by concatenation) — the same fold shape as `hash_wire_surface`.
    fn hash_wire_types() -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for (file, name) in WIRE_TYPE_DEFS {
            let stripped = strip_comments(&read_source(file));
            for byte in normalized_type_def(&stripped, name).bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= u64::from(b'\n');
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    /// The resolved version of a `Cargo.lock` package. Scans the `[[package]]` blocks for the one whose
    /// `name = "…"` matches, then reads its `version = "…"`.
    fn locked_version(lock: &str, crate_name: &str) -> String {
        let needle = format!("name = \"{crate_name}\"");
        let block = lock
            .split("[[package]]")
            .find(|b| b.lines().any(|l| l.trim() == needle))
            .unwrap_or_else(|| panic!("`{crate_name}` not found in Cargo.lock"));
        block
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("version = \"")
                    .and_then(|v| v.strip_suffix('"'))
            })
            .unwrap_or_else(|| panic!("no version for `{crate_name}` in Cargo.lock"))
            .to_string()
    }

    /// FINDING 2 TRIPWIRE. A field-level serde change to an own wire type (a field added to
    /// `VolumeSnapshot`/`CrewSnapshot`/`NetCrew`, etc.) renames no registered type, so `WIRE_SURFACE`
    /// and its hash stay green — yet skewed builds would then CONNECT (same fingerprint) and
    /// misdeserialize. This pins the definition TEXT of every own wire type, and the resolved versions
    /// of the external wire deps (avian/lightyear), so either kind of change trips here and forces the
    /// `PROTOCOL_REV` bump. See the coverage-model block by the consts.
    #[test]
    fn wire_types_are_pinned() {
        let actual = hash_wire_types();
        assert_eq!(
            actual, WIRE_TYPES_HASH,
            "a wire-facing type DEFINITION changed (a field / variant / type / derive on one of the \
             own wire types) without a PROTOCOL_REV bump: skewed builds would connect and \
             misdeserialize. Bump PROTOCOL_REV and re-pin WIRE_TYPES_HASH to {actual:#018x} in the \
             same diff.",
        );

        let lock = read_source("Cargo.lock");
        for (crate_name, pinned) in [
            ("avian3d", WIRE_DEP_AVIAN3D),
            ("lightyear", WIRE_DEP_LIGHTYEAR),
        ] {
            let locked = locked_version(&lock, crate_name);
            assert_eq!(
                &locked, pinned,
                "external wire dep `{crate_name}` moved {pinned} -> {locked}: its types (or wire \
                 framing) can serialize differently, an invisible skew. Bump PROTOCOL_REV and update \
                 the pinned WIRE_DEP_* version in the same diff.",
            );
        }
    }

    /// The fingerprint is a pure function of the build (so a same-build client/server always agree
    /// and connect) and actually MOVES with each of its inputs (so a rev or version skew is refused).
    #[test]
    fn fingerprint_is_deterministic_and_sensitive() {
        assert_eq!(
            PROTOCOL_FINGERPRINT,
            protocol_fingerprint(),
            "the fingerprint must be a pure function of the build",
        );
        // A different PROTOCOL_REV changes the fingerprint.
        let rev1 = fnv1a_64(0xcbf2_9ce4_8422_2325, &1u32.to_le_bytes());
        let rev2 = fnv1a_64(0xcbf2_9ce4_8422_2325, &2u32.to_le_bytes());
        assert_ne!(rev1, rev2, "PROTOCOL_REV must change the fingerprint");
        // The crate version is folded in on top: a different version string changes the fingerprint.
        assert_ne!(
            fnv1a_64(rev1, b"0.3.0-alpha.4"),
            fnv1a_64(rev1, b"0.3.0-alpha.5"),
            "the crate version must change the fingerprint",
        );
    }

    /// A `LocalTimeline` (a `#[derive(Resource)]`) pinned to `tick`. Its field is private, so seed
    /// it from `Default` (tick 0) and step it with the public `apply_delta`.
    fn timeline_at(tick: i32) -> LocalTimeline {
        let mut tl = LocalTimeline::default();
        tl.apply_delta(tick);
        tl
    }

    fn fire_click() -> TankCommand {
        TankCommand {
            fire_primary: true,
            ..default()
        }
    }

    /// STARVED tick: the last real input is old, the current tick is past the buffer end, and the
    /// `ActionState` holds that last input (exactly what the server's `get_predict` produces). The
    /// bridge must NOT re-latch the stale fire edge — not once, and not on any subsequent tick.
    #[test]
    fn starved_tick_does_not_refire_held_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(10));

        // A single real input at tick 5 carried a click; nothing has arrived since — tick 10 is
        // starved. `buffer.get(10)` is therefore `None` (past the buffered range).
        let mut buffer = NativeBuffer::<TankCommand>::default();
        buffer.set(Tick(5), ActionState(fire_click()));
        // The held-last `ActionState` the server's `update_action_state` leaves behind on a starved
        // tick (`get_predict(10) == get_last() ==` the tick-5 click).
        let entity = world
            .spawn((ActionState(fire_click()), buffer, TankCommand::default()))
            .id();

        // Three starved ticks in a row, each clearing the command first (as `consume_edges` would):
        // the bridge is the only thing that could re-latch the edge, and it must never do so.
        for _ in 0..3 {
            world.get_mut::<TankCommand>(entity).unwrap().fire_primary = false;
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();
            assert!(
                !world.get::<TankCommand>(entity).unwrap().fire_primary,
                "a held-last fire edge must not bridge on a starved tick",
            );
        }
    }

    /// A starved tick still carries LEVELS and ABSOLUTES through (hold-last is correct for those),
    /// while both EDGES are cleared.
    #[test]
    fn starved_tick_keeps_levels_clears_edges() {
        let mut world = World::new();
        world.insert_resource(timeline_at(10));

        let held = TankCommand {
            throttle: 0.7,
            steer: -0.3,
            fire_secondary: true,
            aim: Some(Vec3::new(1.0, 2.0, 3.0)),
            range: 850.0,
            fire_primary: true,
            crew_swap: Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Loader)),
            respawn: true,
        };
        let mut buffer = NativeBuffer::<TankCommand>::default();
        buffer.set(Tick(5), ActionState(held));
        let entity = world
            .spawn((ActionState(held), buffer, TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        let cmd = *world.get::<TankCommand>(entity).unwrap();
        assert_eq!(cmd.throttle, 0.7, "throttle level held through starvation");
        assert_eq!(cmd.steer, -0.3, "steer level held through starvation");
        assert!(
            cmd.fire_secondary,
            "secondary level held through starvation"
        );
        assert_eq!(cmd.aim, Some(Vec3::new(1.0, 2.0, 3.0)), "aim absolute held");
        assert_eq!(cmd.range, 850.0, "range absolute held");
        assert!(!cmd.fire_primary, "fire edge cleared on a starved tick");
        assert_eq!(
            cmd.crew_swap, None,
            "crew-swap edge cleared on a starved tick"
        );
        assert!(!cmd.respawn, "respawn edge cleared on a starved tick");
    }

    /// A tick with a REAL buffered input (the non-starved case, and every rollback-replayed tick of
    /// the client's own tank — its buffer retains `max_rollback_ticks + 1` of history) bridges the
    /// whole command, edges included: the fire edge must re-fire.
    #[test]
    fn real_buffered_tick_fires_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(8));

        // A real input for tick 8 (the replayed tick) carrying the click.
        let mut buffer = NativeBuffer::<TankCommand>::default();
        buffer.set(Tick(8), ActionState(fire_click()));
        let entity = world
            .spawn((ActionState(fire_click()), buffer, TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_primary,
            "a real buffered edge at this tick must bridge through (own-tank rollback re-fire)",
        );
    }

    /// A MISSING `InputBuffer` is NOT a hold-last extrapolation — nothing is being held. In the
    /// pre-sync join/spawn window the client's own tank carries `ActionState` (authored THIS tick by
    /// `feed_action_state`) before its `InputTimeline` has produced a buffer, so a genuine click must
    /// pass. (The coarser `get(tick).is_some()` rule wrongly dropped it — the bug FIX 1 targets.)
    #[test]
    fn missing_buffer_passes_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(3));
        let entity = world
            .spawn((ActionState(fire_click()), TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_primary,
            "a missing InputBuffer is not hold-last extrapolation — a real edge must pass",
        );
    }

    /// A present-but-EMPTY buffer is likewise not extrapolating: `get_last()` is `None` (no entry to
    /// hold), so `held_last` is false and the edge passes. Distinguishes "no data yet" (pass) from
    /// "data, but none for this tick" (the starved case above, which clears).
    #[test]
    fn empty_buffer_passes_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(3));
        // An `InputBuffer` component present but with no entries — `get_last()` is `None`.
        let buffer = NativeBuffer::<TankCommand>::default();
        let entity = world
            .spawn((ActionState(fire_click()), buffer, TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_primary,
            "an empty InputBuffer has nothing to hold-last — a real edge must pass",
        );
    }

    /// The crew-swap false-death tripwire (the corruption this slice ends). Builds the exact
    /// post-corruption local state the OLD client path produced on a `Remote` tank — swap A↔B, seat
    /// A (Gunner) LEFT holding the dead occupant and seat B (Loader) LEFT holding the live one after
    /// the client-side flip, `apply_net_health` then re-asserting HP `[full, 0]` by index and
    /// `kill_crew` LATCHING `Dead` onto the seat now holding the live man, `mark_dead_tanks` then
    /// falsely latching `TankKnockedOut` — while the AUTHORITY's snapshot still says seat A is the
    /// live Gunner and seat B the dead Loader.
    ///
    /// `apply_net_crew` (the fix) must HEAL it: it re-derives HP, occupancy (`home`), and `Dead` from
    /// the authoritative snapshot every tick and derives the knockout label from scratch, so the live
    /// crewman ends alive and the tank is not knocked out. The old `apply_net_health` (HP only, no
    /// occupancy/aliveness re-derivation) plus the monotonic `kill_crew`/`mark_dead_tanks` latches
    /// leave the corruption in place — this test fails against that ordering and passes with the fix.
    #[test]
    fn crew_swap_does_not_false_kill_on_replica() {
        use crate::ballistics::ComponentHealth;
        use crate::damage::{Crewman, Dead, KnockoutReason, TankKnockedOut, VolumeOf};

        const FULL: f32 = 100.0;

        let mut world = World::new();

        // The `Remote` tank root carrying the AUTHORITATIVE (server) snapshot, still pre-swap: seat A
        // is the live Gunner (full HP), seat B the dead Loader (0 HP). `swap == None` (the server has
        // not applied any flip). `TankVolumes` is populated by the `VolumeOf` relationship below.
        let root = world
            .spawn((
                Remote,
                NetCrew {
                    volumes: vec![
                        VolumeSnapshot {
                            hp: FULL,
                            crew: Some(CrewSnapshot {
                                home: CrewStation::Gunner,
                                dead: false,
                            }),
                        },
                        VolumeSnapshot {
                            hp: 0.0,
                            crew: Some(CrewSnapshot {
                                home: CrewStation::Loader,
                                dead: true,
                            }),
                        },
                    ],
                    swap: None,
                },
                // The false knockout the old `mark_dead_tanks` latched — the fix must remove it.
                TankKnockedOut {
                    reason: KnockoutReason::CrewLoss,
                },
            ))
            .id();

        // Seat A in `TankVolumes` order (spawned first). Its LOCAL state is the corruption: the
        // client-side flip moved the dead Loader-occupant here (`home = Loader`, `Dead`) and
        // `apply_net_health` wrote its index-0 HP (full) back on — the exact mismatched leftover.
        let seat_a = world
            .spawn((
                CrewStation::Gunner,
                Crewman {
                    home: CrewStation::Loader,
                },
                ComponentHealth {
                    current: FULL,
                    max: FULL,
                },
                Dead,
                VolumeOf(root),
            ))
            .id();

        // Seat B (spawned second → index 1). The flip moved the LIVE Gunner-occupant here, then
        // `apply_net_health` wrote index-1 HP (0) onto it and `kill_crew` LATCHED `Dead` — the live
        // man wrongly killed. `home = Gunner` is the live occupant the flip stranded here.
        let seat_b = world
            .spawn((
                CrewStation::Loader,
                Crewman {
                    home: CrewStation::Gunner,
                },
                ComponentHealth {
                    current: 0.0,
                    max: FULL,
                },
                Dead,
                VolumeOf(root),
            ))
            .id();

        world.run_system_once(apply_net_crew).unwrap();

        // Seat A is the live Gunner again: HP restored, occupant home restored, `Dead` cleared.
        assert_eq!(
            world.get::<ComponentHealth>(seat_a).unwrap().current,
            FULL,
            "seat A keeps the authoritative full HP",
        );
        assert_eq!(
            world.get::<Crewman>(seat_a).unwrap().home,
            CrewStation::Gunner,
            "seat A's occupant home is re-derived from the snapshot (not the flipped Loader)",
        );
        assert!(
            world.get::<Dead>(seat_a).is_none(),
            "the LIVE crewman must not end up dead — the false-death corruption is healed",
        );

        // Seat B stays the authoritative dead Loader (0 HP, dead) — the fix does not resurrect it.
        assert_eq!(
            world.get::<Crewman>(seat_b).unwrap().home,
            CrewStation::Loader,
        );
        assert!(world.get::<Dead>(seat_b).is_some(), "seat B stays dead");

        // With one crew seat living, the tank is NOT knocked out — the false `TankKnockedOut` that
        // would have driven the death screen is removed by the idempotent derivation.
        assert!(
            world.get::<TankKnockedOut>(root).is_none(),
            "the tank must not be knocked out with a living crewman — no false YOU DIED",
        );
    }
}
