//! The networked tank-rig lifecycle: spawning a replicated tank's local sim body and arming the
//! predicted root's render smoothing. Since phase 1 of the sim/view split the body is built
//! SYNCHRONOUSLY from extracted data (`tank::spawn_tank_sim`) the moment the replicated root is
//! usable — colliders, servo frames, `Rig`, `TankSim`, all in one command flush; the glb scene
//! attaches later as pure view. Invariants this module maintains:
//!   1. the sim body attaches only to a valid replicated pose — never to avian's require-inserted
//!      placeholder (`attach_replicated_rig`'s pose gate, the cold-start crash fix);
//!   2. a `RigidBody` is set in the SAME command-flush as the collider inserts — colliders never
//!      wait, unattached or placeholder-posed, for a body that arrives in a later frame (the NaN
//!      ordering class: the step-8 8/8 crash was collider ATTACHMENT/first-propagation racing the
//!      body transition). The server spawns `Dynamic` (always the authority); the client attaches
//!      `Dynamic` if the replicated root is already `Predicted` and `Static` otherwise — an
//!      interpolated remote, whose pose replication owns for good, OR a predicted root whose
//!      `Predicted` marker has not yet replicated. That last case is promoted to `Dynamic` by
//!      `upgrade_predicted_to_dynamic` — a body-TYPE change on a long-prepared body whose
//!      `ColliderTransform`s have had frames of `Prepare` propagation, which is outside the
//!      ordering class (and re-arms invariant 3 regardless);
//!   3. no rollback replays the entity before the end of its first full physics tick under its
//!      current body role (`DisableRollback`, lifted by `enable_rollback_after_first_tick`) —
//!      armed at spawn for the children's first `ColliderTransform` propagation, and re-armed by
//!      the `Static`→`Dynamic` promotion so a freshly-integrating body is never first stepped
//!      inside another entity's replay.
//!
//! The rig's CHILDREN are not rollback participants at all: every carried child state lives
//! root-resident in `tank::TankSim` (one `local_rollback` on the predicted root), and child
//! transforms are derived from it each tick — which is what retired the whole
//! `DeterministicPredicted` decoration / pose-history-stripping / despawn-grace machinery this
//! module used to maintain (steps 7–8's hazard cluster).

use avian3d::prelude::{Position, RigidBody, Rotation};
use bevy::prelude::*;
use lightyear::frame_interpolation::{FrameInterpolate, FrameInterpolationPlugin};
// `Remote` (bevy_replicon's "this entity arrived by replication", re-exported): the honest
// authority-vs-replica discriminator — the server's own tanks are not `Remote`, every client-side
// tank is (see `upgrade_predicted_to_dynamic` on why `Predicted`/`Interpolated` can NOT stand in
// for it: the server entity carries both markers itself).
use lightyear::prelude::client::Remote;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::tank::{Rig, TankSimSource, bind_tank_view, spawn_tank_sim};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(upgrade_predicted_to_dynamic);
    // FixedLast = the earliest point provably AFTER the fresh rig's first full physics tick
    // (collider-transform propagation included) — see `enable_rollback_after_first_tick`.
    app.add_systems(
        FixedLast,
        enable_rollback_after_first_tick.run_if(not(is_in_rollback)),
    );
}

/// The networked composition of the shared spawn core (`tank::tank_rig` — scene-as-view + spec +
/// `Tank`): adds the wire identity marker and the `Transform` the replicated root needs. The
/// `RigidBody` is deliberately NOT here — each spawn path sets it alongside `spawn_tank_sim`'s
/// collider inserts in the same command flush (`net::server::spawn_pending_tanks` always `Dynamic`,
/// the client's `attach_replicated_rig` `Dynamic`/`Static` by prediction role — see the module
/// invariants). Used by both networked spawn paths; both call `spawn_tank_sim` in the same batch.
pub(crate) fn net_tank_rig(assets: &crate::tank::PendingTankAssets) -> impl Bundle {
    (
        crate::tank::tank_rig(assets),
        NetTank,
        // Explicit, because on the CLIENT this bundle lands on a replicon-spawned root that has
        // only the replicated components (Position/Rotation) — without a Transform the hierarchy
        // under it never gets GlobalTransforms (Bevy B0004), collider offsets go wrong, and the
        // client settles at a different rest height than the server (measured: +1.25 vs −0.28 →
        // rollback on every packet). lightyear's avian sync owns writing this from Position
        // afterwards.
        Transform::default(),
    )
}

