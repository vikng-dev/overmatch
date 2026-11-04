//! Shared wire registrations: components, messages, channels, inputs and physics codecs.
//! If a component or input rides the wire, its registration lives here and nowhere else.
//! Both endpoints must register identically; mismatches corrupt replication or refuse connection.
//! The game's damage-model snapshots and shot facts carry game state only.

use avian3d::prelude::{LinearVelocity, Position, RigidBody, Rotation};
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

use super::disclosure::{NetTankStatus, RespawnState, apply_net_tank_status};
use crate::ballistics::ComponentHealth;
use crate::command::{CommandError, DeviceClaim, ParsedTankCommand, TankCommand};
use crate::damage::{CrewStation, Crewman, Dead, LaunchedTurret, PendingSwap, TankVolumes};
use crate::damage_cam::wire::ShotExplanation;
use crate::fingerprint::{fingerprint_field, fnv1a_64};
use crate::state::GameplaySet;
use crate::tank::{
    Controlled as GameControlled, Rig, ServoCommand, ServoIndex, ServoSpec, TankServos, WeaponGate,
    WeaponLoads,
};
use crate::track::sim::{
    DriveSignal, EngineRpm, TankTransmission, TrackDrive, TrackDriveSide, wrapped_phase_delta,
};
use crate::{CombatantId, CombatantLife, ShotId};

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
/// sustained-speed confirmation counter, replicated and rolled back
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
/// REV 23 (naming the victim on an impact): the ricochet message and the impact message each gain
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
/// the `victim` fields on `ricochet message`/`impact message` (REV 23), and the element-grip
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
/// REV 31 (a public impact outcome): the impact message's `penetrated` bool becomes a [`crate::ballistics::Verdict`]
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
/// `Rotation` rides `wire_quant`'s 6-byte smallest-three codec, `Position` its 8-byte millimetre
/// fixed point and the velocities its 6-byte i16 triples; `TrackDrive` keeps the belt state alone,
/// in `wire_quant`'s 10-byte belt codec, and its throttle and steer become the owner-only
/// `DriveSignal`. Surface, own-type graph and REV move.
///
/// REV 41 (walls break): the construction ledger, one globally visible entity whose
/// [`Ledger`](super::construction::Ledger) lists every construction change as `(construction, event,
/// tick)` and replicates by diff, and each fallen group's [`Piece`] on its pose carrier.
/// Surface and own-type graph move.
///
/// REV 42 (#189, holes in walls): the ledger's [`Event`] gains `Hole(chunk, Hole)`, a round's
/// channel or a blast's crater dish in one chunk, drawn only: two `i16` millimetre points about
/// the baked chunk's centroid and a `u16` millimetre radius. Own-type graph and REV move.
///
///
/// REV 45 (#241, the match score): one globally visible entity's
/// [`Scoreboard`](super::score::Scoreboard) lists each combatant's knockouts, losses and its
/// knocked-out life's respawn opening, replicated whole. Surface and own-type graph move.
///
/// REV 46 (#240, the Battle cycle): `BattleState` carries the authority's phase, tick deadlines,
/// Battle number and result. Netcode retains authenticated refusal causes and rechecks admission
/// at challenge completion. The packet format is unchanged; the replicated surface and type graph move.
///
/// [`Piece`]: crate::construction::breakage::Piece
/// [`Event`]: crate::construction::breakage::Event
/// REV 55 (#236): tree motion uses the interpolated debris pose stream. The topology ledger
/// carries only a state word; each freed trunk's TreePiece identifies its map tree and cut.
/// REV 57: environment ledgers and pose carriers carry `WorldGeneration`; the Battle number
/// selects a fresh construction and prevents records and streamed pieces crossing Battles.
/// REV 61 (#238): `WeaponGateState` decodes through its phase rule and refuses paused load work
/// beside a belt with rounds left. Its encoding is unchanged; its definition moves.
/// REV 62 (#247): a fired round carries its birth at the source collider exit beside the muzzle.
/// REV 63 (#238): respawn openings carry the shared SimTick deadline type.
/// REV 64 (#228): the game's damage model carries a positive load rate in the weapon gate.
/// REV 65 (#228): bounded load-work shares replace peer-authored rescalable spans; only load
/// phases carry a rate, and zero-work/cyclic intervals carry none.
/// REV 66 (#258, the game's damage model): `WireCone::strength` and every hit dose of a field's
/// palette carry the energy the field carries (a cone's the energy its round lost times the exit's
/// strength ratio, a burst's its wall's driven energy) instead of HP encoded as joules at the line
/// rate. The scalar types stay unchanged, but a REV 65 client would part its potency bands at the
/// wrong rate, so the protocol fingerprint must differ.
/// REV 67 (#258, the game's damage model): `WireCone::strength` carries a cone's share of the
/// energy its round lost, the debris share times the exit's strength ratio, and a client converts
/// it at the cone's unit over that share; a field below the pool quantum is recorded without
/// doses. The scalar types stay unchanged, but a REV 66 client would read every cone's potency
/// at the wrong unit.
/// REV 68 (#146, driving input): the client sends persistent signed notch intent and hold
/// overrides; the server snaps opposing pedal signs to neutral. Wire types and surface are unchanged.
/// REV 69 (#146, driving model): the main clutch carries Out, Slip or In on the wire.
/// REV 70 (#258, the game's damage model): a kept carrier stops at first resisting contact,
/// so recorded retained work includes its energy and later crossings carry only the core.
/// Returning debris can dose its emitting body after genuine re-entry; every field's receiver
/// contributions are recorded and accumulated before pool rounding.
/// REV 71 (#258, the game's damage model): each public exit leg carries its own drag.
/// REV 72 (#258, the game's damage-model view): first-contact marks retain the flying calibre;
/// packed directions refuse zero at the wire edge, with the same three-byte representation.
/// REV 73: owner-private respawn admission distinguishes waiting space from refused map picks.
/// REV 74 (#146, driving model): support-priced C1-C3 gear selection removes scheduler memory.
/// REV 75 (the game's damage-model view): hull-mark snapshots carry stable serials; diffs evict
/// by physical footprint rather than arrival alone.
/// REV 76 (#146, driving model): the brake law removes the hill-hold latch; a reversal counts its
/// hold at rest.
/// REV 77 (the game's environment destruction model): a freed piece carries its whole hull's mass
/// and meets contacts on a hull 1 mm inside it.
/// REV 78 (#146, driving model): eight canonical fields; construction rest and signed crank.
/// REV 79 (#147, the game's damage model): the pressure contact law removes the entry bend and the
/// overmatch verdict from what a crossing records.
/// REV 80 (the game's environment destruction model): sorted tree carrier cuts partition
/// moving construction; later seams can split and a stump can open a lower cut.
/// REV 81 (#263, the game's damage model): compatibility checkpoint for the plate-capacity
/// rollout. Wire layouts and client derivation are unchanged; the authority exports resolved paths.
/// REV 82 (the game's spawn flow): respawn admission and map-pick refusal are independent owner-private facts.
/// REV 83 (#198, driving model): direct ground grip and nominal-pressure rolling resistance.
/// REV 85 (the game environment destruction model): checked debris-pose receive with a terminal
/// client refusal. Valid pose bytes retain their layout.
/// REV 86 (the game's environment destruction model): construction ledger and piece ordinal
/// fields say `construction`; registrations and type definitions use construction names.
/// Postcard retains the positional field layout; the named wire contract and fingerprint move.
pub const PROTOCOL_REV: u32 = 86;

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

/// Handshake tag: [`PROTOCOL_FINGERPRINT`] plus runtime map, embedded material registry,
/// tank construction and armament digests (ADR-0018). Map terrain/scatter are DERIVED on each
/// peer from files beside the binary; stale files (including half-pulled LFS objects) must
/// refuse before divergent belt contacts, spawn heights or hull collisions reach play. Netcode encrypts the connect token under this
/// tag: a token under another tag cannot decrypt, so the server drops the request.
///
/// Embedded `materials.ron` determines every hard body's resistance but is invisible to the
/// map digest and type manifest. The construction digest includes the embedded tank sheet and
/// adjacent sim artifact, which decide the weapon, stowage and health-volume indices.
///
/// The game's damage model binds fuze modes and round facts from embedded round-type, round and
/// gun registries. Their keys and bytes enter too: handshake-matched receivers can refuse an
/// unresolvable index as malformed input rather than waiting for data skew to resolve.
///
/// Cached per process: map files must stay fixed while running, and the heightmap is tens of MB.
pub fn protocol_id() -> u64 {
    static ID: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ID.get_or_init(|| {
        let root = crate::assets::asset_root();
        handshake_tag(
            crate::map::content_digest(&root),
            crate::substances::MATERIALS_RON.as_bytes(),
            crate::bake::construction_digest(&root),
            armaments_digest(
                crate::rounds::ROUND_TYPES_RON,
                crate::armaments_index::ROUND_FILES,
                crate::armaments_index::GUN_FILES,
            ),
            crate::aim::INPUT_RON.as_bytes(),
        )
    })
}

