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
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};
// Row fields for the shot-lifecycle recorder (`crate::shot_trace`); evaluated only when it is armed.
use serde_json::json;

use crate::ShotId;
use crate::ballistics::{ComponentHealth, Projectile, Shot, ShotSource};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{
    Ammo, CrewStation, Crewman, DamageConsequences, Dead, LaunchedTurret, PendingSwap,
    TankKnockedOut, TankVolumes, knockout_from_counts,
};
use crate::driving::DriveState;
use crate::spec::FireMode;
use crate::state::GameplaySet;
use crate::tank::{
    Muzzle, Rig, ServoCommand, ServoIndex, ServoSpec, TankRoot, TankSim, Weapon, WeaponIndex,
};

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
pub const PROTOCOL_REV: u32 = 5;

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
/// without replicating the sim's own `Tank` marker. Deliberately NOT `Tank`: the sim marker stays
/// local and arrives only with the complete local body, including Tank's required command/drive
/// state.
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
/// ends derive, since both build the rig from one RON spec with sorted-by-name volume
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

/// One belt-fed (`Automatic`) weapon's authoritative fire-supply facts within [`NetBelts`] — the
/// two CORRELATED values a client cannot predict on a lossy input stream and so must be TOLD:
///
/// * `belt` — rounds left on the current belt (`WeaponState::belt_remaining`). This GATES fire
///   (`shooting::fire`: an `Automatic` fires only while `belt > 0`), so the client's own predicted
///   belt drifting below the server's is the exact bug this component fixes: under deep input loss
///   the client predicts a shot the server never fires (`bridge_action_state_to_tank_command`'s
///   documented loss trade), its `belt_remaining` (root-resident in `TankSim`, formerly NOT
///   replicated) drops one under the server's, and — because `TankSim` is `local_rollback` with NO
///   confirmed value to roll back TO — nothing ever corrected it until the next belt swap reset both
///   ends to `belt_size`. That window is a phantom client MG round (no damage — damage is
///   server-authoritative), a lying HUD count, and a per-tick `hrld` divergence flag (belt is folded
///   into `trace::hash_tank_state`).
///
/// * `swap_remaining` — the belt-SWAP countdown, i.e. `WeaponState::reload_remaining` WHILE the belt
///   is dry (`belt == 0`); `0.0` while `belt > 0` (not swapping). It must ride HERE, atomically with
///   `belt`, because pinning `belt` alone leaves a boundary hazard: the moment the server's belt hits
///   0 the client would see `belt == 0` but hold its own near-zero cyclic `reload_remaining`, so its
///   local `tick_reload` would INSTANTLY complete the swap and refill to `belt_size`, which the next
///   `apply_net_belts` overwrites back to 0 — an every-tick refill/overwrite OSCILLATION. Carrying
///   the swap countdown lets the client pin `reload_remaining` to the server's while `belt == 0`, so
///   the refill is server-driven (it arrives as `belt == belt_size`), never raced locally. This is
///   the SAME "replicate the correlated facts atomically" shape as [`NetCrew`]'s `swap` countdown.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct BeltSnapshot {
    /// Rounds left on the belt (`WeaponState::belt_remaining`); `0` = a swap is in flight.
    pub belt: u32,
    /// The belt-swap countdown (`reload_remaining` while `belt == 0`), else `0.0`. See the type doc.
    pub swap_remaining: f32,
}

/// The authoritative per-weapon fire-supply snapshot of a tank, published on the root by the
/// authority and replicated so the client's belt-fed weapons gate fire (and count the HUD) from
/// server truth instead of a divergent local prediction. The net half of the belt-replication fix
/// (Option B, owner 2026-07-12): the server's belt overwrites the client's prediction.
///
/// **One entry per weapon slot, in `TankSim::weapons` order** (the SAME order both ends derive —
/// sorted-by-name weapon spawn assigns `WeaponIndex`, exactly like [`NetCrew`]'s volume order), so
/// index `i` addresses the same weapon on both ends. `None` = a non-belt-fed (`Single`) weapon,
/// which carries no belt and whose `reload_remaining` this fix deliberately does NOT touch (the 88's
/// reload divergence is inherent-and-self-reconciling — the next shot fixes it — per
/// `bridge_action_state_to_tank_command`'s loss-trade note; only the belt's window is long enough to
/// warrant replication). A length mismatch at apply time (rig still spawning) skips the tank.
///
/// Plain replication (no prediction/interpolation), same idiom as [`ServoAngles`]/[`NetCrew`];
/// `set_if_neq` on publish so a resting tank stops churning change-detection. While an MG fires the
/// `belt` changes ~12.5/s and the snapshot resends then (deliberate — lightyear's change-detection
/// handles it, no custom throttle); `swap_remaining` is `0.0` throughout normal fire (so it does not
/// add churn) and only ticks every tick during the rare multi-second belt swap, exactly like
/// [`NetCrew`]'s swap countdown. At rest (belt full, not firing) the snapshot is stable and
/// `set_if_neq` suppresses idle churn.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetBelts {
    /// Every weapon in `TankSim::weapons` order: `Some(BeltSnapshot)` for a belt-fed (`Automatic`)
    /// weapon, `None` for a `Single` weapon (untouched by the belt fix).
    pub weapons: Vec<Option<BeltSnapshot>>,
}

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

impl FireEvent {
    /// This shot's [`ShotId`] — the correlation spine, DERIVED from the fields already on the wire
    /// (`shooter`, `weapon`, `fire_tick`) rather than carried as a separate field. Chosen so the wire
    /// stays minimal (zero extra bytes) and the id can never disagree with the geometry it identifies:
    /// it is a pure function of what's already here. `fire_tick` (a lightyear `Tick`) is unwrapped to
    /// its raw `u32` for the net-neutral [`ShotId`] (see its doc). Both ends call this to key dedup and
    /// to stamp the spawned shell, so a redundantly-retransmitted `FireEvent` (piece 3) resolves to the
    /// SAME id and is deduped, and a `RicochetKeyframe` (piece 2) correlates to the shell it belongs to.
    pub fn shot_id(&self) -> ShotId {
        ShotId {
            shooter: self.shooter,
            weapon: self.weapon,
            fire_tick: self.fire_tick.0,
        }
    }
}

impl MapEntities for FireEvent {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        // Only `shooter` is an entity; every other field is plain geometry or data (the `weapon`
        // slot is read against the receiver's own local rig, so it is NOT mapped).
        self.shooter = mapper.get_mapped(self.shooter);
    }
}

