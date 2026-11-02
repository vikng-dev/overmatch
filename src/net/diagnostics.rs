//! Network diagnostics. Corrupt state stops in every build before it can propagate.

use avian3d::prelude::{
    AngularVelocity, ColliderOf, ColliderTransform, LinearVelocity, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use lightyear::prelude::*;

use crate::tank::{RemoteServos, ServoIndex, ServoState, Tank, TankRoot, TankServos, Turret};
use crate::track::sim::TrackContacts;

/// Refuse corrupt or uninitialized physics state before Avian consumes it.
///
/// A part is checked when its pose was written since the probe's last run (its first run checks
/// every part): a value checked finite and not written since is the same finite value, so the
/// expensive checks read only written parts. Bevy's change filter still scans every matching
/// part's change ticks.
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
        (
            With<ColliderOf>,
            Or<(
                Changed<Position>,
                Changed<Rotation>,
                Changed<ColliderTransform>,
            )>,
        ),
    >,
) {
    // Avian placeholder positions are finite; reject them as well as non-finite values.
    let poisoned = |v: Vec3| !v.is_finite() || v == Position::PLACEHOLDER.0;
    let bad_rotation =
        |q: Quat| !q.is_finite() || !q.length_squared().is_finite() || q.length_squared() == 0.0;
    for (entity, position, rotation, linear, angular) in &bodies {
        let bad_vel =
            linear.is_some_and(|v| poisoned(v.0)) || angular.is_some_and(|v| poisoned(v.0));
        if poisoned(position.0) || bad_rotation(rotation.0) || bad_vel {
            panic!(
                "net: FIXED-NAN root {entity}: pos={:?} rot={:?} linvel={:?} angvel={:?}",
                position.0,
                rotation.0,
                linear.map(|v| v.0),
                angular.map(|v| v.0)
            );
        }
    }
    for (entity, name, position, rotation, collider_transform) in &parts {
        let bad = position.is_some_and(|p| poisoned(p.0))
            || rotation.is_some_and(|r| bad_rotation(r.0))
            || collider_transform
                .is_some_and(|t| poisoned(t.translation) || bad_rotation(t.rotation.0));
        if bad {
            panic!(
                "net: FIXED-NAN part {entity} ({:?}): pos={:?} rot={:?} collider_transform={:?}",
                name.map(|n| n.as_str()),
                position.map(|p| p.0),
                rotation.map(|r| r.0),
                collider_transform
            );
        }
    }
}

/// Refuse a non-finite pose, preserving its hierarchy in the panic.
pub(crate) fn nan_tripwire(
    positions: Query<(Entity, &Position)>,
    transforms: Query<(Entity, &Transform)>,
    names: Query<&Name>,
    parents: Query<&ChildOf>,
) {
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
            panic!(
                "client: NAN-TRIPWIRE Position on {} = {:?}",
                describe(entity),
                position.0
            );
        }
    }
    for (entity, transform) in &transforms {
        if !(transform.translation.is_finite()
            && transform.rotation.is_finite()
            && transform.scale.is_finite())
        {
            panic!(
                "client: NAN-TRIPWIRE Transform on {} = {:?}",
                describe(entity),
                transform
            );
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

/// Log the first replicated tank marker.
pub(crate) fn log_connected(add: On<Add, Connected>) {
    info!("client: connected (entity {})", add.entity);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce;

    /// Avian's finite placeholder is not a world pose. The fixed boundary must stop it before
    /// physics, rather than let a latched diagnostic permit subsequent steps.
    #[test]
    #[should_panic(expected = "FIXED-NAN root")]
    fn an_uninitialized_body_pose_stops_before_physics() {
        let mut world = World::new();
        world.spawn((Tank, Position::PLACEHOLDER, Rotation::IDENTITY));
        world.run_system_once(fixed_nan_probe).unwrap();
    }

    /// The probe skips parts nothing wrote since its last run, so a part checked finite once and
    /// written corrupt later must still stop before physics on the next tick. Mutant: checking
    /// only parts born since the last run lets every later corruption through.
    #[test]
    #[should_panic(expected = "FIXED-NAN part")]
    fn a_part_written_corrupt_after_its_first_check_still_stops() {
        let mut app = App::new();
        app.add_systems(Update, fixed_nan_probe);
        let body = app
            .world_mut()
            .spawn((
                Position(Vec3::ZERO),
                Rotation::IDENTITY,
                GlobalTransform::IDENTITY,
            ))
            .id();
        let part = app
            .world_mut()
            .spawn((
                ColliderOf { body },
                Position(Vec3::X),
                Rotation::IDENTITY,
                GlobalTransform::from_translation(Vec3::X),
            ))
            .id();
        app.update();
        app.update();
        app.world_mut().get_mut::<Position>(part).unwrap().0 = Vec3::splat(f32::NAN);
        app.update();
    }

    /// A corrupt velocity must stop before its integration poisons an otherwise finite pose.
    #[test]
    #[should_panic(expected = "FIXED-NAN root")]
    fn a_non_finite_velocity_stops_before_physics() {
        let mut world = World::new();
        world.spawn((
            Tank,
            Position(Vec3::ZERO),
            Rotation::IDENTITY,
            LinearVelocity(Vec3::splat(f32::NAN)),
        ));
        world.run_system_once(fixed_nan_probe).unwrap();
    }

    /// The presentation tripwire stops at the first bad pose and retains the named hierarchy so
    /// the broken construction can be found; continuing after a diagnostic loses that boundary.
    #[test]
    #[should_panic(expected = "corrupt-child")]
    fn a_corrupt_view_pose_stops_with_its_hierarchy() {
        let mut world = World::new();
        let parent = world.spawn((Name::new("parent"), Transform::IDENTITY)).id();
        world.spawn((
            Name::new("corrupt-child"),
            ChildOf(parent),
            Transform::from_translation(Vec3::splat(f32::NAN)),
        ));
        world.run_system_once(nan_tripwire).unwrap();
    }
}