fn handshake_tag(
    map_digest: u64,
    materials: &[u8],
    tank_digest: u64,
    armaments_digest: u64,
    aim_input: &[u8],
) -> u64 {
    let tag = fingerprint_field(
        PROTOCOL_FINGERPRINT,
        b"map_content",
        &map_digest.to_le_bytes(),
    );
    let tag = fingerprint_field(tag, b"materials_ron", materials);
    let tag = fingerprint_field(tag, b"tank_construction", &tank_digest.to_le_bytes());
    let tag = fingerprint_field(tag, b"armaments", &armaments_digest.to_le_bytes());
    fingerprint_field(tag, b"aim_input", aim_input)
}

/// The game's damage model: fingerprint every embedded armament key and source, in the index's
/// byte-sorted order. These sources are what the shipped registries parse, not a second manifest.
fn armaments_digest(round_types: &str, rounds: &[(&str, &str)], guns: &[(&str, &str)]) -> u64 {
    let mut tag = fingerprint_field(
        crate::fingerprint::FNV_OFFSET_BASIS,
        b"round_types",
        round_types.as_bytes(),
    );
    for (kind, files) in [(b"rounds".as_slice(), rounds), (b"guns".as_slice(), guns)] {
        tag = fingerprint_field(tag, b"registry", kind);
        for (key, text) in files {
            tag = fingerprint_field(tag, b"key", key.as_bytes());
            tag = fingerprint_field(tag, b"ron", text.as_bytes());
        }
    }
    tag
}

/// Version-independent manifest prefix pinned by the tripwire. Routine crate-version bumps
/// need no re-pin; they still change [`PROTOCOL_FINGERPRINT`] and refuse skewed handshakes.
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

/// Fold the crate version last, preserving the single-fold byte order while exposing a
/// version-independent prefix for pinning.
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
#[require(ParsedTankCommand)]
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
    /// The game's damage model: owner-only module repair fraction; absent when idle.
    pub repair: Option<f32>,
}

/// Owner-private authoritative combat state.
///
/// `volumes` follows `health_bearing_volumes` order on both peers. HP, occupancy, death, and the
/// in-flight swap and module repair progress are one atomic snapshot; client UI reads it and
/// never predicts those facts.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetCrew {
    pub volumes: Vec<VolumeSnapshot>,
    /// The in-flight backfill swap, if any: `(source seat, target seat, seconds remaining)`.
    pub swap: Option<(CrewStation, CrewStation, f32)>,
}

/// Authoritative launched-turret pose; client rigs render it without simulating a second launch.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct LaunchedTurretPose(pub Option<(Vec3, Quat)>);

/// Request a persistent respawn XZ, never an immediate teleport or client-chosen Y. The authority
/// validates bounds/standing geometry, resolves ground height and retains the last accepted point.
/// It keys by sending link (`CombatantIds`/`ControlledBy`), so no forged id can move another player.
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
    /// The game's damage model: authoritative collider-exit birth, separate from the muzzle.
    pub birth: Vec3,
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
    /// Authority tick for stable `ShotId` and cosmetic catch-up. The receiver bounds accepted age.
    pub fire_tick: Tick,
}

impl FireEvent {
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
    Outcome(crate::ballistics::FlightOutcome),
}

impl FireVisualFact {
    pub fn shot_id(&self) -> ShotId {
        match self {
            Self::Fire(event) => event.shot_id(),
            Self::Outcome(outcome) => outcome.shot,
        }
    }

