//! Passive network diagnostics. No system here mutates simulation state.

use avian3d::prelude::{
    AngularVelocity, ColliderOf, ColliderTransform, LinearVelocity, Position, RigidBody, Rotation,
};
use bevy::prelude::*;
use lightyear::prelude::*;

use lightyear::prelude::input::native::NativeBuffer;

use super::protocol::NetTank;
use crate::ballistics::ShellPath;
use crate::command::TankCommand;
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

/// Arrival-margin statistics for client inputs, server side: how many ticks of authored input
/// stand between the newest arrival and the tick the server is about to simulate. Negative margin
/// = the server simulates a tick no input was authored for yet — lightyear leaves `ActionState`
/// untouched (hold-last for movement levels; consumables fail closed via the attestation), and
/// that path has no upstream counter. This resource is the counter that gates any shrink of the
/// client's sync margins (`net::sync_margin`).
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct InputArrival {
    /// Cumulative late ticks (margin < 0) since startup, across all clients.
    pub(crate) late_total: u64,
    /// Current heartbeat window, reset by [`log_input_arrival`].
    window: ArrivalWindow,
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
struct ArrivalWindow {
    ticks: u64,
    late: u64,
    min_margin: Option<i32>,
    max_margin: Option<i32>,
}

impl InputArrival {
    /// Fold one buffer's margin for one simulated tick. Margin 0 is on time: the input authored
    /// for this exact tick has arrived.
    fn fold(&mut self, margin: i32) {
        self.window.ticks += 1;
        if margin < 0 {
            self.window.late += 1;
            self.late_total += 1;
        }
        self.window.min_margin = Some(self.window.min_margin.map_or(margin, |m| m.min(margin)));
        self.window.max_margin = Some(self.window.max_margin.map_or(margin, |m| m.max(margin)));
    }

    fn take_window(&mut self) -> ArrivalWindow {
        core::mem::take(&mut self.window)
    }
}

/// Per fixed tick, before lightyear reads the input buffers (`FixedPreUpdate`, ahead of
/// `UpdateActionState`): record each client buffer's `last_remote_tick − tick`. A buffer that has
/// not received its first input yet contributes nothing — there is no authored stream to be late.
pub(crate) fn sample_input_arrival(
    timeline: Res<LocalTimeline>,
    buffers: Query<&NativeBuffer<TankCommand>>,
    mut stats: ResMut<InputArrival>,
) {
    let tick = timeline.tick();
    for buffer in &buffers {
        let Some(last) = buffer.last_remote_tick else {
            continue;
        };
        stats.fold(last - tick);
    }
}

/// The arrival-margin heartbeat, on the same cadence as the SIM-EVIDENCE lines. `late_total` must
/// stay 0 (or the startup transient only) for the shrunken sync margins to be certified; the
/// window min is the number the client's quantization floor must clear.
pub(crate) fn log_input_arrival(
    mut stats: ResMut<InputArrival>,
    mut timer: Local<f32>,
    time: Res<Time>,
) {
    *timer += time.delta_secs();
    if *timer < 2.0 {
        return;
    }
    *timer = 0.0;
    let late_total = stats.late_total;
    let window = stats.take_window();
    if window.ticks == 0 {
        return;
    }
    info!(
        "net: INPUT-ARRIVAL margin min={:?} max={:?} late={}/{} (late_total={})",
        window.min_margin, window.max_margin, window.late, window.ticks, late_total,
    );
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

#[cfg(test)]
mod tests {
    use super::InputArrival;

    /// Margin 0 is ON TIME (the input authored for this exact tick has arrived); only a negative
    /// margin is a late tick. Fails if the boundary moves to `<= 0` or the cumulative counter
    /// stops accumulating across windows.
    #[test]
    fn late_is_strictly_negative_margin_and_survives_the_window_reset() {
        let mut stats = InputArrival::default();
        stats.fold(2);
        stats.fold(0);
        stats.fold(-1);
        assert_eq!(stats.late_total, 1, "only the -1 margin is late");
        let window = stats.take_window();
        assert_eq!((window.ticks, window.late), (3, 1));
        assert_eq!(
            (window.min_margin, window.max_margin),
            (Some(-1), Some(2)),
            "window extrema"
        );
        stats.fold(-3);
        assert_eq!(stats.late_total, 2, "cumulative count crosses windows");
        let next = stats.take_window();
        assert_eq!(
            (next.ticks, next.late, next.min_margin),
            (1, 1, Some(-3)),
            "the window itself must reset"
        );
    }
}
