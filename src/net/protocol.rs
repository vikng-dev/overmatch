//! The wire contract: everything both sides must register identically. lightyear requires the same
//! protocol registration on client and server (replicated components, the input protocol, the avian
//! prediction/rollback registration) — mismatch here desyncs or fails the connection. If a component
//! or input rides the wire, its registration lives here and nowhere else.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, RigidBody, Rotation};
use bevy::ecs::entity::{EntityMapper, MapEntities};
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

use crate::ballistics::ComponentHealth;
use crate::command::TankCommand;
use crate::damage::{DamageConsequences, LaunchedTurret, TankVolumes};
use crate::driving::DriveState;
use crate::state::GameplaySet;
use crate::tank::{Rig, ServoCommand, ServoIndex, ServoSpec, TankSim};

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

/// Authoritative per-volume health snapshot, published on the tank root by the authority and
/// replicated so the client's death/HUD emerge from server-owned state (server-authoritative combat).
/// Holds each health-bearing volume's `ComponentHealth.current` in `TankVolumes` iteration order —
/// the SAME order both ends derive, since both build the rig from one RON spec via `spawn_tank_sim`
/// (sorted-by-name volume spawn). Index `i` therefore maps to the same volume on both ends; a length
/// mismatch at apply time skips the tank rather than misalign. Modeled on [`ServoAngles`]: plain
/// replication (no prediction/interpolation), `set_if_neq` on publish to avoid at-rest churn.
#[derive(Component, Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct NetHealth(pub Vec<f32>);

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
fn health_bearing_volumes(
    volumes: &TankVolumes,
    has_health: impl Fn(Entity) -> bool,
) -> Vec<Entity> {
    volumes.iter().filter(|&v| has_health(v)).collect()
}

/// Authority side: collect each health-bearing volume's live `ComponentHealth.current` into the
/// replicated `NetHealth`. `FixedPostUpdate` (after the damage chain has run this tick),
/// `Without<Remote>` = authority-only in shared code (every client tank carries `Remote` — see
/// `publish_servo_angles`). The collect order — the shared [`health_bearing_volumes`] filter — is
/// exactly the apply order in [`apply_net_health`].
fn publish_net_health(
    mut tanks: Query<(&TankVolumes, &mut NetHealth), Without<Remote>>,
    health: Query<&ComponentHealth>,
) {
    for (volumes, mut net) in &mut tanks {
        let snapshot: Vec<f32> = health_bearing_volumes(volumes, |v| health.contains(v))
            .iter()
            .map(|&v| health.get(v).map_or(0.0, |hp| hp.current))
            .collect();
        // `set_if_neq`: no change-detection churn (and no replication resends) while health is stable.
        net.set_if_neq(NetHealth(snapshot));
    }
}

/// Client side: write the replicated `NetHealth` back onto each `Remote` tank's local volumes, so the
/// damage-consequence systems (`damage.rs`) run off authoritative health. Matches the publish order
/// exactly — `TankVolumes` filtered to health-bearing volumes — so index `i` addresses the same
/// volume the server collected. Ordered `.before(DamageConsequences)` so cookoff/crew-death read this
/// tick's authoritative health. A length mismatch (e.g. rig not fully spawned yet) skips the tank
/// rather than write misaligned values.
fn apply_net_health(
    tanks: Query<(&TankVolumes, &NetHealth), With<Remote>>,
    mut health: Query<&mut ComponentHealth>,
) {
    for (volumes, net) in &tanks {
        // The health-bearing volumes in publish order (the SAME shared filter the server used).
        let bearers = health_bearing_volumes(volumes, |v| health.contains(v));
        // A length mismatch is expected transiently while the client's rig is still spawning and
        // self-heals once it's fully built; a *persistent* mismatch means client/server spec skew
        // (a distribution concern — matched builds never skew). Skip rather than write misaligned.
        if bearers.len() != net.0.len() {
            continue;
        }
        for (volume, &value) in bearers.into_iter().zip(&net.0) {
            if let Ok(mut hp) = health.get_mut(volume) {
                hp.current = value;
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

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub(crate) fn plugin(app: &mut App) {
    app.component::<NetTank>().replicate();
    app.component::<NetBot>().replicate();
    // Plain replication, no `.predict()`/interpolation: predicted tanks simulate their own servos,
    // and non-predicted consumers chase the raw angle through the servo mechanism (see the type).
    app.component::<ServoAngles>().replicate();
    // Server-authoritative per-volume health (same plain-replication shape as `ServoAngles`): the
    // client's damage/death emerge from this, not a divergent local kill.
    app.component::<NetHealth>().replicate();
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
            publish_net_health,
            publish_launched_turret_pose,
        ),
    );
    app.add_systems(
        FixedUpdate,
        (apply_servo_angles, apply_launched_turret_pose).in_set(GameplaySet),
    );
    // Client: land the replicated health before the damage-consequence chain reads it, so cookoff /
    // crew-death interpret this tick's authoritative HP (server-only publish is a no-op there).
    app.add_systems(
        FixedUpdate,
        apply_net_health
            .in_set(GameplaySet)
            .before(DamageConsequences),
    );
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
fn bridge_action_state_to_tank_command(
    mut tanks: Query<(&ActionState<TankCommand>, &mut TankCommand)>,
) {
    for (action, mut command) in &mut tanks {
        // Whole-struct overwrite: matches `ActionState`'s own "absolute snapshot per tick"
        // contract (no per-field diffing needed).
        *command = action.0;
    }
}