    pub fn authority_tick(&self) -> Tick {
        match self {
            Self::Fire(event) => event.fire_tick,
            Self::Outcome(outcome) => Tick(outcome.tick),
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

/// Public reliable channel for individual single-shot [`FireEvent`], the ricochet message, and
/// the impact message facts.
pub struct OutcomeChannel;

/// Owner-private reliable channel for individual [`DamageConfirm`] facts.
pub struct DamageChannel;

/// Recipient-private reliable channel for [`ShotExplanation`]s, each projection to its one
/// recipient. An explanation larger than the link's fragment size travels as fragments, each acked
/// and resent on its own and reassembled on receipt (lightyear's reliable sender). Unordered: an
/// explanation is ordered by its shot, encounter and ticks, never by arrival.
pub struct ShotExplanationChannel;

/// Dedicated ordered-reliable spawn requests: loss must not delay unrelated traffic.
/// The authority keeps the last received choice, so order must preserve clicks A-then-B;
/// unordered delivery could settle on the stale A. Low rate makes head-of-line blocking negligible.
pub struct SpawnChannel;

/// The game's damage model: one `TankVolumes` filter/order for publishing and applying
/// [`NetCrew`] and mapping its indices to world-space hit-feel cues (`net::hit_feel`).
/// `has_health` accepts either query's membership test, including mutable `ComponentHealth`.
pub(crate) fn health_bearing_volumes(
    volumes: &TankVolumes,
    has_health: impl Fn(Entity) -> bool,
) -> impl Iterator<Item = Entity> {
    volumes.iter().filter(move |&v| has_health(v))
}

/// The read side of [`publish_net_crew`]: one health-bearing volume's live facts. `crew`/`crewman`/
/// `dead` are the crew-seat occupancy the snapshot carries (ammo-ness is NOT published — the client
/// reads its own local `Ammo` marker against the replicated HP to spot a cooked-off bin).
#[derive(QueryData)]
struct VolumeSnapshotSource {
    health: &'static ComponentHealth,
    repair: Option<&'static crate::damage::repair::ModuleRepair>,
    crew: Option<&'static CrewStation>,
    crewman: Option<&'static Crewman>,
    dead: Option<&'static Dead>,
}

/// The game's damage model: publish atomic HP/occupancy/swap/repair state in
/// [`health_bearing_volumes`] order after the tick's damage chain (`FixedPostUpdate`).
/// `Without<Remote>` excludes every client tank; role markers do not establish provenance
/// (see [`publish_servo_angles`]). Swap stations replace local seat entities on the wire.
fn publish_net_crew(
    mut tanks: Query<(&TankVolumes, Option<&PendingSwap>, &mut NetCrew), Without<Remote>>,
    sources: Query<VolumeSnapshotSource>,
    stations: Query<&CrewStation>,
) {
    for (volumes, pending, net) in &mut tanks {
        let snapshot = || {
            health_bearing_volumes(volumes, |v| sources.contains(v)).map(|v| {
                let src = sources
                    .get(v)
                    .expect("a health bearer is a snapshot source");
                VolumeSnapshot {
                    held: src.health.held(),
                    repair: src.repair.and_then(|repair| repair.fraction(src.health)),
                    // A crew seat carries its occupant's home (defaulting to the seat before any
                    // backfill) and the authority's monotonic aliveness.
                    crew: src.crew.map(|seat| CrewSnapshot {
                        home: src.crewman.map_or(*seat, |c| c.home),
                        dead: src.dead.is_some(),
                    }),
                }
            })
        };
        // The in-flight swap as STATIONS (stable on the wire, unlike the seat entity ids in
        // `PendingSwap`); dropped if a seat vanished mid-flight, matching `tick_swaps`.
        let swap = pending.and_then(|ps| {
            let (Ok(&a), Ok(&b)) = (stations.get(ps.a), stations.get(ps.b)) else {
                return None;
            };
            Some((a, b, ps.remaining))
        });
        // Compare without allocating a snapshot or triggering at-rest replication resends.
        if net.swap == swap && net.volumes.iter().copied().eq(snapshot()) {
            continue;
        }
        let net = net.into_inner();
        net.volumes.clear();
        net.volumes.extend(snapshot());
        net.swap = swap;
    }
}

/// The write side of [`apply_net_crew`]. The tank root and its volumes are disjoint entities.
#[derive(QueryData)]
#[query_data(mutable)]
struct VolumeSink {
    health: &'static mut ComponentHealth,
    repair: Option<&'static mut crate::damage::repair::ModuleRepair>,
    crewman: Option<&'static mut Crewman>,
    dead: Has<Dead>,
}

/// Apply owner-private authoritative crew state to a replica.
///
/// `NetCrew` is plain replication, so this may run during replay; `Dead` is re-derived from the
/// snapshot, while public life state remains `NetTankStatus`. Run before `DamageConsequences`.
///
/// Applied when its inputs move: a new snapshot, or the tank's volume set changing as its rig is
/// built. Nothing else writes a replica's HP, occupant or `Dead` (the damage chain is gated off on
/// a net client), so between those the sinks already hold the snapshot. A replica whose rig has
/// not attached has no `TankVolumes` yet and is not visited: the snapshot waits on it.
fn apply_net_crew(
    tanks: Query<
        (&TankVolumes, &NetCrew),
        (With<Remote>, Or<(Changed<NetCrew>, Changed<TankVolumes>)>),
    >,
    mut volumes: Query<VolumeSink>,
    repairables: Query<(), crate::damage::repair::Modules>,
    mut commands: Commands,
) {
    for (tank_volumes, net) in &tanks {
        let bearers: Vec<Entity> =
            health_bearing_volumes(tank_volumes, |v| volumes.contains(v)).collect();
        // The rig assembles every health-bearing volume from data in one flush, and the handshake
        // folds the tank's construction, so a snapshot of another length is malformed outside
        // input: refused visibly, never written misaligned.
        if bearers.len() != net.volumes.len() {
            warn!(
                bearers = bearers.len(),
                snapshot = net.volumes.len(),
                "client: refused a crew snapshot that does not index this replica's volumes"
            );
            continue;
        }
        // Parse every peer timer against its prospective pool before applying the atomic record.
        let parsed: Result<Vec<_>, _> = bearers
            .iter()
            .zip(&net.volumes)
            .map(|(&volume, snap)| {
                if snap.repair.is_some() && !repairables.contains(volume) {
                    return Err(crate::damage::repair::RepairSnapshotError::NotModule);
                }
                let sink = volumes
                    .get(volume)
                    .expect("a health bearer was filtered on this query");
                let mut health = *sink.health;
                health.set_held(snap.held);
                crate::damage::repair::ModuleRepair::from_snapshot(snap.repair, &health)
            })
            .collect();
        let repairs = match parsed {
            Ok(repairs) => repairs,
            Err(error) => {
                warn!("client: refused a crew snapshot with invalid module repair: {error}");
                continue;
            }
        };
        for ((volume, snap), parsed_repair) in bearers.into_iter().zip(&net.volumes).zip(repairs) {
            let mut sink = volumes
                .get_mut(volume)
                .expect("a health bearer is a volume sink: `bearers` was filtered on the query");
            sink.health.set_held(snap.held);
            // Non-modules legitimately carry no timer; active work for them was refused above.
            if let Some(mut repair) = sink.repair {
                *repair = parsed_repair;
            }
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

/// The game's damage model: derive cosmetic [`PendingSwap`] seat entities from
/// [`NetCrew::swap`] (ADR-0016), keeping `crew_ui` independent of netcode
/// (`src/net/tests/net_boundary.rs`). `damage::apply_crew_swap_commands` / `damage::tick_swaps`
/// exclude `ClientReplica`, so this timer is never ticked or completed locally. Each fixed tick
/// mirrors the authority, including cancellation/completion.
fn mirror_swap_from_net_crew(
    mut tanks: Query<
        (
            Entity,
            &NetCrew,
            Option<&TankVolumes>,
            Option<&mut PendingSwap>,
        ),
        With<Remote>,
    >,
    seats: Query<&CrewStation>,
    mut commands: Commands,
) {
    for (tank, net, volumes, pending) in &mut tanks {
        match net.swap {
            Some((a_station, b_station, remaining)) => {
                // This tank's own seats, through its volumes; none before its rig is built, and
                // the swap shows once it is.
                let seat_of = |station: CrewStation| {
                    volumes?
                        .iter()
                        .find(|&volume| seats.get(volume).is_ok_and(|seat| *seat == station))
                };
                let (Some(a), Some(b)) = (seat_of(a_station), seat_of(b_station)) else {
                    continue;
                };
                match pending {
                    Some(mut pending) if pending.a == a && pending.b == b => {
                        if pending.remaining != remaining {
                            pending.remaining = remaining;
                        }
                    }
                    _ => {
                        commands
                            .entity(tank)
                            .insert(PendingSwap { a, b, remaining });
                    }
                }
            }
            None => {
                if pending.is_some() {
                    commands.entity(tank).remove::<PendingSwap>();
                }
            }
        }
    }
}

/// A network session cannot advance past Lightyear's numeric timeline. Continuing there would
/// freeze input and replication ordering even if gameplay kept a private wrapping counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NetworkTickExhausted;

impl std::fmt::Display for NetworkTickExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("network timeline exhausted; restart the session before play can continue")
    }
}

/// The sole conversion from the live network timeline to the gameplay deadline domain. The
/// exhausted transport tick is refused, so no independently extended tick can be published.
fn network_gameplay_tick(tick: Tick) -> Result<crate::sim_tick::SimTick, NetworkTickExhausted> {
    if tick.0 == u32::MAX {
        Err(NetworkTickExhausted)
    } else {
        Ok(crate::sim_tick::SimTick::new(tick.0))
    }
}

/// Publish `LocalTimeline` after lightyear_core's `increment_local_tick` (`FixedFirst`) and
/// before every same-tick `GameplaySet` reader. The game's damage model uses this tick for
/// `shooting::fire`'s `ShotId` and `shot_transport::queue_fire`'s catch-up origin; downstream
/// readers trust the publication without comparing the timeline again.
pub(super) fn install_shot_clock(app: &mut App) {
    app.insert_resource(crate::PublishedWeaponClock);
    app.init_resource::<crate::ShotClock>();
    app.init_resource::<crate::WeaponClock>();
    app.add_systems(FixedUpdate, publish_shot_clock.before(GameplaySet));
}

/// Republish the same network tick for every gameplay consumer before the gameplay pass.
/// Offline compositions advance their own wrapping tick; network compositions require an
/// advancing transport timeline and refuse its exhaustion visibly at this boundary.
pub(super) fn publish_shot_clock(
    mut shot_clock: ResMut<crate::ShotClock>,
    mut weapon_clock: ResMut<crate::WeaponClock>,
    timeline: Res<LocalTimeline>,
) {
    let tick = network_gameplay_tick(timeline.tick()).unwrap_or_else(super::harness::refuse_start);
    shot_clock.0 = tick;
    weapon_clock.0 = tick;
}

/// Publish angles in `FixedPostUpdate`, after `drive_servos` (`FixedUpdate`, after `GameplaySet`).
/// `Without<Remote>` excludes every client tank. `Predicted`/`Interpolated` cannot distinguish
/// authority: the server carries both.
fn publish_servo_angles(
    mut tanks: Query<
        (
            &Rig,
            Option<&crate::tank::GunnerYaw>,
            &TankServos,
            &mut ServoAngles,
        ),
        Without<Remote>,
    >,
    servo_slots: Query<&ServoIndex>,
    launched: Query<(), With<LaunchedTurret>>,
) {
    for (rig, yaw, servos, mut angles) in &mut tanks {
        // The game's damage model: a detached assembly follows LaunchedTurretPose, not aim angles.
        if launched.contains(rig.turret) {
            continue;
        }
        let angle = |servo| {
            let slot = servo_slots
                .get(servo)
                .expect("complete spawn gives every attached aim servo its index");
            servos
                .states
                .get(slot.0)
                .expect("complete spawn gives every aim servo a mechanism slot")
                .current()
        };
        angles.set_if_neq(ServoAngles {
            turret: angle(
                yaw.expect("a complete player rig carries its Gunner yaw handle")
                    .0,
            ),
            gun: angle(rig.gun),
        });
    }
}

/// Feed non-owner remote aim handles; the owner's command remains the local input writer's.
fn apply_servo_angles(
    tanks: Query<
        (&ServoAngles, &Rig, Option<&crate::tank::GunnerYaw>),
        (With<Remote>, Without<GameControlled>),
    >,
    mut servos: Query<&mut ServoCommand>,
    launched: Query<(), With<LaunchedTurret>>,
) {
    for (angles, rig, yaw) in &tanks {
        // The game's damage model: launched construction follows its authoritative body pose.
        if launched.contains(rig.turret) {
            continue;
        }
        servos
            .get_mut(
                yaw.expect("a complete player rig carries its Gunner yaw handle")
                    .0,
            )
            .expect("complete spawn gives every attached Gunner yaw servo its command")
            .target = angles.turret;
        servos
            .get_mut(rig.gun)
            .expect("complete spawn gives every attached Gunner pitch servo its command")
            .target = angles.gun;
    }
}

/// The game's damage model: publish the launched turret's solver pose in `FixedPostUpdate`,
/// after `launch_turrets_on_cookoff` and physics. `Rig.turret` joins the root to its free body;
/// before launch, no `LaunchedTurret` matches and the datum stays `None`. `Without<Remote>`
/// excludes clients; `set_if_neq` avoids at-rest replication churn.
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
/// relies on that: the authority owns motion and contacts, and the client owns no constraint
/// state. The presentation boundary (`super::resync`) retains already-presented poses through
/// unknown interpolation history. Avian's bookkeeping defect for a raw body-query gap remains
/// pinned independently by `src/tests/avian_body_query_gap.rs`; its mechanism is recorded in
/// `upstream/avian-body-query-gap-stale-constraints.md`.
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
            if let Ok((mut p, mut r)) = poses.get_mut(rig.turret) {
                p.set_if_neq(Position(position));
                r.set_if_neq(Rotation(rotation));
            }
        } else {
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

/// Cursor-time belt state for cursor-rendered hulls: linear speed and f64 phase along the
/// shorter path across [`crate::track::sim::BELT_PHASE_WRAP_M`]. Interpolation adds no wire type,
/// mode or direction (wire-surface pins guard this); simulated hulls lack `Interpolated`,
/// leaving their sim-written state untouched.
pub(crate) fn track_drive_lerp(start: TrackDrive, end: TrackDrive, t: f32) -> TrackDrive {
    let side = |a: TrackDriveSide, b: TrackDriveSide| TrackDriveSide {
        speed: a.speed + (b.speed - a.speed) * t,
        phase: a.phase + wrapped_phase_delta(a.phase, b.phase) * f64::from(t),
    };
    TrackDrive {
        sides: [
            side(start.sides[0], end.sides[0]),
            side(start.sides[1], end.sides[1]),
        ],
    }
}

/// The owner's drive HUD reads its shaped signal beside the cursor-time belt state, so it is
/// sampled on the same clock: component-wise linear.
pub(crate) fn drive_signal_lerp(start: DriveSignal, end: DriveSignal, t: f32) -> DriveSignal {
    DriveSignal {
        throttle: start.throttle + (end.throttle - start.throttle) * t,
        steer: start.steer + (end.steer - start.steer) * t,
    }
}

/// Ordered registrations, bound to [`plugin`] and [`WIRE_SURFACE_HASH`]. Channels include
/// `Name|Mode|Direction`: changing reliability/order or direction renames no type but changes
/// delivery semantics. Other entries name types; [`WIRE_TYPES_HASH`] covers their serialization.
/// Every wire change also bumps [`PROTOCOL_REV`] for release bookkeeping.
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
    "RespawnState",
    "LaunchedTurretPose",
    "DestructionLedger",
    "HullMarks",
    "DebrisPose",
    "Ledger",
    "Piece",
    "TreePiece",
    "Scoreboard",
    "BattleState",
    "WorldGeneration",
    // Message channels (`Name|Mode|Direction`), followed by their message types.
    "FireChannel|UnorderedUnreliable|ServerToClient",
    "OutcomeChannel|UnorderedReliable|ServerToClient",
    "DamageChannel|UnorderedReliable|ServerToClient",
    "ShotExplanationChannel|UnorderedReliable|ServerToClient",
    "SpawnChannel|OrderedReliable|ClientToServer",
    "FireVisualBatch",
    "FireEvent",
    "FlightOutcome",
    "DamageConfirm",
    "ShotExplanation",
    "SetSpawnPoint",
    // The input protocol — `InputPlugin::<TankCommand>`:
    "TankCommand",
    // Replicated sim-state components, in order:
    "Position",
    "Rotation",
    "LinearVelocity",
    "TrackDrive",
    "DriveSignal",
    "TankTransmission",
    "EngineRpm",
    "WeaponGate",
    "WeaponLoads",
];

/// Pinned hash for the ordered wire surface and a direct handshake-fingerprint input.
const WIRE_SURFACE_HASH: u64 = 0xf6f4efee9a7cda11;

// Wire coverage: `plugin_registrations_match_wire_surface` binds registration order and channel
// semantics to `WIRE_SURFACE`; `wire_surface_is_pinned` binds that list to the handshake. Neither
// sees field-only changes. `WIRE_TYPES_HASH` therefore pins normalized own-type definitions and
// embedded types (`WIRE_TYPE_DEFS`), plus whole custom codecs (`WIRE_CODEC_SOURCES`), stripping
// comments/whitespace. Codec steps, ranges and layouts matter as much as serde fields.
// External Avian fields and Lightyear framing are covered by resolved Cargo.lock versions
// (`WIRE_DEP_AVIAN3D` / `WIRE_DEP_LIGHTYEAR`), since their source is not scanned here. Every pin
// enters the fingerprint directly; compatibility safety does not rely on remembering a REV bump.

/// The pinned hash of the OWN wire-facing type DEFINITIONS (field layout, not just names). Re-pin it
/// whenever a wire-facing struct/enum definition changes; house process also bumps
/// [`PROTOCOL_REV`]. The tripwire prints the new value. See the block above for the coverage model.
const WIRE_TYPES_HASH: u64 = 0x4670e21a88eed9f2;

/// The pinned `Cargo.lock` versions of the external crates whose types ride the wire (avian's
/// replicated physics components; lightyear's wire framing / input protocol). A bump of either can
/// change the on-wire bytes without touching any source in this tree. Re-pinning either version
/// changes the handshake directly; house process also bumps [`PROTOCOL_REV`].
const WIRE_DEP_AVIAN3D: &str = "0.7.0";
const WIRE_DEP_LIGHTYEAR: &str = "0.30.1";

/// Register the exact shared wire surface represented by [`WIRE_SURFACE`].
pub(crate) fn plugin(app: &mut App) {
    install_shot_clock(app);
    app.init_resource::<crate::ballistics::PathClock>();
    app.component::<NetTank>().replicate();
    app.component::<NetBot>().replicate();
    app.component::<CombatantId>().replicate();
    // Immutable spawn identity: which of its combatant's lives a root is, so a record of a shot
    // against an earlier life is never read against this one.
    app.component::<CombatantLife>().replicate();
    // Plain replication, no interpolation: non-owners consume the resolved angle through the servo
    // mechanism (see the type).
    app.component::<ServoAngles>().replicate();
    // Owner-private atomic combat snapshot; public life state is `NetTankStatus`.
    app.component::<NetCrew>().replicate();
    // Minimal public tank-life state. It gives observers death/cookoff presentation without
    // replicating private crew, module, ammunition, or belt facts.
    app.component::<NetTankStatus>().replicate();
    app.component::<RespawnState>().replicate();
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
    // Every server piece's carrier: one pose per piece, written only while it moves, so a sleeping
    // piece sends nothing more. A construction piece's carrier is interpolated, so it draws at the
    // cursor the hulls draw at; the stress lever's are not, and apply on arrival.
    app.component::<super::debris_stress::DebrisPose>()
        .replicate_with(super::debris_stress::pose_replication())
        .add_interpolation_with(super::debris_stress::debris_pose_lerp);
    super::resync::retain_stream::<super::debris_stress::DebrisPose>(app);
    // The construction ledger (one world entity): appended entries ride as diffs, a late joiner takes the
    // list once.
    app.component::<super::construction::Ledger>()
        .replicate_diff();
    // A construction's fallen group on its pose carrier: which construction and which chunks, sent once
    // with the carrier.
    app.component::<crate::construction::breakage::Piece>()
        .replicate();
    app.component::<crate::trees::TreePiece>().replicate();
    // The match score (one world entity): sent whole when a knockout changes it, a late joiner
    // takes it once.
    app.component::<super::score::Scoreboard>().replicate();
    app.component::<super::battle::BattleState>().replicate();
    app.component::<crate::world::WorldGeneration>().replicate();

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
    // `SpawnChannel` owns the reliability/order constraint.
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
    app.register_message::<crate::ballistics::FlightOutcome>()
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
    // OFF, a body spawned with a `Transform` and no `Position` — every scatter construction and trunk,
    // the test course's statics — gets its placeholder `Position` ZEROED, and with avian's
    // transform plugin disabled nothing ever moves it. ON, the hook derives `Position`/`Rotation`
    // from the spawn transform once. No per-frame import is installed either way in this mode.
    app.world_mut()
        .resource_mut::<avian3d::physics_transform::PhysicsTransformConfig>()
        .transform_to_position = true;
    // The hull pose interpolates on the cursor; everything else applies the confirmed value on
    // arrival (per-tick keyframes, so the discrete components step at most one tick apart from
    // the pose they ride beside). Pose and velocity ride `wire_quant`'s codecs, whose module doc states each
    // step, range and error bound.
    app.component::<Position>()
        .replicate_with(super::wire_quant::position_fns())
        .add_linear_interpolation();
    super::resync::retain_stream::<Position>(app);
    app.component::<Rotation>()
        .replicate_with(super::wire_quant::rotation_fns())
        .add_linear_interpolation();
    super::resync::retain_stream::<Rotation>(app);
    app.component::<LinearVelocity>()
        .replicate_with(super::wire_quant::linear_velocity_fns());
    // `track_drive_lerp` keeps the remote belt view on the hull's cursor clock.
    app.component::<TrackDrive>()
        .replicate_with(super::wire_quant::track_drive_fns())
        .add_interpolation_with(track_drive_lerp);
    super::resync::retain_stream::<TrackDrive>(app);
    // The shaped drive signal: owner-only (`disclosure::CombatDisclosure`), read by the owner's
    // drive HUD on the belt state's clock.
    app.component::<DriveSignal>()
        .replicate()
        .add_interpolation_with(drive_signal_lerp);
    super::resync::retain_stream::<DriveSignal>(app);
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
        )
            .in_set(crate::state::SimPhase::Publish),
    );
    app.add_systems(
        FixedUpdate,
        (apply_servo_angles, apply_launched_turret_pose).in_set(GameplaySet),
    );
    // Client: realize owner-private crew details. Public knockout state is applied separately.
    app.add_systems(FixedUpdate, apply_net_crew.in_set(GameplaySet));
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
            .after(crate::command::ParseCommands)
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
    mut tanks: Query<(
        Entity,
        &ActionState<TankCommand>,
        &mut TankCommand,
        &mut ParsedTankCommand,
        Option<&ControlledBy>,
    )>,
    mut refused: Local<bevy::platform::collections::HashMap<Entity, CommandError>>,
) {
    let tick = timeline.tick();
    for (entity, action, mut command, mut parsed, owner) in &mut tanks {
        *parsed = match attest(action.0, tick) {
            Ok(parsed) => {
                refused.remove(&entity);
                parsed
            }
            Err(cause) => {
                if refused.insert(entity, cause) != Some(cause) {
                    warn!(tank = ?entity, link = ?owner.map(|owner| owner.owner), %cause,
                        "net: refused malformed input; filing idle command");
                }
                ParsedTankCommand::default()
            }
        };
        *command = parsed.command();
    }
    // Replicas/links can leave; refusal history belongs only to live input slots.
    refused.retain(|entity, _| tanks.contains(*entity));
}

/// Copy the absolute `ActionState` snapshot, committing consumables only when `for_tick`
/// attests this tick. `net::client::stamp_input_tick` stamps Lightyear's filing tick; inherited,
/// repeated, fabricated or frozen values retain another tick's stamp. Proof, never detection.
pub(super) fn attest(command: TankCommand, tick: Tick) -> Result<ParsedTankCommand, CommandError> {
    ParsedTankCommand::try_from(command).map(|command| command.attest(tick.0))
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

    /// The client's physics world holds no contact constraint, including a launched turret
    /// resting on a hull. This test stages a raw body-query gap independently of the stream
    /// retention boundary, so the no-contact invariant cannot depend on resync handling.
    /// A non-static turret creates a constraint and fails before Avian's body-query-gap defect.
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
                Transform::default(),
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

        // Raw body-query gap: bypass stream retention to test the independent no-contact rule.
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

        // A replica's phase is wrapped on the wire: a pair of samples straddling the wrap
        // interpolates the short way across it, as the authority's unbounded phase moved.
        let wrap = crate::track::sim::BELT_PHASE_WRAP_M;
        let at = |phase: f64| TrackDrive {
            sides: [TrackDriveSide { speed: 4.0, phase }; 2],
        };
        let mid = track_drive_lerp(at(wrap - 0.25), at(0.25), 0.5);
        assert!(
            wrapped_phase_delta(0.0, mid.sides[0].phase).abs() < 1e-9,
            "the midpoint of a pair straddling the wrap sits on the wrap, not half a kilometre \
             back: {}",
            mid.sides[0].phase
        );
    }