/// A server-sanctioned ricochet — the "replicate the cause" (ADR-0016) half of the bounce
/// carry-through. Broadcast the instant the AUTHORITATIVE march resolves a ricochet, so every observer
/// re-seeds its cosmetic shell from server truth instead of improvising a bounce against interpolated
/// geometry (the divergence that made replica shells wander off after a bounce). Loss-tolerant BY
/// CONSTRUCTION, exactly like [`FireEvent`]: a dropped keyframe costs a truncated cosmetic trail, never
/// a wrong hit (damage is server-authoritative), which is why it rides the same loss-tolerant
/// [`FireChannel`] — inside [`FireBurst`], which gives it sliding-window redundancy (piece 3).
///
/// Keyed by [`ShotId`] so it correlates to the right cosmetic shell on every client — an observer's
/// replica AND the shooter's own predicted round (which carries the same id via the shared
/// [`stamp_shot_ids`]; the shooter's fall-of-shot read on a bounced round is the loop this feeds).
/// `sequence` is the bounce's 0-based ordinal within the shot, so multiple ricochets on one shell are
/// consumed strictly in order. The receiver re-ages the re-seeded shell by how long it held (which
/// equals present − `bounce_tick`), so `bounce_tick` itself is carried for audit and a future
/// RTT-adaptive path rather than the hot path (see `ballistics::SanctionedBounce`). `direction` rides
/// as a raw `Vec3` and is guarded to a `Dir3` on receipt, the same discipline as
/// [`FireEvent::direction`].
#[derive(Clone, Serialize, Deserialize)]
pub struct RicochetKeyframe {
    /// The shot this bounce belongs to — its `shooter` is entity-mapped (see [`FireBurst`]).
    pub shot: ShotId,
    /// The exact server bounce point — where the observer re-seeds the shell.
    pub origin: Vec3,
    /// Post-bounce travel direction (guarded to `Dir3` on receipt).
    pub direction: Vec3,
    /// Post-bounce speed (m/s).
    pub speed: f32,
    /// The server `Tick` the bounce resolved on (stamped from the server's `LocalTimeline`).
    pub bounce_tick: Tick,
    /// This bounce's 0-based ordinal within the shot — the SAME count an observer derives from its
    /// shell's own ricochets, so bounces re-seed in the order the server resolved them.
    pub sequence: u32,
}

/// The server-sanctioned TERMINAL of a shot on armor — the confirm that completes the shot state
/// machine on every client: a shot ends in exactly one of {terrain stop (client-local —
/// pose-independent static geometry, both ends already agree, so terrain never confirms), THIS
/// confirmed armor terminal, fail-closed truncation (the lost-confirm fallback)}. Broadcast where the
/// authoritative march resolves an EMBED or a PERFORATION (a ricochet is a [`RicochetKeyframe`]
/// instead); mirrors the authority's own `Impact` read at the struck plate — position, normal, and the
/// `penetrated` verdict that gates the flame lick — so a net client renders the SAME honest armor read
/// SP shows, at the SERVER's position. Perforation ends the COSMETIC shell at the entry-face read even
/// though the authoritative shell continues into the tank interior — the documented choice on
/// `ballistics::ShellTerminal` (what an external viewer sees at the plate IS this read; the client
/// cannot march the interior). No surface enum rides: a confirm is ALWAYS armor (terrain is local).
///
/// Loss-tolerant like its siblings (the [`FireBurst`] redundancy re-carries it); a lost confirm
/// degrades to the fail-closed neutral truncation after the grace window. AT MOST ONE per shot (the
/// authority strips the shell's shot identity after emitting), so the receiver dedups by [`ShotId`]
/// alone — the `SanctionedShots` terminal insert is first-wins-idempotent. `after_bounces` orders it
/// against the shot's ricochet keyframes: the client consumes the terminal only after re-seeding
/// through that many bounces, so a shot's terminal never skips a bounce whose keyframe is merely late.
#[derive(Clone, Serialize, Deserialize)]
pub struct ImpactConfirm {
    /// The shot this terminal belongs to — its `shooter` is entity-mapped (see [`FireBurst`]).
    pub shot: ShotId,
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal, from the server's raycast. No `Dir3` guard on receipt:
    /// `Impact` consumers normalize with a fallback by contract (see `ballistics::Impact::normal`).
    pub normal: Vec3,
    /// The server's penetration verdict — gates the flame lick exactly as the local read does.
    pub penetrated: bool,
    /// The server `Tick` the impact resolved on. Audit / future RTT-adaptive use, like
    /// [`RicochetKeyframe::bounce_tick`] — the client resolves on receipt, not by re-aging.
    pub impact_tick: Tick,
    /// Ricochets the authority resolved before this terminal (the client's ordering gate).
    pub after_bounces: u32,
}

/// The sliding-window redundancy envelope (piece 3 — the input-redundancy pattern applied to cosmetic
/// events): every broadcast carries the last few [`FireEvent`]s, [`RicochetKeyframe`]s, AND
/// [`ImpactConfirm`]s, so ONE delivered burst repairs a multi-packet loss of any stream. Rides the
/// sequenced-unreliable [`FireChannel`] — a newer burst supersedes an older one without acks or
/// head-of-line blocking, and each burst carries the whole current window, so dropping a stale burst
/// loses nothing the next one doesn't re-carry. The receiver DEDUPS: fires by [`ShotId`] (spawn each
/// cosmetic shell exactly once), keyframes by `(ShotId, sequence)`, confirms by [`ShotId`] alone (at
/// most one terminal per shot) — the `SanctionedShots` inserts are idempotent.
///
/// One message type, not three, so the wire surface is a single registration and all streams share the
/// redundancy for free — a fire burst re-carries recent keyframes/confirms and vice versa. Sent to
/// `NetworkTarget::All`; a client drops any FIRE naming a tank IT simulates (the `locally_fired`
/// guard — a fire echo would duplicate the shell), so the shooter discarding its own echo is a
/// receiver concern, not a targeting one — which is what makes the redundancy correct across shooters
/// in one shared burst. KEYFRAMES and CONFIRMS are deliberately NOT dropped for own shots: they spawn
/// nothing, and the shooter's own shell consumes them (see `receive_fire_events`).
#[derive(Clone, Serialize, Deserialize)]
pub struct FireBurst {
    /// The last N fire events (N sized to cover a multi-packet burst at an MG's cyclic rate).
    pub fires: Vec<FireEvent>,
    /// The last N ricochet keyframes (rare — they persist in the window far longer than fires).
    pub keyframes: Vec<RicochetKeyframe>,
    /// The last N impact confirms (one per armor-terminated shot).
    pub confirms: Vec<ImpactConfirm>,
}

impl MapEntities for FireBurst {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        // Every embedded shot's `shooter` entity must resolve to the receiver's local replica: map the
        // fires through their own impl, and each keyframe's/confirm's `ShotId.shooter` directly
        // (ShotId is a net-neutral crate-root type with no MapEntities of its own).
        for fire in &mut self.fires {
            fire.map_entities(mapper);
        }
        for keyframe in &mut self.keyframes {
            keyframe.shot.shooter = mapper.get_mapped(keyframe.shot.shooter);
        }
        for confirm in &mut self.confirms {
            confirm.shot.shooter = mapper.get_mapped(confirm.shot.shooter);
        }
    }
}

/// The dedicated channel [`FireBurst`] rides: SEQUENCED + unreliable. Sequenced (not merely unordered)
/// because each burst carries the whole current sliding window of recent fires and keyframes — a newer
/// burst strictly supersedes an older one, so delivering only the newest and dropping stale/reordered
/// bursts loses nothing (the newest re-carries the window), while still paying no acks/retries/HOL
/// blocking for cosmetic traffic (damage is server-authoritative). A zero-sized marker type — `Channel`
/// is blanket-implemented for any `Send + Sync + 'static` type (lightyear_transport channel/mod.rs), so
/// the type IS the channel; its settings are registered in [`plugin`].
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

