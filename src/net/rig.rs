//! The networked tank-rig lifecycle: spawning a replicated tank's local body, decorating its
//! child parts as rollback participants, and arming the predicted root's render smoothing. Four
//! invariants this module maintains:
//!   1. the rig's non-root children become `DeterministicPredicted` with `skip_despawn: true`
//!      (they roll back with the root but survive the post-bind rollback burst);
//!   2. that derived child pose state (`Position`/`Rotation`) carries NO `PredictionHistory` — the
//!      children's poses are recomputed from the root every tick, and stored history poisons replay;
//!   3. the body goes `Dynamic` only in the same command-flush as the binder's collider inserts —
//!      never before (free-fall) and never after attachment (NaN);
//!   4. no rollback replays the entity between the bind flush and the end of the rig's first full
//!      physics tick — the children's poses are placeholder sentinels until first propagation
//!      (`DisableRollback`, see `attach_replicated_rig` / the cold-server bind crash).

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::asset::LoadState;
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
// `Remote` (bevy_replicon's "this entity arrived by replication", re-exported): the honest
// authority-vs-replica discriminator — see `activate_bound_rig` on why `Predicted`/`Interpolated`
// are not (the server entity carries both markers itself).
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use crate::spec::{TankSpec, TankSpecHandle};
use super::protocol::NetTank;
use crate::tank::{Rig, Roadwheel, Tank, on_tank_ready};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(activate_bound_rig);
    app.add_observer(strip_child_pose_history::<Position>);
    app.add_observer(strip_child_pose_history::<Rotation>);
    app.add_systems(Update, decorate_rig_children);
    // FixedLast = the earliest point provably AFTER the fresh rig's first full physics tick
    // (collider-transform propagation included) — see `enable_rollback_after_first_tick`.
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// The root bundle every networked tank spawn needs regardless of side: the real Tiger scene + spec
/// handle (drives `on_tank_ready`) plus the `RigidBody` the binder itself does not insert (it only
/// adds Mass/AngularInertia/colliders on children — `tank.rs::spawn_tank` inserts `RigidBody`
/// alongside the scene for the same reason, mirrored here). `Tank`/`on_tank_ready` (and, from step 7,
/// the whole `SimPlugin` — driving/aim/shooting) are the real rig's own contract.
pub(crate) fn net_tank_rig(asset_server: &AssetServer, spec: &Handle<TankSpec>) -> impl Bundle {
    (
        WorldAssetRoot(
            asset_server.load(GltfAssetLabel::Scene(0).from_asset("tiger_1/tiger_1.glb")),
        ),
        TankSpecHandle(spec.clone()),
        Tank,
        NetTank,
        // Explicit, because on the CLIENT this bundle lands on a replicon-spawned root that has
        // only the replicated components (Position/Rotation) — without a Transform the scene
        // hierarchy under it never gets GlobalTransforms (Bevy B0004), the binder captures wrong
        // collider offsets, and the client settles at a different rest height than the server
        // (measured: +1.25 vs −0.28 → rollback on every packet). lightyear's avian sync owns
        // writing this from Position afterwards.
        Transform::default(),
        // Static until the rig binds: the glb loads async (~seconds), and a Dynamic body with no
        // collider yet free-falls through the ground for the whole window (measured: y = −425 and
        // still falling when the script ended). `activate_bound_rig` flips it to Dynamic the
        // moment `Rig` lands — the spike-scale version of the game's spawn-before-bind race.
        avian3d::prelude::RigidBody::Static,
    )
}

