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

use super::disclosure::{NetTankStatus, apply_net_tank_status};
use crate::ballistics::ComponentHealth;
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{
    CrewStation, Crewman, DamageConsequences, Dead, LaunchedTurret, PendingSwap, TankVolumes,
};
use crate::driving::DriveState;
use crate::spec::FireMode;
use crate::state::GameplaySet;
use crate::tank::{
    Muzzle, Rig, ServoCommand, ServoIndex, ServoSpec, TankRoot, TankSim, Weapon, WeaponIndex,
};
use crate::{CombatantId, ShotId};

// ---------------------------------------------------------------------------
// Protocol compatibility guard
// ---------------------------------------------------------------------------
// Replicon registration order is wire compatibility. Both netcode endpoints must use
// `PROTOCOL_FINGERPRINT` as their `protocol_id`; ADR-0018 and `wire_surface_is_pinned` own the
// compatibility guard.

/// Bump with [`WIRE_SURFACE_HASH`] for every wire-surface change.
pub const PROTOCOL_REV: u32 = 11;

/// Compatibility tag derived from the protocol revision and crate version.
pub const PROTOCOL_FINGERPRINT: u64 = protocol_fingerprint();

/// Const-evaluable FNV-1a fold for the compatibility tag; it is not a security primitive.
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

/// Fold the protocol revision and crate version into one compatibility tag.
const fn protocol_fingerprint() -> u64 {
    let rev_bytes = PROTOCOL_REV.to_le_bytes();
    let after_rev = fnv1a_64(0xcbf2_9ce4_8422_2325, &rev_bytes);
    fnv1a_64(after_rev, env!("CARGO_PKG_VERSION").as_bytes())
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
    /// The volume's live `ComponentHealth.current`.
    pub hp: f32,
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

/// Correlated authoritative belt state. `belt` and `swap_remaining` must be applied atomically so a
/// client cannot locally complete a server-owned belt swap.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct BeltSnapshot {
    /// Rounds left on the belt (`WeaponState::belt_remaining`); `0` = a swap is in flight.
    pub belt: u32,
    /// The belt-swap countdown (`reload_remaining` while `belt == 0`), else `0.0`. See the type doc.
    pub swap_remaining: f32,
}

/// Owner-private authoritative belt state. Entries follow `TankSim::weapons` order on both peers.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetBelts {
    /// `Some(BeltSnapshot)` for a belt-fed weapon, `None` for a `Single`, in `TankSim::weapons` order.
    pub weapons: Vec<Option<BeltSnapshot>>,
}

/// A public, loss-tolerant reconstruction of an authoritative shot. The receiver maps `shooter` for
/// recoil and self-echo suppression, validates the raw bore, and reconstructs a cosmetic shell.
#[derive(Clone, Serialize, Deserialize)]
pub struct FireEvent {
    pub origin: Vec3,
    /// Raw bore direction, validated before conversion to `Dir3` on receipt.
    pub direction: Vec3,
    pub speed: f32,
    pub caliber: f32,
    pub mass: f32,
    pub mechanism: crate::spec::FireMechanism,
    /// Authoritative tracer selection.
    pub tracer: bool,
    /// Entity-mapped firing root, used for recoil and self-echo suppression.
    pub shooter: Entity,
    /// Stable match-local identity; unlike `shooter`, this is not entity-mapped.
    pub combatant: CombatantId,
    /// Weapon slot in the local rig; receipt code bounds-checks it.
    pub weapon: u8,
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
    /// The server's penetration verdict — gates the flame lick exactly as the local read does.
    pub penetrated: bool,
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
            }
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

/// Pin each replica's belt-fed weapon to authority state.
///
/// `belt` and dry-belt `swap_remaining` must update together. This runs after fire/reload and during
/// replay, so local prediction cannot complete a server-owned belt swap. `Single` weapons are untouched.
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

