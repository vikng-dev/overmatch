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
// authority-vs-replica discriminator — `Predicted`/`Interpolated` are role markers, not provenance.
use lightyear::prelude::client::Remote;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

use super::disclosure::{NetTankStatus, apply_net_tank_status};
use crate::ballistics::{ComponentHealth, Verdict};
use crate::command::{DeviceClaim, TankCommand};
use crate::damage::{
    CrewStation, Crewman, DamageConsequences, Dead, LaunchedTurret, PendingSwap, TankVolumes,
};
use crate::damage_cam::wire::ShotExplanation;
use crate::fingerprint::{fingerprint_field, fnv1a_64};
use crate::state::GameplaySet;
use crate::tank::{
    Controlled as GameControlled, Rig, ServoCommand, ServoIndex, ServoSpec, TankServos, WeaponGate,
    WeaponLoads,
};
use crate::track::sim::{EngineRpm, TankTransmission, TrackDrive, TrackDriveSide};
use crate::{CombatantId, CombatantLife, ShotId};

// ---------------------------------------------------------------------------
// Protocol compatibility guard
// ---------------------------------------------------------------------------
// Replicon registration order is wire compatibility. Both netcode endpoints must use
// `protocol_id()` — the compile-time `PROTOCOL_FINGERPRINT` plus the runtime map digest — as their
// netcode `protocol_id`; ADR-0018 and the manifest-pinning tests own the compatibility guard.

/// Bump and re-pin the affected wire manifest value for every wire-surface change.
///
/// REV 19: added the client→authority spawn-point request ([`SpawnChannel`] + [`SetSpawnPoint`]) for
/// the playtest spawn map — one new channel and one new message on the wire. The same REV also
/// widened the manifest: every channel now rides [`WIRE_SURFACE`] with its delivery mode and
/// direction, so a lane's CONFIG can no longer skew between two ends that shake hands. REV 19 has
/// not shipped, so both landed as edits to it rather than a further bump.
///
/// REV 20: `TransmissionState` grew `band_confirm_ticks` — the upshift
/// sustained-speed confirmation counter (`UPSHIFT_CONFIRM_TICKS`), replicated and rolled back
/// atomically with the rest of the REV-14 transmission root and hashed in the determinism trace
/// like every other discrete field.
///
/// REV 21 (the fire-backward-uphill shift-storm fix): NO wire-format change, but
/// two REPLICATED transmission fields changed semantics/dynamics, which is sim-version skew (the
/// alpha.9 lesson: silent sim skew passes the handshake and manifests as rollback storms):
/// `band_confirm_ticks` now counts the FULL ordinary-upshift predicate for 8 consecutive decision
/// ticks (was: the band sub-condition only, for 13 — it no longer saturates on a lugging climb),
/// and the gated `demand_n` EMA became asymmetric (rise 8 ticks unchanged, fall 32 ticks). Two
/// peers disagreeing on either law re-simulate different gear decisions from identical snapshots.
///
/// REV 22 (the receiving half of combat): one new owner-private replicated component,
/// `HullShock` — an episode counter, the authority tick it changed on, and a
/// cause tag. It carries no force. Its only job is to be a fact the owner cannot predict, so its
/// arrival forces the rollback that restores the sub-threshold hull velocity a hit actually
/// produced. New wire type and new registration: surface, own-type graph, and REV all move.
///
/// REV 23 (naming the victim on an impact): [`RicochetKeyframe`] and [`ImpactConfirm`] each gain
/// `victim: Option<CombatantId>` — the body the authority gave the impulse to. Without it the owner
/// can tell WHICH EPISODE a spark belongs to (the authority tick already rides both facts) but not
/// WHOSE HULL, so `net::adoption`'s ordering rule could release one tank's shove against a spark
/// drawn on another. No new type and no new registration, so the surface hash is unchanged; the
/// own-type graph and REV move.
///
/// REV 24 (the episode's own span): `HullShock` gains `opened` — the authority
/// tick of the FIRST impulse the episode is made of. It is carried rather than derived because
/// `close − SHOCK_EPISODE_TICKS` is only the span of a DEFERRED episode: the first episode a fresh
/// hull publishes spans a single tick, so the derived window claimed fifteen ticks it never covered,
/// and a respawn keeps the combatant identity, so those ticks can hold the previous life's hits.
/// `net::adoption` matches sparks against `[opened, tick]`. No new type and no new registration, so
/// the surface hash is unchanged; the own-type graph and REV move.
///
/// REV 25 (comparator ownership): NO wire-format change — the REV-21 precedent. `HullShock`'s
/// registered rollback condition is permanently inert; `net::adoption` is the sole INTENTIONAL
/// present-value `HullShock` delivery policy (lightyear's comparator-independent presence-mismatch
/// recovery remains, and no production lifecycle exercises it). The bytes are identical, but two
/// peers on opposite sides of this change reconcile the same `HullShock` snapshot with different
/// ownership and presentation timing — the 5-seed capture measured the native comparator landing
/// every belt-first shove 1–3 ticks before its spark, which is exactly the skew class REV exists
/// to refuse. Surface and type hashes are unchanged; only the REV (and with it the manifest
/// fingerprint) moves.
///
/// REV 26 (map2 terrain + 1500 m extent): NO wire-format change. The shipped heightmap and the
/// square it is hung at both changed — a new 4096² map over 1 500 m spanning 0..50 m, declared by
/// the map's own `level.json` instead of the old compile-time 1 000 m / 0..100 m pair. The ground is a
/// sim input every peer integrates against (belt contacts, spawn heights, hull collisions), so two
/// peers on opposite sides of this change simulate different worlds from identical snapshots —
/// exactly the silent sim skew REV exists to refuse. Surface and type hashes are unchanged; only the
/// REV (and with it the manifest fingerprint) moves. The same REV carries the ORIENTATION half of
/// that ground: the decode now honours the map's declared `image_axes` and reverses a `-Z` image's
/// rows once (`terrain_grid::RowOrder`), which mirrors the whole surface north–south against what an
/// earlier REV-26 build integrates. REV 26 has not shipped, so that lands as an edit here rather
/// than a further bump — the REV-19 precedent above.
///
/// REV 27 (one authoritative timeline): the prediction-era wire surface leaves. Every client
/// renders every hull — its own included — from the interpolated server stream, so `.predict()`
/// registrations, rollback conditions, and correction fns are gone; `HullShock` (REV 22/24/25),
/// the `victim` fields on `RicochetKeyframe`/`ImpactConfirm` (REV 23), and the element-grip
/// netcode (`NetTrackGripAnchor`, `TrackGripElements` replicate-once, the grip checkpoint/resync
/// channels and messages — ADR-0027) all leave the wire with their adoption/prediction consumers.
/// Surface, own-type graph, and REV all move. Note on the servo ULP bands that leave with the
/// comparators: dissolving the band deletes the symptom SENSOR, not the underlying cross-machine
/// float divergence — nothing re-simulates servo state client-side anymore, so there is no gate
/// left for it to storm.
///
/// Still REV 27 (pass 3): dropping lightyear's `prediction` cargo feature removes its internal
/// `Predicted` registration, so lightyear's own protocol hash differs from earlier REV-27 builds.
/// REV 27 has never deployed, so the change rides this bump rather than forcing a 28 — the
/// REV-19/REV-26 precedent above.
///
/// Still REV 27: `TankServos` leaves the wire — dead since the promotion path died; clients
/// integrate `RemoteServos` from the `ServoAngles` stream, so the replicated snapshot had no
/// reader. Surface and own-type graph move; the change rides the undeployed bump — the
/// REV-19/REV-26 precedent above.
///
/// REV 28 (the aim intention carries its own frame — ADR-0038): `TankCommand::aim` becomes an
/// `Option<AimIntent>`, a tagged `World`/`HullLocal` point instead of a bare `Vec3` whose frame was
/// convention. Own-type graph and REV move. Two peers on opposite sides lay the same command's
/// servos at different bearings the moment the hull leaves yaw zero, so this is a sim skew as much
/// as a format change; the tag exists so a consumer can never again be wrong about which frame it
/// was handed.
///
/// REV 29 (lightyear 0.28 -> 0.30.1, bevy_replicon 0.41 -> 0.44): no own type moves, but the
/// framing under all of it does — messages serialize with postcard instead of bincode, the packet
/// header is 13 bytes, message ids are wrapping `u16`s, and replicon 0.43+ folds marker components
/// into its own protocol hash. REV 28 shipped (v0.5.2 onward), so this is a real bump.
///
/// REV 30 (force-law capability): `FireEvent` carries the round's `nose_factor`, which the
/// capability law reads beside mass and calibre. Own-type graph and REV move; capability itself
/// never rides the wire.
///
/// REV 31 (a public impact outcome): [`ImpactConfirm`]'s `penetrated` bool becomes a [`Verdict`]
/// (`Through` / `Stopped` / `Ricochet` / `Struck`), so a peer can tell a round that stopped in the
/// plate from one that went through, and a deflection from a stop that resolved no crossing. New
/// embedded enum: own-type graph and REV move.
///
/// REV 32 (integer HP): [`VolumeSnapshot`]'s `hp` becomes a `u32` of whole HP points (one point is
/// 0.1 reference-mm of dose, `ballistics::HP_PER_REFERENCE_MM`), the pools the authority now keeps
/// as integers. Own-type graph and REV move.
///
/// REV 33 (the pen cam's explanation): `ShotExplanation` rides a new recipient-private
/// [`ShotExplanationChannel`] to the shooter and the struck tank's player, and `CombatantLife`
/// replicates beside `CombatantId`, so a client can tell the life an explanation is about from
/// the one its tank is living. Surface, own-type graph and REV move. REV 33 has not shipped, so
/// the round selector rides it, the REV-19 precedent: `TankCommand` gains the `select_round` edge,
/// the authority's per-weapon round choice (`WeaponLoads`) replicates owner-private beside
/// `WeaponGate`, and `FireEvent` names the fired round by its index in the weapon's round list in
/// place of the round's physical fields, so a replica builds the round the authority fired from its
/// own copy of the tank data. Surface and own-type graph move. The cone's field descriptor rides
/// it too: each `WireCone` carries its field law and ratio (`spall_cone::FieldDescriptor`), from
/// which the client rebuilds the cone's directions and weights. Own-type graph moves. The field law
/// gains a burst's: the ball below a ratio of 1 and the ring (`FieldLaw::Ring`). Own-type graph
/// moves. A record carries its round's burst (`WireBurst`) and a round can end in one
/// (`WireEnd::Burst`). Own-type graph moves.
///
/// REV 34 (the fallen tree): the destruction ledger, one globally visible entity whose
/// [`DestructionLedger`](super::destruction::DestructionLedger) lists every tree state transition
/// as `(index, word, tick)` and replicates by diff. Surface and own-type graph move.
///
/// REV 35 (every burst is a ring, playtest fork F7 closed): a record's burst always names the nose
/// piece's mass (`WireBurst::nose_kg` loses its `Option`), and no round ends at a burst
/// (`WireEnd::Burst` leaves, the ball's end). The field law keeps the ball, which every plate
/// exit's cone is thrown with. Own-type graph and REV move.
///
/// REV 36 (the game's damage model uses energy dose): `WireDose::line` and `field` carry HP
/// points before rounding instead of reference-mm. The scalar types stay unchanged, but a REV 35
/// client would read those values in the wrong units, so the protocol fingerprint must differ.
/// REV 36 is unshipped, so the flight-nose drag form rides it: a replica builds the fired round
/// from its own data, and peers on opposite sides would fly one shot on different drag and range
/// tables even though the drag change adds no wire fields.
///
/// REV 37 (the game's damage model keeps fractional pools, and every field's strength is energy):
/// [`VolumeSnapshot`]'s `held` (was `hp`), a record's `WireComponent::held` (was `hp`) and
/// `WireDose::taken` (was `lost`) carry pool units (`ballistics::POOL_UNITS_PER_HP`, a thousandth
/// of an HP point) instead of whole points. Postcard writes a renamed `u32` the same, so a REV 36
/// peer would read every pool a thousand times fuller: own-type graph and REV move. `WireDose`
/// gains `shown_taken`, the shown HP points the shot took counted settle by settle, sent where
/// another round dosed the pool between the shot's marks and what it held at the shot less `taken`
/// does not give them. Own-type graph moves. In the same revision `WireCone::strength` and every
/// hit dose of a cone's palette carry joules instead of reference-mm (a spall cone's strength a
/// share of the energy its round lost at the plate, a burst's scaled from the energy its explosive
/// drives its wall with), and a bounce's shock enters `WireDose::line` at the energy rate: a
/// REV 36 client would read the strengths in the wrong units and part its potency bands at a rate
/// per reference-mm.
/// REV 37 is unshipped, so the game's cosmetic hull marks ride it: every tank root carries
/// public `HullMarks` (`hull_marks`), replicated by diff. Surface and own-type graph move.
///
/// REV 38: the debris stress lever's carriers (`debris_stress::DebrisPose`), one replicated pose
/// per server-simulated fragment. Surface and own-type graph move.
///
/// REV 39 (the game's damage model picks a fuze mode): `TankCommand` gains the `toggle_fuze` edge,
/// each `WeaponLoad` the crew's fuze mode per round type, and [`FireEvent`] the fired round's mode
/// index, which the receiver binds the round in ([`crate::tank::WeaponRound::in_mode`]). Own-type
/// graph and REV move.
///
/// REV 40 (#185, a client's download per remote tank): `TankTransmission` joins the owner-only
/// disclosure scope and every tank root carries a public `EngineRpm` for the engine sound;
/// `Rotation` rides `wire_quant`'s 6-byte smallest-three codec. Surface, own-type graph and REV
/// move.
pub const PROTOCOL_REV: u32 = 40;