/// Give the replicated tank its LOCAL sim body (map §6's `handle_new_character` pattern), built
/// synchronously from the extracted geometry the moment the replicated root is usable: avian
/// components are not replicated, and a predicted entity without a body cannot be re-simulated
/// during rollback replay — the symptom is continuous rollback from spawn, every confirmed packet
/// disagreeing with a frozen prediction. A plain system (not an observer on `Predicted`) because
/// `NetTank` arrives by replication and may land before OR after the prediction marker; waits on
/// the asset gate (the spec feeds the spawner, the preloaded glb keeps the view pop-in short).
///
/// `With<Remote>` = every replicated tank, whichever markers rode along: the own (predicted)
/// tank today, other players' (interpolated) tanks at step 9. Every replicated tank gets the same
/// full sim skeleton — node mapping, servos, and view anchors are what the camera/HUD and
/// `apply_servo_angles` lay the model with. The `RigidBody` is chosen HERE, in the same flush as
/// `spawn_tank_sim`'s colliders: `Dynamic` when the root already carries `Predicted` (this client
/// simulates it), `Static` otherwise. `Static` covers both an interpolated remote (replication owns
/// its pose forever) AND a predicted root whose `Predicted` marker has not yet replicated — the
/// pose (a `.predict()` component) and the marker ride different replication-visibility paths and
/// need not arrive in the same message; `upgrade_predicted_to_dynamic` promotes that latter case
/// the instant the marker lands (a predicted body left `Static` mispredicts every packet).
pub(crate) fn attach_replicated_rig(
    // `With<Position>, With<Rotation>`: THE COLD-START PLACEHOLDER GUARD. The bundle's
    // `RigidBody` require-inserts `Position::PLACEHOLDER`/`Rotation::PLACEHOLDER` (f32::MAX)
    // if the entity doesn't have them yet — and the replicated pose can land a few frames after
    // the `NetTank` marker (`.predict()` components ride the prediction sync, plain markers the
    // replication apply). Lose that race and the body's first Dynamic tick integrates from
    // f32::MAX and NaNs the root (measured: 9/9 cold-cache runs, root pos/rot = 3.4e38 at the
    // first post-spawn probe; warm runs won the race by luck). Gating the body on the pose closes
    // the hole for every timing.
    tanks: Query<
        (Entity, Has<Predicted>),
        (
            With<Remote>,
            With<NetTank>,
            With<Position>,
            With<Rotation>,
            Without<RigidBody>,
        ),
    >,
    assets: Option<Res<crate::tank::PendingTankAssets>>,
    asset_server: Res<AssetServer>,
    source: TankSimSource,
    mut commands: Commands,
) {
    if tanks.is_empty() {
        return;
    }
    let Some(assets) = assets else { return };
    if !assets.loaded(&asset_server) {
        return;
    }
    let Some((geometry, spec)) = source.get(&assets.spec) else {
        return;
    };
    for (entity, predicted) in &tanks {
        // `Dynamic` only where THIS client simulates the body — the predicted root. Everything
        // else starts `Static`: an interpolated remote stays there for good, a predicted root
        // still awaiting its marker is promoted by `upgrade_predicted_to_dynamic`. The `RigidBody`
        // rides the same `insert` as `spawn_tank_sim`'s collider inserts, so it lands in one flush
        // — colliders never sit unattached/placeholder-posed waiting for a body in a later frame
        // (the step-8 NaN-crash class; see the module invariants for its exact boundary).
        let body = if predicted {
            RigidBody::Dynamic
        } else {
            RigidBody::Static
        };
        info!(
            "client: {entity} replicated tank gets local sim body (assets loaded, predicted={predicted})"
        );
        commands
            .entity(entity)
            .insert((
                net_tank_rig(&assets),
                body,
                // Defense-in-depth (NOT the placeholder crash — that's the pose gate above,
                // verified separately): no rollback may replay this entity until its body has
                // taken one full physics tick, because a replay before that steps physics over
                // child colliders whose `ColliderTransform`s haven't had their first
                // `PhysicsSystems::Prepare` propagation — the rollback check (PreUpdate) runs
                // before the first post-spawn FixedMain tick can clean them. `check_rollback`
                // skips `DisableRollback` entities entirely (and stamps them `Disabled` during
                // other entities' replays); `enable_rollback_after_first_tick` lifts it.
                DisableRollback,
            ))
            .observe(bind_tank_view);
        spawn_tank_sim(&mut commands, entity, geometry, spec);
    }
}