/// Give the replicated tank its LOCAL rig (map §6's `handle_new_character` pattern, increment 6's
/// swap for the primitive cuboid): avian components are not replicated, and a predicted entity
/// without a body cannot be re-simulated during rollback replay — the symptom is continuous
/// rollback from spawn, every confirmed packet disagreeing with a frozen prediction. A plain
/// system (not an observer on `Predicted`) because `Tank` arrives by replication and may
/// land after the marker; also waits on the spec load (§8's spawn-before-bind race, mirrored from
/// `tank.rs`/`sandbox.rs` — `on_tank_ready` would panic on an unloaded spec). This is the exact
/// moment the §8 UNCERTAIN gets exercised: `Predicted`/`PredictionTarget` is already on the entity
/// (attached server-side at spawn) several ticks *before* the glb scene finishes loading and
/// `on_tank_ready` binds the rig — see the spike log for what was observed in that window.
///
/// `With<Remote>` = every replicated tank, whichever markers rode along: the own (predicted)
/// tank today, other players' (interpolated) tanks at step 9. A remote tank gets the same full
/// rig — the binder's node mapping, servos, and view anchors are what the camera/HUD and
/// `apply_servo_angles` lay the model with — but its body stays `Static` (`activate_bound_rig`
/// skips it): replication owns its pose, nothing local simulates it.
pub(crate) fn attach_replicated_rig(
    // `With<Position>, With<Rotation>`: THE COLD-START PLACEHOLDER GUARD. The bundle's
    // `RigidBody` require-inserts `Position::PLACEHOLDER`/`Rotation::PLACEHOLDER` (f32::MAX)
    // if the entity doesn't have them yet — and the replicated pose can land a few frames after
    // the `NetTank` marker (`.predict()` components ride the prediction sync, plain markers the
    // replication apply). Lose that race and the body's first Dynamic tick integrates from
    // f32::MAX and NaNs the root (measured: 9/9 cold-cache runs, root pos/rot = 3.4e38 at the
    // first post-bind probe; warm runs won the race by luck). Gating the rig on the pose closes
    // the hole for every timing.
    tanks: Query<
        Entity,
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            Without<RigidBody>,
        ),
    >,
    spec: Option<Res<crate::tank::PendingTankSpec>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(spec) = spec else { return };
    if !matches!(asset_server.load_state(&spec.0), LoadState::Loaded) {
        return;
    }
    for entity in &tanks {
        info!("client: {entity} replicated tank gets local rig (spec loaded)");
        commands
            .entity(entity)
            .insert((
                net_tank_rig(&asset_server, &spec.0),
                // Defense-in-depth for the bind window (NOT the placeholder crash — that's the
                // pose gate above, verified separately): no rollback may replay this entity until
                // its rig has taken one full physics tick, because a replay in that window steps
                // physics over child colliders whose `ColliderTransform`s haven't had their first
                // `PhysicsSystems::Prepare` propagation — the rollback check (PreUpdate) runs
                // before the first post-bind FixedMain tick can clean them. `check_rollback`
                // skips `DisableRollback` entities entirely (and stamps them `Disabled` during
                // other entities' replays); `enable_rollback_after_first_tick` lifts it.
                DisableRollback,
            ))
            .observe(on_tank_ready);
    }
}

