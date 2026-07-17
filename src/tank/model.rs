use std::collections::HashMap;

use bevy::prelude::*;

use super::servo::ServoState;
use crate::command::TankCommand;
use crate::damage::Requirement;
use crate::spec::{FireMode, RecoilSpec, Trigger, ViewKind};
use crate::track::sim::{TrackContacts, TrackDrive};

#[derive(Component)]
pub struct Turret;

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Local simulation root. Command and drive state are required in the same construction flush.
#[derive(Component)]
#[require(TankCommand, TrackDrive, TrackContacts)]
pub struct Tank;

/// Selects the tank that local input and player-facing systems address.
#[derive(Component)]
pub struct Controlled;

/// Required rig handles, resolved and validated during complete construction.
#[derive(Component)]
pub struct Rig {
    pub hull: Entity,
    pub turret: Entity,
    pub gun: Entity,
    pub muzzle: Entity,
}

/// Runtime camera configuration and availability gate for one authored view.
pub struct ViewConfig {
    pub fov: f32,
    pub requires: Requirement,
}

/// Authored view configurations keyed by player-facing view kind.
#[derive(Component)]
pub struct TankViews(pub HashMap<ViewKind, ViewConfig>);

#[derive(Component)]
pub struct Muzzle;

/// The recoiling barrel node (child of `Gun`, parent of `Muzzle`).
#[derive(Component)]
pub struct GunBarrel;

/// Runtime weapon configuration attached to its muzzle.
#[derive(Component)]
pub struct Weapon {
    /// Logical authored name, distinct from the muzzle node name.
    pub name: String,
    pub speed: f32,
    pub caliber: f32,
    pub mass: f32,
    /// Reload/cyclic mechanism and its edge-versus-level input semantics.
    pub fire_mode: FireMode,
    pub recoil: Option<RecoilSpec>,
    pub barrel: Option<Entity>,
    /// Damage/crew gates for firing and loading.
    pub fire: Requirement,
    pub load: Requirement,
    /// Command channel for this weapon; independent of its mechanism.
    pub trigger: Trigger,
}

/// Which track a roadwheel drives (for differential thrust). Left wheels sit at −X, right at +X.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TrackSide {
    Left,
    Right,
}

/// Load-bearing belt-contact station; sprockets and idlers are excluded.
#[derive(Component)]
pub struct Roadwheel {
    pub side: TrackSide,
}

/// Direct owner link for rig-part systems that need root state.
#[derive(Component)]
pub struct TankRoot(pub Entity);

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WeaponState {
    /// Reload timer for single-shot weapons, or cyclic/belt-swap timer for automatics.
    pub reload_remaining: f32,
    pub recoil_offset: f32,
    pub recoil_velocity: f32,
    /// Belt phase used to derive tracer cadence. It rolls back, but stays outside the determinism
    /// hash because a phase mismatch is cosmetic and does not gate simulation.
    pub rounds_fired: u32,
    /// Remaining automatic rounds. Unlike tracer phase, this gates fire and is hashed.
    pub belt_remaining: u32,
}

impl WeaponState {
    /// Loaded spawn state; automatic weapons start with a full belt.
    pub fn for_mode(mode: &FireMode) -> Self {
        Self {
            belt_remaining: match mode {
                FireMode::Single { .. } => 0,
                FireMode::Automatic { belt_size, .. } => *belt_size,
            },
            ..Self::default()
        }
    }
}

/// Rollback state lives on the replicated root. Children carry deterministic, name-sorted indices
/// into these vectors and derive their transforms from the restored root state.
#[derive(Component, Clone, PartialEq, Debug, Default)]
pub struct TankSim {
    pub servos: Vec<ServoState>,
    pub weapons: Vec<WeaponState>,
}

/// This weapon's slot in [`TankSim::weapons`] — on the muzzle AND the recoiling barrel (both
/// actuate from the same weapon state), assigned at spawn in sorted-name order.
#[derive(Component, Clone, Copy)]
pub struct WeaponIndex(pub usize);

/// Compose tick-truth local transforms from the physics root. Simulation must not read
/// `GlobalTransform`, which belongs to the interpolated render frame. Returns `None` when the node
/// is no longer under `root`.
pub(crate) fn rig_world_pose(
    node: Entity,
    root: Entity,
    root_position: Vec3,
    root_rotation: Quat,
    parents: &Query<&ChildOf>,
    locals: &Query<&Transform>,
) -> Option<(Vec3, Quat)> {
    let mut chain = Vec::new();
    let mut entity = node;
    while entity != root {
        chain.push(entity);
        entity = parents.get(entity).ok()?.parent();
    }
    let mut position = root_position;
    let mut rotation = root_rotation;
    for &link in chain.iter().rev() {
        let local = locals.get(link).ok()?;
        position += rotation * local.translation;
        rotation *= local.rotation;
    }
    Some((position, rotation))
}