/// Lift [`DisableRollback`] once the fresh body has completed one full physics tick under its
/// current role — at `FixedLast` of that tick, `PhysicsSystems::Prepare` (FixedPostUpdate) has
/// already replaced the children's placeholder `ColliderTransform`s with propagated ones (the
/// spawn case) or the promoted body has taken one clean Dynamic step (the
/// `upgrade_predicted_to_dynamic` re-arm), so replays are safe from here on: `check_rollback`
/// builds its query `.without::<DisableRollback>()` (lightyear_prediction rollback.rs:192) and
/// stamps these entities `DisabledDuringRollback` during other entities' replays (rollback.rs:1103),
/// so a replay cannot step the body before that first tick. Gated on `Rig` (present from the
/// spawn flush, so this fires at the next FixedLast after arming) and not-in-rollback (during a
/// replay triggered by another entity, this one is disabled and must stay protected).
fn enable_rollback_after_first_tick(
    fresh: Query<Entity, (With<Rig>, With<NetTank>, With<DisableRollback>)>,
    mut commands: Commands,
) {
    for entity in &fresh {
        info!("net: {entity} first physics tick complete — rollback enabled");
        commands.entity(entity).remove::<DisableRollback>();
    }
}

/// Promote a client's predicted tank from `Static` to `Dynamic` the instant its `Predicted` marker
/// lands — the fallback for when the marker arrives AFTER `attach_replicated_rig` already built the
/// sim body `Static`. `Predicted` is a replicated marker (a required component of `PredictionTarget`
/// on the server — lightyear_replication send.rs:1112; replicated via `app.replicate::<Predicted>()`
/// lib.rs:183; there is NO separate predicted entity, the replicated root IS the predicted one), and
/// it rides a per-component visibility filter (`PredictedBit`, send.rs:359-417) distinct from the
/// entity-level `Replicate` visibility that carries Position/Rotation — so it is NOT guaranteed to
/// reach the client in the same init message as the pose. `attach_replicated_rig` gates on the pose,
/// and picks `Dynamic` only if `Predicted` is already present by then; this observer closes the
/// otherwise-fatal window where the pose (hence the sim body) lands first and `Predicted` a few
/// frames later — a predicted body stuck `Static` mispredicts every packet (continuous rollback
/// from spawn, the worst failure here).
///
/// `With<Remote>` scopes it to replicated (client) tanks: the server's own tanks are not `Remote`
/// and are spawned `Dynamic` directly, so this never touches them (and the interpolated remote never
/// receives `Predicted`, so it stays `Static` for good). NOT keyed off `Interpolated` as the
/// authority discriminator, because the server entity carries BOTH markers itself — `PredictionTarget`
/// and `InterpolationTarget` each require their marker (send.rs:1112/1119); `Remote` is the honest
/// authority-vs-replica split.
///
/// This is a body-TYPE change on a long-prepared body — its colliders attached at spawn and their
/// `ColliderTransform`s have had frames of `PhysicsSystems::Prepare` propagation, so it sits
/// outside the step-8 attachment-races-transition NaN class (module invariant 2). It still re-arms
/// [`DisableRollback`]: `Predicted` is what makes the entity rollback-checkable (`check_rollback`
/// requires it), so without the re-arm the body's first-ever Dynamic tick could run inside another
/// entity's replay with a near-empty history; `enable_rollback_after_first_tick` lifts it after
/// one clean Dynamic tick, exactly as at spawn.
fn upgrade_predicted_to_dynamic(
    add: On<Add, Predicted>,
    eligible: Query<(), (With<Remote>, With<NetTank>, With<Rig>)>,
    mut commands: Commands,
) {
    if !eligible.contains(add.entity) {
        return;
    }
    info!(
        "net: {} predicted marker arrived after spawn — body goes Dynamic",
        add.entity
    );
    commands
        .entity(add.entity)
        .insert((RigidBody::Dynamic, DisableRollback));
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
///     rubber-banding). Since 597ec21 the correction policy is `instant_correction()` — the sim
///     snaps to the corrected present in one frame by design and the render-space error layer
///     (`net/render_error.rs`) absorbs the discontinuity on the view side; this arming remains
///     load-bearing for the between-ticks overstep blend and lightyear's correction plumbing.
pub fn client_smoothing_plugin(app: &mut App) {
    app.add_plugins((
        FrameInterpolationPlugin::<Position>::default(),
        FrameInterpolationPlugin::<Rotation>::default(),
    ));
    app.add_systems(
        Update,
        (arm_predicted_smoothing, demote_predicted_interpolated),
    );
}

/// Strip the `Interpolated` marker off the client's OWN predicted tank — the fix for the
/// "silent Position desync" (multi-second windows where predicted Position ran 6–16 cm off the
/// server at the same tick with NO rollback, though the 0.05 m Position rollback condition sat
/// tripped the whole time).
///
/// ROOT CAUSE (traced against vendored lightyear 0.28). The server's authoritative tank carries
/// BOTH `Predicted` and `Interpolated` (each is a required component of its target — `PredictionTarget`
/// / `InterpolationTarget`, send.rs:1112/1119), and BOTH replicate to the owning client's predicted
/// root — verified: `Predicted=true Interpolated=true` on the controlled tank. Our protocol registers
/// Position/Rotation with BOTH `.predict()` (sets the `Predicted`-marker replicon write fn =
/// prediction's `write_history`, which records confirmed values AND checks for a rollback mismatch)
/// and `.add_linear_interpolation()` (sets the `Interpolated`-marker write fn = interpolation's
/// `write_history`, which only `insert_present`s into `ConfirmedHistory<C>` — no mismatch check).
/// When an entity carries both markers, bevy_replicon's `ComponentFns::write` picks the FIRST present
/// marker's fn in priority order (`component_fns.rs:113`); `Predicted` and `Interpolated` are both
/// priority 100, and the `Interpolated` slot wins the tie — so every replicated Position/Rotation
/// update on the predicted tank went through the interpolation path. That fills
/// `ConfirmedHistory<Position>` with correct server values (so `conft` advances and `confp` matches
/// the server to ~0 m) but NEVER records a mismatch into `StateRollbackMetadata`. The velocities are
/// registered with `.predict()` only, so they still route through prediction and still roll back.
///
/// The rollback check (`check_rollback`, `RollbackMode::Check`) then has no way to catch a Position
/// drift on this entity: its explicit-mismatch branch only fires on components that recorded a
/// mismatch (never Position), and its unchanged-component scan is SKIPPED for any entity whose
/// Replicon `ConfirmHistory` already contains the completed tick (`rollback.rs:583`) — which our tank
/// always does, being explicitly confirmed every tick by its velocity mutations. Net effect: Position
/// only ever corrected as a passenger of a velocity-triggered rollback; when velocity agreed while
/// Position drifted, the desync was silent.
///
/// THE FIX is client-side and leaves the shared wire protocol untouched: an entity that is both
/// predicted and interpolated is contradictory (they are lightyear's two mutually-exclusive views),
/// and for the owner Prediction must win. Removing `Interpolated` (a) makes replicon fall through to
/// the `Predicted` write fn for Position/Rotation, restoring the rollback mismatch check, and (b)
/// stops the interpolation systems (`With<Interpolated>`, Update schedule) from also writing an
/// interpolated Position onto the predicted root each frame. `.add_linear_interpolation()` STAYS in
/// the protocol so genuinely-remote (interpolated-only) tanks keep interpolating. Removing the marker
/// has no despawn side effect — it only downgrades the (cosmetic) delayed-interpolated-despawn back to
/// immediate despawn (`lightyear_interpolation despawn.rs:55`), irrelevant for a predicted entity.
///
/// A polling system, not an `Add` observer, for the same reason as [`arm_predicted_smoothing`]: the
/// two markers ride different replication-visibility paths and land in no guaranteed order, so we
/// re-check every frame and strip whenever both are present (the query is empty once stripped).
///
/// (Upstream report candidate: two same-priority marker write fns for one component make the
/// confirmed-write path — and thus whether prediction can even see a mismatch — depend on marker
/// registration/tie-break order, silently. lightyear could either forbid an entity carrying both
/// view markers or make `Predicted` outrank `Interpolated` for the write-fn selection.)
fn demote_predicted_interpolated(
    tanks: Query<Entity, (With<Predicted>, With<Interpolated>, With<NetTank>)>,
    mut commands: Commands,
) {
    for entity in &tanks {
        info!(
            "net: {entity} predicted tank also carries Interpolated — stripping it so Position/Rotation route through the prediction (rollback-checked) receive path"
        );
        commands.entity(entity).remove::<Interpolated>();
    }
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