/// Authority side: collect each tank's per-weapon belt supply — for every belt-fed (`Automatic`)
/// weapon, its `belt_remaining` plus the swap countdown while dry — into the replicated [`NetBelts`],
/// one entry per `TankSim::weapons` slot in slot order (`None` for a `Single` weapon). `FireMode`
/// lives on the muzzle's [`Weapon`] (not in `TankSim`), so this joins the muzzles to their root by
/// `TankRoot` and scatters into the slot-indexed vector by [`WeaponIndex`] — the same slot both ends
/// derive. `FixedPostUpdate` (after `shooting::tick_reload`/`fire` have stepped this tick),
/// `Without<Remote>` = authority-only in shared code (every client tank carries `Remote`, see
/// `publish_servo_angles`). The collect order is exactly the apply order in [`apply_net_belts`].
fn publish_net_belts(
    mut tanks: Query<(Entity, &TankSim, &mut NetBelts), Without<Remote>>,
    muzzles: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
) {
    for (root, sim, mut belts) in &mut tanks {
        // `None` by default — a slot with no belt (a `Single` weapon) or whose muzzle hasn't spawned.
        let mut snapshot: Vec<Option<BeltSnapshot>> = vec![None; sim.weapons.len()];
        for (weapon, slot, tank_root) in &muzzles {
            if tank_root.0 != root {
                continue;
            }
            // Bounds-guarded: a muzzle whose slot outruns this tank's `TankSim::weapons` (rig still
            // spawning) is skipped rather than indexed past the end.
            let Some(state) = sim.weapons.get(slot.0) else {
                continue;
            };
            if let FireMode::Automatic { .. } = weapon.fire_mode {
                snapshot[slot.0] = Some(BeltSnapshot {
                    belt: state.belt_remaining,
                    // The swap countdown ONLY while the belt is dry (`reload_remaining` is then the
                    // swap timer); `0.0` during cyclic fire so it adds no change-detection churn.
                    swap_remaining: if state.belt_remaining == 0 {
                        state.reload_remaining
                    } else {
                        0.0
                    },
                });
            }
        }
        // `set_if_neq`: no change-detection churn (nor replication resends) while at rest.
        belts.set_if_neq(NetBelts { weapons: snapshot });
    }
}

/// Client side: realize the replicated [`NetBelts`] onto each `Remote` tank's local `TankSim` — pin
/// every belt-fed weapon's `belt_remaining` to server truth, and (while the belt is dry) its
/// `reload_remaining` to the server's swap countdown. This is Option B (owner 2026-07-12): the
/// server's belt OVERWRITES the client's prediction. `belt_remaining` is root-resident in the
/// `local_rollback`-tracked `TankSim`, with NO replicated confirmed value to roll back to, so before
/// this a client's mispredicted belt (a phantom shot the server never fired) stayed wrong until the
/// next belt swap; here it snaps back every tick.
///
/// **Why pin `reload_remaining` only when `belt == 0`.** While the belt has rounds, `reload_remaining`
/// is the sub-0.1 s cyclic interval — cheap to let the client predict, self-correcting within one
/// cycle, and it never gate-diverges because the belt (which actually gates fire) is authoritative.
/// While the belt is DRY it is the multi-second swap timer, and pinning it is what stops the boundary
/// oscillation the [`BeltSnapshot::swap_remaining`] doc describes (a client would otherwise instantly
/// complete the swap locally and fight the overwrite every tick). A `None` entry (a `Single` weapon)
/// is left entirely untouched — its reload divergence is inherent and self-reconciling (see
/// [`NetBelts`]).
///
/// **This system is TICK-AGNOSTIC and runs during rollback replay too** — the same discipline as
/// [`apply_net_crew`], and for the same reason: [`NetBelts`] is plain-replicated (no `.predict()`,
/// hence no `ConfirmedHistory`), so it holds exactly one value, and a rollback replay of ticks
/// `T..present` pins that newest-confirmed value onto every replayed tick. That is deliberate — we
/// WANT the belt pinned to server truth on forward AND replayed ticks alike; gating it off during
/// rollback would let the replay re-derive the divergent local belt from prediction history and
/// re-open the very gap this closes. The residual is the same cross-entity replication lag
/// `apply_net_crew` documents: the pinned belt is the newest CONFIRMED value (a few ticks old), so
/// during a burst the client's belt trails the server's live belt by that lag — a bounded,
/// transient, self-healing delta (the divergence analyzer's belt field reads transient-then-zero),
/// not the old accumulate-until-swap divergence. A snap that flips fire-gating mid-burst is
/// acceptable by owner decision: a predicted MG round silently does not happen (it was only a
/// damage-free tracer — damage is server-authoritative).
///
/// Ordered `.after(ConsumeCommandEdges)` so it runs after `shooting::tick_reload`/`fire` (which order
/// `.before(ConsumeCommandEdges)`) each tick — the confirmed belt is the tick's last word, so the
/// end-of-tick state (and the `hrld` hash) reads exactly server truth. `With<Remote>` = replica-only
/// in shared code, exactly like [`apply_net_crew`]; a length mismatch (rig still spawning) skips.
fn apply_net_belts(mut tanks: Query<(&NetBelts, &mut TankSim), With<Remote>>) {
    for (belts, mut sim) in &mut tanks {
        // A length mismatch is expected transiently while the client's rig is still spawning and
        // self-heals once built; a persistent mismatch means client/server spec skew. Skip rather
        // than write misaligned (same discipline as `apply_net_crew`).
        if belts.weapons.len() != sim.weapons.len() {
            continue;
        }
        for (snap, state) in belts.weapons.iter().zip(sim.weapons.iter_mut()) {
            // `None` = a non-belt-fed weapon: leave its `belt_remaining`/`reload_remaining` alone.
            let Some(snap) = snap else {
                continue;
            };
            state.belt_remaining = snap.belt;
            if snap.belt == 0 {
                // Swap in flight on the authority: pin the countdown so the client neither completes
                // a phantom swap early nor oscillates refilling against the overwrite (see the doc).
                state.reload_remaining = snap.swap_remaining;
            }
        }
    }
}

