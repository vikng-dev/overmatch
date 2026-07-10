//! The player's gun control: fire on click (raising a `ballistics::FireShell`), enforce the reload
//! cooldown (gated by the Loader position), and recoil the barrel. The trajectory itself lives in
//! `ballistics` — this module owns only what makes it the *player's* gun. The armor sandbox drives
//! the same `FireShell` from its free-fly camera instead.

use avian3d::prelude::{Forces, Position, Rotation, WriteRigidBodyForces};
use bevy::prelude::*;

use crate::ballistics::{FireShell, ShotSource};
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{TankVolumes, VolumeFacets, requirement_met};
use crate::spec::Trigger;
use crate::state::GameplaySet;
use crate::tank::{Muzzle, Tank, TankRoot, TankSim, Weapon, WeaponIndex, rig_world_pose};

/// Feel multiplier on the hull recoil impulse (1.0 = physical momentum). On a 57 t hull true momentum
/// is a gentle rock by design; bump this if the firing kick should read more dramatically.
const RECOIL_FEEL: f32 = 1.0;

/// Procedural barrel recoil CONFIG: the damped-spring tuning + the barrel's rest (battery)
/// position, built by `tank::spawn_tank_sim` from the weapon's `recoil` spec and the barrel
/// node's authored translation — spawn-time data, not a bind-time transform capture. The recoil
/// STATE (offset/velocity) is sim truth — the muzzle rides the barrel — and lives root-resident
/// in `TankSim::weapons` (see `TankSim`), keyed by the barrel's `WeaponIndex`. The translational
/// cousin of `Servo`.
#[derive(Component)]
pub(crate) struct RecoilParams {
    pub(crate) rest: Vec3,
    pub(crate) stiffness: f32,
    pub(crate) damping: f32,
}

/// THE single expression of how a fired weapon kicks its barrel: add `weapon`'s `recoil.kick` to
/// slot `slot`'s recoil velocity in `sim`. Owns the WHOLE decision — a weapon with no `recoil` spec
/// (a coax), no `barrel` node, or a slot absent from `sim.weapons` (a rig still spawning, a bad slot
/// off the wire) kicks nothing. Both the local shooter ([`fire`], this module) and the remote-recoil
/// applier (`net::client::apply_pending_recoil_kicks`) pass `(sim, slot, weapon)` and are IDENTICAL
/// by construction: "how a shot recoils the gun" is one implementation, not two that agree today
/// (the derive-the-consequence doctrine, ADR-0016 — a derive that branches differently per end is
/// two implementations).
///
/// The `barrel` gate lives HERE, not at the call sites, and is load-bearing: `apply_recoil` only
/// steps slots that have `RecoilParams`, which `spawn_tank_sim` builds on the barrel node — so a
/// kick on a barrel-less slot would land in `recoil_velocity` and NEVER decay, accumulating without
/// bound in rollback-tracked `TankSim` state, shot after shot. Gating in one place makes that
/// unreachable on both ends.
pub(crate) fn kick_recoil(sim: &mut TankSim, slot: usize, weapon: &Weapon) {
    // No barrel node to recoil (no `RecoilParams`, so `apply_recoil` never springs it back), or a
    // recoil-less weapon — either way, no kick.
    let (Some(_), Some(recoil)) = (weapon.barrel, weapon.recoil.as_ref()) else {
        return;
    };
    if let Some(state) = sim.weapons.get_mut(slot) {
        state.recoil_velocity += recoil.kick;
    }
}

pub fn plugin(app: &mut App) {
    // The gun is sim: reload and firing run on the fixed clock, driven by each tank's `TankCommand`
    // — `fire` consumes the click edge, so it must precede the command layer's edge clear.
    //
    // `apply_recoil.after(fire)` is DETERMINISM-LOAD-BEARING, not a preference: both systems take
    // `&mut TankSim`, and without an explicit edge Bevy's executor may serialize them in either
    // order — an order that measurably differed between client and server processes (2026-07-10,
    // divergence instrument): on the fire tick one end integrated the spring before the kick and
    // the other after, a one-tick recoil phase offset that read as a 33-tick `hrec` divergence
    // window per shot. The canonical order is kick-then-integrate — a shot's kick springs the
    // barrel the SAME tick, matching what the remote-fire path already promises
    // (`net::client::apply_pending_recoil_kicks` runs `.before(GameplaySet)` for exactly that).
    //
    // The remaining unordered `&mut TankSim` neighbors (driving.rs's suspension/drive chain) write
    // DISJOINT TankSim fields (anchors, never weapons), so their order against these systems
    // cannot change values today. If a shooting system ever touches anchors — or a driving system
    // touches weapons — that pair must be ordered explicitly too.
    app.add_systems(
        FixedUpdate,
        (
            (tick_reload, fire).chain().before(ConsumeCommandEdges),
            apply_recoil.after(fire),
        )
            .in_set(GameplaySet),
    );
}

/// Tick every weapon's reload timer down — but only while its own tank meets the weapon's `load`
/// requirement (Loader staffed + Breech intact). A dead Loader or broken Breech freezes the
/// reload partway through; a backfilled Loader (slice 2) would resume it. Per-tank, not
/// controlled-only: a tank keeps loading whether you're in it or (later) it's a network peer's.
fn tick_reload(
    time: Res<Time>,
    tanks: Query<Option<&TankVolumes>, With<Tank>>,
    volumes: Query<VolumeFacets>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
) {
    for (weapon, slot, root) in &weapons {
        let Ok(tank_volumes) = tanks.get(root.0) else {
            continue;
        };
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.weapons.get_mut(slot.0) else {
            continue;
        };
        if state.reload_remaining > 0.0 && requirement_met(tank_volumes, &weapon.load, &volumes) {
            state.reload_remaining = (state.reload_remaining - time.delta_secs()).max(0.0);
        }
    }
}