/// Compatibility tag derived from the complete pinned wire manifest plus the crate version. This
/// is the runtime handshake value: version-exact, so a version bump intentionally changes it.
pub const PROTOCOL_FINGERPRINT: u64 = protocol_fingerprint_for(
    WIRE_SURFACE_HASH,
    WIRE_TYPES_HASH,
    WIRE_DEP_AVIAN3D,
    WIRE_DEP_LIGHTYEAR,
    PROTOCOL_REV,
    env!("CARGO_PKG_VERSION"),
);

/// THE HANDSHAKE TAG both endpoints hand netcode: [`PROTOCOL_FINGERPRINT`] — the compile-time wire
/// manifest — folded with the digest of the WORLD this process will build
/// ([`crate::map::content_digest`]).
///
/// The map is a RUNTIME choice (`OVERMATCH_MAP`) over content shipped beside the binary, and its
/// terrain and scatter are DERIVED on both ends rather than replicated. Without this term a
/// same-build server and client on different maps complete the handshake and then integrate
/// different ground: belt contacts, spawn heights and hull collisions all diverge, and the first
/// symptom is a rollback storm nobody can trace to a file. With it the refusal lands at the
/// connect: netcode encrypts the connect token under `protocol_id` (ADR-0018), so a client holding
/// another map mints a token the server cannot decrypt and the request is dropped.
///
/// The embedded substance registry (`materials.ron`) is folded too: every hard body's resistance
/// — a tank's plates, a tree's trunk — resolves through it at load, so two builds embedding
/// different registries integrate different walks over identical assets. The map digest cannot
/// see it (the registry ships inside the binary, not beside it), and the pinned wire manifest
/// does not (no type moves), so it enters here.
///
/// Computed once per process. The map's files cannot change under a running peer, and the heightmap
/// is tens of megabytes.
pub fn protocol_id() -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let digest = crate::map::content_digest(&crate::assets::asset_root());
        handshake_tag(digest, crate::substances::MATERIALS_RON.as_bytes())
    })
}

/// [`PROTOCOL_FINGERPRINT`] folded with the world's content digest and the embedded registry.
fn handshake_tag(map_digest: u64, materials: &[u8]) -> u64 {
    let tag = fingerprint_field(
        PROTOCOL_FINGERPRINT,
        b"map_content",
        &map_digest.to_le_bytes(),
    );
    fingerprint_field(tag, b"materials_ron", materials)
}

/// Fold the VERSION-INDEPENDENT wire manifest: surface, own-type graph, wire dependencies, and
/// protocol revision. This is the portion a real wire skew moves, and it is what the pin tripwire
/// guards. The crate version is deliberately NOT folded here, so a routine release bump does not
/// force a re-pin — it still enters the runtime [`PROTOCOL_FINGERPRINT`] below (version-exact
/// handshakes), just not this pinned invariant.
const fn wire_manifest_fingerprint(
    wire_surface_hash: u64,
    wire_types_hash: u64,
    avian_version: &str,
    lightyear_version: &str,
    protocol_rev: u32,
) -> u64 {
    let hash = fnv1a_64(
        0xcbf2_9ce4_8422_2325,
        b"overmatch-protocol-fingerprint-v1\0",
    );
    let hash = fingerprint_field(hash, b"wire_surface_hash", &wire_surface_hash.to_le_bytes());
    let hash = fingerprint_field(hash, b"wire_types_hash", &wire_types_hash.to_le_bytes());
    let hash = fingerprint_field(hash, b"avian3d", avian_version.as_bytes());
    let hash = fingerprint_field(hash, b"lightyear", lightyear_version.as_bytes());
    fingerprint_field(hash, b"protocol_rev", &protocol_rev.to_le_bytes())
}

/// Derive a handshake tag from the pinned wire manifest PLUS the crate version. The version is
/// folded LAST onto [`wire_manifest_fingerprint`], so this value is byte-identical to the
/// pre-split single fold — the split only exposes the version-independent prefix for pinning.
const fn protocol_fingerprint_for(
    wire_surface_hash: u64,
    wire_types_hash: u64,
    avian_version: &str,
    lightyear_version: &str,
    protocol_rev: u32,
    crate_version: &str,
) -> u64 {
    let hash = wire_manifest_fingerprint(
        wire_surface_hash,
        wire_types_hash,
        avian_version,
        lightyear_version,
        protocol_rev,
    );
    fingerprint_field(hash, b"crate_version", crate_version.as_bytes())
}

/// Replicated tank identity; local `Tank` simulation state is never replicated.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct NetTank;

/// Replicated bot identity for client presentation.
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct NetBot;

/// Authoritative parent-local turret/gun targets for remote local rigs.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct ServoAngles {
    pub turret: f32,
    pub gun: f32,
}

/// Authoritative occupant facts transmitted atomically with `NetCrew` health.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct CrewSnapshot {
    /// Occupant's native station; it can differ from its current seat.
    pub home: CrewStation,
    /// Authority-owned death fact; clients derive their local marker from it.
    pub dead: bool,
}

/// One health-bearing volume and, for crew seats, its occupant facts.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    /// What the volume's pool holds (`ComponentHealth::held`), in pool units
    /// (`ballistics::POOL_UNITS_PER_HP` per HP point): the sim's own figure, which a view rounds
    /// (`ComponentHealth::shown`).
    pub held: u32,
    /// The occupant facts for a crew seat; `None` for a module or ammo volume.
    pub crew: Option<CrewSnapshot>,
}

/// Owner-private authoritative combat state.
///
/// `volumes` follows `health_bearing_volumes` order on both peers. HP, occupancy, death, and the
/// in-flight swap are one atomic snapshot; client UI reads it and never predicts those facts.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetCrew {
    /// Every health-bearing volume in [`health_bearing_volumes`] order: HP + (for seats) occupancy.
    pub volumes: Vec<VolumeSnapshot>,
    /// The in-flight backfill swap, if any: `(source seat, target seat, seconds remaining)`.
    pub swap: Option<(CrewStation, CrewStation, f32)>,
}

/// Authoritative launched-turret pose; client rigs render it without simulating a second launch.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct LaunchedTurretPose(pub Option<(Vec3, Quat)>);

/// A client's REQUEST to place its own next respawn at a world XZ (the spawn map). It is a request,
/// not a fact: the authority validates the bounds, resolves the ground height itself, and consumes
/// the override on that client's next respawn — the client never names a Y and never teleports.
///
/// It carries no entity or combatant reference: the authority keys the override by the SENDING LINK
/// (the same key `CombatantIds`/`ControlledBy` use), so a forged id cannot move another player.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct SetSpawnPoint {
    /// World X, metres. Server-clamped to the playable square.
    pub x: f32,
    /// World Z, metres. Server-clamped to the playable square.
    pub z: f32,
}

/// A public, loss-tolerant reconstruction of an authoritative shot. The receiver maps `shooter` for
/// recoil and self-echo suppression, validates the raw bore, binds the fired round from its own
/// copy of the shooter's weapon, and reconstructs a cosmetic shell.
#[derive(Clone, Serialize, Deserialize)]
pub struct FireEvent {
    pub origin: Vec3,
    /// Raw bore direction, validated before conversion to `Dir3` on receipt.
    pub direction: Vec3,
    pub mechanism: crate::spec::FireMechanism,
    /// Authoritative tracer selection.
    pub tracer: bool,
    /// Entity-mapped firing root, used for recoil and self-echo suppression.
    pub shooter: Entity,
    /// Stable match-local identity; unlike `shooter`, this is not entity-mapped.
    pub combatant: CombatantId,
    /// Weapon slot in the local rig; receipt code bounds-checks it.
    pub weapon: u8,
    /// The fired round's index in that weapon's `Weapon::rounds`, the list every role binds from the
    /// same tank data; receipt code bounds-checks it.
    pub round: u8,
    /// The fired round's fuze mode, its index in that round's authored fuze modes (0 for an inert
    /// round); receipt binds the round in it and refuses an index the fuze does not list.
    pub fuze_mode: u8,
    /// Authority tick used to derive the stable `ShotId` and fast-forward the cosmetic shell to the
    /// receiver's predicted present. The receiver bounds its accepted age.
    pub fire_tick: Tick,
}

impl FireEvent {
    /// Stable identity derived from the wire fields.
    pub fn shot_id(&self) -> ShotId {
        ShotId {
            combatant: self.combatant,
            weapon: self.weapon,
            fire_tick: self.fire_tick.0,
        }
    }
}

impl MapEntities for FireEvent {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        // `CombatantId`, weapon, and tick are stable data, not replica entities.
        self.shooter = mapper.get_mapped(self.shooter);
    }
}

/// Shared public trajectory facts, before delivery policy chooses their carrier.
///
/// Automatic weapons pack these into loss-bounded [`FireVisualBatch`] messages; sparse single-shot
/// facts leave individually on the reliable outcome channel. Gameplay truth does not ride either
/// carrier.
#[derive(Clone, Serialize, Deserialize)]
pub enum FireVisualFact {
    Fire(FireEvent),
    Ricochet(RicochetKeyframe),
    Impact(ImpactConfirm),
}

impl FireVisualFact {
    pub fn shot_id(&self) -> ShotId {
        match self {
            Self::Fire(event) => event.shot_id(),
            Self::Ricochet(keyframe) => keyframe.shot,
            Self::Impact(confirm) => confirm.shot,
        }
    }

    pub fn authority_tick(&self) -> Tick {
        match self {
            Self::Fire(event) => event.fire_tick,
            Self::Ricochet(keyframe) => keyframe.bounce_tick,
            Self::Impact(confirm) => confirm.impact_tick,
        }
    }
}

