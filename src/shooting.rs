//! The player's gun control: fire on click (raising a `ballistics::FireShell`), enforce the reload
//! cooldown (gated by the Loader position), and recoil the barrel. The trajectory itself lives in
//! `ballistics` — this module owns only what makes it the *player's* gun. The armor sandbox drives
//! the same `FireShell` from its free-fly camera instead.

use bevy::ecs::lifecycle::Add;
use bevy::prelude::*;

use crate::ballistics::FireShell;
use crate::damage::{Capability, ControlledTank};
use crate::state::GameplaySet;
use crate::tank::{Gun, GunBarrel, Muzzle};

/// Muzzle velocity of the 88mm gun (m/s). The world is in meters, so this is literal.
const MUZZLE_SPEED: f32 = 773.0;
/// Shell calibre (m) — the 88. Drives overmatch in the penetration march.
const CALIBER: f32 = 0.088;
/// Projectile mass (kg) — the 88's PzGr 39 (~10.2 kg). Primary driver of penetration capability.
const SHELL_MASS: f32 = 10.2;
/// Reload cooldown before the gun can fire again (s). Placeholder — tune to the gun later.
const RELOAD_SECS: f32 = 3.0;

/// Backward impulse on firing (m/s along the bore). Higher = harder, longer kick.
const RECOIL_KICK: f32 = 14.0;
/// Spring stiffness pulling the barrel back to battery. Lower = longer stroke + slower return.
const RECOIL_STIFFNESS: f32 = 90.0;
/// Damping; slightly underdamped, so the barrel lumbers home with a small settle.
const RECOIL_DAMPING: f32 = 14.0;

/// Procedural barrel recoil: a 1-DOF damped spring on the barrel. Firing kicks it back along
/// the bore (+local Z); the spring returns it to battery. The translational cousin of `Servo`.
#[derive(Component)]
struct Recoil {
    rest: Vec3,
    offset: f32,
    velocity: f32,
}

/// Gun reload state: seconds remaining before the next shot. 0 = ready (loaded). A component on the
/// `Gun` entity (per-gun, not a singleton resource). Ticks down only while the Loader position is
/// staffed by a living crewman (design §7a) — a dead Loader freezes the reload partway through.
#[derive(Component)]
pub struct Reload {
    pub remaining: f32,
}

pub fn plugin(app: &mut App) {
    // attach_recoil + attach_reload react to rig binding (observers), so they stay out of the set.
    app.add_observer(attach_recoil)
        .add_observer(attach_reload)
        .add_systems(Update, (tick_reload, fire).chain().in_set(GameplaySet))
        .add_systems(FixedUpdate, apply_recoil.in_set(GameplaySet));
}

/// Attach `Recoil` the moment the rig binds `GunBarrel`, capturing its rest (battery) position
/// from the barrel's (parent-local) Transform. Keeps recoil (a shooting concern) out of the rig.
fn attach_recoil(add: On<Add, GunBarrel>, barrels: Query<&Transform>, mut commands: Commands) {
    let Ok(transform) = barrels.get(add.entity) else {
        return;
    };
    commands.entity(add.entity).insert(Recoil {
        rest: transform.translation,
        offset: 0.0,
        velocity: 0.0,
    });
}

/// Attach `Reload` the moment the rig binds `Gun`. Starts ready (0 = loaded).
fn attach_reload(add: On<Add, Gun>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert(Reload { remaining: 0.0 });
}

/// Tick the reload timer down — but only while the Load capability is available (Loader staffed +
/// Breech intact). A dead Loader or broken Breech freezes the reload partway through; a backfilled
/// Loader (slice 2) would resume it.
fn tick_reload(
    time: Res<Time>,
    controlled: ControlledTank,
    mut gun: Query<&mut Reload, With<Gun>>,
) {
    let Some(gun_entity) = controlled.rig().map(|rig| rig.gun) else {
        return;
    };
    let Ok(mut reload) = gun.get_mut(gun_entity) else {
        return;
    };
    if reload.remaining > 0.0 && controlled.available(Capability::Load) {
        reload.remaining = (reload.remaining - time.delta_secs()).max(0.0);
    }
}

fn fire(
    mouse: Res<ButtonInput<MouseButton>>,
    controlled: ControlledTank,
    muzzle: Query<&GlobalTransform, With<Muzzle>>,
    mut gun: Query<&mut Reload, With<Gun>>,
    mut barrel: Query<&mut Recoil>,
    mut commands: Commands,
) {
    let Some(rig) = controlled.rig() else {
        return;
    };
    let (gun_entity, muzzle_entity, barrel_entity) = (rig.gun, rig.muzzle, rig.barrel);
    let Ok(mut reload) = gun.get_mut(gun_entity) else {
        return;
    };
    if reload.remaining > 0.0 || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Ok(muzzle) = muzzle.get(muzzle_entity) else {
        return;
    };
    if !controlled.available(Capability::Fire) {
        return;
    }

    // Hand off to ballistics: fire down the bore at muzzle speed. The trajectory is its concern.
    commands.trigger(FireShell {
        origin: muzzle.translation(),
        direction: muzzle.forward(),
        speed: MUZZLE_SPEED,
        caliber: CALIBER,
        mass: SHELL_MASS,
    });

    // Kick the barrel back; apply_recoil springs it home.
    if let Ok(mut recoil) = barrel.get_mut(barrel_entity) {
        recoil.velocity += RECOIL_KICK;
    }
    reload.remaining = RELOAD_SECS;
}

fn apply_recoil(mut barrel: Query<(&mut Transform, &mut Recoil)>, time: Res<Time>) {
    let dt = time.delta_secs();
    for (mut transform, mut recoil) in &mut barrel {
        // Damped spring back to battery: offset'' = -k·offset - c·offset'.
        let accel = -RECOIL_STIFFNESS * recoil.offset - RECOIL_DAMPING * recoil.velocity;
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
