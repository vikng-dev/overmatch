//! The wire contract: everything both sides must register identically. lightyear requires the same
//! protocol registration on client and server (replicated components, the input protocol, the avian
//! prediction/rollback registration) — mismatch here desyncs or fails the connection. If a component
//! or input rides the wire, its registration lives here and nowhere else.

use avian3d::prelude::{AngularVelocity, LinearVelocity, Position, Rotation};
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

use crate::command::TankCommand;
use crate::driving::DriveState;
use crate::state::GameplaySet;
use crate::tank::{Rig, ServoCommand, ServoIndex, TankSim};

/// Replicated tank-identity marker — how the client recognizes a replicated entity as a tank
/// (before its local sim body exists) without replicating the sim's own `Tank` marker. Deliberately
/// NOT `Tank`: replicating `Tank` fires its `On<Add, Tank>` observers at replication-receive time,
/// ahead of the client's sim-body build, and that ordering deterministically NaN'd the tank at
/// `Dynamic` activation (4/4 crash, 2026-07-05 restructure regression — root pos/rot/velocities all
/// NaN within a frame). The sim's `Tank` stays a local component that arrives with the sim body,
/// exactly like every other rig component (`spawn_tank_sim`).
#[derive(Component, Clone, Copy, Default, Serialize, Deserialize)]
pub struct NetTank;

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

/// Coarsened rollback thresholds for the tank root (map §1): the reference examples' 1 cm / 0.01
/// rad bar is tuned for a single-collider capsule character, not a 16-contact 57 t rig — solver
/// noise on a body this complex trips that bar far more often than genuine misprediction (measured:
/// ~430 rollbacks/15s at 100ms latency vs 13 for the increment-5 primitive, all invisible/converging
/// per the increment-6 log). Correction smoothing (`add_linear_correction_fn`, already wired) hides
/// a ≤5 cm snap; coarsening to 0.05 trades some correctness-under-genuine-desync for a large CPU
/// win on the honest-noise case. Position in metres, Rotation in radians, velocities in m/s or
/// rad/s-equivalent — same shape as the map §1(b) reference thresholds, five times coarser.
///
/// Velocity is coarser still: rough terrain (the course's bump/washboard) puts sustained vertical-
/// velocity transients through the suspension, and client/server solver noise on those transients
/// tripped 0.05 at 20–60 rollbacks/s at ZERO latency — every recorded cause was `LinearVelocity`
/// (step-8 washboard investigation). 0.20 cut that stream ~64% with convergence unchanged
/// (positions agree to centimetres mid-washboard); velocity errors self-damp through the
/// suspension, and the position/rotation bars still catch real drift.
const ROLLBACK_POSITION_M: f32 = 0.05;
const ROLLBACK_ROTATION_RAD: f32 = 0.05;
const ROLLBACK_VELOCITY: f32 = 0.20;

/// Registers everything both sides of the wire must agree on: replicated components and the
/// `TankCommand` input protocol. Grows as later increments add more (§5/§7 of the spike map).
pub(crate) fn plugin(app: &mut App) {
    app.component::<NetTank>().replicate();
    // Plain replication, no `.predict()`/interpolation: predicted tanks simulate their own servos,
    // and non-predicted consumers chase the raw angle through the servo mechanism (see the type).
    app.component::<ServoAngles>().replicate();
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
            crate::trace::note_if_tripped("Position", (a.0 - b.0).length(), ROLLBACK_POSITION_M)
        })
        .add_linear_correction_fn()
        .add_linear_interpolation();
    app.component::<Rotation>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &Rotation, b: &Rotation| {
            crate::trace::note_if_tripped("Rotation", a.angle_between(*b), ROLLBACK_ROTATION_RAD)
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
            crate::trace::note_if_tripped("LinearVelocity", (a.0 - b.0).length(), ROLLBACK_VELOCITY)
        });
    app.component::<AngularVelocity>()
        .replicate()
        .predict()
        .with_rollback_condition(|a: &AngularVelocity, b: &AngularVelocity| {
            crate::trace::note_if_tripped("AngularVelocity", (a.0 - b.0).length(), ROLLBACK_VELOCITY)
        });

    // Non-replicated rollback state — ROOT-RESIDENT ONLY, by design: the root is the predicted
    // entity, so plain `local_rollback` attaches history with no child decoration machinery
    // (`TankSim` centralizes what used to live on turret/gun/muzzle/wheel children — see its doc
    // for the hazard cluster that design retired).
    app.local_rollback::<DriveState>();
    app.local_rollback::<TankSim>();
    app.add_observer(strip_confirmed_history::<DriveState>);
    app.add_observer(strip_confirmed_history::<TankSim>);

    app.add_systems(FixedPostUpdate, publish_servo_angles);
    app.add_systems(FixedUpdate, apply_servo_angles.in_set(GameplaySet));
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
    commands.entity(add.entity).try_remove::<ConfirmedHistory<C>>();
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
