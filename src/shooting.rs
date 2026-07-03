//! The player's gun control: fire on click (raising a `ballistics::FireShell`), enforce the reload
//! cooldown (gated by the Loader position), and recoil the barrel. The trajectory itself lives in
//! `ballistics` — this module owns only what makes it the *player's* gun. The armor sandbox drives
//! the same `FireShell` from its free-fly camera instead.

use avian3d::prelude::{Forces, WriteRigidBodyForces};
use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;

use crate::ballistics::FireShell;
use crate::command::{ConsumeCommandEdges, TankCommand};
use crate::damage::{TankVolumes, VolumeFacets, requirement_met};
use crate::spec::Trigger;
use crate::state::GameplaySet;
use crate::tank::{Tank, TankRoot, Weapon};

/// Feel multiplier on the hull recoil impulse (1.0 = physical momentum). On a 57 t hull true momentum
/// is a gentle rock by design; bump this if the firing kick should read more dramatically.
const RECOIL_FEEL: f32 = 1.0;

/// Procedural barrel recoil: a 1-DOF damped spring on the barrel. Firing kicks it back along
/// the bore (+local Z); the spring returns it to battery. The translational cousin of `Servo`.
/// `stiffness`/`damping` are baked in from the weapon's `recoil` spec at setup, so `apply_recoil`
/// needs only this component (the per-weapon tuning travels with the barrel).
#[derive(Component)]
struct Recoil {
    rest: Vec3,
    offset: f32,
    velocity: f32,
    stiffness: f32,
    damping: f32,
}

/// Weapon reload state: seconds remaining before the next shot. 0 = ready (loaded). A component on
/// the weapon's muzzle entity (per-weapon, not a singleton). Ticks down only while the Load
/// capability holds (Loader staffed + Breech intact) — a dead Loader freezes it partway through.
#[derive(Component)]
pub struct Reload {
    pub remaining: f32,
}

pub fn plugin(app: &mut App) {
    // attach_weapon reacts to the binder attaching `Weapon` (an observer), so it stays out of the set.
    // The gun is sim: reload and firing run on the fixed clock, driven by each tank's `TankCommand`
    // — `fire` consumes the click edge, so it must precede the command layer's edge clear.
    app.add_observer(attach_weapon).add_systems(
        FixedUpdate,
        (
            (tick_reload, fire).chain().before(ConsumeCommandEdges),
            apply_recoil,
        )
            .in_set(GameplaySet),
    );
}

/// React to the binder attaching a `Weapon`: start its `Reload` (ready), and — if it recoils — set
/// up the barrel's `Recoil` from the barrel's rest pose plus the weapon's recoil tuning. Keeps the
/// shooting state out of the rig binder; the per-weapon numbers ride in from the spec via `Weapon`.
fn attach_weapon(
    add: On<Add, Weapon>,
    weapons: Query<&Weapon>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let Ok(weapon) = weapons.get(add.entity) else {
        return;
    };
    commands
        .entity(add.entity)
        .insert(Reload { remaining: 0.0 });
    if let (Some(barrel), Some(recoil)) = (weapon.barrel, weapon.recoil.as_ref())
        && let Ok(transform) = transforms.get(barrel)
    {
        commands.entity(barrel).insert(Recoil {
            rest: transform.translation,
            offset: 0.0,
            velocity: 0.0,
            stiffness: recoil.stiffness,
            damping: recoil.damping,
        });
    }
}

/// Tick every weapon's reload timer down — but only while its own tank meets the weapon's `load`
/// requirement (Loader staffed + Breech intact). A dead Loader or broken Breech freezes the
/// reload partway through; a backfilled Loader (slice 2) would resume it. Per-tank, not
/// controlled-only: a tank keeps loading whether you're in it or (later) it's a network peer's.
fn tick_reload(
    time: Res<Time>,
    tanks: Query<Option<&TankVolumes>, With<Tank>>,
    volumes: Query<VolumeFacets>,
    mut weapons: Query<(&mut Reload, &Weapon, &TankRoot)>,
) {
    for (mut reload, weapon, root) in &mut weapons {
        let Ok(tank_volumes) = tanks.get(root.0) else {
            continue;
        };
        if reload.remaining > 0.0 && requirement_met(tank_volumes, &weapon.load, &volumes) {
            reload.remaining = (reload.remaining - time.delta_secs()).max(0.0);
        }
    }
}

/// Fire each tank's weapons whose trigger its command holds this tick: `fire_primary` → the main
/// gun (single shot — the command layer latches the click edge to exactly one tick),
/// `fire_secondary` (held) → the MGs (cyclic via their short reload). Each weapon fires from its
/// *own* muzzle and ballistics, gated by its `fire` requirement + reload — the gate lives here in
/// the sim, where the server will enforce it, not in the input path.
fn fire(
    tanks: Query<(&TankCommand, Option<&TankVolumes>), With<Tank>>,
    volumes: Query<VolumeFacets>,
    mut weapons: Query<(&GlobalTransform, &Weapon, &mut Reload, &TankRoot)>,
    mut barrels: Query<&mut Recoil>,
    mut bodies: Query<Forces, With<Tank>>,
    mut commands: Commands,
) {
    for (muzzle, weapon, mut reload, root) in &mut weapons {
        let Ok((command, tank_volumes)) = tanks.get(root.0) else {
            continue;
        };
        let triggered = match weapon.trigger {
            Trigger::Primary => command.fire_primary,
            Trigger::Secondary => command.fire_secondary,
        };
        if !triggered
            || reload.remaining > 0.0
            || !requirement_met(tank_volumes, &weapon.fire, &volumes)
        {
            continue;
        }

        // Hand off to ballistics: fire down the bore at the weapon's muzzle speed.
        commands.trigger(FireShell {
            origin: muzzle.translation(),
            direction: muzzle.forward(),
            speed: weapon.speed,
            caliber: weapon.caliber,
            mass: weapon.mass,
        });
        // Kick the barrel back; apply_recoil springs it home.
        if let (Some(barrel), Some(recoil)) = (weapon.barrel, weapon.recoil.as_ref())
            && let Ok(mut state) = barrels.get_mut(barrel)
        {
            state.velocity += recoil.kick;
        }
        // Recoil reaction on the hull: the shell's momentum, opposite the bore, applied on the bore
        // axis. The line of action passes above the centre of mass, so the impulse-at-point also
        // pitches the nose up (gun climb), not just shoves the hull back. Each weapon kicks by its
        // own momentum, so the MGs barely register.
        if let Ok(mut forces) = bodies.get_mut(root.0) {
            let impulse = muzzle.forward() * (-weapon.mass * weapon.speed * RECOIL_FEEL);
            forces.apply_linear_impulse_at_point(impulse, muzzle.translation());
        }
        reload.remaining = weapon.reload;
    }
}

fn apply_recoil(mut barrel: Query<(&mut Transform, &mut Recoil)>, time: Res<Time>) {
    let dt = time.delta_secs();
    for (mut transform, mut recoil) in &mut barrel {
        // Damped spring back to battery: offset'' = -k·offset - c·offset'.
        let accel = -recoil.stiffness * recoil.offset - recoil.damping * recoil.velocity;
        recoil.velocity += accel * dt;
        recoil.offset += recoil.velocity * dt;
        // Battery stop — the barrel can't return past its rest position.
        if recoil.offset < 0.0 {
            recoil.offset = 0.0;
            recoil.velocity = 0.0;
        }
        // Recoil rides back along the bore (+local Z), measured from the rest position.
        transform.translation = recoil.rest + Vec3::Z * recoil.offset;
    }
}
