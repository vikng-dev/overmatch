use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use super::model::GunBarrel;
use super::servo::ServoSpec;

/// Name-keyed sim part table used to attach a separately loaded view tree.
#[derive(Component)]
pub struct SimParts(pub HashMap<String, Entity>);

/// View-to-simulation link for a same-named part.
#[derive(Component, Clone, Copy)]
pub struct ViewOf(pub Entity);

/// Simulation-to-view link used by render readers and pose writers.
#[derive(Component, Clone, Copy)]
pub struct ViewNode(pub Entity);

impl ViewNode {
    /// Resolve the view node, falling back to fixed-step sim pose before presentation attaches.
    pub fn resolve(view: Option<&ViewNode>, sim: Entity) -> Entity {
        view.map_or(sim, |view| view.0)
    }
}

/// A view node written by servo interpolation; sim-node transforms remain fixed-step truth.
#[derive(Component)]
pub struct ViewServo;

/// Join an instantiated GLB view to the already-complete sim skeleton by node name. This hides
/// authored physics meshes, seeds moving parts from current sim pose, and repairs a turret view
/// whose sim part detached before presentation arrived. It never creates simulation state.
pub fn bind_tank_view(
    ready: On<WorldInstanceReady>,
    roots: Query<&SimParts>,
    children: Query<&Children>,
    names: Query<&Name>,
    meshes: Query<(), With<Mesh3d>>,
    servos: Query<(), With<ServoSpec>>,
    barrels: Query<(), With<GunBarrel>>,
    launched: Query<(), With<crate::damage::LaunchedTurret>>,
    transforms: Query<&Transform>,
    mut commands: Commands,
) {
    let Ok(parts) = roots.get(ready.entity) else {
        return;
    };
    // Both trees share names; skip sim entities so links cannot point back to themselves.
    let skeleton: HashSet<Entity> = parts.0.values().copied().collect();
    for entity in children.iter_descendants(ready.entity) {
        if skeleton.contains(&entity) {
            continue;
        }
        let Ok(name) = names.get(entity) else {
            continue;
        };
        if name.as_str().ends_with("_Collider") || name.as_str().ends_with("_Ballistic") {
            commands.entity(entity).insert(Visibility::Hidden);
        }
        // Mesh leaves may share object names but are not part anchors.
        if meshes.contains(entity) {
            continue;
        }
        let Some(&sim) = parts.0.get(name.as_str()) else {
            continue;
        };
        // The launch observer may have fired before this view existed.
        if launched.contains(sim) {
            commands.entity(sim).insert(Visibility::default());
            commands
                .entity(entity)
                .insert((ChildOf(sim), Transform::IDENTITY));
            continue;
        }
        commands.entity(entity).insert(ViewOf(sim));
        commands.entity(sim).insert(ViewNode(entity));
        // Avoid flashing authored rest pose when presentation attaches mid-motion.
        if (servos.contains(sim) || barrels.contains(sim))
            && let Ok(&pose) = transforms.get(sim)
        {
            commands.entity(entity).insert(pose);
        }
        if servos.contains(sim) {
            commands.entity(entity).insert(ViewServo);
        }
    }
}

/// Copy fixed-step barrel recoil into the corresponding view node.
fn sync_view_barrels(
    barrels: Query<(&Transform, &ViewNode), With<GunBarrel>>,
    mut views: Query<&mut Transform, Without<GunBarrel>>,
) {
    for (source, view) in &barrels {
        if let Ok(mut dest) = views.get_mut(view.0) {
            dest.set_if_neq(*source);
        }
    }
}

/// Reparent a launched turret's view subtree under its free simulation body.
fn detach_view_on_turret_launch(
    add: On<Add, crate::damage::LaunchedTurret>,
    views: Query<&ViewNode>,
    mut commands: Commands,
) {
    let Ok(view) = views.get(add.entity) else {
        return;
    };
    // The detached sim body becomes a new visibility root.
    commands.entity(add.entity).insert(Visibility::default());
    commands
        .entity(view.0)
        .insert((ChildOf(add.entity), Transform::IDENTITY))
        .remove::<(ViewOf, ViewServo)>();
}

/// Install presentation attachment and barrel pose synchronization.
pub fn view_attach_plugin(app: &mut App) {
    app.add_observer(detach_view_on_turret_launch);
    app.add_systems(Update, sync_view_barrels);
}
