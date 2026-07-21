use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::servo::ServoState;
use crate::command::TankCommand;
use crate::damage::Requirement;
use crate::spec::{FireMode, RecoilSpec, Trigger, ViewKind};
use crate::track::sim::{TrackContacts, TrackDrive, TrackGrip};

#[derive(Component)]
pub struct Turret;

#[derive(Component)]
pub struct Gun;

#[derive(Component)]
pub struct Hull;

/// Local simulation root. Command and drive state are required in the same construction flush.
#[derive(Component)]
#[require(TankCommand, TrackDrive, TrackGrip, TrackContacts)]
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

/// Local per-weapon rollback state that does not decide whether a round may fire. The authoritative
/// fire gate lives in the separately replicated [`WeaponGate`] component.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct WeaponState {
    pub recoil_offset: f32,
    pub recoil_velocity: f32,
    /// Belt phase used to derive tracer cadence. It rolls back, but stays outside the determinism
    /// hash because a phase mismatch is cosmetic and does not gate simulation.
    pub rounds_fired: u32,
}

/// One weapon's complete fire-eligibility state. `ready_tick` is an absolute simulation deadline:
/// `None` with no pause means loaded/ready, while `Some(tick)` means cyclic, single reload, or belt
/// swap according to the weapon's authored [`FireMode`] and `belt_remaining`. `paused_at_tick`
/// freezes crew work without changing this component again until work resumes; there is no per-tick
/// countdown.
///
/// Lightyear ticks saturate rather than wrap. If arming or shifting a deadline would exceed
/// `u32::MAX`, the gate fail-stops as `(ready_tick: None, paused_at_tick: Some(u32::MAX))`. That
/// otherwise-unreachable pair is permanently not-ready, so a clock pinned at its maximum cannot
/// repeatedly fire a deadline that saturated to the same tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WeaponGateState {
    pub ready_tick: Option<u32>,
    pub paused_at_tick: Option<u32>,
    pub belt_remaining: u32,
}

impl WeaponGateState {
    const EXHAUSTED_TICK: u32 = u32::MAX;

    /// Loaded spawn state; automatic weapons start with a full belt.
    pub fn for_mode(mode: &FireMode) -> Self {
        Self {
            ready_tick: None,
            paused_at_tick: None,
            belt_remaining: match mode {
                FireMode::Single { .. } => 0,
                FireMode::Automatic { belt_size, .. } => *belt_size,
            },
        }
    }

    pub fn is_ready(self) -> bool {
        self.ready_tick.is_none() && self.paused_at_tick.is_none()
    }

    pub fn is_exhausted(self) -> bool {
        self.ready_tick.is_none() && self.paused_at_tick == Some(Self::EXHAUSTED_TICK)
    }

    /// Whether an armed deadline is due in Lightyear's ordinary numeric tick order. An absent
    /// deadline remains due; [`Self::is_ready`] separately distinguishes ready from fail-stopped.
    pub fn deadline_reached(self, now: u32) -> bool {
        self.ready_tick.is_none_or(|ready_tick| now >= ready_tick)
    }

    pub fn arm(&mut self, now: u32, delay_ticks: u32) {
        if let Some(ready_tick) = now.checked_add(delay_ticks.max(1)) {
            self.ready_tick = Some(ready_tick);
            self.paused_at_tick = None;
        } else {
            self.freeze_exhausted();
        }
    }

    /// Freeze a crew-gated reload/swap. Repeated unmet ticks leave the gate byte-identical.
    pub fn pause(&mut self, now: u32) {
        if let (Some(ready_tick), None) = (self.ready_tick, self.paused_at_tick) {
            // This first unmet tick must not consume work, so defer once on entry. Later paused
            // ticks are accounted for in one shift on resume.
            if let Some(ready_tick) = ready_tick.checked_add(1) {
                self.ready_tick = Some(ready_tick);
                self.paused_at_tick = Some(now);
            } else {
                self.freeze_exhausted();
            }
        }
    }

    /// Resume crew work by shifting the deadline once by the paused interval.
    pub fn resume(&mut self, now: u32) {
        let (Some(ready_tick), Some(paused_at_tick)) = (self.ready_tick, self.paused_at_tick)
        else {
            return;
        };
        let later_paused_ticks = now.saturating_sub(paused_at_tick).saturating_sub(1);
        if let Some(ready_tick) = ready_tick.checked_add(later_paused_ticks) {
            self.ready_tick = Some(ready_tick);
            self.paused_at_tick = None;
        } else {
            self.freeze_exhausted();
        }
    }

    pub fn remaining_ticks(self, now: u32) -> u32 {
        if self.is_exhausted() {
            return u32::MAX;
        }
        let effective_now = self.paused_at_tick.unwrap_or(now);
        self.ready_tick
            .map_or(0, |ready_tick| ready_tick.saturating_sub(effective_now))
    }

    fn freeze_exhausted(&mut self) {
        self.ready_tick = None;
        self.paused_at_tick = Some(Self::EXHAUSTED_TICK);
    }
}

/// Tick-correlated authority state for every weapon slot, in deterministic name-sorted slot order.
/// This is one atomic replicated + owner-predicted component, following `TankTransmission`: a
/// confirmed value is restored at its producing tick and normal replay derives the present.
#[derive(Component, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct WeaponGate {
    pub weapons: Vec<WeaponGateState>,
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
