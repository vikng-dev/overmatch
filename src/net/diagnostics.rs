//! The measurement instruments for prediction health — passive observers and periodic logs, kept
//! deliberately as permanent tooling. Nothing here drives the sim; each system only reads state and
//! reports. The composition roots (`net::client`/`net::server`) wire the subset each side needs.

use avian3d::prelude::{
    AngularVelocity, ColliderOf, ColliderTransform, LinearVelocity, Position, Rotation,
};
use bevy::diagnostic::DiagnosticsStore;
use bevy::prelude::*;
use lightyear::prediction::diagnostics::PredictionDiagnosticsPlugin;
use lightyear::prelude::*;

use crate::ballistics::ShellPath;
use crate::driving::Suspension;
use crate::shooting::Reload;
use super::protocol::NetTank;
use crate::tank::{Hull, Rig, ServoState, Tank, Turret};

/// Diagnostic (bind-window NaN): at the top of each physics tick, name every entity whose
/// physics state or `ColliderTransform` is non-finite — with values — then latch. Runs before
/// `PhysicsSystems::Prepare`, i.e. before the step that would hit avian's panicking asserts.
pub(crate) fn fixed_nan_probe(
    bodies: Query<
        (
            Entity,
            &Position,
            &Rotation,
            Option<&LinearVelocity>,
            Option<&AngularVelocity>,
        ),
        With<Tank>,
    >,
    parts: Query<
        (
            Entity,
            Option<&Name>,
            Option<&Position>,
            Option<&Rotation>,
            Option<&ColliderTransform>,
        ),
        With<ColliderOf>,
    >,
    mut latched: Local<bool>,
) {
    if *latched {
        return;
    }
    // Non-finite OR placeholder-magnitude: avian's require-inserted `PLACEHOLDER` sentinels are
    // f32::MAX — *finite*, so a pure `is_finite` probe is blind to exactly the poison the
    // bind-window family injects. Anything past 1e30 m is equally impossible and equally fatal.
    let poisoned = |v: Vec3| !v.is_finite() || v.abs().max_element() > 1.0e30;
    let mut corrupt = false;
    for (entity, position, rotation, linear, angular) in &bodies {
        let bad_vel = linear.is_some_and(|v| poisoned(v.0)) || angular.is_some_and(|v| poisoned(v.0));
        if poisoned(position.0) || !rotation.0.is_finite() || bad_vel {
            error!(
                "net: FIXED-NAN root {entity}: pos={:?} rot={:?} linvel={:?} angvel={:?}",
                position.0,
                rotation.0,
                linear.map(|v| v.0),
                angular.map(|v| v.0)
            );
            corrupt = true;
        }
    }
    for (entity, name, position, rotation, collider_transform) in &parts {
        let bad = position.is_some_and(|p| poisoned(p.0))
            || rotation.is_some_and(|r| !r.0.is_finite())
            || collider_transform
                .is_some_and(|t| poisoned(t.translation) || !t.rotation.0.is_finite());
        if bad {
            error!(
                "net: FIXED-NAN part {entity} ({:?}): pos={:?} rot={:?} collider_transform={:?}",
                name.map(|n| n.as_str()),
                position.map(|p| p.0),
                rotation.map(|r| r.0),
                collider_transform
            );
            corrupt = true;
        }
    }
    if corrupt {
        *latched = true;
    }
}