    /// Parsed input belongs to the received root's construction, before a local view/sim rig
    /// attaches. A late required component on the raw command alone would violate that boundary.
    #[test]
    fn a_replica_has_parsed_input_before_its_rig_attaches() {
        let mut world = World::new();
        let root = world.spawn(NetTank).id();
        let parsed = world
            .get::<ParsedTankCommand>(root)
            .expect("identity requires parsed input at spawn");
        assert_eq!(parsed.command(), TankCommand::default());
        assert!(
            world.get::<Rig>(root).is_none(),
            "the probe precedes rig attachment"
        );
    }

    /// The live registry must match the explicit protocol even after the Avian integration is
    /// mounted. Its automatic defaults would silently reintroduce unused angular-velocity bytes.
    #[test]
    fn the_live_wire_has_no_unused_angular_velocity() {
        use lightyear::prelude::client::ClientPlugins;
        let mut app = crate::net::test_harness::base_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        plugin(&mut app);
        let registry = app
            .world()
            .resource::<lightyear::prelude::ComponentRegistry>();
        assert!(
            registry.is_registered::<Position>(),
            "the probe sees the installed physics protocol"
        );
        assert!(!registry.is_registered::<avian3d::prelude::AngularVelocity>());
    }

    /// A construction's fallen piece draws at the cursor the hulls and the construction record draw at
    /// (ADR-0037): its carrier's pose is registered for interpolation, and the law between two
    /// samples moves the position along the line and turns along the shorter arc. Unregistered,
    /// a client writes each pose on arrival and the piece leads the round that freed it by the
    /// interpolation delay.
    #[test]
    fn a_piece_pose_is_registered_for_cursor_interpolation() {
        use lightyear::interpolation::prelude::InterpolationRegistry;
        use lightyear::prelude::client::ClientPlugins;

        use crate::net::debris_stress::{DebrisPose, debris_pose_lerp};

        let mut app = crate::net::test_harness::base_app();
        app.add_plugins(ClientPlugins {
            tick_duration: crate::net::test_harness::TICK,
        });
        plugin(&mut app);
        assert!(
            app.world()
                .resource::<InterpolationRegistry>()
                .interpolated::<DebrisPose>()
        );

        let start = DebrisPose {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
        };
        let end = DebrisPose {
            position: Vec3::new(2.0, -4.0, 0.0),
            rotation: Quat::from_rotation_y(-0.2),
        };
        let mid = debris_pose_lerp(start, end, 0.5);
        assert!(mid.position.abs_diff_eq(Vec3::new(1.0, -2.0, 0.0), 1e-6));
        assert!(
            mid.rotation.abs_diff_eq(Quat::from_rotation_y(-0.1), 1e-6),
            "{:?}",
            mid.rotation
        );
        // `-q` is `q`'s turn: the law takes the shorter arc to it, never the long way round.
        let flipped = DebrisPose {
            rotation: -end.rotation,
            ..end
        };
        let mid = debris_pose_lerp(start, flipped, 0.5).rotation;
        assert!(
            mid.angle_between(Quat::from_rotation_y(-0.1)) < 1e-3,
            "{mid:?}"
        );
    }

