use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use super::{GunBarrel, ServoSpec};

/// The sim skeleton's part table, on the tank root: node name → sim part entity, for every part
/// [`spawn_tank_sim`] spawned. The **name-keyed join between sim and view** (design §6C): the
/// view binder ([`bind_tank_view`]) resolves each instantiated glb node against this map to tag
/// it [`ViewOf`] its sim part. Phase 2's artifact part table is this map, serialized.
#[derive(Component)]
pub struct SimParts(pub HashMap<String, Entity>);

/// A glb view node's link to the sim part of the same name, inserted by [`bind_tank_view`]. The
/// name-keyed sim/view join, view→sim direction: the render writers (`interpolate_servos`)
/// resolve their sim source through it, and the sandbox's volume painter reads the sim part's
/// volume role through it.
#[derive(Component, Clone, Copy)]
pub struct ViewOf(pub Entity);

/// The sim→view back-link, inserted on each sim part by [`bind_tank_view`] when its view node
/// attaches: the part table in entity form, for consumers that start from a sim entity —
/// `sync_view_barrels` (the recoil pose copy), the cook-off view detach, and every render
/// reader that must follow the smoothed view pose (gunner camera, bore dot, HP labels).
#[derive(Component, Clone, Copy)]
pub struct ViewNode(pub Entity);

impl ViewNode {
    /// THE render-reader fallback rule, single-sourced: a sim part's render-side node is its
    /// attached view node, or the sim part itself before the scene attaches (cosmetic — the sim
    /// pose steps at tick rate, but nothing slews during the spawn pop-in). Degrades per part:
    /// a partially-instantiated scene only falls back where the join is actually missing.
    pub fn resolve(view: Option<&ViewNode>, sim: Entity) -> Entity {
        view.map_or(sim, |view| view.0)
    }
}

/// Marks a glb view node whose sim part is a servo frame — `interpolate_servos`' write set (the
/// render blend targets VIEW nodes; sim node transforms are pure tick truth since step 2).
#[derive(Component)]
pub struct ViewServo;

/// The view binder: when the tank's glb scene instantiates, join its named nodes against the sim
/// skeleton's part table ([`SimParts`]) — presentation attaching onto an already-complete sim.
/// Nothing here constructs sim state; the scene is free to arrive seconds late (a visual pop-in,
/// not a bind window). Observed per spawn path via `.observe(…)`, like the binder it replaces.
///
/// All render-side (design §6C):
///   - hide the authored physics geometry (collision proxies, ballistic volumes — their sim
///     colliders are built from data; the glb copies are just meshes);
///   - tag every glb node that has a same-named sim part with [`ViewOf`] (+ [`ViewServo`] where
///     the part is a servo frame — `interpolate_servos`' write set) and back-link the sim part
///     with [`ViewNode`] — the join every render reader resolves through ([`ViewNode::resolve`]);
///   - seed each moving view node (servo, barrel) at its sim part's CURRENT pose, so a scene
///     attaching mid-slew never shows the authored rest pose, not even for the one frame before
///     the render writers first run;
///   - if a sim part already detached (cook-off fired during the scene load), attach its view
///     subtree to the free body now — the `Add<LaunchedTurret>` observer fired before this join
///     existed and never re-fires.
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
    // The root's descendants hold TWO same-named trees: the sim skeleton and the instantiated
    // scene. This walk's subject is the SCENE — skip the sim parts, or every skeleton node gets
    // a self-referential `ViewOf` (which would corrupt the cook-off detach) and the hide rule
    // stamps `Visibility` onto bare skeleton nodes (B0004 warning per node, measured 48/run).
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
        // Primitive leaves (`Mesh3d`) are render geometry, not part-named nodes — a mesh sharing
        // a part's name (Blender mesh data often shares its object's name) must not be joined.
        // `Mesh3d` presence is the reliable discriminator, NOT `GltfMaterialName` (absent on
        // unnamed-material primitives — the step-0 shadow lesson).
        if meshes.contains(entity) {
            continue;
        }
        let Some(&sim) = parts.0.get(name.as_str()) else {
            continue;
        };
        // Already launched: same attach `detach_view_on_turret_launch` performs, done here
        // because that observer fired (and no-oped) before the scene existed. No `ViewOf` — the
        // subtree rides the free body whole; its child parts below still join normally.
        if launched.contains(sim) {
            commands.entity(sim).insert(Visibility::default());
            commands
                .entity(entity)
                .insert((ChildOf(sim), Transform::IDENTITY));
            continue;
        }
        commands.entity(entity).insert(ViewOf(sim));
        commands.entity(sim).insert(ViewNode(entity));
        // Runtime-written parts start at the sim's current pose (tick truth), not the authored
        // rest the glb shipped — a scene attaching mid-slew must never flash the rest pose.
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

/// Copy each recoiling barrel's tick-truth transform onto its view node. The recoil spring steps
/// on the fixed clock (`apply_recoil` writes the SIM barrel — the muzzle chain `fire` composes
/// must carry the offset), so the view copy renders at fixed rate too — exactly the pre-split
/// look (barrel recoil was never overstep-blended). A local-space copy is exact: the view node
/// sits in a parent chain identical to its sim part's.
fn sync_view_barrels(
    barrels: Query<(&Transform, &ViewNode), With<GunBarrel>>,
    mut views: Query<&mut Transform, Without<GunBarrel>>,
) {
    // A launched turret's subtree keeps its barrel link: both trees ride the same free body, so
    // the local copy stays exact there too.
    for (source, view) in &barrels {
        if let Ok(mut dest) = views.get_mut(view.0) {
            dest.set_if_neq(*source);
        }
    }
}

/// The view half of the cook-off detach (design §6C): when the sim decides the turret comes off
/// (`damage::launch_turrets_on_cookoff` strips its `ChildOf` and makes it a free rigid body), the
/// view turret subtree reparents under that free sim body with an identity offset and follows it
/// whole. Its `ViewOf`/`ViewServo` come off — the launched sim turret has no servo components
/// left, so nothing would (or should) keep writing the view node's local transform.
fn detach_view_on_turret_launch(
    add: On<Add, crate::damage::LaunchedTurret>,
    views: Query<&ViewNode>,
    mut commands: Commands,
) {
    let Ok(view) = views.get(add.entity) else {
        return;
    };
    // The free sim body becomes the view subtree's new visibility root — without its own
    // `Visibility` the reparented view node's inheritance chain breaks (B0004).
    commands.entity(add.entity).insert(Visibility::default());
    commands
        .entity(view.0)
        .insert((ChildOf(add.entity), Transform::IDENTITY))
        .remove::<(ViewOf, ViewServo)>();
}

/// The render-side view-attach systems every tank-spawning composition mounts exactly once:
/// [`sim_plugin`] pulls it in for the game and the net bins; the armor sandbox (which runs no
/// servo sim) mounts it directly. `interpolate_servos` is NOT here — it needs the `Playing`
/// gate and the fixed-clock state only sim compositions have. `sync_view_barrels` runs ungated
/// deliberately: the sandbox has no `AppState`, and outside gameplay the copy is a no-op
/// (`set_if_neq` over a handful of entities).
pub fn view_attach_plugin(app: &mut App) {
    app.add_observer(detach_view_on_turret_launch);
    app.add_systems(Update, sync_view_barrels);
}
