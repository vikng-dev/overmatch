//! Passive network diagnostics. No system here mutates simulation state.

use avian3d::prelude::{
    AngularVelocity, ColliderOf, ColliderTransform, LinearVelocity, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use lightyear::prelude::*;

use super::protocol::NetTank;
use crate::ballistics::ShellPath;
use crate::tank::{RemoteServos, Rig, ServoIndex, ServoState, Tank, TankRoot, TankServos, Turret};
use crate::track::sim::TrackContacts;

/// Log and latch corrupt physics state before Avian consumes it.
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
    // Avian placeholder positions are finite; reject them as well as non-finite values.
    let poisoned = |v: Vec3| !v.is_finite() || v.abs().max_element() > 1.0e30;
    let mut corrupt = false;
    for (entity, position, rotation, linear, angular) in &bodies {
        let bad_vel =
            linear.is_some_and(|v| poisoned(v.0)) || angular.is_some_and(|v| poisoned(v.0));
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

/// Log the first non-finite pose with its hierarchy, then latch.
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

/// Periodically log grounded track sides and each root's turret/reload state.
pub(crate) fn log_sim_evidence(
    turrets: Query<(&ServoIndex, &TankRoot), With<Turret>>,
    sims: Query<
        (
            Entity,
            Option<&TankServos>,
            Option<&RemoteServos>,
            Option<&crate::tank::WeaponGate>,
        ),
        With<Tank>,
    >,
    tracks: Query<(&TrackContacts, &RigidBody)>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    // SIMULATED tanks only. `TrackContacts` is required by `Tank`, so every tank carries one — but
    // the belt sim skips any body that is not `Dynamic` (`track::sim`), and a client's remote tanks
    // are driven by interpolation, never simulated. Counting them left two permanently empty sides
    // in the denominator, so a perfectly grounded 2-client session read `2/4` and looked like half
    // the running gear had fallen through the map.
    let (grounded, simulated) = tracks
        .iter()
        .filter(|(_, body)| matches!(**body, RigidBody::Dynamic))
        .fold((0usize, 0usize), |(grounded, tanks), (contacts, _)| {
            (
                grounded + contacts.0.iter().filter(|side| !side.is_empty()).count(),
                tanks + 1,
            )
        });
    let observed = tracks.iter().count();
    info!(
        "net: SIM-EVIDENCE track_sides_grounded={grounded}/{} ({simulated} simulated of {observed} tanks)",
        simulated * 2,
    );
    for (root, servos, remote_servos, gate) in &sims {
        // `TankRoot` owns the turret-to-simulation join.
        let turret = turrets
            .iter()
            .find(|(_, tank_root)| tank_root.0 == root)
            .and_then(|(slot, _)| {
                remote_servos
                    .and_then(|servos| servos.0.get(slot.0))
                    .or_else(|| servos.and_then(|servos| servos.states.get(slot.0)))
            })
            .map(ServoState::current);
        let weapon_gate = gate.map(|gate| &gate.weapons);
        info!("net: SIM-EVIDENCE {root} turret_angle={turret:?} weapon_gate={weapon_gate:?}");
    }
}

/// Periodically log network-tank positions.
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

/// Log the first replicated tank marker.
pub(crate) fn log_connected(add: On<Add, Connected>) {
    info!("client: connected (entity {})", add.entity);
}

/// Count locally spawned shell/tracer presentation effects.
pub(crate) fn count_shell_spawns(shells: Query<Entity, Added<ShellPath>>, mut total: Local<u32>) {
    for entity in &shells {
        *total += 1;
        info!("client: SHELL-SPAWN {entity} (total={})", *total);
    }
}

/// Per-root previous hull-to-turret offset for child-rig desync diagnostics.
#[derive(Resource, Default)]
pub(crate) struct TurretWatch {
    /// Previous hull-to-turret offset keyed by root.
    last_relative: std::collections::HashMap<Entity, Vec3>,
}

/// Log discontinuities in each turret's hull-relative pose, keyed by root.
pub(crate) fn watch_turret_pose(
    roots: Query<(Entity, &Rig)>,
    globals: Query<&GlobalTransform>,
    mut watch: ResMut<TurretWatch>,
) {
    for (root, rig) in &roots {
        let (Ok(hull), Ok(turret)) = (globals.get(rig.hull), globals.get(rig.turret)) else {
            continue;
        };
        let relative_vec = turret.translation() - hull.translation();
        if let Some(&previous) = watch.last_relative.get(&root) {
            let delta = (relative_vec - previous).length();
            if delta > 0.1 {
                let relative = relative_vec.length();
                warn!(
                    "client: TURRET-DRIFT {root} relative offset moved {delta:.3} m in one tick \
                     (hull-relative distance now {relative:.3} m) — child rig desynced from root"
                );
            }
        }
        watch.last_relative.insert(root, relative_vec);
    }
}