impl MapEntities for FireVisualFact {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        if let Self::Fire(event) = self {
            event.map_entities(mapper);
        }
    }
}

/// One independently droppable automatic-fire visual payload.
///
/// The server bounds its encoded size below Lightyear's fragmentation threshold and may emit
/// several batches in one tick. [`FireChannel`] is unordered so a newer batch never suppresses an
/// older independent batch.
#[derive(Clone, Serialize, Deserialize)]
pub struct FireVisualBatch {
    pub facts: Vec<FireVisualFact>,
}

impl MapEntities for FireVisualBatch {
    fn map_entities<M: EntityMapper>(&mut self, mapper: &mut M) {
        for fact in &mut self.facts {
            fact.map_entities(mapper);
        }
    }
}

/// An authority-sanctioned post-bounce continuation.
///
/// Clients re-seed the cosmetic shell from this state instead of colliding it against interpolated
/// armor. Delivery follows the shot mechanism; `sequence` supplies causality independent of arrival
/// order.
#[derive(Clone, Serialize, Deserialize)]
pub struct RicochetKeyframe {
    /// The stable shot identity this bounce belongs to. It is never entity-mapped.
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

/// The authority-sanctioned terminal of a shot at armor.
///
/// Perforation ends the external cosmetic shell at the entry face while the authority may continue
/// through internal volumes. Delivery follows the shot mechanism. `after_bounces` prevents a terminal
/// from visually skipping an earlier continuation that arrived later.
#[derive(Clone, Serialize, Deserialize)]
pub struct ImpactConfirm {
    /// The stable shot identity this terminal belongs to. It is never entity-mapped.
    pub shot: ShotId,
    /// The server's impact position (embed point, or the perforation's entry face).
    pub position: Vec3,
    /// The struck face's outward normal, from the server's raycast. No `Dir3` guard on receipt:
    /// `Impact` consumers normalize with a fallback by contract (see `ballistics::Impact::normal`).
    pub normal: Vec3,
    /// The server's verdict at the struck face: whether the round stopped in the material or went
    /// through. Its `entered()` gates the flame lick exactly as the local read does.
    pub verdict: Verdict,
    /// The server `Tick` the impact resolved on. Audit / future RTT-adaptive use, like
    /// [`RicochetKeyframe::bounce_tick`] — the client resolves on receipt, not by re-aging.
    pub impact_tick: Tick,
    /// Ricochets the authority resolved before this terminal (the client's ordering gate).
    pub after_bounces: u32,
}

/// Stable identity for an owner-private damage confirmation. The receipt is plain data and remains
/// valid if the firing root is no longer replicated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DamageReceipt {
    /// Stable match-local firing combatant.
    pub combatant: CombatantId,
    /// The firing weapon slot.
    pub weapon: u8,
    /// The network tick on which that weapon fired.
    pub fire_tick: u32,
}

impl From<ShotId> for DamageReceipt {
    fn from(shot: ShotId) -> Self {
        Self {
            combatant: shot.combatant,
            weapon: shot.weapon,
            fire_tick: shot.fire_tick,
        }
    }
}

/// The minimum live disclosure that one authored shot damaged something on the authority.
///
/// Exact damage, target internals, and the shooter entity deliberately do not ride this fact. The
/// server sends it only to the peer captured as owner when the round fired; [`NetCrew`] remains the
/// authoritative resulting state.
#[derive(Clone, Serialize, Deserialize)]
pub struct DamageConfirm {
    /// A compact owner-private receipt; it has no entity reference to map on receipt.
    pub receipt: DamageReceipt,
    /// Server tick on which the first damage from this shot was confirmed.
    pub damage_tick: Tick,
}

/// Public unordered-unreliable channel for bounded automatic-fire visual batches.
pub struct FireChannel;

/// Public reliable channel for individual single-shot [`FireEvent`], [`RicochetKeyframe`], and
/// [`ImpactConfirm`] facts.
pub struct OutcomeChannel;

/// Owner-private reliable channel for individual [`DamageConfirm`] facts.
pub struct DamageChannel;

/// Recipient-private reliable channel for [`ShotExplanation`]s, each projection to its one
/// recipient. An explanation larger than the link's fragment size travels as fragments, each acked
/// and resent on its own and reassembled on receipt (lightyear's reliable sender). Unordered: an
/// explanation is ordered by its shot, encounter and ticks, never by arrival.
pub struct ShotExplanationChannel;

/// Ordered-reliable client-to-authority lane for spawn-placement requests ([`SetSpawnPoint`]). Its
/// own lane rather than a shared one: a lost spawn choice must not be repaired by, or delay, grip
/// resync. Ordered because the authority resolves the requests by keeping the last one it received,
/// so delivery order IS the decision (see the registration in [`plugin`]).
pub struct SpawnChannel;

/// The tank's health-bearing volumes in `TankVolumes` order — the SINGLE definition of which volumes
/// (and in what order) [`NetCrew`] snapshots, so publish and apply can never drift out of alignment
/// (index `i` addresses the same volume on both ends). `has_health` is the caller's query membership
/// test, so this serves both the immutable (publish) and mutable (apply) `ComponentHealth` query.
///
/// `pub(crate)` so the view-layer hit-feel cue (`net::hit_feel`) can map a `NetCrew` index back to
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
                            held: 0,
                            crew: None,
                        };
                    };
                    VolumeSnapshot {
                        held: src.health.held(),
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

/// The write side of [`apply_net_crew`]. The tank root and its volumes are disjoint entities.
#[derive(QueryData)]
#[query_data(mutable)]
struct VolumeSink {
    health: &'static mut ComponentHealth,
    crewman: Option<&'static mut Crewman>,
    dead: Has<Dead>,
}

/// Apply owner-private authoritative crew state to a replica.
///
/// `NetCrew` is plain replication, so this may run during replay; `Dead` is re-derived from the
/// snapshot, while public life state remains `NetTankStatus`. Run before `DamageConsequences`.
fn apply_net_crew(
    tanks: Query<(&TankVolumes, &NetCrew), With<Remote>>,
    mut volumes: Query<VolumeSink>,
    mut commands: Commands,
) {
    for (tank_volumes, net) in &tanks {
        // The health-bearing volumes in publish order (the SAME shared filter the server used).
        let bearers = health_bearing_volumes(tank_volumes, |v| volumes.contains(v));
        // A length mismatch is expected transiently while the client's rig is still spawning and
        // self-heals once it's fully built; a persistent mismatch means client/server spec skew
        // (a distribution concern — matched builds never skew). Skip rather than write misaligned.
        if bearers.len() != net.volumes.len() {
            continue;
        }
        for (volume, snap) in bearers.into_iter().zip(&net.volumes) {
            let Ok(mut sink) = volumes.get_mut(volume) else {
                continue;
            };
            sink.health.set_held(snap.held);
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
            }
        }
    }
}

/// Client side: mirror the replicated [`NetCrew::swap`] into a LOCAL, cosmetic [`PendingSwap`] on each
/// `Remote` tank, resolving the wire's station pair back to this tank's seat entities. This is the net
/// half of ADR-0016 for the swap timer — replicate the cause (the server's in-flight swap), derive the
/// view state — and it keeps the crew bar (`crew_ui`) a pure SIM-layer view: it reads `PendingSwap`
/// exactly as in single-player, never naming the netcode (the `src/net/tests/net_boundary.rs`
/// contract).
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

/// Republish the network timeline as net-neutral sim vocabulary before every gameplay tick.
fn publish_shot_clock(
    mut shot_clock: ResMut<crate::ShotClock>,
    mut weapon_clock: ResMut<crate::WeaponClock>,
    timeline: Res<LocalTimeline>,
) {
    let tick = timeline.tick().0;
    shot_clock.0 = tick;
    weapon_clock.0 = tick;
}

/// Authority side: mirror the live `ServoState` angles onto the replicated root component.
/// `FixedPostUpdate`, so it reads what `drive_servos` (FixedUpdate, after `GameplaySet`) just
/// stepped. `Without<Remote>` makes it authority-only in shared code: every client-side tank
/// arrived by replication and carries `Remote` (the `Predicted`/`Interpolated` markers can NOT
/// discriminate here — the server carries both).
fn publish_servo_angles(
    mut tanks: Query<(&Rig, &TankServos, &mut ServoAngles), Without<Remote>>,
    servo_slots: Query<&ServoIndex>,
) {
    for (rig, servos, mut angles) in &mut tanks {
        let angle = |servo| {
            servo_slots
                .get(servo)
                .ok()
                .and_then(|slot| servos.states.get(slot.0))
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
/// targets land before the mechanism steps.
///
/// `ServoCommand.target` has exactly one writer per tank, and the filter is what enforces it.
/// `drive_aim_servos` (also in the set, unordered against this) writes it for every tank whose
/// `TankCommand.aim` is `Some`, i.e. every tank holding an input slot. `Without<GameControlled>`
/// excludes that tank here — by the OWNER marker, which rides `ControlledBy`. Every
/// other remote's `TankCommand` stays default (no input slot, and the bridge below skips
/// non-simulated tanks), so `aim` is `None` and `drive_aim_servos` never touches its servos.
fn apply_servo_angles(
    tanks: Query<(&ServoAngles, &Rig), (With<Remote>, Without<GameControlled>)>,
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
///     re-spawn the turret as a free `Static` body seeded AT the authoritative pose. `Position`/
///     `Rotation` go in the tuple BEFORE `RigidBody` (the placeholder-NaN discipline — a body must
///     never flush with a `PLACEHOLDER` pose). Inserting `LaunchedTurret` fires the existing
///     `detach_view_on_turret_launch` observer, which reparents the glb turret subtree for free.
///   - Already detached: write the pose straight onto the turret's `Position`/`Rotation` — avian
///     never moves a static body, so this direct write is the turret's only drive, tracking the
///     server's flying-then-resting turret. `set_if_neq` to avoid at-rest churn.
///
/// `Static`, never `Kinematic` or `Dynamic`: every body in the client's physics world is static
/// (the replica hulls are, [`super::rig::attach_replicated_rig`]), avian's broad phase never pairs
/// two static bodies, so no contact constraint exists on the client. The client's physics step
/// relies on that. A resync of lightyear's interpolation timeline removes `Position` from every
/// replica hull for a frame (`queue_history_presence` in `lightyear_interpolation` 0.30.1), and
/// avian 0.7 keeps the constraints of a pair whose body leaves its body query, then adds them again
/// when the body returns, so its solver indexes past the pair's manifolds and panics
/// (`upstream/avian-body-query-gap-stale-constraints.md`, pinned by
/// `src/tests/avian_body_query_gap.rs`). A kinematic turret resting on a hull is such a pair.
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
            // Already detached: move the free turret to the authoritative pose.
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
                    RigidBody::Static,
                    LaunchedTurret,
                ));
        }
    }
}

/// CURSOR-clock source for the track view (the per-channel clock map): an interpolated hull
/// renders at the interpolation cursor, so the belt speed/phase its track view draws must be
/// sampled on the same clock — component-wise linear, the belt phase in f64. Client-side
/// presentation only: registering interpolation adds no wire type, mode, or direction (the
/// wire-surface pins prove it), and a simulated hull carries no `Interpolated` marker, so its
/// sim-written component is untouched.
pub(crate) fn track_drive_lerp(start: TrackDrive, end: TrackDrive, t: f32) -> TrackDrive {
    let side = |a: TrackDriveSide, b: TrackDriveSide| TrackDriveSide {
        speed: a.speed + (b.speed - a.speed) * t,
        phase: a.phase + (b.phase - a.phase) * f64::from(t),
    };
    TrackDrive {
        throttle: start.throttle + (end.throttle - start.throttle) * t,
        steer: start.steer + (end.steer - start.steer) * t,
        sides: [
            side(start.sides[0], end.sides[0]),
            side(start.sides[1], end.sides[1]),
        ],
    }
}