/// Bind-window forensics: name every entity that carries a `GlobalTransform` while its parent has
/// none (the B0004 pairs Bevy warns about during the net bind window) — once per pair. The broken
/// link means transform propagation skips that subtree, which corrupts anything composed through
/// it (collider offsets, COM capture).
pub(crate) fn report_orphan_transforms(
    world: &World,
    children: Query<(Entity, &ChildOf), With<GlobalTransform>>,
    has_global: Query<(), With<GlobalTransform>>,
    mut seen: Local<bevy::platform::collections::HashSet<(Entity, Entity)>>,
) {
    for (child, child_of) in &children {
        let parent = child_of.parent();
        if has_global.contains(parent) || !seen.insert((child, parent)) {
            continue;
        }
        // Full archetypes, not just names — the pairs seen so far are anonymous, so the component
        // lists are the only identification available.
        let archetype = |e: Entity| -> String {
            world.inspect_entity(e).map_or_else(
                |_| "<despawned>".into(),
                |infos| {
                    infos
                        .map(|i| i.name().shortname().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            )
        };
        warn!(
            "net: ORPHAN-TRANSFORM child {child} [{}] under transform-less parent {parent} [{}]",
            archetype(child),
            archetype(parent)
        );
    }
}

/// NaN tripwire (bind-window crash diagnostic): names the first entity whose physics `Position`
/// or local `Transform` goes non-finite, with its ancestry — runs before avian's own finite
/// assert kills the app, so the culprit node is in the log.
pub(crate) fn nan_tripwire(
    positions: Query<(Entity, &Position)>,
    transforms: Query<(Entity, &Transform)>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
    mut tripped: Local<bool>,
) {
    if *tripped {
        return;
    }
    let describe = |entity: Entity| {
        let mut chain = String::new();
        let mut e = entity;
        loop {
            let name = names
                .get(e)
                .map(|n| n.as_str().to_owned())
                .unwrap_or_else(|_| "?".into());
            chain.push_str(&format!("{e}({name}) <- "));
            match parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => break,
            }
        }
        chain
    };
    for (entity, position) in &positions {
        if !position.0.is_finite() {
            error!(
                "client: NAN-TRIPWIRE Position on {} = {:?}",
                describe(entity),
                position.0
            );
            *tripped = true;
        }
    }
    for (entity, transform) in &transforms {
        if !(transform.translation.is_finite() && transform.rotation.is_finite()) {
            error!(
                "client: NAN-TRIPWIRE Transform on {} = {:?}",
                describe(entity),
                transform
            );
            *tripped = true;
        }
    }
}

/// Log `PredictionDiagnosticsPlugin`'s ROLLBACKS/ROLLBACK_DEPTH diagnostics periodically (every
/// ~5s) — `PredictionPlugin::build` already mounts the plugin unconditionally (spike log,
/// increment-5 setup notes), so this only reads `DiagnosticsStore`, it does not mount it again
/// (mounting a second `PredictionDiagnosticsPlugin` would panic on the duplicate-plugin check).
pub(crate) fn log_prediction_diagnostics(
    diagnostics: Res<DiagnosticsStore>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 5.0 {
        return;
    }
    *timer = 0.0;
    let rollbacks = diagnostics
        .get(&PredictionDiagnosticsPlugin::ROLLBACKS)
        .and_then(|d| d.value())
        .unwrap_or_default();
    let depth = diagnostics
        .get(&PredictionDiagnosticsPlugin::ROLLBACK_DEPTH)
        .and_then(|d| d.value())
        .unwrap_or_default();
    info!("net: PredictionDiagnostics rollbacks={rollbacks} rollback_depth={depth:.2}");
}

/// Step-7 verification readout (every ~2 s, both sides): proof the *real* sim is running, not the
/// retired stub — grounded-wheel count (suspension rays actually hitting the Terrain-layer game
/// ground), the main turret's servo angle (slewing toward the scripted aim), and every weapon's
/// reload timer (the Tiger has two — MainGun + Coax; the MainGun's goes non-zero after a fire
/// consumes the click). One tank per side in this spike, so the single-turret read is unambiguous.
pub(crate) fn log_sim_evidence(
    turrets: Query<&ServoState, With<Turret>>,
    reloads: Query<&Reload>,
    wheels: Query<&Suspension>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    let grounded = wheels.iter().filter(|s| s.contact.is_some()).count();
    let total = wheels.iter().count();
    let turret = turrets.iter().next().map(ServoState::current);
    let reloads: Vec<f32> = reloads.iter().map(|r| r.remaining).collect();
    info!(
        "net: SIM-EVIDENCE wheels_grounded={grounded}/{total} turret_angle={turret:?} reloads={reloads:?}"
    );
}

/// Verdict 1 (increment 6): the binder must fire exactly once per tank despite rollback replays —
/// rollback only re-runs `FixedMain` (map §8), and `on_tank_ready` fires from `WorldInstanceReady`
/// (outside `FixedMain`), so a count > 1 per tank would mean that assumption was wrong. `Rig` is
/// the observer's own terminal insert, so counting `Added<Rig>` is an external, non-invasive proxy
/// for "the binder ran" without touching `tank.rs`. On the client this is the side that actually
/// matters for the verdict — the predicted root is exactly where a rollback replay could plausibly
/// re-fire an async-load observer if the "rollback re-runs FixedMain only" assumption were wrong.
pub(crate) fn count_rig_binds(binds: Query<Entity, Added<Rig>>) {
    for entity in &binds {
        info!("net: {entity} Rig bound (on_tank_ready fired)");
    }
}

/// Periodic authoritative/replicated position log (every ~2 s), so client and server positions can
/// be diffed for the increment-5/6 convergence success criterion.
pub(crate) fn log_positions(
    tanks: Query<(Entity, &Position), With<NetTank>>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    for (entity, position) in &tanks {
        info!("net: {entity} position={:?}", position.0);
    }
}

/// Increment-5 success signal: the predicted tank arrives carrying `Predicted`.
pub(crate) fn log_predicted_tank(add: On<Add, Predicted>, tanks: Query<(), With<NetTank>>) {
    if tanks.contains(add.entity) {
        info!(
            "client: {} predicted (carries Predicted) — moves immediately under input",
            add.entity
        );
    }
}

/// Step-1 success signal.
pub(crate) fn log_connected(add: On<Add, Connected>) {
    info!("client: connected (entity {})", add.entity);
}

/// Counts local shell/tracer spawns (`Added<ShellPath>` — inserted by `on_fire_shell` on every
/// shell). The script fires exactly once, so a count above one during the forced-rollback pass is
/// the "replayed fire duplicates the local tracer" wart the coordinator accepted for this step
/// (fixed later by `PreSpawned`, map §2 — deliberately not added yet).
pub(crate) fn count_shell_spawns(shells: Query<Entity, Added<ShellPath>>, mut total: Local<u32>) {
    for entity in &shells {
        *total += 1;
        info!("client: SHELL-SPAWN {entity} (total={})", *total);
    }
}

/// Tracks the predicted tank's previous-tick `Position` so a big jump can be logged as a
/// `ROLLBACK-SNAP` (the map's suggested fallback detector alongside `PredictionMetrics`).
#[derive(Component, Default)]
pub(crate) struct LastPosition(pub Option<Vec3>);

/// Backup rollback detector (map's fallback): a same-tick `Position` discontinuity > 0.5 m on the
/// predicted entity. Also logs final positions for the convergence check.
pub(crate) fn log_snap(
    mut tanks: Query<(Entity, &Position, &mut LastPosition), (With<Predicted>, With<NetTank>)>,
) {
    for (entity, position, mut last) in &mut tanks {
        if let Some(previous) = last.0 {
            let delta = (position.0 - previous).length();
            if delta > 0.5 {
                info!(
                    "client: ROLLBACK-SNAP {entity} moved {delta:.2} m in one tick (from {previous:?} to {:?})",
                    position.0
                );
            }
        }
        last.0 = Some(position.0);
    }
}

/// Polls `PredictionMetrics` each frame and logs on change — the primary "a rollback fired"
/// signal (map's suggested mechanism; `lightyear_prediction`'s own diagnostics counter).
#[derive(Resource, Default)]
pub(crate) struct RollbackWatch {
    last_count: u32,
}

pub(crate) fn watch_rollback_metrics(
    metrics: Res<PredictionMetrics>,
    mut watch: ResMut<RollbackWatch>,
) {
    if metrics.rollbacks != watch.last_count {
        info!(
            "client: ROLLBACK fired (PredictionMetrics.rollbacks={}, rollback_ticks={})",
            metrics.rollbacks, metrics.rollback_ticks
        );
        watch.last_count = metrics.rollbacks;
    }
}

/// Verdict 2 (increment 6): the turret node's previous-tick pose relative to the hull, so a jump
/// in that *relative* pose (as opposed to the hull's own world-space rollback snap) would mean
/// `update_child_collider_position` failed to keep the child rig tracking the root through a
/// replay. Logged around the perturbation window only.
#[derive(Resource, Default)]
pub(crate) struct TurretWatch {
    last_relative: Option<Vec3>,
}

/// Verdict 2 (increment 6): the turret's pose *relative to the hull* — logged only when it moves
/// more than the map's 0.1 m bar in one tick, which should never happen (the turret doesn't slew
/// in this spike; nothing drives `ServoCommand`) unless `update_child_collider_position` failed to
/// keep the child rig glued to the root through a rollback replay. Absolute world deltas are
/// expected (the perturbation moves the whole tank); only the hull-relative offset is diagnostic.
pub(crate) fn watch_turret_pose(
    hulls: Query<&GlobalTransform, With<Hull>>,
    turrets: Query<&GlobalTransform, With<Turret>>,
    mut watch: ResMut<TurretWatch>,
) {
    let (Ok(hull), Ok(turret)) = (hulls.single(), turrets.single()) else {
        return;
    };
    let relative = hull.translation().distance(turret.translation());
    let relative_vec = turret.translation() - hull.translation();
    if let Some(previous) = watch.last_relative {
        let delta = (relative_vec - previous).length();
        if delta > 0.1 {
            warn!(
                "client: TURRET-DRIFT relative offset moved {delta:.3} m in one tick \
                 (hull-relative distance now {relative:.3} m) — child rig desynced from root"
            );
        }
    }
    watch.last_relative = Some(relative_vec);
}