/// Fire each tank's weapons whose trigger its command holds this tick: `fire_primary` → the main
/// gun (single shot — the command layer latches the click edge to exactly one tick),
/// `fire_secondary` (held) → the MGs (cyclic via their short reload). Each weapon fires from its
/// *own* muzzle and ballistics, gated by its `fire` requirement + reload — the gate lives here in
/// the sim, where the server will enforce it, not in the input path.
fn fire(
    tanks: Query<(&TankCommand, Option<&TankVolumes>, &Position, &Rotation), With<Tank>>,
    volumes: Query<VolumeFacets>,
    weapons: Query<(Entity, &Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
    mut sims: Query<&mut TankSim>,
    mut bodies: Query<Forces, With<Tank>>,
    parents: Query<&ChildOf>,
    locals: Query<&Transform>,
    mut commands: Commands,
) {
    for (muzzle_entity, weapon, slot, root) in &weapons {
        let Ok((command, tank_volumes, position, rotation)) = tanks.get(root.0) else {
            continue;
        };
        let triggered = match weapon.trigger {
            Trigger::Primary => command.fire_primary,
            Trigger::Secondary => command.fire_secondary,
        };
        let ready = sims
            .get(root.0)
            .ok()
            .and_then(|sim| sim.weapons.get(slot.0))
            .is_some_and(|w| w.reload_remaining <= 0.0);
        if !triggered || !ready || !requirement_met(tank_volumes, &weapon.fire, &volumes) {
            continue;
        }

        // Tick-truth muzzle pose (`rig_world_pose`, never `GlobalTransform` — see its doc): the
        // muzzle decides where the shell goes, so it must be the pose the server's tick also
        // computes, not the render picture. The chain composes the servo angles (tick-truth in
        // the fixed loop) and the barrel's recoil offset (stepped in `FixedUpdate`).
        let Some((muzzle_position, muzzle_rotation)) = rig_world_pose(
            muzzle_entity,
            root.0,
            position.0,
            rotation.0,
            &parents,
            &locals,
        ) else {
            continue;
        };
        let Ok(bore) = Dir3::new(muzzle_rotation * Vec3::NEG_Z) else {
            continue; // corrupt pose frame — hold the shot rather than fire NaN
        };

        // Hand off to ballistics: fire down the bore at the weapon's muzzle speed.
        commands.trigger(FireShell {
            origin: muzzle_position,
            direction: bore,
            speed: weapon.speed,
            caliber: weapon.caliber,
            mass: weapon.mass,
            // This shell belongs to a tank: name its root AND the firing weapon slot so the net
            // server can broadcast the cosmetic tracer and the barrel-recoil cause to every OTHER
            // client (`net::server`'s FireShell observer), which each derive the kick locally.
            shooter: Some(ShotSource {
                tank: root.0,
                weapon: slot.0,
            }),
            // Locally fired: the shell spawns at the muzzle THIS tick — no net catch-up.
            catch_up_ticks: 0,
        });
        // Kick the barrel back (root-resident recoil state); apply_recoil springs it home. The
        // shared `kick_recoil` owns the whole decision (barrel + recoil spec present, slot valid), so
        // this path and the opponent-view path (`net::client`) can't diverge on how a shot recoils.
        if let Ok(mut sim) = sims.get_mut(root.0) {
            kick_recoil(&mut sim, slot.0, weapon);
        }
        // Recoil reaction on the hull: the shell's momentum, opposite the bore, applied on the bore
        // axis. The line of action passes above the centre of mass, so the impulse-at-point also
        // pitches the nose up (gun climb), not just shoves the hull back. Each weapon kicks by its
        // own momentum, so the MGs barely register.
        if let Ok(mut forces) = bodies.get_mut(root.0) {
            let impulse = bore * (-weapon.mass * weapon.speed * RECOIL_FEEL);
            forces.apply_linear_impulse_at_point(impulse, muzzle_position);
        }
        if let Ok(mut sim) = sims.get_mut(root.0)
            && let Some(state) = sim.weapons.get_mut(slot.0)
        {
            state.reload_remaining = weapon.reload;
        }
    }
}

/// Step each recoiling barrel's damped spring (state root-resident in `TankSim::weapons`, so a
/// rollback replay re-derives the barrel — and therefore the muzzle — from restored state) and
/// write the barrel's node `Transform`.
fn apply_recoil(
    mut barrels: Query<(&mut Transform, &RecoilParams, &WeaponIndex, &TankRoot)>,
    mut sims: Query<&mut TankSim>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for (mut transform, params, slot, root) in &mut barrels {
        let Ok(mut sim) = sims.get_mut(root.0) else {
            continue;
        };
        let Some(state) = sim.weapons.get_mut(slot.0) else {
            continue;
        };
        // Damped spring back to battery: offset'' = -k·offset - c·offset'.
        let accel =
            -params.stiffness * state.recoil_offset - params.damping * state.recoil_velocity;
        state.recoil_velocity += accel * dt;
        state.recoil_offset += state.recoil_velocity * dt;
        // Battery stop — the barrel can't return past its rest position.
        if state.recoil_offset < 0.0 {
            state.recoil_offset = 0.0;
            state.recoil_velocity = 0.0;
        }
        // Recoil rides back along the bore (+local Z), measured from the rest position.
        transform.translation = params.rest + Vec3::Z * state.recoil_offset;
    }
}