/// BOTH ENDS: complete each freshly-spawned attributed shell's [`ShotId`] from its [`ShotSource`]
/// (tank + weapon slot, attached at spawn by `on_fire_shell`) plus the fire tick — the one part the
/// sim layer cannot know, since the tick lives in lightyear's timeline (`tests/net_boundary`). Runs
/// `FixedPostUpdate`, so the timeline still reads the FIRE tick (unchanged until the next
/// `FixedFirst`) — the SAME value the server's `broadcast_fire` stamps into the `FireEvent`.
///
/// This lives in the SHARED protocol plugin, not `net::server`, because the id must be stamped
/// identically on every net composition for the keyframe correlation to close end-to-end:
///   * **Server**: the authoritative shell carries the id so its ricochet raises a `ShellRicochet`
///     naming the shot (`net::server::on_shell_ricochet` puts it on the wire).
///   * **Shooter's client**: the OWN predicted shell fires at the SAME tick number the server
///     simulates that input on (prediction is tick-indexed: both ends run `shooting::fire` for tick T
///     with the input for tick T — a late input is dropped, never fired late, so if the server fires
///     at all it fires at T), against the SAME tank root the keyframe's entity-mapped `shooter`
///     resolves to (the mapped root is the root the local rig hangs off — the same entity
///     `ShotSource.tank` named at fire time, as `apply_pending_recoil_kicks` already relies on). So
///     the locally-stamped id equals the wire-derived one, and the shooter's own bounced round
///     re-seeds from the server keyframe — the fall-of-shot read the gunnery loop needs.
///   * **Observer clients**: their shells are stamped directly from the wire
///     (`receive_fire_events` passes `FireShell.shot`), so this system finds no `ShotSource` on them
///     (the client re-raise passes `shooter: None`) and leaves them alone.
///
/// SP/sandbox never mount this plugin, so their shells carry no `Shot` — irrelevant there: the
/// authority march consults `Shot` only to emit `ShellRicochet`, and there is no wire to carry one.
/// A shell reaches armor only on a later tick (it spawns at the muzzle), so `Shot` is always present
/// before it can ricochet or hold. `Without<Shot>` makes this idempotent (and skips wire-stamped
/// observer shells even if they ever gained a source); a slot past `u8` is skipped, matching
/// `broadcast_fire` (which also skips the whole shot — so no keyframe would name it anyway). On the
/// client this also runs during rollback replay, stamping a replay-refired shell with the replayed
/// tick — which IS its fire tick, so the duplicate carries the same id as the original (cosmetic
/// duplicate, same correlation).
fn stamp_shot_ids(
    shells: Query<(Entity, &ShotSource), (With<Projectile>, Without<Shot>)>,
    timeline: Res<LocalTimeline>,
    // Shot-lifecycle recorder (`SPIKE_SHOT_TRACE`), absent unless armed: this is where a LOCALLY-FIRED
    // shell's id first exists, so it is where that shell's `spawn` row belongs (the observer shells
    // `on_fire_shell` stamps from the wire write their own). `ClientReplica` tells the two roles apart
    // without naming a lightyear type: the shooter's own predicted shell ("own") vs the authoritative
    // one ("auth").
    replica: Option<Res<crate::ClientReplica>>,
    mut shot_trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut commands: Commands,
) {
    let fire_tick = timeline.tick().0;
    let src = if replica.is_some() { "own" } else { "auth" };
    for (entity, source) in &shells {
        let Ok(weapon) = u8::try_from(source.weapon) else {
            continue;
        };
        let shot = ShotId {
            shooter: source.tank,
            weapon,
            fire_tick,
        };
        crate::shot_trace::record(
            &mut shot_trace,
            "spawn",
            fire_tick,
            shot,
            || json!({ "src": src }),
        );
        commands.entity(entity).insert(Shot(shot));
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
    "NetBelts",
    // The cosmetic-fire channel then message — `app.add_channel` / `app.register_message`. The message
    // is now the redundancy envelope `FireBurst`; `FireEvent` rides INSIDE it (a wire type, not a
    // registered message), covered by the field-level `WIRE_TYPES_HASH` below.
    "FireChannel",
    "FireBurst",
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
const WIRE_SURFACE_HASH: u64 = 0xc977_9452_059a_6423;

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
const WIRE_TYPES_HASH: u64 = 0xfb57_e5aa_ccd8_3d53;

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
    // Server-authoritative per-weapon belt supply (same plain-replication shape): each belt-fed
    // weapon's `belt_remaining` + swap countdown, so the client's fire-gating belt (root-resident in
    // the un-replicated `TankSim`) snaps to server truth instead of a divergent local prediction.
    app.component::<NetBelts>().replicate();

    // The cosmetic opponent-fire broadcast (`FireBurst`: a sliding window of recent `FireEvent`s +
    // `RicochetKeyframe`s) and its dedicated loss-tolerant channel. A MESSAGE, not a replicated
    // component: fire-and-forget events, not ongoing state. `ServerToClient` (the server is the sole
    // broadcaster — see `net::server`); `add_map_entities` registers `FireBurst`'s `MapEntities` so
    // every embedded shot's `shooter` entity resolves to the receiver's local replica on deserialize.
    // Registered in this SHARED plugin so both ends agree on the message id, direction, and channel —
    // exactly like the `.replicate()` block above.
    app.add_channel::<FireChannel>(ChannelSettings {
        // Sequenced + unreliable: each burst carries the whole current redundancy window, so delivering
        // only the newest and dropping stale/reordered bursts loses nothing (the newest re-carries it),
        // while paying no acks/retries/HOL blocking on high-frequency cosmetic traffic (damage is
        // server-authoritative via `NetCrew`). See `FireChannel`/`FireBurst`.
        mode: ChannelMode::SequencedUnreliable,
        ..default()
    })
    // The CHANNEL's own direction — NOT just the message's. This installs the per-link
    // sender/receiver observers (`add_sender_channel`/`add_receiver_channel` in lightyear_transport)
    // that populate each new `Transport`'s channel senders from the registry; without it the channel
    // exists in the `ChannelRegistry` but no link ever gets a `FireChannel` sender, so every send
    // fails `ChannelNotFound` at runtime (compiles fine — the bug only shows live). Same idiom as
    // lightyear's own `InputChannel`/`RepliconUpdatesChannel` registrations.
    .add_direction(NetworkDirection::ServerToClient);
    app.register_message::<FireBurst>()
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
            publish_net_belts,
            // BOTH ends: complete each attributed shell's `ShotId` from `ShotSource` + this tick —
            // the shared stamp that makes the server's ricocheting shell, the shooter's OWN predicted
            // shell, and the observers' catch-up shells all carry ONE id (see the system doc).
            stamp_shot_ids,
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
    // Client: pin each belt-fed weapon's supply to server truth (Option B). `.after(ConsumeCommandEdges)`
    // so it runs after `shooting::tick_reload`/`fire` (which order `.before(ConsumeCommandEdges)`) —
    // the confirmed belt is the tick's last word. Runs during rollback replay too (tick-agnostic,
    // like `apply_net_crew`), so the belt stays pinned on every replayed tick; see the system doc.
    app.add_systems(
        FixedUpdate,
        apply_net_belts
            .in_set(GameplaySet)
            .after(ConsumeCommandEdges),
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
/// client module); remote (interpolated) tanks never carry one. `TankCommand` is a required
/// component of the local `Tank` sim marker.
///
/// # A CONSUMABLE commits only on an ATTESTED tick
///
/// The whole struct is copied — matching `ActionState`'s "absolute snapshot per tick" contract —
/// EXCEPT that the CONSUMABLES (`TankCommand::fail_consumables_closed`: the edges, plus the
/// automatic-fire level) are failed closed on any tick the command cannot attest it was authored
/// for. The test is a POSITIVE ATTESTATION, not a detector:
///
/// ```ignore
/// if next.for_tick != tick.0 { next.fail_consumables_closed(); }
/// ```
///
/// `TankCommand::for_tick` is stamped ONCE, on the client (`net::client`'s `stamp_input_tick`), with
/// the exact tick lightyear's `buffer_action_state` files the command under (`local_tick +
/// input_delay`), and then rides the input buffer, the wire, the server's buffer and rollback replay
/// unmodified. So `for_tick == tick` iff the player really authored THIS command FOR THIS tick.
///
/// **Why a positive attestation and not a detector.** A value handed back by lightyear's
/// `InputBuffer` for tick T is not necessarily an input the player gave for tick T. It can be:
///
/// 1. **Hold-last extrapolation.** Past the buffered range, the server's `update_action_state` calls
///    `InputBuffer::get_predict(tick)`, which returns `get_last()` (`lightyear_inputs`
///    input_buffer.rs:316 / server.rs:707) — "the player will keep playing the last action".
/// 2. **A `SameAsPrecedent` gap-fill.** `InputBuffer::set_raw` fills any tick the writer SKIPS with
///    `Compressed::SameAsPrecedent` (input_buffer.rs:212), a fabricated repeat of the last command
///    on a tick nobody authored. The client skips a tick exactly when its `input_delay` GROWS.
/// 3. **A stale entry the correction could not overwrite.** When the client's `input_delay` SHRINKS,
///    two local ticks author the SAME buffer tick; the client fixes its own entry, but
///    `update_buffer` refuses to write any tick `<= last_remote_tick` (input_message.rs:195), so the
///    SERVER keeps the superseded value forever.
/// 4. **An `Absent`-anchored freeze.** An `Absent` entry in the server's buffer makes `get` return
///    `None` for the whole `SameAsPrecedent` tail behind it, `get_predict` return `None` (so
///    `update_action_state` SKIPS the apply and the server's `ActionState` FREEZES at its last
///    value), and — because `get_last` recurses back through `SameAsPrecedent` and DEAD-ENDS on the
///    `Absent` — `get_last()` return `None` as well (input_buffer.rs:339/305). Upstream: lightyear
///    issue #1559, open. See `.agents/scratch/upstream-reports/lightyear-absent-anchor-input-freeze.md`.
///
/// Every one of those returns an ordinary `Some(command)`, and cases 2/3/4 are invisible to the
/// buffer's SHAPE — a fabricated gap-fill and a genuinely HELD trigger are the byte-identical
/// `Compressed::SameAsPrecedent`. The detector this replaced (`get(tick).is_none() &&
/// get_last().is_some()`, commit 2ea6cf5) saw only case 1, and case 4 defeats even that (its second
/// conjunct goes FALSE precisely when the freeze bites). `for_tick` sees all four, and — the point —
/// it sees the NEXT one too, because it never enumerates them: it asks the command to prove itself.
/// `tests/net_fire_release.rs` drives all four over the real lightyear pipeline.
///
/// **Levels and absolutes are deliberately NOT gated.** Hold-last is CORRECT for `throttle`/`steer`
/// and `aim`/`range`: a starved stream keeping the last drive and lay is the right guess, and none
/// of it commits anything that cannot be taken back. Only the consumables spend ammo, deal damage,
/// or change an entity's lifetime — see `TankCommand::fail_consumables_closed` for why that rule is
/// OURS rather than practitioner canon.
///
/// # The four cases, per end
///
/// - **Client own tank, forward tick:** `stamp_input_tick` wrote `for_tick = tick + input_delay`
///   this tick and `buffer_action_state` files it there; `input_delay` ticks later that tick comes
///   round and `for_tick == tick`. Attested → the edge passes. In the PRE-SYNC window
///   `input_delay()` is 0, so `for_tick == tick` immediately and a genuine click still passes.
/// - **Client own tank, rollback replay:** lightyear restores the historical `ActionState` per
///   replayed tick, stamp and all, and `LocalTimeline::tick()` IS the replayed tick — so the own
///   fire edge re-fires during replay exactly as it must.
/// - **Server tank, no input yet:** `ActionState::default()` carries `for_tick == 0` ≠ the server's
///   tick → failed closed. There is no edge to carry anyway. Harmless.
/// - **Server tank, starved / fabricated / stale / frozen:** the value's stamp names a DIFFERENT
///   tick → consumables failed closed. The starvation re-latch (`701d0a7`) and the MG release leak
///   both stay fixed, and so does every variant of them we have not met yet.
///
/// **Known non-coverage (honest).** Case 3 (`input_delay` SHRINKS) strands a value that IS correctly
/// stamped for its own tick — the player authored it for that tick, then revised it, and the server
/// never got the revision. No stamp can see that; the revision simply never arrived. That is closed
/// upstream of here, by pinning `input_delay` CONSTANT (`net::client`'s
/// `SHIPPING_INPUT_DELAY_TICKS`), which makes the client's write tick advance by exactly +1 and so
/// makes cases 2 and 3 impossible to construct. The two fixes are complementary, not redundant: the
/// pin removes the seeds it can, the attestation refuses to commit on any seed that survives.
///
/// **Loss trade (deliberate, NON-FIX).** A fire edge whose input arrives AFTER its tick was
/// simulated is dropped, not fired late — firing an edge on a tick it was not issued for is the bug
/// (the shot leaves at the wrong muzzle pose and diverges from what the client predicted), so past
/// ticks are dropped in every netcode. lightyear's per-message redundancy normally prevents it: an
/// `InputMessage` carries the inputs for the last N ticks before T (`num_ticks *= packet_redundancy`,
/// client.rs:686), so an isolated packet loss does NOT lose the edge. Only under loss deep enough to
/// outlast that window is an edge dropped rather than fired late; the client may then have predicted
/// a shot the server never fires, leaving its `reload_remaining` (root-resident in `TankSim`, NOT
/// replicated) disagreeing until the next shot reconciles it. Inherent to predicting fire on a lossy
/// input stream.
///
/// **Load-bearing invariant.** Two consecutive buffered `fire_primary: true`s both bridge as edges
/// and fire twice. That is fine because it can only happen for two DISTINCT clicks on back-to-back
/// ticks (two intended shots): `gather_commands` latches the click from `just_pressed` (true for one
/// frame per physical press) and `consume_edges` clears it before the next `feed_action_state`, so a
/// single held mouse button produces exactly ONE buffered `true`. If `gather_commands` is ever
/// changed to latch from `pressed`, a hold would put a run of `true`s in the buffer and this bridge
/// would have to dedupe consecutive edges as well.
fn bridge_action_state_to_tank_command(
    timeline: Res<LocalTimeline>,
    mut tanks: Query<(&ActionState<TankCommand>, &mut TankCommand)>,
) {
    let tick = timeline.tick();
    for (action, mut command) in &mut tanks {
        // Whole-struct copy (matches `ActionState`'s "absolute snapshot per tick" contract) …
        let mut next = action.0;
        // … but a CONSUMABLE commits ONLY on a tick this command can ATTEST it was authored for.
        // `for_tick` was stamped by `net::client`'s `stamp_input_tick` with the tick lightyear
        // files the command under; anything lightyear inherited, repeated, fabricated or froze
        // carries some OTHER tick's stamp. Fail closed — never a detector, always a proof. See the
        // doc above for the four ways an unattested value reaches this line.
        if next.for_tick != tick.0 {
            next.fail_consumables_closed();
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
        ("src/net/protocol.rs", "NetBelts"),
        ("src/net/protocol.rs", "BeltSnapshot"),
        ("src/net/protocol.rs", "FireChannel"),
        // The redundancy envelope and both stream element types it embeds (fires + keyframes), plus the
        // correlation spine `ShotId` (defined at the crate root, net-neutral — see its doc).
        ("src/net/protocol.rs", "FireBurst"),
        ("src/net/protocol.rs", "FireEvent"),
        ("src/net/protocol.rs", "RicochetKeyframe"),
        ("src/net/protocol.rs", "ImpactConfirm"),
        ("src/lib.rs", "ShotId"),
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

    /// [`FireEvent::shot_id`] is a pure function of the fields already on the wire (no extra bytes),
    /// unwrapping the lightyear `Tick` to the net-neutral `u32` the sim keys on — so the shell both
    /// ends spawn and the keyframe that re-seeds it share ONE id.
    #[test]
    fn fire_event_shot_id_is_derived_from_wire_fields() {
        let event = FireEvent {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 800.0,
            caliber: 0.088,
            mass: 10.2,
            tracer: true,
            shooter: Entity::PLACEHOLDER,
            weapon: 3,
            fire_tick: Tick(77),
        };
        assert_eq!(
            event.shot_id(),
            ShotId {
                shooter: event.shooter,
                weapon: 3,
                fire_tick: 77,
            },
        );
    }

    /// THE OWN-SHELL CORRELATION, end to end: [`stamp_shot_ids`] (the shared BOTH-ends stamp) gives a
    /// locally-fired attributed shell — the shooter's own predicted round, the server's authoritative
    /// round — the EXACT [`ShotId`] the wire derives for that shot ([`FireEvent::shot_id`], and hence
    /// the id a `RicochetKeyframe` names). This equality is what lets the shooter's own bounced round
    /// re-seed from the server's keyframe: same shooter root (`ShotSource.tank` is the root `fire`
    /// used, the entity the keyframe's mapped `shooter` resolves to), same slot, same tick-indexed
    /// fire tick (both ends run `fire` for tick T with the input for tick T).
    #[test]
    fn stamp_shot_ids_matches_the_wire_derived_id() {
        use bevy::ecs::system::RunSystemOnce;

        use crate::ballistics::{Projectile, Shot, ShotSource};

        const FIRE_TICK: i32 = 42;
        let mut world = World::new();
        world.insert_resource(timeline_at(FIRE_TICK));
        let shooter = world.spawn_empty().id();
        // The own-shell shape at spawn: `Projectile` + `ShotSource`, no `Shot` yet (`shooting::fire`
        // passes `shot: None` — the sim cannot read the tick).
        let shell = world
            .spawn((
                Projectile::test_88(Vec3::X * 800.0),
                ShotSource {
                    tank: shooter,
                    weapon: 1,
                },
            ))
            .id();

        world.run_system_once(stamp_shot_ids).unwrap();

        // What the server's broadcast derives for the SAME shot on the wire.
        let wire_id = FireEvent {
            origin: Vec3::ZERO,
            direction: Vec3::X,
            speed: 800.0,
            caliber: 0.0079,
            mass: 0.0118,
            tracer: true,
            shooter,
            weapon: 1,
            fire_tick: Tick(FIRE_TICK as u32),
        }
        .shot_id();
        assert_eq!(
            world.get::<Shot>(shell).expect("stamped").0,
            wire_id,
            "the locally-stamped ShotId equals the wire-derived one — the keyframe correlates",
        );

        // Idempotent: a second run must not re-stamp (Without<Shot>).
        world.run_system_once(stamp_shot_ids).unwrap();
        assert_eq!(world.get::<Shot>(shell).unwrap().0, wire_id);
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

    /// A command the player authored FOR `tick` — the attested case.
    fn authored_for(tick: i32, cmd: TankCommand) -> TankCommand {
        TankCommand {
            for_tick: Tick(tick as u32).0,
            ..cmd
        }
    }

    fn fire_click() -> TankCommand {
        TankCommand {
            fire_primary: true,
            ..default()
        }
    }

    /// UNATTESTED tick: the `ActionState` carries a command the player authored for tick 5, and the
    /// sim is stepping tick 10. This is what EVERY way lightyear can hand back a value that is not
    /// this tick's input looks like at the seam — hold-last extrapolation, a `SameAsPrecedent`
    /// gap-fill, a stale un-overwritable entry, or an `Absent`-frozen `ActionState`. The bridge must
    /// NOT re-latch the stale fire edge — not once, and not on any subsequent tick.
    #[test]
    fn unattested_tick_does_not_refire_held_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(10));
        let entity = world
            .spawn((
                ActionState(authored_for(5, fire_click())),
                TankCommand::default(),
            ))
            .id();

        // Three unattested ticks in a row, each clearing the command first (as `consume_edges`
        // would): the bridge is the only thing that could re-latch the edge, and it must never.
        for _ in 0..3 {
            world.get_mut::<TankCommand>(entity).unwrap().fire_primary = false;
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();
            assert!(
                !world.get::<TankCommand>(entity).unwrap().fire_primary,
                "an unattested fire edge must not bridge",
            );
        }
    }

    /// An unattested tick carries the MOVEMENT levels and ABSOLUTES through (hold-last is CORRECT
    /// for those — a starved stream keeping the last drive and lay is the right guess, and neither
    /// commits anything irreversible), and fails every CONSUMABLE closed.
    #[test]
    fn unattested_tick_keeps_movement_levels_fails_consumables_closed() {
        let mut world = World::new();
        world.insert_resource(timeline_at(10));

        let held = authored_for(
            5,
            TankCommand {
                throttle: 0.7,
                steer: -0.3,
                fire_secondary: true,
                aim: Some(Vec3::new(1.0, 2.0, 3.0)),
                range: 850.0,
                fire_primary: true,
                crew_swap: Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Loader)),
                respawn: true,
                for_tick: 0,
            },
        );
        let entity = world
            .spawn((ActionState(held), TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        let cmd = *world.get::<TankCommand>(entity).unwrap();
        assert_eq!(cmd.throttle, 0.7, "throttle level held through starvation");
        assert_eq!(cmd.steer, -0.3, "steer level held through starvation");
        assert_eq!(cmd.aim, Some(Vec3::new(1.0, 2.0, 3.0)), "aim absolute held");
        assert_eq!(cmd.range, 850.0, "range absolute held");
        // …and every consumable fails closed.
        assert!(
            !cmd.fire_secondary,
            "automatic-fire level fails closed on an unattested tick",
        );
        assert!(!cmd.fire_primary, "fire edge cleared on an unattested tick");
        assert_eq!(
            cmd.crew_swap, None,
            "crew-swap edge cleared on an unattested tick"
        );
        assert!(!cmd.respawn, "respawn edge cleared on an unattested tick");
    }

    /// The "extra shot after release" report, at the seam: however the release went missing, the
    /// server ends up presenting a trigger-down command stamped for an OLD tick. The bridge must
    /// fail `fire_secondary` closed on EVERY such tick, so the count of ticks that could cycle the
    /// MG after release is ZERO — not merely reduced.
    #[test]
    fn unattested_held_trigger_fires_no_tick_for_the_whole_window() {
        let mut world = World::new();
        world.insert_resource(timeline_at(20));

        let held = authored_for(
            5,
            TankCommand {
                fire_secondary: true,
                ..default()
            },
        );
        let entity = world
            .spawn((ActionState(held), TankCommand::default()))
            .id();

        let mut fired_ticks = 0;
        for _ in 0..10 {
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();
            if world.get::<TankCommand>(entity).unwrap().fire_secondary {
                fired_ticks += 1;
            }
        }
        assert_eq!(
            fired_ticks, 0,
            "a trigger held on unattested ticks must fire zero extra rounds after release",
        );
    }

    /// THE 88 COROLLARY. An `Absent`-anchored server buffer FREEZES `ActionState` outright:
    /// `get_predict` returns `None`, so lightyear's `update_action_state` SKIPS the apply and the
    /// component keeps whatever it last held — forever, across an unbounded number of ticks. If a
    /// `fire_primary: true` is what it froze on, the edge is re-presented to the sim EVERY tick,
    /// `consume_edges` clears it every tick, and `shooting::fire` (reload-gated) lands ONE
    /// unrequested 88 round per reload cycle, indefinitely. The old `held_last` detector could not
    /// see this at all: `get_last()` recurses back through `SameAsPrecedent` and DEAD-ENDS on the
    /// `Absent`, returning `None`, so its second conjunct was FALSE precisely when the freeze bit.
    ///
    /// Attestation does not care WHY the value is stale. The frozen command names an old tick; the
    /// gate refuses it, every tick, for as long as the freeze lasts.
    #[test]
    fn frozen_action_state_cannot_relatch_the_88() {
        let mut world = World::new();
        let entity = world
            .spawn((
                // Frozen on a click authored for tick 5 and never updated since.
                ActionState(authored_for(5, fire_click())),
                TankCommand::default(),
            ))
            .id();

        // Four seconds of ticks at 64 Hz — more than a full 88 reload cycle, so a single re-latch
        // would be a live round downrange.
        let mut latched = 0;
        for tick in 6..262 {
            world.insert_resource(timeline_at(tick));
            world.get_mut::<TankCommand>(entity).unwrap().clear_edges();
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();
            if world.get::<TankCommand>(entity).unwrap().fire_primary {
                latched += 1;
            }
        }
        assert_eq!(
            latched, 0,
            "a frozen fire edge must never re-latch — that is an unrequested 88 round per reload",
        );
    }

    /// The counterpart the gate must NOT break: a tick whose command ATTESTS it was authored for
    /// this very tick holds `fire_secondary` through, so a genuinely-held trigger keeps the
    /// `Automatic` cycling. Only unattested fire fails closed; confirmed fire is exactly what the
    /// player asked for.
    #[test]
    fn attested_tick_keeps_fire_secondary_for_sustained_fire() {
        let mut world = World::new();
        world.insert_resource(timeline_at(8));

        let held = authored_for(
            8,
            TankCommand {
                fire_secondary: true,
                ..default()
            },
        );
        let entity = world
            .spawn((ActionState(held), TankCommand::default()))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_secondary,
            "an attested held trigger must bridge through — sustained fire is not fabricated fire",
        );
    }

    /// An attested tick bridges the whole command, EDGES included. This is the non-starved case and
    /// every rollback-replayed tick of the client's own tank: lightyear restores the historical
    /// `ActionState` (stamp and all) per replayed tick, and `LocalTimeline::tick()` IS the replayed
    /// tick — so the own fire edge must re-fire during replay.
    #[test]
    fn attested_tick_fires_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(8));
        let entity = world
            .spawn((
                ActionState(authored_for(8, fire_click())),
                TankCommand::default(),
            ))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_primary,
            "an attested edge must bridge through (own-tank rollback re-fire)",
        );
    }

    /// The PRE-SYNC window: before the `InputTimeline` syncs, `input_delay()` is 0, so
    /// `stamp_input_tick` stamps the CURRENT tick and a genuine click attests immediately — even
    /// though no `InputBuffer` exists yet. (The bridge no longer reads the buffer at all; this pins
    /// that a joining player's first click is not swallowed.)
    #[test]
    fn pre_sync_click_attests_and_passes() {
        let mut world = World::new();
        world.insert_resource(timeline_at(3));
        let entity = world
            .spawn((
                ActionState(authored_for(3, fire_click())),
                TankCommand::default(),
            ))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            world.get::<TankCommand>(entity).unwrap().fire_primary,
            "a pre-sync click is authored for the current tick — it must pass",
        );
    }

    /// The server's own `ActionState::default()` before ANY input message lands carries
    /// `for_tick == 0`, which attests to nothing once the server's tick has moved — so it fails
    /// closed. (There is no edge in a default command anyway; this pins that the default is not
    /// accidentally attested at tick 0 forever.)
    #[test]
    fn default_action_state_is_unattested() {
        let mut world = World::new();
        world.insert_resource(timeline_at(7));
        let entity = world
            .spawn((
                ActionState(TankCommand {
                    fire_secondary: true,
                    ..default()
                }),
                TankCommand::default(),
            ))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        assert!(
            !world.get::<TankCommand>(entity).unwrap().fire_secondary,
            "an unstamped (default) command attests nothing — fail closed",
        );
    }

    /// **The `for_tick` wire cost, measured — not modelled.** lightyear serializes an
    /// `InputMessage` with `bincode::serde::encode_into_std_write(.., config::standard())`
    /// (`lightyear_serde` registry.rs:175), so this encodes a real `NativeStateSequence<TankCommand>`
    /// — the exact `states` payload of one input message — with the exact same crate and config.
    ///
    /// The honest downside of provenance: `for_tick` changes EVERY tick, so it defeats
    /// `Compressed::SameAsPrecedent` run-compression. A message that used to be one full command plus
    /// four one-byte "same as the last one" markers becomes five full commands
    /// (`packet_redundancy: 5`).
    ///
    /// This prints both regimes so the cost is on the record rather than assumed. It asserts only a
    /// CEILING, so it fails loudly if `TankCommand` ever grows fat enough to make the input stream a
    /// real bandwidth problem — at which point the mitigation is to shrink the ATTESTED payload (a
    /// `u8` tick-LSB, or a fire-fields-only sub-struct carrying the stamp), not to drop attestation.
    ///
    /// Context for the number: this is UPSTREAM traffic only (client → server), one message per
    /// frame. The comparison is also somewhat theoretical — `aim` is an `Option<Vec3>` of a
    /// HULL-LOCAL point, so it changes every tick whenever the player is aiming OR the hull is
    /// moving OR the turret is slewing. Run-compression was already dead in all of those; it only
    /// ever paid off for a tank sitting perfectly still doing nothing, which is exactly when nobody
    /// cares about bandwidth.
    #[test]
    fn input_message_wire_cost() {
        use lightyear::prelude::input::native::NativeStateSequence;

        const REDUNDANCY: u32 = 5; // lightyear `InputConfig::packet_redundancy` default
        const TICK_HZ: usize = 64;

        // `ActionStateSequence` is the trait carrying `build_from_input_buffer`.
        use lightyear::prelude::input::native::NativeBuffer;
        use lightyear_inputs::input_message::ActionStateSequence;

        fn encoded_len(buffer: &NativeBuffer<TankCommand>, end: Tick) -> usize {
            let seq = NativeStateSequence::<TankCommand>::build_from_input_buffer(
                buffer, REDUNDANCY, end,
            )
            .expect("buffer has entries");
            bincode::serde::encode_to_vec(&seq, bincode::config::standard())
                .expect("bincode encodes the sequence")
                .len()
        }

        // A player holding the trigger, perfectly still: the ONLY thing that changes tick to tick is
        // the stamp. This is the worst case for compression, i.e. the best case for the old wire.
        let held = |tick: i32, stamped: bool| {
            ActionState(TankCommand {
                throttle: 1.0,
                fire_secondary: true,
                aim: Some(Vec3::new(0.0, 1.5, -40.0)),
                range: 800.0,
                for_tick: if stamped { Tick(tick as u32).0 } else { 0 },
                ..default()
            })
        };

        let mut unstamped = NativeBuffer::<TankCommand>::default();
        let mut stamped = NativeBuffer::<TankCommand>::default();
        // REALISTIC tick values: bincode's `standard()` config VARINT-encodes, so a `for_tick` of
        // 100 costs one byte while a real mid-session tick (100_000 ≈ 26 min in at 64 Hz) costs
        // several. Measuring at tick 100 would flatter the stamp.
        const T0: i32 = 100_000;
        for tick in T0..T0 + 10 {
            unstamped.set(Tick(tick as u32), held(tick, false));
            stamped.set(Tick(tick as u32), held(tick, true));
        }

        let before = encoded_len(&unstamped, Tick((T0 + 9) as u32));
        let after = encoded_len(&stamped, Tick((T0 + 9) as u32));
        let delta = after - before;
        let up_bytes_per_s = delta * TICK_HZ;

        println!(
            "IDLE (worst case for the stamp — nothing but `for_tick` changes, so run-compression \
             was fully paying off): {before} B -> {after} B (+{delta} B/message) = +{:.1} KB/s \
             upstream per client at {TICK_HZ} Hz",
            up_bytes_per_s as f64 / 1024.0,
        );

        // The REALISTIC regime: the player is aiming, so the hull-local `aim` point already differs
        // every tick and every slot was ALREADY a full command. The stamp adds only its own bytes.
        let aiming = |tick: i32, stamped: bool| {
            ActionState(TankCommand {
                throttle: 1.0,
                fire_secondary: true,
                aim: Some(Vec3::new(0.01 * tick as f32, 1.5, -40.0)),
                range: 800.0,
                for_tick: if stamped { Tick(tick as u32).0 } else { 0 },
                ..default()
            })
        };
        let mut aim_unstamped = NativeBuffer::<TankCommand>::default();
        let mut aim_stamped = NativeBuffer::<TankCommand>::default();
        for tick in T0..T0 + 10 {
            aim_unstamped.set(Tick(tick as u32), aiming(tick, false));
            aim_stamped.set(Tick(tick as u32), aiming(tick, true));
        }
        let aim_before = encoded_len(&aim_unstamped, Tick((T0 + 9) as u32));
        let aim_after = encoded_len(&aim_stamped, Tick((T0 + 9) as u32));
        let aim_delta = aim_after - aim_before;
        println!(
            "AIMING (the realistic regime — `aim` is a hull-local point, so it already changed \
             every tick and compression was already dead): {aim_before} B -> {aim_after} B \
             (+{aim_delta} B/message) = +{:.1} KB/s upstream per client at {TICK_HZ} Hz",
            (aim_delta * TICK_HZ) as f64 / 1024.0,
        );

        assert!(
            after <= 256,
            "the attested input payload is {after} B/message ({:.1} KB/s upstream) — TankCommand has \
             grown fat enough that per-tick attestation costs real bandwidth. Shrink the ATTESTED \
             payload (u8 tick-LSB, or a fire-only sub-struct carrying the stamp); do NOT drop \
             attestation, which is what keeps the server from firing rounds nobody asked for.",
            up_bytes_per_s as f64 / 1024.0,
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

    /// A `Remote` tank carrying a two-slot `TankSim` whose belt-fed weapon has DIVERGED below the
    /// authoritative belt (the phantom-shot bug), plus a `Single` weapon whose reload must be left
    /// alone. Seeds the root's [`NetBelts`] with server truth for the belt weapon and `None` for the
    /// `Single`.
    fn diverged_belt_tank(world: &mut World, server_belt: u32, server_swap: f32) -> Entity {
        use crate::tank::{TankSim, WeaponState};
        world
            .spawn((
                Remote,
                NetBelts {
                    weapons: vec![
                        // Slot 0: the belt-fed weapon, server truth.
                        Some(BeltSnapshot {
                            belt: server_belt,
                            swap_remaining: server_swap,
                        }),
                        // Slot 1: a `Single` weapon — no belt, must stay untouched.
                        None,
                    ],
                },
                TankSim {
                    servos: Vec::new(),
                    anchors: Vec::new(),
                    weapons: vec![
                        // Slot 0: the client's MISPREDICTED belt (fired a phantom round the server
                        // never fired), with a stale cyclic `reload_remaining`.
                        WeaponState {
                            belt_remaining: 3,
                            reload_remaining: 0.05,
                            ..WeaponState::default()
                        },
                        // Slot 1: the `Single` weapon mid-reload — its `reload_remaining` is the 88's
                        // reload countdown and must survive `apply_net_belts` intact.
                        WeaponState {
                            belt_remaining: 0,
                            reload_remaining: 4.2,
                            ..WeaponState::default()
                        },
                    ],
                },
            ))
            .id()
    }

    /// THE BELT-CONVERGENCE TRIPWIRE. Injects a belt divergence (client belt != server belt) and
    /// asserts `apply_net_belts` snaps the client's `belt_remaining` to server truth in ONE apply —
    /// which clears the `hrld` divergence, since `belt_remaining` is exactly the term
    /// `trace::hash_tank_state` folds into that hash. Also proves the fix stays off the `Single`
    /// weapon (whose reload divergence is out of scope) and off the belt weapon's cyclic reload while
    /// rounds remain (that stays client-predicted).
    #[test]
    fn diverged_belt_converges_to_server_truth() {
        use crate::tank::TankSim;

        let mut world = World::new();
        // Server belt is 6 (client mispredicted down to 3); belt has rounds so no swap.
        let tank = diverged_belt_tank(&mut world, 6, 0.0);

        world.run_system_once(apply_net_belts).unwrap();

        let sim = world.get::<TankSim>(tank).unwrap();
        assert_eq!(
            sim.weapons[0].belt_remaining, 6,
            "the client's mispredicted belt must snap to server truth (hrld belt term now matches)",
        );
        assert_eq!(
            sim.weapons[0].reload_remaining, 0.05,
            "while the belt has rounds the cyclic reload stays client-predicted (not pinned)",
        );
        // The `Single` weapon (a `None` entry) is untouched — its reload divergence is out of scope.
        assert_eq!(
            sim.weapons[1].belt_remaining, 0,
            "a Single weapon carries no belt and is left alone",
        );
        assert_eq!(
            sim.weapons[1].reload_remaining, 4.2,
            "a Single weapon's reload countdown must survive apply_net_belts intact",
        );
    }

    /// While the authoritative belt is DRY, `apply_net_belts` pins the client's swap countdown to the
    /// server's — the anti-oscillation guarantee ([`BeltSnapshot::swap_remaining`]): without it the
    /// client, seeing `belt == 0` with a near-zero cyclic reload, would instantly complete the swap
    /// locally and fight the overwrite every tick. Convergence in one apply.
    #[test]
    fn dry_belt_pins_swap_countdown_no_oscillation() {
        use crate::tank::TankSim;

        let mut world = World::new();
        // Server belt is DRY, 2.5 s into the swap; the client wrongly thinks it still has 3 rounds
        // with a near-zero reload (the boundary the oscillation would have exploited).
        let tank = diverged_belt_tank(&mut world, 0, 2.5);

        // Two applies (two ticks) — the belt stays pinned at 0 and the countdown at server truth,
        // never refilling locally: the oscillation would show up as belt flipping back to belt_size.
        for _ in 0..2 {
            world.run_system_once(apply_net_belts).unwrap();
            let sim = world.get::<TankSim>(tank).unwrap();
            assert_eq!(
                sim.weapons[0].belt_remaining, 0,
                "a dry authoritative belt stays dry on the client — no local refill oscillation",
            );
            assert_eq!(
                sim.weapons[0].reload_remaining, 2.5,
                "the swap countdown is pinned to server truth while the belt is dry",
            );
        }
    }
}