/// Republish the network timeline as net-neutral sim vocabulary before every gameplay tick. It runs
/// during rollback too: `LocalTimeline` is the replayed tick there, so `shooting::fire` re-derives
/// the same `ShotId` even though [`crate::Replaying`] suppresses its cosmetic `FireShell` trigger.
fn publish_shot_clock(mut shot_clock: ResMut<crate::ShotClock>, timeline: Res<LocalTimeline>) {
    shot_clock.0 = timeline.tick().0;
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

/// Shared rollback thresholds for Lightyear and `net::watchdog`.
///
/// Position and rotation own normal reconciliation. Velocity is deliberately a gross-desync gate;
/// all conditions must remain registered because Lightyear otherwise falls back to float equality.
pub(crate) const ROLLBACK_POSITION_M: f32 = 0.05;
pub(crate) const ROLLBACK_ROTATION_RAD: f32 = 0.05;
pub(crate) const ROLLBACK_VELOCITY: f32 = 1.0;

// The registered conditions and watchdog share these metrics and thresholds.

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

/// Ordered wire registrations. Keep this list aligned with [`plugin`]; the test requires a matching
/// [`WIRE_SURFACE_HASH`] and [`PROTOCOL_REV`] bump for every add, removal, rename, or reorder.
#[cfg(test)]
const WIRE_SURFACE: &[&str] = &[
    // Plain-replicated markers/snapshots — `app.component::<_>().replicate()`, in order:
    "NetTank",
    "NetBot",
    "CombatantId",
    "ServoAngles",
    "NetCrew",
    "NetTankStatus",
    "LaunchedTurretPose",
    "NetBelts",
    // Shot transport channels, followed by their message types.
    "FireChannel",
    "OutcomeChannel",
    "DamageChannel",
    "FireVisualBatch",
    "FireEvent",
    "RicochetKeyframe",
    "ImpactConfirm",
    "DamageConfirm",
    // The input protocol — `InputPlugin::<TankCommand>`:
    "TankCommand",
    // Predicted+rollback avian components — `app.component::<_>().replicate().predict()`, in order:
    "Position",
    "Rotation",
    "LinearVelocity",
    "AngularVelocity",
];

/// Pinned hash for the ordered wire surface; updated with [`PROTOCOL_REV`] by its tripwire.
#[cfg(test)]
const WIRE_SURFACE_HASH: u64 = 0xa21f_954b_03e6_a9cd;

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
const WIRE_TYPES_HASH: u64 = 0x792b_eb49_ae46_2fce;

/// The pinned `Cargo.lock` versions of the external crates whose types ride the wire (avian's
/// replicated physics components; lightyear's wire framing / input protocol). A bump of either can
/// change the on-wire bytes without touching any source in this tree, so it must also bump
/// [`PROTOCOL_REV`]; the `wire_types_are_pinned` tripwire enforces it. `#[cfg(test)]` for the same
/// reason as `WIRE_SURFACE`.
#[cfg(test)]
const WIRE_DEP_AVIAN3D: &str = "0.7.0";
#[cfg(test)]
const WIRE_DEP_LIGHTYEAR: &str = "0.28.0";

/// Register the exact shared wire surface represented by [`WIRE_SURFACE`].
pub(crate) fn plugin(app: &mut App) {
    // `LocalTimeline` is incremented by lightyear in `FixedFirst` (lightyear_core 0.28's
    // `increment_local_tick`); publish it before every `GameplaySet` consumer, especially
    // `shooting::fire`, which must put the id on its initial FireShell event.
    app.init_resource::<crate::ShotClock>();
    app.add_systems(FixedUpdate, publish_shot_clock.before(GameplaySet));
    app.component::<NetTank>().replicate();
    app.component::<NetBot>().replicate();
    // Immutable spawn state, but the owning predicted root needs it before `shooting::fire`.
    app.component::<CombatantId>().replicate().predict();
    // Plain replication, no `.predict()`/interpolation: predicted tanks simulate their own servos,
    // and non-predicted consumers chase the raw angle through the servo mechanism (see the type).
    app.component::<ServoAngles>().replicate();
    // Owner-private atomic combat snapshot; public life state is `NetTankStatus`.
    app.component::<NetCrew>().replicate();
    // Minimal public tank-life state. It gives observers death/cookoff presentation without
    // replicating private crew, module, ammunition, or belt facts.
    app.component::<NetTankStatus>().replicate();
    // Authoritative launched-turret world pose (same plain-replication shape): the client shows the
    // cooked-off toss it does NOT simulate locally, driving its own rig turret kinematically.
    app.component::<LaunchedTurretPose>().replicate();
    // Server-authoritative per-weapon belt supply (same plain-replication shape): each belt-fed
    // weapon's `belt_remaining` + swap countdown, so the client's fire-gating belt (root-resident in
    // the un-replicated `TankSim`) snaps to server truth instead of a divergent local prediction.
    app.component::<NetBelts>().replicate();

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
    app.add_systems(
        FixedUpdate,
        apply_net_tank_status
            .in_set(GameplaySet)
            .before(DamageConsequences),
    );
    // Authority belt state is the final word each tick, including replay.
    app.add_systems(
        FixedUpdate,
        apply_net_belts
            .in_set(GameplaySet)
            .after(ConsumeCommandEdges),
    );
    app.add_systems(FixedUpdate, mirror_swap_from_net_crew.in_set(GameplaySet));
    // The sim reads `TankCommand`; bridge it before all `GameplaySet` consumers, including replay.
    app.add_systems(
        FixedUpdate,
        bridge_action_state_to_tank_command.before(GameplaySet),
    );
}

/// Local-only rollback components must have no confirmed history: rollback restores them from
/// prediction history, not their add-time value.
fn strip_confirmed_history<C: Component + Clone>(
    add: On<Add, ConfirmedHistory<C>>,
    mut commands: Commands,
) {
    commands
        .entity(add.entity)
        .try_remove::<ConfirmedHistory<C>>();
}

/// Bridge Lightyear input to the simulation command.
///
/// Invariant: consumable actions commit only when `for_tick == LocalTimeline::tick()`. Level inputs
/// may hold last; stale, fabricated, or frozen input must never spend ammo or create an entity.
/// `SHIPPING_INPUT_DELAY_TICKS` removes delay-wobble sources, while this attestation fails closed for
/// all remaining input-buffer substitutions. See `tests/net_fire_release.rs`.
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

/// In-crate tests for the input-attestation seam.
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

    #[test]
    fn combatant_identity_is_predicted_with_the_owner_root() {
        let source = strip_comments(&read_source("src/net/protocol.rs"));
        assert!(
            source.contains("app.component::<CombatantId>().replicate().predict();"),
            "the owning predicted root must receive its immutable CombatantId before shooting::fire"
        );
    }

    /// The own wire-facing types whose DEFINITION TEXT rides [`WIRE_TYPES_HASH`], each as
    /// `(source file, type name)`. This is the `WIRE_SURFACE` own-type graph followed through its
    /// embeds (see the coverage-model block by the const): protocol types, the public status in
    /// `disclosure.rs`, `TankCommand`/`CrewSwap` from `command.rs`, and `CrewStation` from
    /// `damage.rs`. External wire types (avian/lightyear) are covered by dependency version.
    const WIRE_TYPE_DEFS: &[(&str, &str)] = &[
        ("src/net/protocol.rs", "NetTank"),
        ("src/net/protocol.rs", "NetBot"),
        ("src/lib.rs", "CombatantId"),
        ("src/net/protocol.rs", "ServoAngles"),
        ("src/net/protocol.rs", "NetCrew"),
        ("src/net/disclosure.rs", "NetTankStatus"),
        ("src/net/protocol.rs", "VolumeSnapshot"),
        ("src/net/protocol.rs", "CrewSnapshot"),
        ("src/net/protocol.rs", "LaunchedTurretPose"),
        ("src/net/protocol.rs", "NetBelts"),
        ("src/net/protocol.rs", "BeltSnapshot"),
        ("src/net/protocol.rs", "FireChannel"),
        ("src/net/protocol.rs", "OutcomeChannel"),
        ("src/net/protocol.rs", "DamageChannel"),
        ("src/net/protocol.rs", "FireVisualBatch"),
        ("src/net/protocol.rs", "FireVisualFact"),
        ("src/net/protocol.rs", "FireEvent"),
        ("src/spec.rs", "FireMechanism"),
        ("src/net/protocol.rs", "RicochetKeyframe"),
        ("src/net/protocol.rs", "ImpactConfirm"),
        ("src/net/protocol.rs", "DamageReceipt"),
        ("src/net/protocol.rs", "DamageConfirm"),
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

    /// [`FireEvent::shot_id`] is a pure function of the stable identity fields already on the wire,
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
            mechanism: crate::spec::FireMechanism::Single,
            tracer: true,
            shooter: Entity::PLACEHOLDER,
            combatant: CombatantId(7),
            weapon: 3,
            fire_tick: Tick(77),
        };
        assert_eq!(
            event.shot_id(),
            ShotId {
                combatant: CombatantId(7),
                weapon: 3,
                fire_tick: 77,
            },
        );
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