/// Ordered wire registrations. Keep this list aligned with [`plugin`]; its pinned hash is a direct
/// handshake-fingerprint input. House process also bumps [`PROTOCOL_REV`] for release bookkeeping.
///
/// A CHANNEL is listed as `Name|Mode|Direction`, because a channel's identity on the wire is not its
/// type name alone: the delivery MODE (ordering/reliability) and the DIRECTION are the lane's
/// contract, and changing either — `UnorderedReliable` to `OrderedReliable`, `ClientToServer` to
/// `ServerToClient` — renames nothing. Without them in the manifest, two ends built with different
/// channel config would shake hands and then disagree about how a lane delivers. Everything else
/// (components, messages, the input protocol) is identified by type name alone; what those types
/// SERIALIZE is covered separately by [`WIRE_TYPES_HASH`].
#[cfg(test)]
const WIRE_SURFACE: &[&str] = &[
    // Plain-replicated markers/snapshots — `app.component::<_>().replicate()`, in order:
    "NetTank",
    "NetBot",
    "CombatantId",
    "CombatantLife",
    "ServoAngles",
    "NetCrew",
    "NetTankStatus",
    "LaunchedTurretPose",
    "DestructionLedger",
    "HullMarks",
    "DebrisPose",
    // Message channels (`Name|Mode|Direction`), followed by their message types.
    "FireChannel|UnorderedUnreliable|ServerToClient",
    "OutcomeChannel|UnorderedReliable|ServerToClient",
    "DamageChannel|UnorderedReliable|ServerToClient",
    "ShotExplanationChannel|UnorderedReliable|ServerToClient",
    "SpawnChannel|OrderedReliable|ClientToServer",
    "FireVisualBatch",
    "FireEvent",
    "RicochetKeyframe",
    "ImpactConfirm",
    "DamageConfirm",
    "ShotExplanation",
    "SetSpawnPoint",
    // The input protocol — `InputPlugin::<TankCommand>`:
    "TankCommand",
    // Replicated sim-state components, in order:
    "Position",
    "Rotation",
    "LinearVelocity",
    "AngularVelocity",
    "TrackDrive",
    "TankTransmission",
    "EngineRpm",
    "WeaponGate",
    "WeaponLoads",
];

/// Pinned hash for the ordered wire surface and a direct handshake-fingerprint input.
const WIRE_SURFACE_HASH: u64 = 0xa74c_507b_a869_c2e0;

// ---------------------------------------------------------------------------
// Deep wire-surface coverage (field-level + external-dep skew)
// ---------------------------------------------------------------------------
//
// [`WIRE_SURFACE`] pins the ordered SET OF TYPES that ride the wire; the `plugin_registrations_match_
// wire_surface` tripwire binds that list to the actual `plugin` registration block, and the
// `wire_surface_is_pinned` tripwire pins the list's hash. Together those catch a type ADDED, REMOVED,
// RENAMED, or REORDERED — and, for channels, a delivery MODE or DIRECTION changed, since a channel
// rides the list annotated as `Name|Mode|Direction` (see [`WIRE_SURFACE`]).
// They do NOT catch a change to what a type SERIALIZES: adding a field to
// `VolumeSnapshot`/`CrewSnapshot`/`NetCrew` renames no registered type, so the surface hash stays put
// while the two ends misdeserialize each other — the exact silent skew the guard exists to refuse.
//
// COVERAGE MODEL, in two halves that together cover every byte on the wire:
//   * OWN types (defined in this crate) — covered by [`WIRE_TYPES_HASH`]: the `wire_types_are_pinned`
//     tripwire source-scans each wire-facing struct/enum's DEFINITION TEXT (comments/whitespace
//     stripped, so a doc or reformat edit is invisible; a field/variant/type change is not) and hashes
//     the lot. This is the whole `WIRE_SURFACE` own-type graph, followed through embeds: `NetCrew`
//     carries `VolumeSnapshot` which carries `CrewSnapshot`, `TankTransmission` carries
//     `TransmissionState`/`SchedulerState`, `WeaponGate` carries `WeaponGateState`, `WeaponLoads`
//     carries `WeaponLoad`, `TankCommand` (src/command.rs) carries `CrewSwap` and `RoundSelect`
//     (which carries `RoundStep`), and both
//     `CrewSnapshot` and `CrewSwap` carry `CrewStation` (src/damage.rs).
//     A component whose bytes a CODEC decides rather than its serde derive (`wire_quant`'s rule
//     functions) is covered by that codec's whole source, comments and whitespace stripped, folded
//     into the same hash (the test module's `WIRE_CODEC_SOURCES`): a step, range or layout change
//     there moves it.
//   * EXTERNAL types (avian `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity`, plus lightyear's
//     own wire framing) — their source is not in this tree to scan, so they are covered by DEP VERSION:
//     [`WIRE_DEP_AVIAN3D`]/[`WIRE_DEP_LIGHTYEAR`] pin the resolved `Cargo.lock` versions, so a bump of
//     either dep (which can silently change how those types serialize or how lightyear frames them)
//     also trips a tripwire and changes the fingerprint when re-pinned.
//
// Every pinned value is folded directly into the fingerprint. Failure messages also request a
// `PROTOCOL_REV` bump for release bookkeeping, but compatibility safety does not rely on that step.

/// The pinned hash of the OWN wire-facing type DEFINITIONS (field layout, not just names). Re-pin it
/// whenever a wire-facing struct/enum definition changes; house process also bumps
/// [`PROTOCOL_REV`]. The tripwire prints the new value. See the block above for the coverage model.
const WIRE_TYPES_HASH: u64 = 0x0cfc_5592_2f9d_f206;

/// The pinned `Cargo.lock` versions of the external crates whose types ride the wire (avian's
/// replicated physics components; lightyear's wire framing / input protocol). A bump of either can
/// change the on-wire bytes without touching any source in this tree. Re-pinning either version
/// changes the handshake directly; house process also bumps [`PROTOCOL_REV`].
const WIRE_DEP_AVIAN3D: &str = "0.7.0";
const WIRE_DEP_LIGHTYEAR: &str = "0.30.1";

/// Register the exact shared wire surface represented by [`WIRE_SURFACE`].
pub(crate) fn plugin(app: &mut App) {
    // `LocalTimeline` is incremented by lightyear in `FixedFirst` (lightyear_core's
    // `increment_local_tick`); publish it before every `GameplaySet` consumer, especially
    // `shooting::fire`, which must put the id on its initial FireShell event.
    app.init_resource::<crate::ShotClock>();
    app.init_resource::<crate::WeaponClock>();
    app.add_systems(FixedUpdate, publish_shot_clock.before(GameplaySet));
    app.component::<NetTank>().replicate();
    app.component::<NetBot>().replicate();
    // Immutable spawn identity.
    app.component::<CombatantId>().replicate();
    // Immutable spawn identity: which of its combatant's lives a root is, so a record of a shot
    // against an earlier life is never read against this one.
    app.component::<CombatantLife>().replicate();
    // Plain replication, no interpolation: consumers chase the raw angle through the servo
    // mechanism (see the type).
    app.component::<ServoAngles>().replicate();
    // Owner-private atomic combat snapshot; public life state is `NetTankStatus`.
    app.component::<NetCrew>().replicate();
    // Minimal public tank-life state. It gives observers death/cookoff presentation without
    // replicating private crew, module, ammunition, or belt facts.
    app.component::<NetTankStatus>().replicate();
    // Authoritative launched-turret world pose (same plain-replication shape): the client shows the
    // cooked-off toss it does NOT simulate locally, writing the pose onto its own rig turret.
    app.component::<LaunchedTurretPose>().replicate();
    // The destruction ledger (one world entity): appended entries ride as diffs, a late joiner
    // takes the list once.
    app.component::<super::destruction::DestructionLedger>()
        .replicate_diff();
    // Each tank root's hull marks, public (ADR-0039): new marks ride as diffs, a late joiner takes
    // the held set once.
    app.component::<crate::hull_marks::HullMarks>()
        .replicate_diff();
    // The debris stress lever's carriers: one pose per server-simulated fragment, written only
    // while it moves, so a frozen fragment sends nothing more.
    app.component::<super::debris_stress::DebrisPose>()
        .replicate();

    // Automatic-fire visuals may be lost or reordered. Application ShotId dedup and bounded copies
    // repair ordinary loss without retaining stale cosmetic debt in the transport.
    app.add_channel::<FireChannel>(ChannelSettings {
        mode: ChannelMode::UnorderedUnreliable,
        ..default()
    })
    .add_direction(NetworkDirection::ServerToClient);
    app.add_channel::<OutcomeChannel>(ChannelSettings {
        mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
        ..default()
    })
    .add_direction(NetworkDirection::ServerToClient);
    app.add_channel::<DamageChannel>(ChannelSettings {
        mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
        ..default()
    })
    .add_direction(NetworkDirection::ServerToClient);
    app.add_channel::<ShotExplanationChannel>(ChannelSettings {
        mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
        ..default()
    })
    .add_direction(NetworkDirection::ServerToClient);
    // Spawn placement is a rare, decisive choice: reliable so one click always lands, and ORDERED
    // because the authority keeps only the LAST request per client. Under an unordered lane, rapid
    // clicks A-then-B can be delivered B-then-A and the authority would settle on the stale A —
    // the click the player visibly overrode. Ordering is what makes "last received" mean "last
    // clicked"; the lane is low-rate enough that head-of-line blocking costs nothing.
    app.add_channel::<SpawnChannel>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        ..default()
    })
    .add_direction(NetworkDirection::ClientToServer);

    app.register_message::<FireVisualBatch>()
        .add_map_entities()
        .add_direction(NetworkDirection::ServerToClient);
    // Single-shot starts share the reliable outcome lane with their continuations.
    app.register_message::<FireEvent>()
        .add_map_entities()
        .add_direction(NetworkDirection::ServerToClient);
    app.register_message::<RicochetKeyframe>()
        .add_direction(NetworkDirection::ServerToClient);
    app.register_message::<ImpactConfirm>()
        .add_direction(NetworkDirection::ServerToClient);
    // Damage receipts deliberately contain no entity references: their channel is already
    // owner-private, so mapping a shooter replica would make post-respawn confirmation fragile.
    app.register_message::<DamageConfirm>()
        .add_direction(NetworkDirection::ServerToClient);
    // No entity references: an explanation names its shot and struck life by match identity.
    app.register_message::<ShotExplanation>()
        .add_direction(NetworkDirection::ServerToClient);
    // No entity mapping: the authority keys the spawn override by the sending link, never by a
    // client-named entity.
    app.register_message::<SetSpawnPoint>()
        .add_direction(NetworkDirection::ClientToServer);

    app.add_plugins(input::native::InputPlugin::<TankCommand>::default());

    // Avian replication (map §5): mount lightyear_avian3d's ordering fixes, then register the
    // root's Position/Rotation/velocities ourselves. `AvianReplicationMode::Position` replicates the
    // sim pose, never `Transform`; `sync_to_transform: false` keeps physics authority one-way
    // (Position -> Transform in PostUpdate only). Nothing here moves a body by writing its
    // `Transform`: child colliders (servo nodes) reach physics through avian's own
    // `ColliderTransform` propagation from their LOCAL transforms. `register_physics_components`
    // is OFF because the plugin's defaults predict all four components and this client predicts
    // nothing — the registrations below are the protocol.
    app.add_plugins(LightyearAvianPlugin {
        replication_mode: AvianReplicationMode::Position {
            sync_to_transform: false,
        },
        register_physics_components: false,
        ..default()
    });
    // The plugin mirrors `sync_to_transform` into `PhysicsTransformConfig::transform_to_position`,
    // and in this mode that flag's only reader is avian's spawn hook (`init_physics_transform`):
    // OFF, a body spawned with a `Transform` and no `Position` — every scatter building and trunk,
    // the test course's statics — gets its placeholder `Position` ZEROED, and with avian's
    // transform plugin disabled nothing ever moves it. ON, the hook derives `Position`/`Rotation`
    // from the spawn transform once. No per-frame import is installed either way in this mode.
    app.world_mut()
        .resource_mut::<avian3d::physics_transform::PhysicsTransformConfig>()
        .transform_to_position = true;
    // The hull pose interpolates on the cursor; everything else applies the confirmed value on
    // arrival (per-tick keyframes, so the discrete components step at most one tick apart from
    // the pose they ride beside).
    app.component::<Position>()
        .replicate()
        .add_linear_interpolation();
    app.component::<Rotation>()
        .replicate_with(super::wire_quant::rotation_fns())
        .add_linear_interpolation();
    app.component::<LinearVelocity>().replicate();
    app.component::<AngularVelocity>().replicate();
    // The tracked drivetrain (phase B): velocity-like continuous sim state, replicated for the
    // remote track view. Interpolated like Position/Rotation so a cursor-rendered hull's track
    // view reads cursor-time belt speed/phase, never the arrival-fresh value that visibly leads
    // the hull.
    app.component::<TrackDrive>()
        .replicate()
        .add_interpolation_with(track_drive_lerp);
    // The declared transmission's correlated state: one atomic snapshot, owner-only
    // (`disclosure::CombatDisclosure`). Every other client hears the crank through `EngineRpm`,
    // applied on arrival like the transmission it is read off.
    app.component::<TankTransmission>().replicate();
    app.component::<EngineRpm>().replicate();
    // Complete weapon eligibility state: one atomic snapshot. `net::fire_presentation` reconciles
    // it as a legality report against the locally authored cadence.
    app.component::<WeaponGate>().replicate();
    // Each weapon's loaded and selected round: authority-written, read beside the gate.
    app.component::<WeaponLoads>().replicate();

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
    // Client: realize owner-private crew details. Public knockout state is applied separately.
    app.add_systems(
        FixedUpdate,
        apply_net_crew
            .in_set(GameplaySet)
            .before(DamageConsequences),
    );
    // Realized once a frame, after replication has received the status and before the device is
    // claimed and read: every tick of the frame, and the device, see the knockout from the frame it
    // arrives in.
    app.add_systems(
        RunFixedMainLoop,
        apply_net_tank_status
            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
            .in_set(GameplaySet)
            .before(DeviceClaim),
    );
    app.add_systems(FixedUpdate, mirror_swap_from_net_crew.in_set(GameplaySet));
    // The sim reads `TankCommand`; bridge it before all `GameplaySet` consumers, including replay.
    app.add_systems(
        FixedUpdate,
        bridge_action_state_to_tank_command
            .in_set(InputBridge)
            .before(GameplaySet),
    );
}