/// Lift [`DisableRollback`] once the bound rig has completed one full physics tick — at
/// `FixedLast` of that tick, `PhysicsSystems::Prepare` (FixedPostUpdate) has already replaced the
/// children's placeholder poses with propagated ones, so replays are safe from here on. Gated on
/// `Rig` (pre-bind ticks don't count — the colliders don't exist yet) and not-in-rollback (during
/// a replay triggered by another entity, this one is `Disabled` and must stay protected).
fn enable_rollback_after_first_tick(
    fresh: Query<Entity, (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for entity in &fresh {
        info!("net: {entity} first post-bind tick complete — rollback enabled");
        commands.entity(entity).remove::<DisableRollback>();
    }
}

/// Wake a networked tank's physics the instant its rig binds — an OBSERVER on the binder's terminal
/// `Rig` insert, so `RigidBody::Dynamic` applies in the same command-flush cascade as the binder's
/// collider constructors, BEFORE any avian system runs that frame. Ordering is load-bearing,
/// established empirically (step-8 NaN-crash bisection at 100 ms):
///   - Dynamic landing after avian attached the constructed child colliders → every child
///     collider's `Position` goes NaN within a frame (avian finite assert, 8/8 with an
///     attachment-gated flip, ~60% with the old `Added<Rig>` Update system whose one-frame gap
///     sometimes let attachment win the race);
///   - colliders attaching to an already-Dynamic body → clean, and it is the only ordering the
///     rest of the game exercises (SP spawns tanks Dynamic from birth).
///
/// Only where the local side simulates the body: the authority (`Without<Remote>` — the server
/// spawned it) or the client's own predicted tank. A remote (interpolated) tank — other players'
/// tanks, from step 9 — stays `Static`: its `Position` is written by replication+interpolation
/// (the same sync that already carries the pre-bind Static body), and a Dynamic body would
/// free-run local physics against it.
///
/// NOT keyed on `Interpolated`: `PredictionTarget`/`InterpolationTarget` are
/// `ReplicationTarget<Predicted>`/`<Interpolated>` with the marker as a *required component*, so
/// the server entity carries BOTH markers itself (send.rs registers the pairs; the markers are
/// then target-filtered replicated components) — `Without<Interpolated>` excludes the authority.
/// Measured: server rig bound but never went Dynamic, wheels 0/16 both ends.
fn activate_bound_rig(
    add: On<Add, Rig>,
    eligible: Query<Entity, (With<NetTank>, Or<(With<Predicted>, Without<Remote>)>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!("net: {} rig bound — body goes Dynamic", add.entity);
    commands.entity(add.entity).insert(RigidBody::Dynamic);
}

/// Decorate a bound rig's non-root parts as rollback-participant the moment both `Rig` (the
/// binder's terminal insert) and `Predicted` are present on the root (order either can land in —
/// `Predicted` arrives at spawn, `Rig` seconds later on glb load, so this is really gated on `Rig`
/// alone in practice, but querying both is the honest precondition per the step-7 map §7 design).
///
/// `DeterministicPredicted` marks each part as predicted-but-uncompared: it gets a
/// `PredictionHistory` and rolls back with the root, but never itself trips a rollback from state
/// mismatch (it isn't replicated, so it has nothing to mismatch against). Without this,
/// `local_rollback::<ServoState>()` etc. below would silently no-op on these entities (map §3: the
/// history-attach observer gates on the trigger entity carrying `Predicted`/`PreSpawned`/
/// `DeterministicPredicted`/`CatchUpGated` directly — no hierarchy traversal exists).
///
/// `skip_despawn: true`, REVERSING the map §7 amendment 1 on live evidence: with the default
/// (`skip_despawn: false`) every rollback whose target tick predates the decoration tick despawns
/// the children (`deterministic_despawn` drain, rollback.rs) — and rollbacks fire *continuously*
/// through the post-bind suspension-settle burst, so the "vanishingly narrow" collision window is
/// actually the common case: measured, all 19 children despawned ~16 ms after decoration, rig
/// permanently broken client-side, 201 rollbacks/15 s. The skip_despawn variant instead stamps
/// `DisableRollback` during the grace window (`enable_rollback_after`, default 20 ticks) and then
/// lifts it — the children survive the burst and become full rollback participants ~300 ms later.
fn decorate_rig_children(
    ready: Query<(Entity, &Rig), (Added<Rig>, With<Predicted>)>,
    all_children: Query<&Children>,
    roadwheels: Query<(), With<Roadwheel>>,
    mut commands: Commands,
) {
    let decoration = DeterministicPredicted {
        skip_despawn: true,
        ..default()
    };
    for (root, rig) in &ready {
        let mut decorated = 3;
        for part in [rig.turret, rig.gun, rig.muzzle] {
            commands.entity(part).insert(decoration);
        }
        // Roadwheels aren't in `Rig` (no fixed count/field) — walk the root's whole subtree for
        // the `Roadwheel` marker, the same descendant-walk shape `on_tank_ready` itself uses
        // (from the ROOT: wheels are siblings of `Hull` in the model, not under it).
        for wheel in all_children.iter_descendants(root) {
            if roadwheels.contains(wheel) {
                commands.entity(wheel).insert(decoration);
                decorated += 1;
            }
        }
        info!("net: {root} rig children decorated DeterministicPredicted (count={decorated})");
    }
}

/// Strip pose history from decorated rig children. `add_prediction_history` attaches
/// `PredictionHistory<C>` for EVERY prediction-registered type present on a
/// `DeterministicPredicted` entity — including avian `Position`/`Rotation`, whose first recorded
/// value can be the require-inserted `PLACEHOLDER` sentinel (f32::MAX) if collider-transform
/// propagation hasn't run yet that tick; a later rollback then restores the literal sentinel into
/// the live component and NaNs the solver (the bind-window crash — see
/// `nan-crash-research.md` for the fully-cited chain). The children's poses are DERIVED state
/// (avian recomputes them from the root pose ∘ `ColliderTransform` every tick, replay included),
/// so pose history on them has zero value. `prepare_rollback` uses the history component itself
/// as its membership marker, so removal cleanly excludes exactly these two components while the
/// `local_rollback` histories (`ServoState`/`Reload`/`Suspension`/`DriveState`) keep working.
///
/// An OBSERVER on the history insert itself (was: an Update-polled system), so the removal lands
/// in the same command-flush cascade as lightyear's attach — a rollback in the next frame's
/// PreUpdate can never see (and restore from) a pose history that only existed for part of a
/// frame. The polled version left exactly that gap open.
fn strip_child_pose_history<C: Component>(
    add: On<Add, PredictionHistory<C>>,
    decorated: Query<(), With<DeterministicPredicted>>,
    mut commands: Commands,
) {
    if !decorated.contains(add.entity) {
        return;
    }
    info!(
        "net: {} pose history stripped (derived state, rollback poison vector)",
        add.entity
    );
    commands
        .entity(add.entity)
        .remove::<PredictionHistory<C>>();
}

/// Client-side render smoothing for the predicted tank — the half of lightyear's prediction stack
/// `LightyearAvianPlugin` does NOT mount (it only *orders* these systems' sets; the plugins and the
/// per-entity `FrameInterpolate` markers are the app's job, per `lightyear_frame_interpolation`'s
/// docs and the `avian_3d_character` example). Two effects:
///   - between fixed ticks the root's Position/Rotation render as an overstep blend instead of raw
///     64 Hz steps;
///   - rollback *correction* arms: `update_frame_interpolation_post_rollback` requires
///     `FrameInterpolate<C>` on the entity, so without it the registered correction fn is inert and
///     every rollback SNAPS the tank (measured 10–26 rollbacks/s while driving at 80 ms — the
///     rubber-banding) instead of decaying the error over `CorrectionPolicy` (~200 ms half-life).
pub fn client_smoothing_plugin(app: &mut App) {
    app.add_plugins((
        FrameInterpolationPlugin::<Position>::default(),
        FrameInterpolationPlugin::<Rotation>::default(),
    ));
    app.add_systems(Update, arm_predicted_smoothing);
}

/// Decorate the predicted tank ROOT with `FrameInterpolate` once `Predicted` and `Position` are
/// both present. A polling system, not an `Add` observer: the prediction sync copies components
/// from the confirmed entity in no guaranteed order (same shape as `strip_child_pose_history`).
/// Root only — the children's poses are DERIVED state (root pose ∘ collider/servo transforms);
/// frame-interpolating them would fight the systems that derive them.
fn arm_predicted_smoothing(
    tanks: Query<
        Entity,
        (
            With<Predicted>,
            With<NetTank>,
            With<Position>,
            Without<FrameInterpolate<Position>>,
        ),
    >,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!("net: {entity} predicted root armed for frame interpolation + correction");
        commands.entity(entity).insert((
            FrameInterpolate::<Position>::default(),
            FrameInterpolate::<Rotation>::default(),
        ));
    }
}