    /// An assembly need not be a servo. The remote angle stream must read the declared aim
    /// handles, and a launched assembly must stop publishing even when its yaw is a child.
    #[test]
    fn public_angles_address_yaw_independently_of_the_assembly() {
        let mut world = World::new();
        let assembly = world.spawn_empty().id();
        let yaw = world.spawn(ServoIndex(0)).id();
        let pitch = world.spawn(ServoIndex(1)).id();
        // Fixture live bearings; the interface publishes each slot without integrating a law.
        let root = world
            .spawn((
                Rig {
                    hull: assembly,
                    turret: assembly,
                    gun: pitch,
                    muzzle: pitch,
                },
                crate::tank::GunnerYaw(yaw),
                TankServos {
                    states: vec![
                        crate::tank::ServoState::test_new(0.7, 0.7, 0.0),
                        crate::tank::ServoState::test_new(-0.2, -0.2, 0.0),
                    ],
                },
                ServoAngles::default(),
            ))
            .id();
        world.run_system_once(publish_servo_angles).unwrap();
        let angles = world.get::<ServoAngles>(root).unwrap();
        assert_eq!((angles.turret, angles.gun), (0.7, -0.2));
        world.entity_mut(assembly).insert(LaunchedTurret);
        world.entity_mut(yaw).remove::<ServoIndex>();
        world.run_system_once(publish_servo_angles).unwrap();
        let angles = world.get::<ServoAngles>(root).unwrap();
        assert_eq!((angles.turret, angles.gun), (0.7, -0.2));
    }

