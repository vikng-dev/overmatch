use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use super::model::GunBarrel;
use super::servo::ServoSpec;
use crate::render_policy::VisualScope;

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
    blueprint: Option<Res<crate::bake::TankBlueprint>>,
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
    // Which glb nodes are physics-only, and therefore must not render. The BAKE decides: a
    // collision proxy is a declared node and a ballistic volume is one whose primitives wear a
    // substance material (§12, classifier precedent 2026-08-07). The `*_Ballistic` suffix that used
    // to answer this is retired and stripped from the source .blend — asking the extraction is the
    // replacement, and it is the same verdict the march itself rides on, so the two cannot drift.
    let geometry = blueprint.as_deref().map(|blueprint| &blueprint.geometry);
    let hidden = |name: &str| geometry.is_some_and(|geometry| geometry.is_physics_only(name));
    // Both trees share names; skip sim entities so links cannot point back to themselves.
    let skeleton: HashSet<Entity> = parts.0.values().copied().collect();
    for entity in children.iter_descendants(ready.entity) {
        if skeleton.contains(&entity) {
            continue;
        }
        let Ok(name) = names.get(entity) else {
            continue;
        };
        if hidden(name.as_str()) {
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
            // Same repair as `detach_view_on_turret_launch`, including the scope: a turret that
            // came off before its presentation arrived is no less escaped.
            commands
                .entity(sim)
                .insert((Visibility::default(), VisualScope::WORLD_SOLID));
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
///
/// The free body is no longer part of ANY tank, which makes it no longer part of the view subject's
/// body either: it becomes ordinary world geometry, visible from the gunner optic like any other
/// wreckage. Stated here rather than left to `render_policy`'s "no ancestor scope" default, because
/// this is a claim about the turret and not about the absence of a parent.
///
/// The mechanism this replaces walked descendants of a `Tank` and therefore lost track of the
/// subtree the moment it escaped: a turret blown off while the player sat in the optic stayed
/// invisible for the rest of the round.
fn detach_view_on_turret_launch(
    add: On<Add, crate::damage::LaunchedTurret>,
    views: Query<&ViewNode>,
    mut commands: Commands,
) {
    let Ok(view) = views.get(add.entity) else {
        return;
    };
    // The detached sim body becomes a new visibility root — and a new rendering-policy root.
    commands
        .entity(add.entity)
        .insert((Visibility::default(), VisualScope::WORLD_SOLID));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_policy::{self, CameraProfile};

    /// A turret blown off the body the player is riding stops being part of that body.
    ///
    /// The regression this pins is silent and permanent: the mechanism this replaces swept
    /// descendants of a `Tank`, so an escaped subtree froze at whatever it last held. Brew up while
    /// in the gunner optic and the flying turret stayed invisible for the rest of the round —
    /// visible to every other player, and to you from third person, but not from the sight you were
    /// looking through when it happened.
    #[test]
    fn a_launched_turret_leaves_the_view_subject() {
        let mut app = App::new();
        app.add_plugins((render_policy::plugin, view_attach_plugin));
        let world = app.world_mut();
        let optic = world.spawn(CameraProfile::BattlefieldOptic).id();
        let tank = world.spawn(VisualScope::VIEW_SUBJECT_BODY).id();
        let view = world.spawn(ChildOf(tank)).id();
        let armour = world.spawn((Mesh3d(Handle::default()), ChildOf(view))).id();
        let turret = world.spawn((ChildOf(tank), ViewNode(view))).id();
        app.update();
        assert!(
            !render_policy::reaches(app.world(), optic, armour),
            "while attached, the turret is part of the body the optic drops"
        );

        // The launch, exactly as `damage::launch_turrets_on_cookoff` batches it.
        app.world_mut()
            .entity_mut(turret)
            .remove::<ChildOf>()
            .insert(crate::damage::LaunchedTurret);
        app.update();
        assert!(
            render_policy::reaches(app.world(), optic, armour),
            "an escaped turret is world geometry — visible from the sight it was blown off in"
        );
    }
}