/// The input bridge's ordering handle: every writer that must land on top of the attested command
/// orders after this set (`net::fire_presentation`, which replaces the owner's consumables with
/// what this client authored).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct InputBridge;

/// Bridge Lightyear input to the simulation command.
///
/// Invariant: consumable actions commit only when `for_tick == LocalTimeline::tick()`. Level inputs
/// may hold last; stale, fabricated, or frozen input must never spend ammo or create an entity.
/// `SHIPPING_INPUT_DELAY_TICKS` removes delay-wobble sources, while this attestation fails closed for
/// all remaining input-buffer substitutions. See `src/net/tests/net_fire_release.rs`.
fn bridge_action_state_to_tank_command(
    timeline: Res<LocalTimeline>,
    mut tanks: Query<(&ActionState<TankCommand>, &mut TankCommand)>,
) {
    let tick = timeline.tick();
    for (action, mut command) in &mut tanks {
        *command = attest(action.0, tick);
    }
}

/// The bridge's per-tank rule: a whole-struct copy (matches `ActionState`'s "absolute snapshot per
/// tick" contract), except that a CONSUMABLE commits ONLY on a tick this command can ATTEST it was
/// authored for. `for_tick` was stamped by `net::client`'s `stamp_input_tick` with the tick
/// lightyear files the command under; anything lightyear inherited, repeated, fabricated or froze
/// carries some OTHER tick's stamp. Fail closed — never a detector, always a proof.
pub(super) fn attest(mut command: TankCommand, tick: Tick) -> TankCommand {
    if command.for_tick != tick.0 {
        command.fail_consumables_closed();
    }
    command
}