    /// The public `ServoAngles` drive every remote tank's servo targets, but the owner's belong to
    /// `aim::drive_aim_servos` alone. The OWN tank is `Remote` like every replicated tank, so only
    /// the owner marker keeps this pump off it — mutate `Without<GameControlled>` away and the
    /// replicated angle overwrites the local intent here.
    #[test]
    fn public_servo_angles_skip_the_tank_that_holds_the_input_slot() {
        fn spawn_tank(world: &mut World, owned: bool) -> (Entity, Entity) {
            let turret = world.spawn_empty().id();
            let yaw = world.spawn(ServoCommand::default()).id();
            let gun = world.spawn(ServoCommand::default()).id();
            let hull = world.spawn_empty().id();
            let muzzle = world.spawn_empty().id();
            let mut root = world.spawn((
                Remote,
                crate::tank::GunnerYaw(yaw),
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
            (yaw, gun)
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
            .split_once("\n}")
            .expect("the attachment function's closing brace bounds its body")
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

    /// Registration changes require a re-pin plus [`PROTOCOL_REV`] bump. The pin directly
    /// changes the handshake; [`protocol_id`] also includes content digests.
    #[test]
    fn wire_surface_is_pinned() {
        let actual = hash_wire_surface();
        assert_eq!(
            actual, WIRE_SURFACE_HASH,
            "wire surface changed: bump PROTOCOL_REV and re-pin WIRE_SURFACE_HASH to {actual:#018x}",
        );
    }

    // Source-scan tripwires (`CARGO_MANIFEST_DIR`), like `net_boundary`: line/substring scanners,
    // not Rust parsers. Strip comments/whitespace to ignore prose/layout; prefer harmless false
    // trips requiring a re-pin over false passes admitting wire skew.

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

    /// Bind [`WIRE_SURFACE`] to actual registrations, including channel mode/direction. Editing
    /// only `plugin` fails here; updating the list trips `wire_surface_is_pinned`, whose re-pin
    /// changes the handshake even without the required [`PROTOCOL_REV`] bump.
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

    /// Own wire-type graph as `(source, name)`, including embedded types. External types use
    /// dependency pins. Each embedded enum needs a row: postcard encodes declaration indices,
    /// so reordered variants change meaning without changing the embedding definition.
    const WIRE_TYPE_DEFS: &[(&str, &str)] = &[
        ("src/net/protocol.rs", "NetTank"),
        ("src/track/sim.rs", "TrackDrive"),
        ("src/track/sim.rs", "TrackDriveSide"),
        ("src/track/sim.rs", "DriveSignal"),
        ("src/track/sim.rs", "TankTransmission"),
        ("src/track/sim.rs", "EngineRpm"),
        ("src/track/transmission.rs", "TransmissionState"),
        ("src/track/transmission.rs", "ClutchState"),
        ("src/tank/model.rs", "WeaponGate"),
        ("src/tank/model.rs", "WeaponGateState"),
        ("src/tank/model.rs", "LoadRate"),
        ("src/tank/model.rs", "LoadShare"),
        ("src/tank/model.rs", "LoadWork"),
        ("src/tank/model.rs", "LoadWorkWire"),
        ("src/tank/model.rs", "WeaponTiming"),
        ("src/sim_tick.rs", "SimTick"),
        ("src/sim_tick.rs", "TickSpan"),
        ("src/tank/model.rs", "WeaponLoads"),
        ("src/tank/model.rs", "WeaponLoad"),
        ("src/net/protocol.rs", "NetBot"),
        ("src/lib.rs", "CombatantId"),
        ("src/lib.rs", "CombatantLife"),
        ("src/net/protocol.rs", "ServoAngles"),
        ("src/net/protocol.rs", "NetCrew"),
        ("src/net/disclosure.rs", "NetTankStatus"),
        ("src/net/disclosure.rs", "RespawnState"),
        ("src/net/disclosure.rs", "SpawnPickRefusal"),
        ("src/net/protocol.rs", "VolumeSnapshot"),
        ("src/net/protocol.rs", "CrewSnapshot"),
        ("src/net/protocol.rs", "LaunchedTurretPose"),
        ("src/net/destruction.rs", "DestructionLedger"),
        ("src/net/destruction.rs", "LedgerEntry"),
        ("src/hull_marks.rs", "HullMarks"),
        ("src/hull_marks.rs", "HullMark"),
        ("src/hull_marks.rs", "PackedDirection"),
        ("src/hull_marks.rs", "MarkCaliber"),
        ("src/substances/feedback.rs", "SurfaceFeedback"),
        ("src/hull_marks.rs", "MarkKind"),
        ("src/net/debris_stress.rs", "DebrisPose"),
        ("src/net/debris_stress/pose.rs", "WirePose"),
        ("src/net/construction.rs", "Ledger"),
        ("src/construction/breakage.rs", "Entry"),
        ("src/construction/breakage.rs", "Event"),
        ("src/construction/breakage.rs", "Hole"),
        ("src/construction/breakage.rs", "Piece"),
        ("src/trees/motion.rs", "TreePiece"),
        ("src/net/score.rs", "Scoreboard"),
        ("src/net/battle.rs", "BattleState"),
        ("src/world.rs", "WorldGeneration"),
        ("src/net/battle.rs", "BattlePhase"),
        ("src/net/battle.rs", "LateJoin"),
        ("src/net/battle.rs", "BattleOutcome"),
        ("src/net/score.rs", "Standing"),
        ("src/net/score.rs", "Respawn"),
        ("src/net/protocol.rs", "FireChannel"),
        ("src/net/protocol.rs", "OutcomeChannel"),
        ("src/net/protocol.rs", "DamageChannel"),
        ("src/net/protocol.rs", "ShotExplanationChannel"),
        ("src/net/protocol.rs", "SpawnChannel"),
        ("src/net/protocol.rs", "FireVisualBatch"),
        ("src/net/protocol.rs", "FireVisualFact"),
        ("src/net/protocol.rs", "FireEvent"),
        ("src/spec.rs", "FireMechanism"),
        ("src/ballistics/path.rs", "FlightOutcome"),
        ("src/ballistics/path.rs", "Span"),
        ("src/ballistics/path.rs", "PathImpact"),
        ("src/ballistics.rs", "RoundBody"),
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
    /// registered component rides as (`replicate_with`), or the custom netcode refusal code an
    /// application decodes. Each file's whole source, comments and whitespace stripped, folds
    /// into [`WIRE_TYPES_HASH`] after the type definitions. Codec tests live outside this fold.
    const WIRE_CODEC_SOURCES: &[&str] = &[
        "src/net/wire_quant.rs",
        "src/net/admission.rs",
        "src/net/debris_stress/pose.rs",
    ];

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

    /// Field/variant/derive changes can leave [`WIRE_SURFACE`] unchanged. Pin own definitions
    /// and external dependency versions so either change requires a handshake-changing re-pin.
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
    /// hit doses in the joules the field carries, and every pool figure (a crew snapshot's, a
    /// record's) in pool units. Reference-mm record doses used the same scalar types through REV
    /// 35, whole-point pools and reference-mm field strengths through REV 36, and field strengths
    /// as HP encoded at the line rate through REV 65, so the protocol revision must separate each
    /// meaning even when the wire bytes are laid out the same; the pen cam's bands, the damage panel
    /// and the HUD's cues rely on it. A unit change that forgets the bump fails here.
    #[test]
    fn every_dose_and_pool_unit_uses_a_distinct_protocol() {
        for (rev, what) in [
            (35, "reference-mm record doses"),
            (36, "whole-point pools and reference-mm field strengths"),
            (65, "field strengths encoded at the line rate"),
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

    /// Same-build peers with different world/construction data must refuse at connect (ADR-0018).
    /// The game's damage model relies on this when refusing unresolved weapon/volume indices.
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
        let root = crate::assets::asset_root();
        let tank = crate::bake::construction_digest(&root);
        let armaments = armaments_digest(
            crate::rounds::ROUND_TYPES_RON,
            crate::armaments_index::ROUND_FILES,
            crate::armaments_index::GUN_FILES,
        );
        let tag = |digest: u64| {
            handshake_tag(
                digest,
                registry.as_bytes(),
                tank,
                armaments,
                crate::aim::INPUT_RON.as_bytes(),
            )
        };
        assert_ne!(tag(1), tag(2), "a different map must be a different tag");
        assert_eq!(
            tag(crate::map::content_digest(&root)),
            protocol_id(),
            "the tag must be the fold of THIS process's map, registry and tank",
        );
        assert_ne!(
            handshake_tag(
                1,
                registry.as_bytes(),
                tank ^ 1,
                armaments,
                crate::aim::INPUT_RON.as_bytes()
            ),
            tag(1),
            "another tank construction over the same world: a different tag",
        );
        assert_ne!(
            handshake_tag(
                1,
                registry.as_bytes(),
                tank,
                armaments,
                b"changed aim construction"
            ),
            tag(1),
            "input admission policy must agree before peers connect",
        );
        let edited = registry.replacen("26.2", "26.3", 1);
        assert_ne!(
            edited, registry,
            "the fixture edit must land on a registry value"
        );
        assert_ne!(
            handshake_tag(
                1,
                edited.as_bytes(),
                tank,
                armaments,
                crate::aim::INPUT_RON.as_bytes()
            ),
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

    /// The game's damage model: a shot's identity and the tick its fire ships under are the
    /// published clock, which `shot_transport::queue_fire` trusts without a cross-check against
    /// the timeline. So the publication precedes every gameplay reader of the same fixed tick.
    /// The mutant that drops the ordering leaves the probe ambiguous with the publisher, which
    /// this schedule refuses to build. FIXTURE DATA: the timeline is stepped from tick 41.
    #[test]
    fn the_clock_is_published_before_every_gameplay_reader() {
        use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};

        fn probe(clock: Res<crate::ShotClock>, timeline: Res<LocalTimeline>) {
            assert_eq!(
                clock.0.raw(),
                timeline.tick().0,
                "a gameplay reader sees this tick's publication"
            );
        }

        let mut app = App::new();
        install_shot_clock(&mut app);
        app.insert_resource(timeline_at(41));
        app.add_systems(FixedUpdate, probe.in_set(GameplaySet));
        app.edit_schedule(FixedUpdate, |schedule| {
            schedule.set_build_settings(ScheduleBuildSettings {
                ambiguity_detection: LogLevel::Error,
                ..ScheduleBuildSettings::new()
            });
        });
        for _ in 0..3 {
            app.world_mut()
                .resource_mut::<LocalTimeline>()
                .apply_delta(1);
            app.world_mut().run_schedule(FixedUpdate);
        }
        assert_eq!(app.world().resource::<crate::ShotClock>().0.raw(), 44);
    }

    /// The game's damage model: shot identity, deadline gates and replica catch-up all read
    /// the transport timeline. Joining processes cannot invent a private epoch at its end.
    #[test]
    fn network_publication_has_no_private_epoch_and_refuses_exhaustion() {
        let mut world = World::new();
        let mut timeline = timeline_at(i32::MAX);
        timeline.apply_delta(i32::MAX);
        world.insert_resource(timeline);
        world.init_resource::<crate::ShotClock>();
        world.init_resource::<crate::WeaponClock>();
        let mut schedule = Schedule::default();
        schedule.add_systems(publish_shot_clock);
        for _ in 0..3 {
            schedule.run(&mut world);
            let transport = world.resource::<LocalTimeline>().tick();
            let now = world.resource::<crate::WeaponClock>().0;
            assert_eq!(now.raw(), transport.0);
            assert_eq!(world.resource::<crate::ShotClock>().0, now);
            assert_eq!(
                network_gameplay_tick(transport),
                Ok(now),
                "a new process reads the same tick without local history"
            );
        }
        world.resource_mut::<LocalTimeline>().apply_delta(1);
        assert_eq!(
            network_gameplay_tick(world.resource::<LocalTimeline>().tick()),
            Err(NetworkTickExhausted)
        );
    }

    /// The game's damage model: a registry-only fuze or gun edit must refuse at connect even
    /// when tank construction and wire types match. Receiver fire bindings rely on this guarantee.
    #[test]
    fn every_armament_registry_input_reaches_the_handshake() {
        // Fixture bytes test the generic source fingerprint, not any asset's physical values.
        let rounds = [("round", "round source")];
        let guns = [("gun", "gun source")];
        let base = armaments_digest("round types", &rounds, &guns);
        let tag = |digest| handshake_tag(1, b"materials", 2, digest, b"aim input");
        for changed in [
            armaments_digest("changed types", &rounds, &guns),
            armaments_digest("round types", &[("round", "changed source")], &guns),
            armaments_digest("round types", &[("renamed round", "round source")], &guns),
            armaments_digest("round types", &rounds, &[("gun", "changed source")]),
            armaments_digest("round types", &rounds, &[("renamed gun", "gun source")]),
        ] {
            assert_ne!(tag(base), tag(changed));
        }
    }

    /// The game's damage model: even an attested malformed command files idle input. A peer
    /// cannot poison movement, a servo or an action by making another field appear valid.
    #[test]
    fn malformed_attested_commands_file_idle_at_the_link_boundary() {
        let mut world = World::new();
        world.init_resource::<LocalTimeline>();
        let link = world.spawn_empty().id();
        let slot = world
            .spawn((
                ActionState(TankCommand::default()),
                TankCommand::default(),
                ControlledBy {
                    owner: link,
                    lifetime: default(),
                },
            ))
            .id();
        for mut malformed in [
            TankCommand {
                throttle: f32::NAN,
                ..default()
            },
            TankCommand {
                steer: 2.0,
                ..default()
            },
            TankCommand {
                range: -1.0,
                ..default()
            },
            TankCommand {
                aim: Some(AimIntent::World(Vec3::splat(f32::NAN))),
                ..default()
            },
            TankCommand {
                aim: Some(AimIntent::HullLocal(Vec3::splat(
                    *crate::aim::AIM_LIMIT * 2.0,
                ))),
                ..default()
            },
        ] {
            malformed.fire_primary = true;
            malformed.fire_secondary = true;
            world.get_mut::<ActionState<TankCommand>>(slot).unwrap().0 = malformed;
            world
                .run_system_once(bridge_action_state_to_tank_command)
                .unwrap();
            assert_eq!(
                *world.get::<TankCommand>(slot).unwrap(),
                TankCommand::default()
            );
            let parsed = world.get::<ParsedTankCommand>(slot).unwrap();
            assert_eq!(parsed.command(), TankCommand::default());
            assert!(parsed.aim().is_none());
        }
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

    /// The game's damage model: every stale input form must fail consumables closed at every lag
    /// while holding movement levels and aim/range absolutes. These include hold-last,
    /// `SameAsPrecedent` gap-fill, stale un-overwritable entries and `Absent` freezing
    /// (`update_action_state` skips application). A tolerance window would re-latch a frozen
    /// click each reload or keep automatic fire cycling after release; provenance needs no
    /// diagnosis of why input is stale.
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
            repair: true,
            for_tick: 0,
        };
        let mut world = World::new();
        let entity = world
            .spawn((ActionState(held), TankCommand::default()))
            .id();

        // Fixture: authored at tick 5, lags 1..=256 (DERIVED four seconds at 64 Hz), plus
        // an unstamped default after the clock moves. No reload mechanism is mounted.
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
            assert!(!cmd.repair, "repair edge cleared ({lag})");
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

    /// MEASURED input-state payload cost using real `NativeStateSequence<TankCommand>` postcard
    /// encoding (`lightyear_serde::default_serialize`), excluding packet framing. `for_tick`
    /// changes each tick and defeats `SameAsPrecedent`: at `packet_redundancy: 5`, an idle stream
    /// needs five full commands instead of one plus four one-byte repeat markers. Aiming already
    /// changes every slot, so the stamp adds only its own bytes there. Both regimes print below.
    ///
    /// The ceiling guards bandwidth growth. Shrink the attested payload (a fire-only substruct
    /// with full stamp) rather than removing/narrowing provenance: a held input re-attests at
    /// stamp wrap (`u8`: 256 ticks, DERIVED 4 s at 64 Hz), re-latching edges. At most one message
    /// per tick (`net::client::install_input_send_cadence`) makes payload bytes × tick rate the
    /// steady client→server payload rate regardless of frame rate. Unstamped compression pays
    /// only for unchanged commands; shipping stamps defeat it even without aim movement.
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

        // Fixed-command fixture: only the stamp changes, maximizing its compression penalty.
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
        // Postcard varints: tick 100 costs one byte; fixture tick 100_000 (DERIVED ≈26 min at
        // 64 Hz) costs several. Low ticks would understate stamp cost.
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

        // Moving-aim fixture: commands already differ each tick; only stamp bytes are extra.
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

    /// The game's damage model: the owner's atomic snapshot must carry both the reduced
    /// module's held share and its progress, then the full pool and idle timer together. Either a missing
    /// publish or a missing apply strands the owner's panel and fails this round trip.
    #[test]
    fn module_repair_progress_and_completion_round_trip_in_private_snapshot() {
        use crate::damage::repair::ModuleRepair;
        use crate::damage::{FunctionRole, VolumeOf};
        let mut world = World::new();
        let source = world.spawn(NetCrew::default()).id();
        // Fixture: 21 HP capacity, 1 HP held, independent of a tank asset.
        let mut health = ComponentHealth::full(21);
        health.set_held(crate::ballistics::POOL_UNITS_PER_HP);
        let repair = ModuleRepair::from_snapshot(Some(0.25), &health)
            .expect("fixture repair is active on a reduced module");
        let module = world
            .spawn((VolumeOf(source), FunctionRole::Engine, health, repair))
            .id();
        world.run_system_once(publish_net_crew).unwrap();
        let snapshot = world.get::<NetCrew>(source).unwrap().clone();
        assert_eq!(snapshot.volumes[0].repair, Some(0.25));
        let replica = world.spawn((Remote, snapshot)).id();
        let view_module = world
            .spawn((
                VolumeOf(replica),
                FunctionRole::Engine,
                ComponentHealth::full(21),
            ))
            .id();
        world.run_system_once(apply_net_crew).unwrap();
        let view_health = world.get::<ComponentHealth>(view_module).unwrap();
        assert_eq!(view_health.held(), crate::ballistics::POOL_UNITS_PER_HP);
        assert_eq!(
            world
                .get::<ModuleRepair>(view_module)
                .unwrap()
                .fraction(view_health),
            Some(0.25)
        );
        world.get_mut::<ComponentHealth>(module).unwrap().refill();
        world.get_mut::<ModuleRepair>(module).unwrap().elapsed = None;
        world.run_system_once(publish_net_crew).unwrap();
        *world.get_mut::<NetCrew>(replica).unwrap() = world.get::<NetCrew>(source).unwrap().clone();
        world.run_system_once(apply_net_crew).unwrap();
        let health = world.get::<ComponentHealth>(view_module).unwrap();
        assert_eq!(health.held(), health.capacity());
        assert!(
            world
                .get::<ModuleRepair>(view_module)
                .unwrap()
                .elapsed
                .is_none()
        );
    }

    /// The game's damage model: a full pool has no repair work denominator. The client must
    /// refuse an active repair beside it atomically, preserving the previous valid snapshot.
    #[test]
    fn a_full_pool_cannot_arrive_with_active_repair_progress() {
        use crate::damage::{FunctionRole, VolumeOf};
        let mut world = World::new();
        let full = ComponentHealth::full(21); // Fixture pool.
        let root = world
            .spawn((
                Remote,
                NetCrew {
                    volumes: vec![VolumeSnapshot {
                        held: full.capacity(),
                        repair: Some(0.5),
                        crew: None,
                    }],
                    swap: None,
                },
            ))
            .id();
        let initial = ComponentHealth::holding(21, crate::ballistics::POOL_UNITS_PER_HP);
        let module = world
            .spawn((VolumeOf(root), FunctionRole::Engine, initial))
            .id();
        world.run_system_once(apply_net_crew).unwrap();
        assert_eq!(*world.get::<ComponentHealth>(module).unwrap(), initial);
        assert!(
            world
                .get::<crate::damage::repair::ModuleRepair>(module)
                .unwrap()
                .elapsed
                .is_none()
        );
    }

    /// The game's damage model: peer repair metadata cannot be silently discarded on a
    /// non-module while the rest of its record changes HP. Refuse the whole record instead.
    #[test]
    fn repair_progress_on_a_non_module_is_refused_atomically() {
        use crate::damage::VolumeOf;
        let mut world = World::new();
        let initial = ComponentHealth::holding(21, crate::ballistics::POOL_UNITS_PER_HP); // Fixture.
        let root = world
            .spawn((
                Remote,
                NetCrew {
                    volumes: vec![VolumeSnapshot {
                        held: 2 * initial.held(),
                        repair: Some(0.5),
                        crew: None,
                    }],
                    swap: None,
                },
            ))
            .id();
        let volume = world.spawn((VolumeOf(root), initial)).id();
        world.run_system_once(apply_net_crew).unwrap();
        assert_eq!(*world.get::<ComponentHealth>(volume).unwrap(), initial);
    }

    /// The game's damage model: a new authoritative snapshot must replace mismatched local
    /// seat pools, occupant homes and `Dead` markers atomically. Fixture: local seats hold swapped
    /// homes and both `Dead` markers, while authority has a full Gunner pool and a knocked-out
    /// Loader. Local swapping followed by index-based HP application and latched knockout markers
    /// would mismatch occupant and pool. Public knockout state is outside this system.
    #[test]
    fn crew_swap_does_not_false_kill_on_replica() {
        use crate::ballistics::ComponentHealth;
        use crate::damage::{Crewman, Dead, VolumeOf};

        const POINTS: u32 = 1000;
        const FULL: u32 = POINTS * crate::ballistics::POOL_UNITS_PER_HP;

        let mut world = World::new();

        // The game's damage-model fixture: authority has full Gunner / empty Loader pools, no
        // pending swap. `VolumeOf` populates `TankVolumes` in seat spawn order.
        let root = world
            .spawn((
                Remote,
                NetCrew {
                    volumes: vec![
                        VolumeSnapshot {
                            repair: None,
                            held: FULL,
                            crew: Some(CrewSnapshot {
                                home: CrewStation::Gunner,
                                dead: false,
                            }),
                        },
                        VolumeSnapshot {
                            repair: None,
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

        // The game's damage-model fixture: seat A has full HP but the wrong home and `Dead`.
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

        // The game's damage-model fixture: seat B has empty HP and `Dead`, but the wrong home.
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

        assert_eq!(
            world.get::<Crewman>(seat_b).unwrap().home,
            CrewStation::Loader,
        );
        assert!(world.get::<Dead>(seat_b).is_some(), "seat B stays dead");
    }
}
