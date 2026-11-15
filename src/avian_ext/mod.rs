//! The game's half of the vendored avian3d fork (`vendor/avian3d-0.8.0-overmatch`): the physics
//! laws the game owns, attached to Avian through public schedule slots, components and resources,
//! or through a narrow hook the fork exposes. Each file is one seam; the fork's
//! `OVERMATCH_PATCH.md` names each seam's form and its guard.
//!
//! These modules depend on Avian and Bevy only, never on the rest of the game, so the fork's guard
//! crate can build them beside it.
//!
//! [`plugin`] installs every seam; each composition of Avian's `PhysicsPlugins` adds it after them.

pub mod attachment;
pub mod child_poses;
pub mod rest_test;
pub mod sleeping_increments;

use avian3d::prelude::*;
use bevy::prelude::*;

/// Every seam of the game's half of the fork, installed into Avian's `PhysicsSchedule`. Added after
/// `PhysicsPlugins`, whose schedule it extends.
pub fn plugin(app: &mut App) {
    if !app.is_plugin_added::<child_poses::ReenabledPlugin>() {
        app.add_plugins(child_poses::ReenabledPlugin);
    }
    app.add_observer(attachment::refuse_unposed_attachment);
    app.get_schedule_mut(PhysicsSchedule)
        .expect("avian_ext::plugin is added after Avian's PhysicsPlugins")
        .add_systems(child_poses::place_child_colliders.in_set(PhysicsStepSystems::First));
    sleeping_increments::plugin(app);
}