/// In-crate tests for the input-attestation seam.
#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use lightyear::prelude::Tick;

    use super::*;
    use crate::command::{AimIntent, CrewSwap};
    use crate::damage::CrewStation;

    /// A static body spawned by `Transform` alone — the shape `scatter` and the test course use —
    /// lands where its transform says under the NETCODE physics composition, not at the origin.
    /// Fails if the spawn-time Transform -> Position derivation is off (see `plugin`).
    #[test]
    fn a_transform_only_static_body_spawns_at_its_transform() {
        use avian3d::prelude::Collider;
        use lightyear::prelude::client::ClientPlugins;

        let mut app = crate::net::test_harness::net_physics_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        plugin(&mut app);
        let body = app
            .world_mut()
            .spawn((
                Transform::from_xyz(40.0, 2.0, -15.0),
                RigidBody::Static,
                Collider::cuboid(1.0, 1.0, 1.0),
            ))
            .id();
        let position = app
            .world()
            .get::<Position>(body)
            .expect("body has a Position")
            .0;
        assert!(
            position.distance(Vec3::new(40.0, 2.0, -15.0)) < 1e-4,
            "a Transform-only static body must spawn at its transform, got {position:?}"
        );
    }

    /// The client's physics world holds no contact constraint, and a launched turret is the body
    /// that would bring one: it lands on or beside a hull. A resync of lightyear's interpolation
    /// timeline takes `Position` off every replica hull for a frame, and avian 0.7 re-adds a
    /// constraint it kept through that gap, so its solver panics
    /// (`src/tests/avian_body_query_gap.rs`). A turret launched as any non-static body reds the
    /// zero-constraint assertion; the gap below replays a resync.
    #[test]
    fn a_launched_turret_on_a_replica_hull_holds_no_contact_constraint() {
        use avian3d::dynamics::solver::constraint_graph::ConstraintGraph;
        use avian3d::prelude::Collider;

        let mut app = crate::net::test_harness::net_physics_app();
        super::super::physics::plugin(&mut app);
        app.add_systems(FixedUpdate, apply_launched_turret_pose);
        crate::net::test_harness::finish(&mut app);
        let constraints = |app: &App| -> usize {
            app.world()
                .resource::<ConstraintGraph>()
                .colors
                .iter()
                .map(|color| color.manifold_handles.len())
                .sum()
        };

        // A replica hull as `rig::attach_replicated_rig` leaves it, and its turret: the turret's
        // box sits 5 cm into the hull's, so a non-static turret touches the hull from its launch.
        let hull_at = Vec3::new(0.0, 1.0, 0.0);
        let resting = hull_at + Vec3::Y * 0.95;
        let world = app.world_mut();
        let root = world
            .spawn((
                Remote,
                RigidBody::Static,
                Position(hull_at),
                Rotation::default(),
            ))
            .id();
        world.spawn((
            ChildOf(root),
            Transform::IDENTITY,
            Collider::cuboid(3.0, 1.0, 6.0),
        ));
        let turret = world
            .spawn((
                ChildOf(root),
                Transform::from_translation(resting - hull_at),
            ))
            .id();
        world.spawn((
            ChildOf(turret),
            Transform::IDENTITY,
            Collider::cuboid(2.0, 1.0, 2.0),
        ));
        world.entity_mut(root).insert((
            Rig {
                hull: root,
                turret,
                gun: turret,
                muzzle: turret,
            },
            LaunchedTurretPose(Some((resting, Quat::IDENTITY))),
        ));

        for _ in 0..4 {
            app.update();
            assert_eq!(
                constraints(&app),
                0,
                "a launched turret joined a contact constraint"
            );
        }
        assert!(
            app.world().get::<LaunchedTurret>(turret).is_some(),
            "the turret launched"
        );
        assert_eq!(
            app.world().get::<Position>(turret).map(|p| p.0),
            Some(resting),
            "the launched turret rests at the authoritative pose"
        );

        // The resync: `Position` off the hull for one step, then back.
        let pose = *app.world().get::<Position>(root).expect("the hull's pose");
        app.world_mut().entity_mut(root).remove::<Position>();
        app.update();
        app.world_mut().entity_mut(root).insert(pose);
        for _ in 0..4 {
            app.update();
            assert_eq!(
                constraints(&app),
                0,
                "a contact constraint survived the resync"
            );
        }
    }

    #[test]
    fn track_drive_is_registered_for_cursor_interpolation() {
        use lightyear::interpolation::prelude::InterpolationRegistry;
        use lightyear::prelude::client::ClientPlugins;

        let mut app = crate::net::test_harness::base_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        plugin(&mut app);

        let registry = app.world().resource::<InterpolationRegistry>();
        assert!(
            registry.interpolated::<TrackDrive>(),
            "an interpolated hull's belt speed/phase must be a cursor-time value: TrackDrive \
             needs an interpolation registration, or the track view reads arrival-fresh state"
        );

        let start = TrackDrive {
            throttle: 0.0,
            steer: -1.0,
            sides: [
                TrackDriveSide {
                    speed: 2.0,
                    phase: 10.0,
                },
                TrackDriveSide {
                    speed: -4.0,
                    phase: 6.0,
                },
            ],
        };
        let end = TrackDrive {
            throttle: 1.0,
            steer: 1.0,
            sides: [
                TrackDriveSide {
                    speed: 6.0,
                    phase: 30.0,
                },
                TrackDriveSide {
                    speed: 0.0,
                    phase: 8.0,
                },
            ],
        };
        // What this does NOT pin: WHICH fn is registered. Lightyear 0.30 keeps a rule's
        // interpolation fn crate-private, so a swap to `.add_linear_interpolation()` at the
        // registration would still pass here. This pins that TrackDrive is interpolated at all
        // (above) and the law `track_drive_lerp` computes (below).
        let mid = track_drive_lerp(start, end, 0.5);
        assert_eq!(
            mid,
            TrackDrive {
                throttle: 0.5,
                steer: 0.0,
                sides: [
                    TrackDriveSide {
                        speed: 4.0,
                        phase: 20.0,
                    },
                    TrackDriveSide {
                        speed: -2.0,
                        phase: 7.0,
                    },
                ],
            },
            "component-wise linear midpoint, belt phase carried in f64"
        );
    }

    /// The public `ServoAngles` drive every remote tank's servo targets, but the owner's belong to
    /// `aim::drive_aim_servos` alone. The OWN tank is `Remote` like every replicated tank, so only
    /// the owner marker keeps this pump off it — mutate `Without<GameControlled>` away and the
    /// replicated angle overwrites the local intent here.
    #[test]
    fn public_servo_angles_skip_the_tank_that_holds_the_input_slot() {
        fn spawn_tank(world: &mut World, owned: bool) -> (Entity, Entity) {
            let turret = world.spawn(ServoCommand::default()).id();
            let gun = world.spawn(ServoCommand::default()).id();
            let hull = world.spawn_empty().id();
            let muzzle = world.spawn_empty().id();
            let mut root = world.spawn((
                Remote,
                ServoAngles {
                    turret: 0.75,
                    gun: -0.2,
                },
                Rig {
                    hull,
                    turret,
                    gun,
                    muzzle,
                },
            ));
            if owned {
                root.insert(GameControlled);
            }
            (turret, gun)
        }
        let mut world = World::new();
        let (remote_turret, remote_gun) = spawn_tank(&mut world, false);
        let (own_turret, own_gun) = spawn_tank(&mut world, true);

        world.run_system_once(apply_servo_angles).unwrap();

        assert_eq!(
            world.get::<ServoCommand>(remote_turret).unwrap().target,
            0.75
        );
        assert_eq!(world.get::<ServoCommand>(remote_gun).unwrap().target, -0.2);
        assert_eq!(world.get::<ServoCommand>(own_turret).unwrap().target, 0.0);
        assert_eq!(world.get::<ServoCommand>(own_gun).unwrap().target, 0.0);
    }

    #[test]
    fn replicated_rig_preserves_join_in_progress_authoritative_sim_snapshots() {
        let rig = strip_comments(&read_source("src/net/rig.rs"));
        assert!(
            rig.contains("With<EngineRpm>"),
            "client rig attachment must wait for the replicated public crank speed"
        );

        let spawn = strip_comments(&read_source("src/tank/spawn.rs"));
        let attach = spawn
            .split_once("pub(crate) fn attach_replicated_tank_body")
            .expect("replicated attachment function exists")
            .1
            .split_once("fn first_geometry_ancestor")
            .expect("next function bounds the attachment body")
            .0;
        assert!(
            !attach.contains("tank_transmission("),
            "client attachment must not overwrite a JIP snapshot with fresh from-spec state"
        );
        assert!(
            !attach.contains("weapon_gate("),
            "client attachment must not overwrite a JIP weapon-gate snapshot with spawn defaults"
        );
        assert!(
            !attach.contains("TankServos::for_count"),
            "client attachment must not construct authoritative servo state; RemoteServos is the only client integrator"
        );
        assert!(
            !attach.contains("TrackGripElements::for_links"),
            "client attachment must not overwrite the exact JIP grip field with an empty field"
        );
    }

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
    /// component/message/channel/input set changed — re-pin [`WIRE_SURFACE_HASH`] to the value this
    /// prints and, by house process, bump [`PROTOCOL_REV`] in the same diff. The required re-pin is
    /// itself a fingerprint input, so mismatched builds refuse at the netcode handshake (both ends'
    /// `protocol_id` =
    /// [`PROTOCOL_FINGERPRINT`]).
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
    // pattern `src/net/tests/net_boundary.rs` uses to enforce an architectural contract. They are
    // honest-but-simple line/substring scanners, NOT a Rust parser — resilient to reformatting, and
    // biased toward FALSE TRIPS (which merely force a harmless re-pin) over false passes (which
    // would let a wire skew through). Comments/whitespace are stripped first so prose and layout
    // edits never trip them.

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
    /// makes.
    const REG_MARKERS: &[&str] = &[
        ".component::<",        // `app.component::<T>().replicate()` chains
        CHANNEL_MARKER,         // `app.add_channel::<T>(..)` — also carries mode + direction
        ".register_message::<", // `app.register_message::<T>()`
        "InputPlugin::<",       // `InputPlugin::<T>::default()` — the input protocol
    ];

    /// The marker that identifies a channel registration, whose lane CONFIG (not just its type name)
    /// rides the manifest.
    const CHANNEL_MARKER: &str = ".add_channel::<";

    /// The `|Mode|Direction` annotation a channel carries into [`WIRE_SURFACE`], read from the
    /// `add_channel` statement's own text: the `ChannelMode::` variant and the `NetworkDirection::`
    /// variant, each taken as the bare identifier that follows (so `UnorderedReliable(ReliableSettings
    /// ::default())` reads as `UnorderedReliable` — the settings inside are transport tuning, not lane
    /// semantics). Missing either is a hard panic rather than a silent empty annotation: a channel with
    /// no readable mode or direction must never hash as if it had been checked.
    fn channel_config(statement: &str) -> String {
        fn variant(statement: &str, prefix: &str) -> String {
            let at = statement.find(prefix).unwrap_or_else(|| {
                panic!("add_channel statement has no `{prefix}`: {statement:?}")
            });
            let after = &statement[at + prefix.len()..];
            let end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            after[..end].to_string()
        }
        let mode = variant(statement, "ChannelMode::");
        let direction = variant(statement, "NetworkDirection::");
        format!("|{mode}|{direction}")
    }

    /// Derive the ordered wire-type list straight from `plugin`'s registration calls, left-to-right,
    /// top-to-bottom. Each marker is immediately followed by `<Type>`; the last `::`-segment is the
    /// bare type name `WIRE_SURFACE` lists. A channel additionally carries its `|Mode|Direction`,
    /// scanned from the rest of its statement (up to the terminating `;`), so a lane reconfigured in
    /// `plugin` without touching `WIRE_SURFACE` trips exactly like a renamed type.
    fn registered_types(body: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = body;
        while let Some((idx, marker)) = REG_MARKERS
            .iter()
            .filter_map(|m| rest.find(m).map(|i| (i, *m)))
            .min_by_key(|(i, _)| *i)
        {
            let after = &rest[idx + marker.len()..];
            // `<Type>` never spans a line, so bound the NAME scan to this line: a marker without a
            // closing `>` skips its line instead of swallowing the rest of the body.
            let line_end = after.find('\n').unwrap_or(after.len());
            let Some(end) = after[..line_end].find('>') else {
                rest = &after[line_end..];
                continue;
            };
            let name = after[..end]
                .trim()
                .rsplit("::")
                .next()
                .expect("split yields at least one segment")
                .trim()
                .to_string();
            let tail = &after[end + 1..];
            // The channel's own statement — `add_channel::<T>(ChannelSettings { .. }).add_direction(..);`
            // — holds both config facts; every other registration form is its type name alone.
            let annotation = if marker == CHANNEL_MARKER {
                let statement_end = tail.find(';').unwrap_or(tail.len());
                channel_config(&tail[..statement_end])
            } else {
                String::new()
            };
            out.push(format!("{name}{annotation}"));
            rest = tail;
        }
        out
    }

    /// FINDING 1 TRIPWIRE. The types `plugin` actually registers must equal [`WIRE_SURFACE`], in order.
    /// This is what stops the hand list from silently diverging from the code: add / remove / reorder a
    /// registration in `plugin` (e.g. `app.component::<NetSmoke>().replicate()`) WITHOUT editing
    /// `WIRE_SURFACE` and this fails. Updating the list then trips `wire_surface_is_pinned`; its
    /// required re-pin moves the handshake directly. House process pairs that re-pin with a
    /// `PROTOCOL_REV` bump, but the refusal does not rely on remembering it.
    #[test]
    fn plugin_registrations_match_wire_surface() {
        let src = read_source("src/net/protocol.rs");
        let derived = registered_types(plugin_body(&strip_comments(&src)));
        let derived: Vec<&str> = derived.iter().map(String::as_str).collect();
        assert_eq!(
            derived.as_slice(),
            WIRE_SURFACE,
            "plugin()'s registration block no longer matches WIRE_SURFACE. A replicated component, \
             channel, message, or input was added / removed / reordered — or a channel's delivery \
             mode / direction was changed — in plugin() without updating the hand-maintained \
             WIRE_SURFACE list beside it. Update WIRE_SURFACE to match plugin() (which then fails \
             wire_surface_is_pinned), then bump PROTOCOL_REV and re-pin WIRE_SURFACE_HASH — all in \
             the same diff.",
        );
    }

    /// The own wire-facing types whose DEFINITION TEXT rides [`WIRE_TYPES_HASH`], each as
    /// `(source file, type name)`. This is the `WIRE_SURFACE` own-type graph followed through its
    /// embeds (see the coverage-model block by the const): protocol types, the public status in
    /// `disclosure.rs`, `WeaponGate`/`WeaponGateState`/`WeaponLoads`/`WeaponLoad` from
    /// `tank/model.rs`, `TankCommand`/`AimIntent`/`CrewSwap`/`RoundSelect`/`RoundStep` from
    /// `command.rs`, `CrewStation` and `KnockoutReason`
    /// from `damage.rs`, and the shot explanation's graph from `damage_cam/wire.rs`,
    /// `damage_cam/mod.rs` and, for a cone's field descriptor, `ballistics/spall_cone.rs`.
    /// External wire types (avian/lightyear) are covered by dependency version. An embedded ENUM
    /// earns its own row: postcard encodes the variant as its declaration index, so reordering
    /// variants changes what the bytes mean without touching one character of the type that embeds
    /// it.
    const WIRE_TYPE_DEFS: &[(&str, &str)] = &[
        ("src/net/protocol.rs", "NetTank"),
        ("src/track/sim.rs", "TrackDrive"),
        ("src/track/sim.rs", "TrackDriveSide"),
        ("src/track/sim.rs", "TankTransmission"),
        ("src/track/sim.rs", "EngineRpm"),
        ("src/track/transmission.rs", "TransmissionState"),
        ("src/track/transmission.rs", "SchedulerState"),
        ("src/tank/model.rs", "WeaponGate"),
        ("src/tank/model.rs", "WeaponGateState"),
        ("src/tank/model.rs", "WeaponLoads"),
        ("src/tank/model.rs", "WeaponLoad"),
        ("src/net/protocol.rs", "NetBot"),
        ("src/lib.rs", "CombatantId"),
        ("src/lib.rs", "CombatantLife"),
        ("src/net/protocol.rs", "ServoAngles"),
        ("src/net/protocol.rs", "NetCrew"),
        ("src/net/disclosure.rs", "NetTankStatus"),
        ("src/net/protocol.rs", "VolumeSnapshot"),
        ("src/net/protocol.rs", "CrewSnapshot"),
        ("src/net/protocol.rs", "LaunchedTurretPose"),
        ("src/net/destruction.rs", "DestructionLedger"),
        ("src/net/destruction.rs", "LedgerEntry"),
        ("src/hull_marks.rs", "HullMarks"),
        ("src/hull_marks.rs", "HullMark"),
        ("src/hull_marks.rs", "MarkKind"),
        ("src/net/debris_stress.rs", "DebrisPose"),
        ("src/net/protocol.rs", "FireChannel"),
        ("src/net/protocol.rs", "OutcomeChannel"),
        ("src/net/protocol.rs", "DamageChannel"),
        ("src/net/protocol.rs", "ShotExplanationChannel"),
        ("src/net/protocol.rs", "SpawnChannel"),
        ("src/net/protocol.rs", "FireVisualBatch"),
        ("src/net/protocol.rs", "FireVisualFact"),
        ("src/net/protocol.rs", "FireEvent"),
        ("src/spec.rs", "FireMechanism"),
        ("src/net/protocol.rs", "RicochetKeyframe"),
        ("src/net/protocol.rs", "ImpactConfirm"),
        ("src/ballistics.rs", "Verdict"),
        ("src/net/protocol.rs", "DamageReceipt"),
        ("src/net/protocol.rs", "DamageConfirm"),
        ("src/damage_cam/wire.rs", "ShotExplanation"),
        ("src/damage_cam/wire.rs", "TankLife"),
        ("src/damage_cam/wire.rs", "Audience"),
        ("src/damage_cam/wire.rs", "Pose"),
        ("src/damage_cam/wire.rs", "Mm"),
        ("src/damage_cam/wire.rs", "Rotation"),
        ("src/damage_cam/wire.rs", "Record"),
        ("src/damage_cam/wire.rs", "Turn"),
        ("src/damage_cam/wire.rs", "Oct"),
        ("src/damage_cam/wire.rs", "WireImpact"),
        ("src/damage_cam/mod.rs", "ShotVerdict"),
        ("src/damage_cam/wire.rs", "WirePlate"),
        ("src/damage_cam/wire.rs", "WireBounce"),
        ("src/damage_cam/wire.rs", "WireCone"),
        ("src/ballistics/spall_cone.rs", "FieldDescriptor"),
        ("src/ballistics/spall_cone.rs", "FieldLaw"),
        ("src/damage_cam/wire.rs", "WireHit"),
        ("src/damage_cam/wire.rs", "WireBurst"),
        ("src/damage_cam/wire.rs", "WireEnd"),
        ("src/damage_cam/wire.rs", "WireDose"),
        ("src/damage_cam/mod.rs", "ConsequenceState"),
        ("src/damage_cam/wire.rs", "WireComponent"),
        ("src/damage.rs", "KnockoutReason"),
        ("src/net/protocol.rs", "SetSpawnPoint"),
        ("src/lib.rs", "ShotId"),
        ("src/command.rs", "TankCommand"),
        ("src/command.rs", "AimIntent"),
        ("src/command.rs", "CrewSwap"),
        ("src/command.rs", "RoundSelect"),
        ("src/command.rs", "RoundStep"),
        ("src/damage.rs", "CrewStation"),
    ];

    /// Whether `line` is the `struct NAME`/`enum NAME` DEFINITION line — the keyword immediately
    /// followed by the exact name and a non-identifier boundary, so `VolumeSnapshot` never matches
    /// `VolumeSnapshotSource`.
    fn is_def_line(line: &str, name: &str) -> bool {
        header_end(line, name).is_some()
    }

    /// The byte offset in `line` just past `struct name` / `enum name`, where the type's generics,
    /// fields or body begin: anything before it (a `pub(crate)`) is not the definition's.
    fn header_end(line: &str, name: &str) -> Option<usize> {
        ["struct ", "enum "].iter().find_map(|kw| {
            let needle = format!("{kw}{name}");
            line.find(&needle).and_then(|idx| {
                let end = idx + needle.len();
                line[end..]
                    .chars()
                    .next()
                    .is_none_or(|c| !c.is_alphanumeric() && c != '_')
                    .then_some(end)
            })
        })
    }

    /// Extract a type's DEFINITION from comment-stripped source `stripped`: its preceding attribute
    /// lines (the `#[derive(..)]` that governs its serde), the `struct`/`enum` header, and the body up
    /// to the closing `}` (or the `;` after a tuple struct's fields, or a unit struct's). Whitespace is then removed entirely, so only a
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
        // Forward from the type's name: a tuple struct paren-matches its fields to the `;` after
        // them (a field may hold one, `[u8; 8]`), a unit struct ends at the first `;`, else
        // brace-match the body.
        let region: String = lines[kw..].join("\n");
        let body = header_end(lines[kw], name).expect("the header line names the type");
        let after = |c: char| region[body..].find(c).map(|i| body + i);
        let semi = after(';');
        let brace = after('{');
        let paren = after('(')
            .filter(|&p| p < brace.unwrap_or(usize::MAX) && p < semi.unwrap_or(usize::MAX));
        let end = if let Some(open) = paren {
            let mut depth = 0i32;
            let close = region[open..]
                .char_indices()
                .find_map(|(i, c)| {
                    match c {
                        '(' => depth += 1,
                        ')' => depth -= 1,
                        _ => {}
                    }
                    (depth == 0).then_some(open + i)
                })
                .unwrap_or_else(|| panic!("unbalanced parens in `{name}` definition"));
            region[close..]
                .find(';')
                .map(|s| close + s + 1)
                .unwrap_or_else(|| panic!("no `;` after the fields of `{name}`"))
        } else {
            match (brace, semi) {
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
            }
        };
        let attrs = lines[start..kw].join("\n");
        let def = format!("{attrs}\n{}", &region[..end]);
        def.chars().filter(|c| !c.is_whitespace()).collect()
    }

    /// A tuple struct's definition runs to the `;` after its fields, not to the first `;` inside
    /// them, and a restricted visibility's parens are not its fields: each definition ends where
    /// its own fields or body do, never in the next item.
    #[test]
    fn a_tuple_struct_definition_spans_its_fields() {
        for (source, name, def) in [
            (
                "\n#[derive(Serialize)]\nstruct Rotation([u8; 8]);\nstruct Next;\n",
                "Rotation",
                "#[derive(Serialize)]structRotation([u8;8]);",
            ),
            (
                "\n#[derive(Serialize)]\npub(crate) struct X([u8; 8]);\nstruct Next;\n",
                "X",
                "#[derive(Serialize)]pub(crate)structX([u8;8]);",
            ),
            (
                "\n#[derive(Serialize)]\npub(crate) struct Unit;\nfn next(a: u8) {}\n",
                "Unit",
                "#[derive(Serialize)]pub(crate)structUnit;",
            ),
            (
                "\n#[derive(Serialize)]\npub(super) struct Id(pub u32);\nstruct Next { a: u8 }\n",
                "Id",
                "#[derive(Serialize)]pub(super)structId(pubu32);",
            ),
            (
                "\n#[derive(Serialize)]\npub(crate) struct Braced {\n    a: [u8; 2],\n}\nstruct Next(u8);\n",
                "Braced",
                "#[derive(Serialize)]pub(crate)structBraced{a:[u8;2],}",
            ),
        ] {
            assert_eq!(normalized_type_def(source, name), def, "{name}");
        }
    }

    /// The own wire CODECS: modules whose functions, not a type's serde derive, decide the bytes a
    /// registered component rides as (`replicate_with`). Each file's whole source, comments and
    /// whitespace stripped, folds into [`WIRE_TYPES_HASH`] after the type definitions, so a step,
    /// range or bit-layout change re-pins it. Codec tests live in a child file, outside the fold.
    const WIRE_CODEC_SOURCES: &[&str] = &["src/net/wire_quant.rs"];

    /// FNV-1a over every own wire type's normalized definition, then every codec's normalized
    /// source, each terminated by `\n` (so no two distinct sets collide by concatenation) — the same
    /// fold shape as `hash_wire_surface`.
    fn hash_wire_types() -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let mut fold = |text: &str| {
            for byte in text.bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash ^= u64::from(b'\n');
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        };
        for (file, name) in WIRE_TYPE_DEFS {
            let stripped = strip_comments(&read_source(file));
            fold(&normalized_type_def(&stripped, name));
        }
        for file in WIRE_CODEC_SOURCES {
            let stripped = strip_comments(&read_source(file));
            fold(
                &stripped
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>(),
            );
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
    /// and its hash stay green. This pins every own wire definition and the external wire-dependency
    /// versions, so either kind of change requires a re-pin that moves the handshake fingerprint.
    #[test]
    fn wire_types_are_pinned() {
        let actual = hash_wire_types();
        assert_eq!(
            actual, WIRE_TYPES_HASH,
            "a wire-facing type DEFINITION changed (a field / variant / type / derive on one of the \
             own wire types) without a manifest re-pin: skewed builds would connect and \
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

    /// Every pinned wire-manifest value participates in the handshake fingerprint. This exercises
    /// the production fold with one changed input at a time, rather than reimplementing it in test.
    #[test]
    fn fingerprint_couples_every_pinned_wire_manifest_value() {
        let changed_avian = format!("{WIRE_DEP_AVIAN3D}-changed");
        let changed_lightyear = format!("{WIRE_DEP_LIGHTYEAR}-changed");
        let changed_crate = format!("{}-changed", env!("CARGO_PKG_VERSION"));
        let baseline = protocol_fingerprint_for(
            WIRE_SURFACE_HASH,
            WIRE_TYPES_HASH,
            WIRE_DEP_AVIAN3D,
            WIRE_DEP_LIGHTYEAR,
            PROTOCOL_REV,
            env!("CARGO_PKG_VERSION"),
        );
        assert_eq!(
            PROTOCOL_FINGERPRINT, baseline,
            "the handshake must use the pinned wire manifest",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH ^ 1,
                WIRE_TYPES_HASH,
                WIRE_DEP_AVIAN3D,
                WIRE_DEP_LIGHTYEAR,
                PROTOCOL_REV,
                env!("CARGO_PKG_VERSION"),
            ),
            "the ordered wire-surface pin must change the handshake",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH ^ 1,
                WIRE_DEP_AVIAN3D,
                WIRE_DEP_LIGHTYEAR,
                PROTOCOL_REV,
                env!("CARGO_PKG_VERSION"),
            ),
            "the own-wire-type pin must change the handshake",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH,
                &changed_avian,
                WIRE_DEP_LIGHTYEAR,
                PROTOCOL_REV,
                env!("CARGO_PKG_VERSION"),
            ),
            "the avian wire dependency pin must change the handshake",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH,
                WIRE_DEP_AVIAN3D,
                &changed_lightyear,
                PROTOCOL_REV,
                env!("CARGO_PKG_VERSION"),
            ),
            "the lightyear wire dependency pin must change the handshake",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH,
                WIRE_DEP_AVIAN3D,
                WIRE_DEP_LIGHTYEAR,
                PROTOCOL_REV + 1,
                env!("CARGO_PKG_VERSION"),
            ),
            "the protocol revision must change the handshake",
        );
        assert_ne!(
            baseline,
            protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH,
                WIRE_DEP_AVIAN3D,
                WIRE_DEP_LIGHTYEAR,
                PROTOCOL_REV,
                &changed_crate,
            ),
            "the crate version must change the handshake",
        );
    }

    /// The game's damage model sends box doses in HP points before conversion, field strengths and
    /// hit doses in joules, and every pool figure (a crew snapshot's, a record's) in pool units.
    /// Reference-mm record doses used the same scalar types through REV 35, and whole-point pools
    /// and reference-mm field strengths through REV 36, so the protocol revision must separate each
    /// meaning even when the wire bytes are laid out the same; the pen cam's bands, the damage panel
    /// and the HUD's cues rely on it. A unit change that forgets the bump fails here.
    #[test]
    fn every_dose_and_pool_unit_uses_a_distinct_protocol() {
        for (rev, what) in [
            (35, "reference-mm record doses"),
            (36, "whole-point pools and reference-mm field strengths"),
        ] {
            let older = protocol_fingerprint_for(
                WIRE_SURFACE_HASH,
                WIRE_TYPES_HASH,
                WIRE_DEP_AVIAN3D,
                WIRE_DEP_LIGHTYEAR,
                rev,
                env!("CARGO_PKG_VERSION"),
            );
            assert_ne!(
                PROTOCOL_FINGERPRINT, older,
                "a consumer of {what} must not share a protocol"
            );
        }
    }

    /// THE WORLD IS PART OF THE HANDSHAKE. The tag both endpoints hand netcode is not the wire
    /// manifest alone: the map's content digest is folded onto it, so a same-build peer that loaded
    /// different terrain mints a token the other end cannot decrypt (ADR-0018) instead of
    /// connecting and then integrating different ground.
    #[test]
    fn the_map_content_reaches_the_handshake_tag() {
        assert_ne!(
            protocol_id(),
            PROTOCOL_FINGERPRINT,
            "the handshake tag must carry a map term, not the wire manifest alone",
        );
        assert_eq!(
            protocol_id(),
            protocol_id(),
            "the tag is stable per process"
        );
        // Sensitivity, through the production fold rather than a reimplementation of it: two maps
        // are two tags, and so are two registries over one map — a build whose embedded
        // `materials.ron` differs by one value resolves every hard body differently with every
        // asset file identical, and must not connect.
        let registry = crate::substances::MATERIALS_RON;
        let tag = |digest: u64| handshake_tag(digest, registry.as_bytes());
        assert_ne!(tag(1), tag(2), "a different map must be a different tag");
        assert_eq!(
            tag(crate::map::content_digest(&crate::assets::asset_root())),
            protocol_id(),
            "the tag must be the fold of THIS process's map and registry",
        );
        let edited = registry.replacen("26.2", "26.3", 1);
        assert_ne!(
            edited, registry,
            "the fixture edit must land on a registry value"
        );
        assert_ne!(
            handshake_tag(1, edited.as_bytes()),
            tag(1),
            "a registry value moved under identical assets: a different tag",
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

    /// UNATTESTED tick: the `ActionState` carries a command authored for an older tick, or the
    /// unstamped default (`for_tick == 0`, before any input message lands). This is what EVERY way
    /// lightyear can hand back a value that is not this tick's input looks like at the seam —
    /// hold-last extrapolation, a `SameAsPrecedent` gap-fill, a stale un-overwritable entry, or an
    /// `Absent`-frozen `ActionState` (lightyear's `update_action_state` skips the apply, so the
    /// frozen value is re-presented every tick for as long as the freeze lasts). The bridge carries
    /// the MOVEMENT levels and ABSOLUTES through (hold-last is CORRECT for those — a starved stream
    /// keeping the last drive and lay is the right guess, and neither commits anything
    /// irreversible), and fails every CONSUMABLE closed at EVERY lag: a tolerance window of any
    /// width would re-latch a frozen click (one unrequested 88 round per reload) or cycle the MG
    /// after release. Attestation does not care WHY the value is stale.
    #[test]
    fn unattested_tick_keeps_movement_levels_fails_consumables_closed() {
        let held = TankCommand {
            throttle: 0.7,
            steer: -0.3,
            fire_secondary: true,
            aim: Some(AimIntent::World(Vec3::new(1.0, 2.0, 3.0))),
            range: 850.0,
            fire_primary: true,
            crew_swap: Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Loader)),
            select_round: Some(crate::command::RoundSelect {
                weapon: 0,
                step: crate::command::RoundStep::Next,
            }),
            toggle_fuze: Some(0),
            respawn: true,
            for_tick: 0,
        };
        let mut world = World::new();
        let entity = world
            .spawn((ActionState(held), TankCommand::default()))
            .id();

        // Authored for tick 5 and stepped at lags 1..=256 — four seconds at 64 Hz, more than a
        // full 88 reload cycle — then the unstamped default once the tick has moved.
        let rows = (6..262).map(|now| (5, now)).chain([(0, 7)]);
        for (authored, now) in rows {
            world.insert_resource(timeline_at(now));
            world.get_mut::<ActionState<TankCommand>>(entity).unwrap().0 =
                authored_for(authored, held);
            world.get_mut::<TankCommand>(entity).unwrap().clear_edges();
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();

            let cmd = *world.get::<TankCommand>(entity).unwrap();
            let lag = format!("authored for {authored}, stepped at {now}");
            assert_eq!(cmd.throttle, 0.7, "throttle level held ({lag})");
            assert_eq!(cmd.steer, -0.3, "steer level held ({lag})");
            assert_eq!(cmd.aim, held.aim, "aim absolute held ({lag})");
            assert_eq!(cmd.range, 850.0, "range absolute held ({lag})");
            assert!(
                !cmd.fire_secondary,
                "automatic-fire level fails closed ({lag})"
            );
            assert!(!cmd.fire_primary, "fire edge cleared ({lag})");
            assert_eq!(cmd.crew_swap, None, "crew-swap edge cleared ({lag})");
            assert_eq!(cmd.select_round, None, "round-select edge cleared ({lag})");
            assert_eq!(cmd.toggle_fuze, None, "fuze-toggle edge cleared ({lag})");
            assert!(!cmd.respawn, "respawn edge cleared ({lag})");
        }
    }

    /// An attested tick bridges the whole command, EDGES and the held trigger included: a
    /// genuinely-held trigger keeps the `Automatic` cycling (sustained fire is not fabricated
    /// fire), and the own click commits on the tick it was authored for.
    #[test]
    fn attested_tick_fires_edge() {
        let mut world = World::new();
        world.insert_resource(timeline_at(8));
        let entity = world
            .spawn((
                ActionState(authored_for(
                    8,
                    TankCommand {
                        fire_secondary: true,
                        ..fire_click()
                    },
                )),
                TankCommand::default(),
            ))
            .id();

        world
            .run_system_once(bridge_action_state_to_tank_command)
            .unwrap();

        let cmd = world.get::<TankCommand>(entity).unwrap();
        assert!(cmd.fire_primary, "an attested edge must bridge through");
        assert!(
            cmd.fire_secondary,
            "an attested held trigger must bridge through — sustained fire is not fabricated fire",
        );
    }

    /// **The `for_tick` wire cost, measured — not modelled.** lightyear serializes an
    /// `InputMessage` with postcard (`lightyear_serde`'s `default_serialize`), so this encodes a real
    /// `NativeStateSequence<TankCommand>` — the exact `states` payload of one input message — with
    /// the exact same encoding.
    ///
    /// The honest downside of provenance: `for_tick` changes EVERY tick, so it defeats
    /// `Compressed::SameAsPrecedent` run-compression. A message that used to be one full command plus
    /// four one-byte "same as the last one" markers becomes five full commands
    /// (`packet_redundancy: 5`).
    ///
    /// This prints both regimes so the cost is on the record rather than assumed. It asserts only a
    /// CEILING, so it fails loudly if `TankCommand` ever grows fat enough to make the input stream a
    /// real bandwidth problem — at which point the mitigation is to shrink the ATTESTED payload (a
    /// fire-fields-only sub-struct carrying the full stamp), not to drop attestation and not to
    /// narrow the stamp: inputs hold their last value under starvation, so a wrapping stamp
    /// re-attests a held command once per wrap (a `u8` every 256 ticks, DERIVED 4 s at 64 Hz) and
    /// re-latches its fire edges.
    ///
    /// Context for the number: this is UPSTREAM traffic only (client → server), and the client
    /// sends at most one input message per tick (`net::client::install_input_send_cadence`), so
    /// bytes per message × the tick rate is the steady upstream rate at any frame rate (states
    /// payload only; packet framing is extra). The comparison is also somewhat theoretical — the
    /// `for_tick` stamp alone differs every tick (`net::client::stamp_input_tick`), so
    /// run-compression is dead in the shipping configuration whatever `aim` does. It only ever paid off unstamped, for a tank sitting
    /// perfectly still doing nothing, which is exactly when nobody cares about bandwidth.
    #[test]
    fn input_message_wire_cost() {
        use lightyear::prelude::input::native::NativeStateSequence;

        const REDUNDANCY: usize = 5; // lightyear `InputConfig::packet_redundancy` default
        // One input message per tick: `net::client::install_input_send_cadence`.
        const MESSAGES_PER_S: usize = 64;

        // `ActionStateSequence` is the trait carrying `build_from_input_buffer`.
        use lightyear::prelude::input::native::NativeBuffer;
        use lightyear_inputs::input_message::ActionStateSequence;

        fn encoded_len(buffer: &NativeBuffer<TankCommand>, end: Tick) -> usize {
            let seq = NativeStateSequence::<TankCommand>::build_from_input_buffer(
                buffer, REDUNDANCY, end,
            )
            .expect("buffer has entries");
            postcard::to_allocvec(&seq)
                .expect("postcard encodes the sequence")
                .len()
        }

        // A player holding the trigger, perfectly still: the ONLY thing that changes tick to tick is
        // the stamp. This is the worst case for compression, i.e. the best case for the old wire.
        let held = |tick: i32, stamped: bool| {
            ActionState(TankCommand {
                throttle: 1.0,
                fire_secondary: true,
                aim: Some(AimIntent::World(Vec3::new(0.0, 1.5, -40.0))),
                range: 800.0,
                for_tick: if stamped { Tick(tick as u32).0 } else { 0 },
                ..default()
            })
        };

        let mut unstamped = NativeBuffer::<TankCommand>::default();
        let mut stamped = NativeBuffer::<TankCommand>::default();
        // REALISTIC tick values: postcard VARINT-encodes integers, so a `for_tick` of
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

        println!(
            "IDLE (worst case for the stamp — nothing but `for_tick` changes, so run-compression \
             was fully paying off): {before} B -> {after} B (+{delta} B/message) = +{:.1} KB/s \
             upstream per client at {MESSAGES_PER_S} messages/s",
            (delta * MESSAGES_PER_S) as f64 / 1024.0,
        );

        // The REALISTIC regime: the player is aiming, so the `aim` point already differs every tick
        // and every slot was ALREADY a full command. The stamp adds only its own bytes.
        let aiming = |tick: i32, stamped: bool| {
            ActionState(TankCommand {
                throttle: 1.0,
                fire_secondary: true,
                aim: Some(AimIntent::World(Vec3::new(0.01 * tick as f32, 1.5, -40.0))),
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
            "AIMING (the realistic regime — the `aim` point already changed every tick and \
             compression was already dead): {aim_before} B -> {aim_after} B \
             (+{aim_delta} B/message) = +{:.1} KB/s upstream per client at {MESSAGES_PER_S} \
             messages/s",
            (aim_delta * MESSAGES_PER_S) as f64 / 1024.0,
        );

        assert!(
            after <= 256,
            "the attested input payload is {after} B/message ({:.1} KB/s upstream) — TankCommand has \
             grown fat enough that per-tick attestation costs real bandwidth. Shrink the ATTESTED \
             payload (a fire-only sub-struct carrying the full stamp); do NOT drop attestation, \
             which keeps the server from firing rounds nobody asked for, and do NOT narrow the \
             stamp, which a held command would re-attest once per wrap.",
            (after * MESSAGES_PER_S) as f64 / 1024.0,
        );
    }

    /// The crew-swap false-death tripwire (the corruption this slice ends). Builds the exact
    /// post-corruption local state the OLD client path produced on a `Remote` tank — swap A↔B, seat
    /// A (Gunner) LEFT holding the dead occupant and seat B (Loader) LEFT holding the live one after
    /// the client-side flip, `apply_net_health` then re-asserting HP `[full, 0]` by index and
    /// `kill_crew` latching `Dead` onto the seat now holding the live man — while the authority
    /// snapshot still says seat A is the live Gunner and seat B the dead Loader.
    ///
    /// `apply_net_crew` (the fix) must HEAL it: it re-derives HP, occupancy (`home`), and `Dead` from
    /// the authoritative snapshot every tick, so the live crewman ends alive. Public knockout state
    /// is deliberately outside this private-snapshot system.
    #[test]
    fn crew_swap_does_not_false_kill_on_replica() {
        use crate::ballistics::ComponentHealth;
        use crate::damage::{Crewman, Dead, VolumeOf};

        const POINTS: u32 = 1000;
        const FULL: u32 = POINTS * crate::ballistics::POOL_UNITS_PER_HP;

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
                            held: FULL,
                            crew: Some(CrewSnapshot {
                                home: CrewStation::Gunner,
                                dead: false,
                            }),
                        },
                        VolumeSnapshot {
                            held: 0,
                            crew: Some(CrewSnapshot {
                                home: CrewStation::Loader,
                                dead: true,
                            }),
                        },
                    ],
                    swap: None,
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
                ComponentHealth::full(POINTS),
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
                ComponentHealth::holding(POINTS, 0),
                Dead,
                VolumeOf(root),
            ))
            .id();

        world.run_system_once(apply_net_crew).unwrap();

        // Seat A is the live Gunner again: HP restored, occupant home restored, `Dead` cleared.
        assert_eq!(
            world.get::<ComponentHealth>(seat_a).unwrap().held(),
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
    }
}
