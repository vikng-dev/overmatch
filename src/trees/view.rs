//! The tree's view, under the tree's root (ADR-0014: the sim never waits on it; ADR-0016: it
//! derives from the state).
//!
//! TWO REPRESENTATIONS. A standing tree draws its kind's standing meshes: every glb primitive
//! outside a cut cap, merged in the view root's frame, one mesh per material (trunk and canopy for
//! the shipped kinds) and shared by every tree of the kind, so the renderer instances them. A tree
//! that is down draws the glb's segmented hierarchy, cut caps included, rebuilt from the kind's
//! [`TreeTemplate`] in the frame its state leaves `Standing` ([`draw_trees`]), and the fall is
//! presented on it from the state and its transition tick ([`present_fall`]). A tree standing
//! again draws its standing meshes again; a settled tree keeps the segmented hierarchy.
//!
//! THE TEMPLATE. Read once per kind off one hidden, unparented instance of the kind's glb scene
//! ([`read_template`]), bound to the trunk by the generator's node extras (`tree_part`,
//! `tree_segment`), then despawned. A view drawn before its kind's template is read draws nothing
//! until it is.

use std::collections::{BTreeSet, HashMap};

use bevy::app::SceneSpawnerSystems;
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use super::{FallClock, Fell, Resting, TreeBody, TreeKind, TreeState, fall_rotation, seat_in_root};
use crate::{ClientReplica, ShotClock};

/// Marks a tree's view root, a child of the tree's root at the instance's uniform scale: every
/// mesh the tree draws hangs under it.
#[derive(Component)]
#[require(Transform, Visibility)]
pub(crate) struct TreeView;

/// The representation a view draws; absent until its kind's template is read.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum Drawn {
    /// The kind's standing meshes.
    Standing,
    /// The glb's segmented hierarchy, indexed by [`TreeViewNodes`].
    Segmented,
}

/// Attach the view of `kind` under the tree `root` spawned by [`super::spawn`], at the instance's
/// uniform `scale`; the kind's template loads through `assets` on its first attach. Called by a
/// composition that draws, never by the sim.
pub(crate) fn attach(
    commands: &mut Commands,
    root: Entity,
    kind: &TreeKind,
    scale: f32,
    assets: &AssetServer,
) {
    // The root is the view's parent: visibility inherits through it.
    commands.entity(root).insert(Visibility::default());
    commands.spawn((
        ChildOf(root),
        Transform::from_scale(Vec3::splat(scale)),
        TreeView,
        crate::render_cost::CasterClass::Tree,
    ));
    let (id, assets) = (kind.id.clone(), assets.clone());
    commands.queue(move |world: &mut World| request_template(world, id, &assets));
}

/// Every tree kind's view template by kind id: `None` while its glb loads.
#[derive(Resource, Default)]
pub(crate) struct TreeTemplates(HashMap<String, Option<TreeTemplate>>);

/// The hidden glb instance a kind's template is read from, by kind id.
#[derive(Component)]
struct TreeViewSource(String);

/// Start reading kind `id`'s template unless it is read or loading: one hidden, unparented instance
/// of the kind's glb scene, read and despawned once it is ready ([`read_template`]).
fn request_template(world: &mut World, id: String, assets: &AssetServer) {
    {
        let mut templates = world.get_resource_or_init::<TreeTemplates>();
        if templates.0.contains_key(&id) {
            return;
        }
        templates.0.insert(id.clone(), None);
    }
    let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("trees/{id}/{id}.glb")));
    world
        .spawn((
            WorldAssetRoot(scene),
            Visibility::Hidden,
            TreeViewSource(id),
        ))
        .observe(read_template);
}

/// Which part of the trunk a glb node presents, by the generator's `tree_part`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Trunk,
    CapTop,
    CapBottom,
    /// Any other part (the canopy): drawn standing and down, never indexed.
    Other,
}

/// One entity of the kind's scene instance, as the segmented view rebuilds it.
#[derive(Debug, Clone)]
struct TemplateNode {
    /// The node it hangs under, an index below its own; `None` under the view root.
    parent: Option<usize>,
    transform: Transform,
    name: Option<Name>,
    /// The generator's `(tree_part, tree_segment)`, on the node that carries the extras.
    part: Option<(Part, usize)>,
    mesh: Option<(Handle<Mesh>, Option<Handle<StandardMaterial>>)>,
}

/// A kind's view: its scene's entities parent first, and its standing meshes with their
/// materials — `None` where the primitives cannot merge, and the kind's trees draw segmented
/// throughout.
pub(crate) struct TreeTemplate {
    nodes: Vec<TemplateNode>,
    standing: Option<Vec<(Handle<Mesh>, Option<Handle<StandardMaterial>>)>>,
}

impl TreeTemplate {
    /// The template of `nodes`, its standing meshes merged ([`standing_meshes`]) and added to
    /// `meshes`; a merge that fails is logged against kind `id`.
    fn new(id: &str, nodes: Vec<TemplateNode>, meshes: &mut Assets<Mesh>) -> Self {
        let standing = match standing_meshes(&nodes, meshes) {
            Ok(merged) => {
                info!(
                    "trees: {id}'s view read — a standing tree draws {} merged meshes, a fallen one \
                     {} entities of which {} draw",
                    merged.len(),
                    nodes.len(),
                    nodes.iter().filter(|node| node.mesh.is_some()).count()
                );
                Some(
                    merged
                        .into_iter()
                        .map(|(mesh, material)| (meshes.add(mesh), material))
                        .collect(),
                )
            }
            Err(why) => {
                error!(
                    "trees: {id}'s standing meshes do not merge ({why}); its trees draw segmented"
                );
                None
            }
        };
        Self { nodes, standing }
    }
}

/// The per-node extras the generator writes.
#[derive(serde::Deserialize)]
struct PartExtras {
    tree_part: String,
    tree_segment: usize,
}

/// Read a kind's template off its ready scene instance, parent first, and despawn the instance.
fn read_template(
    ready: On<WorldInstanceReady>,
    sources: Query<&TreeViewSource>,
    children: Query<&Children>,
    entities: Query<(
        Option<&Transform>,
        Option<&Name>,
        Option<&GltfExtras>,
        Option<&Mesh3d>,
        Option<&MeshMaterial3d<StandardMaterial>>,
    )>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut templates: ResMut<TreeTemplates>,
    mut commands: Commands,
) {
    let Ok(TreeViewSource(id)) = sources.get(ready.entity) else {
        return;
    };
    // Breadth first, so every entity follows its parent.
    let mut order: Vec<(Entity, Option<usize>)> = children
        .get(ready.entity)
        .map(|top| top.iter().map(|entity| (entity, None)).collect())
        .unwrap_or_default();
    let mut k = 0;
    while k < order.len() {
        if let Ok(below) = children.get(order[k].0) {
            order.extend(below.iter().map(|entity| (entity, Some(k))));
        }
        k += 1;
    }
    let nodes = order
        .into_iter()
        .map(|(entity, parent)| {
            let (transform, name, extras, mesh, material) =
                entities.get(entity).unwrap_or_default();
            let part = extras
                .and_then(|extras| serde_json::from_str::<PartExtras>(&extras.value).ok())
                .map(|extras| {
                    let part = match extras.tree_part.as_str() {
                        "trunk" => Part::Trunk,
                        "cap_top" => Part::CapTop,
                        "cap_bottom" => Part::CapBottom,
                        _ => Part::Other,
                    };
                    (part, extras.tree_segment)
                });
            TemplateNode {
                parent,
                transform: transform.copied().unwrap_or_default(),
                name: name.cloned(),
                part,
                mesh: mesh
                    .map(|mesh| (mesh.0.clone(), material.map(|material| material.0.clone()))),
            }
        })
        .collect();
    let template = TreeTemplate::new(id, nodes, &mut meshes);
    templates.0.insert(id.clone(), Some(template));
    commands.entity(ready.entity).despawn();
}

/// The standing view's meshes and their materials: every mesh node outside a cap — a node whose
/// own or nearest marked ancestor's part is a cap — moved into the view root's frame by its
/// transforms composed down from the view root ([`Mesh::try_transformed_by`]), and merged in node
/// order with the others of its material, its indices offset past the vertices before it. Under
/// a composition with no rotation and unit scale — the generator's nesting is translations only —
/// a position is its own plus that translation, and normals, UVs and every other attribute are
/// copied bit for bit.
fn standing_meshes(
    nodes: &[TemplateNode],
    meshes: &Assets<Mesh>,
) -> Result<Vec<(Mesh, Option<Handle<StandardMaterial>>)>, String> {
    let mut chain: Vec<(Transform, Option<Part>)> = Vec::with_capacity(nodes.len());
    let mut merged: Vec<(Mesh, Option<Handle<StandardMaterial>>)> = Vec::new();
    for node in nodes {
        let (above, above_part) = node
            .parent
            .map_or((Transform::IDENTITY, None), |p| chain[p]);
        let at = above.mul_transform(node.transform);
        let part = node.part.map(|(part, _)| part).or(above_part);
        chain.push((at, part));
        let Some((handle, material)) = &node.mesh else {
            continue;
        };
        if matches!(part, Some(Part::CapTop | Part::CapBottom)) {
            continue;
        }
        let mesh = meshes
            .get(handle)
            .ok_or_else(|| format!("{:?}'s mesh is not loaded", node.name))?
            .clone()
            .try_transformed_by(at)
            .map_err(|err| err.to_string())?;
        match merged.iter_mut().find(|(_, other)| other == material) {
            Some((into, _)) => append(into, &mesh)?,
            None => merged.push((mesh, material.clone())),
        }
    }
    Ok(merged)
}

/// Append `mesh` to `into` ([`Mesh::merge`]) when nothing is lost: the same topology, the same
/// vertex attributes (a merge drops one side's extra attribute) and indices on both or neither.
fn append(into: &mut Mesh, mesh: &Mesh) -> Result<(), String> {
    let ids = |mesh: &Mesh| -> Result<BTreeSet<_>, String> {
        Ok(mesh
            .try_attributes()
            .map_err(|err| err.to_string())?
            .map(|(attribute, _)| attribute.id)
            .collect())
    };
    let indexed = |mesh: &Mesh| -> Result<bool, String> {
        Ok(mesh
            .try_indices_option()
            .map_err(|err| err.to_string())?
            .is_some())
    };
    if ids(into)? != ids(mesh)? || indexed(into)? != indexed(mesh)? {
        return Err("primitives with different vertex attributes or indexing".to_owned());
    }
    into.merge(mesh).map_err(|err| err.to_string())
}

/// Which glb nodes present which part of the trunk, by segment: indexed when the segmented
/// hierarchy is built, from each node's extras.
#[derive(Component, Debug, Default)]
pub(crate) struct TreeViewNodes {
    trunks: Vec<Option<Entity>>,
    /// Each trunk node's translation in its parent's frame as the glb binds it, asset metres:
    /// where the node stands, which the cut's node leaves during a fall and every node returns
    /// to when the tree stands again. Taken from the template, never read back off the node.
    standing: Vec<Vec3>,
    caps_top: Vec<Option<Entity>>,
    caps_bottom: Vec<Option<Entity>>,
}

/// How far a tree's meshes draw, measured to each mesh entity's origin. DERIVED: a 0.4 m trunk at
/// 600 m subtends about 1 px at the gunner's 1× field; the frame-cost measurement (stage 5)
/// decides whether this stays.
const TREE_VIEW_RANGE_M: f32 = 600.0;

/// Draw each tree's representation from its state: the kind's standing meshes while it stands,
/// the segmented hierarchy otherwise. A tree whose state leaves `Standing` is redrawn in that
/// frame, so [`present_fall`] poses the new hierarchy before it renders: the first frame down is
/// the standing tree at the fall's start, and nothing pops. A view waiting on its kind's template
/// draws in the frame the template is read. In `SpawnScene`, after the scene spawner: after every
/// state writer (the laws in `FixedPostUpdate`, a replica's ledger in `Update`) and before
/// `PostUpdate`, whose render policy resolves the new meshes in the same frame.
fn draw_trees(
    templates: Res<TreeTemplates>,
    changed: Query<&Children, (Changed<TreeState>, With<TreeBody>)>,
    pending: Query<(Entity, &ChildOf), (With<TreeView>, Without<Drawn>)>,
    drawn: Query<(Entity, &ChildOf, &Drawn), With<TreeView>>,
    trees: Query<(&TreeState, &TreeBody)>,
    mut commands: Commands,
) {
    let waiting = pending
        .iter()
        .map(|(view, parent)| (view, parent.parent(), None));
    let moved = changed
        .iter()
        .flat_map(|children| children.iter().filter_map(|child| drawn.get(child).ok()))
        .map(|(view, parent, drawn)| (view, parent.parent(), Some(*drawn)));
    for (view, root, now) in waiting.chain(moved) {
        let Ok((state, body)) = trees.get(root) else {
            continue;
        };
        let Some(Some(template)) = templates.0.get(&body.kind.id) else {
            continue;
        };
        let want = if *state == TreeState::Standing && template.standing.is_some() {
            Drawn::Standing
        } else {
            Drawn::Segmented
        };
        if now != Some(want) {
            draw(
                &mut commands,
                view,
                want,
                template,
                body.kind.segments.len(),
            );
        }
    }
}

/// Replace whatever `view` draws with `want` from `template`, for a trunk of `segments`.
fn draw(
    commands: &mut Commands,
    view: Entity,
    want: Drawn,
    template: &TreeTemplate,
    segments: usize,
) {
    commands
        .entity(view)
        .despawn_related::<Children>()
        .remove::<TreeViewNodes>()
        .insert(want);
    let range = VisibilityRange::abrupt(0.0, TREE_VIEW_RANGE_M);
    if want == Drawn::Standing {
        for (mesh, material) in template.standing.iter().flatten() {
            let mut entity = commands.spawn((
                ChildOf(view),
                Transform::IDENTITY,
                Mesh3d(mesh.clone()),
                range.clone(),
            ));
            if let Some(material) = material {
                entity.insert(MeshMaterial3d(material.clone()));
            }
        }
        return;
    }
    let mut index = TreeViewNodes {
        trunks: vec![None; segments],
        standing: vec![Vec3::ZERO; segments],
        caps_top: vec![None; segments],
        caps_bottom: vec![None; segments],
    };
    let mut spawned: Vec<Entity> = Vec::with_capacity(template.nodes.len());
    for node in &template.nodes {
        let parent = node.parent.map_or(view, |p| spawned[p]);
        let mut entity = commands.spawn((ChildOf(parent), node.transform, Visibility::Inherited));
        if let Some(name) = &node.name {
            entity.insert(name.clone());
        }
        if let Some((mesh, material)) = &node.mesh {
            entity.insert((Mesh3d(mesh.clone()), range.clone()));
            if let Some(material) = material {
                entity.insert(MeshMaterial3d(material.clone()));
            }
        }
        let id = entity.id();
        match node.part {
            Some((Part::Trunk, k)) if k < segments => {
                index.trunks[k] = Some(id);
                index.standing[k] = node.transform.translation;
            }
            // Every cap is hidden until a cut reveals it.
            Some((Part::CapTop, k)) if k < segments => {
                index.caps_top[k] = Some(id);
                entity.insert(Visibility::Hidden);
            }
            Some((Part::CapBottom, k)) if k < segments => {
                index.caps_bottom[k] = Some(id);
                entity.insert(Visibility::Hidden);
            }
            _ => {}
        }
        spawned.push(id);
    }
    commands.entity(view).insert(index);
}

/// The fall's duration, seconds: longer for a longer piece, within a watchable window.
/// DERIVED (contract §6): 0.08 s per metre of fallen length, clamped to [1, 3] s.
fn fall_duration_s(fallen_length_m: f32) -> f32 {
    (0.08 * fallen_length_m).clamp(1.0, 3.0)
}

/// The butt's slide off the stump to its seat, seconds: the free fall over its drop,
/// `sqrt(2 × drop / g)` at g = 9.81 m/s² (DERIVED), within a watchable window of [0.2, 1] s.
/// A seat at or above the stump's top (an uphill fall) drops nothing and takes the floor.
fn slide_duration_s(drop_m: f32) -> f32 {
    (2.0 * drop_m / 9.81).sqrt().clamp(0.2, 1.0)
}

/// The cut's trunk node `seconds` into its fall: the angle of its turn and its translation in
/// its parent's frame, from its bound `standing` translation (asset metres). Stage A, over
/// [`fall_duration_s`] of the fallen length: the node turns about its own origin from upright to
/// the hinge with a `phase²` easing (a fall accelerates), translated as it stood. Stage B, for a
/// cut above the base, over [`slide_duration_s`] of the butt's drop from the stump's top to the
/// seat: the origin moves to its rest with `phase²` (a drop accelerates) while the turn runs
/// from the hinge to the resting tilt linearly; at its end the translation is exactly the rest
/// and the angle exactly the tilt. For a cut at the base the hinge is the tilt and the seat is
/// zero, so the fall ends with A. The rest translation is the standing one moved by the seat's
/// offset from the cut's bottom in the root's frame ([`seat_in_root`]), in asset metres: the
/// chain is under the view root's uniform scale and carries no rotation (A5), so a parent-frame
/// offset is the root-frame offset over that scale.
fn fall_pose(
    body: &TreeBody,
    resting: &Resting,
    root: Quat,
    cut: usize,
    standing: Vec3,
    seconds: f32,
) -> (f32, Vec3) {
    let tip = body.prefix.len() - 1;
    let fall = fall_duration_s(body.prefix[tip] - body.prefix[cut]);
    let phase = (seconds / fall).clamp(0.0, 1.0);
    if cut == 0 || phase < 1.0 {
        return (resting.hinge * phase * phase, standing);
    }
    let drop = (body.prefix[cut] - resting.seat.y).max(0.0);
    let slide = ((seconds - fall) / slide_duration_s(drop)).clamp(0.0, 1.0);
    let rest = standing + (seat_in_root(resting, root) - Vec3::Y * body.prefix[cut]) / body.scale;
    if slide >= 1.0 {
        return (resting.tilt, rest);
    }
    (
        resting.hinge + (resting.tilt - resting.hinge) * slide,
        standing + (rest - standing) * (slide * slide),
    )
}

/// Present a tree's state on its segmented hierarchy, from the state alone (ADR-0016): standing,
/// every trunk node upright at its bound translation and every cap hidden; down, the cut's trunk
/// node — the chain's translations carry the piece above — falls in two stages ([`fall_pose`]):
/// it turns about its own origin by [`fall_rotation`] from upright to the hinge, then its origin
/// drops to the seat while it turns on to the resting tilt, where its rotation is the sim's lying
/// segments' own and its origin the seat. The time is read off [`FallClock`] against the
/// transition tick (ADR-0037: the fall STARTS when the cursor crosses its tick, however late the
/// entry arrived, and completes [`fall_duration_s`] and then, above the base,
/// [`slide_duration_s`] later), and of the caps exactly the pair the cut reveals (A5: `cap_top`
/// below the cut and `cap_bottom` of the cut) shows. Written when the state, the body's resting
/// pose or the built hierarchy is new, and on every frame the cut's trunk is not at the turn and
/// the translation the clock derives — through the fall and on the frame it completes; a
/// hierarchy built for a tree in either state is presented in the frame it is built, a tree long
/// down with both stages complete. A tree drawing its standing meshes has no hierarchy and is
/// skipped. The sim never waits on this.
fn present_fall(
    clock: Res<FallClock>,
    step: Res<Time<Fixed>>,
    trees: Query<(
        Ref<TreeState>,
        &TreeBody,
        Option<&Fell>,
        Option<Ref<Resting>>,
        &Transform,
        &Children,
    )>,
    views: Query<Ref<TreeViewNodes>>,
    mut nodes: Query<(&mut Transform, &mut Visibility), Without<TreeBody>>,
) {
    let dt = step.timestep().as_secs_f32();
    for (state, body, fell, resting, transform, children) in &trees {
        let Some(view) = children.iter().find_map(|child| views.get(child).ok()) else {
            continue;
        };
        // The cut and its trunk node's pose this frame: none standing, none until the body's
        // pose is derived.
        let turned = match (*state, fell, resting.as_deref()) {
            (TreeState::Down { cut, .. }, Some(fell), Some(resting)) => {
                let cut = usize::from(cut);
                let seconds = clock.since(fell.tick) as f32 * dt;
                let (angle, translation) = fall_pose(
                    body,
                    resting,
                    transform.rotation,
                    cut,
                    view.standing[cut],
                    seconds,
                );
                Some((
                    cut,
                    fall_rotation(resting, transform.rotation, angle),
                    translation,
                ))
            }
            _ => None,
        };
        let fresh = state.is_changed()
            || view.is_added()
            || resting.as_ref().is_some_and(|resting| resting.is_changed());
        // An unchanged tree is redrawn while its cut's trunk is not at the pose this frame
        // derives: every frame of the fall, and the frame the last stage completes, which writes
        // the resting pose; at rest the two agree and the tree is skipped.
        let behind = turned.is_some_and(|(cut, turn, translation)| {
            view.trunks[cut]
                .and_then(|trunk| nodes.get(trunk).ok())
                .is_some_and(|(local, _)| {
                    local.rotation != turn || local.translation != translation
                })
        });
        if !fresh && !behind {
            continue;
        }
        for (k, trunk) in view.trunks.iter().enumerate() {
            let (rotation, translation) = match turned {
                Some((cut, turn, translation)) if k == cut => (turn, translation),
                _ => (Quat::IDENTITY, view.standing[k]),
            };
            if let Some(trunk) = trunk
                && let Ok((mut local, _)) = nodes.get_mut(*trunk)
            {
                if local.rotation != rotation {
                    local.rotation = rotation;
                }
                if local.translation != translation {
                    local.translation = translation;
                }
            }
        }
        let cut = turned.map(|(cut, _, _)| cut);
        let tops = view
            .caps_top
            .iter()
            .enumerate()
            .map(|(k, cap)| (cap, cut.is_some_and(|cut| k + 1 == cut)));
        let bottoms = view
            .caps_bottom
            .iter()
            .enumerate()
            .map(|(k, cap)| (cap, cut == Some(k)));
        for (cap, shown) in tops.chain(bottoms) {
            if let Some(cap) = cap
                && let Ok((_, mut visibility)) = nodes.get_mut(*cap)
            {
                let visibility_target = if shown {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                };
                visibility.set_if_neq(visibility_target);
            }
        }
    }
}

/// The authority's (and single-player's) presentation clock: the sim tick plus the fixed step's
/// overstep. A replica's is the interpolation cursor, written by the net client instead.
fn publish_sim_fall_clock(
    tick: Option<Res<ShotClock>>,
    step: Res<Time<Fixed>>,
    mut clock: ResMut<FallClock>,
) {
    // A composition with no clock (the sandboxes, the headless sim) keeps its falls unkeyed, as
    // its shells are.
    let Some(tick) = tick else {
        return;
    };
    clock.tick = tick.0;
    clock.overstep = step.overstep_fraction_f64();
}

/// The view half, mounted by the windowed roots: the presentation clock, each tree's
/// representation, and the fall on the segmented hierarchy.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<FallClock>()
        .init_resource::<TreeTemplates>()
        .add_systems(
            Update,
            publish_sim_fall_clock.run_if(not(resource_exists::<ClientReplica>)),
        )
        .add_systems(
            SpawnScene,
            draw_trees.after(SceneSpawnerSystems::WorldInstanceSpawn),
        )
        .add_systems(PostUpdate, present_fall.before(TransformSystems::Propagate));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trees::tests::{derive_bed, fixture_grid, fixture_kind, segments_of};
    use bevy::mesh::VertexAttributeValues;
    use std::f32::consts::FRAC_PI_2;

    /// The fixture's view under a tree root at `pose` (the root's own transform, unit scale) in
    /// `state` with `resting`, presented at clock tick `tick` after a transition at tick 0: the
    /// view's root under the tree at the instance's `scale`, the trunk nodes nested under it
    /// bottom-up, each at its segment's base in its parent's frame in asset metres (fixture
    /// data, the glb's nesting); the trunk nodes' world rotations and positions composed down
    /// the chain, bottom first.
    fn presented(
        pose: Transform,
        scale: f32,
        state: TreeState,
        resting: Resting,
        tick: u32,
    ) -> Vec<(Quat, Vec3)> {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let body = TreeBody::new(fixture_kind(), scale);
        let world = app.world_mut();
        let root = world
            .spawn((state, Fell { tick: 0 }, resting, body.clone(), pose))
            .id();
        let view = world
            .spawn((ChildOf(root), Transform::from_scale(Vec3::splat(scale))))
            .id();
        let mut parent = view;
        let mut trunks = Vec::new();
        let mut standing = Vec::new();
        for k in 0..body.kind.segments.len() {
            let lift = if k == 0 {
                0.0
            } else {
                body.kind.segments[k - 1].height_m
            };
            let translation = Vec3::new(0.0, lift, 0.0);
            parent = world
                .spawn((
                    ChildOf(parent),
                    Transform::from_translation(translation),
                    Visibility::Inherited,
                ))
                .id();
            trunks.push(parent);
            standing.push(translation);
        }
        let n = trunks.len();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
            standing,
            caps_top: vec![None; n],
            caps_bottom: vec![None; n],
        });
        app.update();
        let world = app.world();
        let mut chain = pose.mul_transform(*world.get::<Transform>(view).unwrap());
        trunks
            .iter()
            .map(|trunk| {
                chain = chain.mul_transform(*world.get::<Transform>(*trunk).unwrap());
                (chain.rotation, chain.translation)
            })
            .collect()
    }

    /// THE SIM AND THE VIEW LAY THE PIECE DOWN TO THE SAME BITS: for every row of the lying-pose
    /// pins (cuts 0 and 1 on flat ground, on the slope and at the grid's edge, in three bins,
    /// under a yaw), for the yawed fixture down at cut 2 and for one at scale 1.5, each lying
    /// segment's world rotation composed from the sim's body — the root's over the segment's —
    /// equals, bit for bit, its trunk node's composed down the view's chain at rest, and each
    /// lying segment's world position is within 1e-4 m of its node's (DERIVED: the chain's sum
    /// and the scale division round differently from the sim's one addition; positions are
    /// root-relative, tens of metres, where an f32 ulp is under 1e-5). Both take
    /// [`fall_rotation`] and [`seat_in_root`] under the same root; a sim or a view that builds
    /// its turn by another route — the world tilt conjugated by the root's inverse, say —
    /// differs in the last bits, a view that leaves the butt on the stump is `2r` off, and a
    /// rest translation not divided by the scale is off by the seat's offset times the scale's
    /// excess; each reds this.
    #[test]
    fn the_sim_and_the_view_lay_the_piece_down_to_the_same_bits() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let flat: fn(f32, f32) -> f32 = |_, _| 0.0;
        let slope: fn(f32, f32) -> f32 = |x, _| x / 10.0;
        let mut cases = Vec::new();
        for (height, base) in [
            (flat, Vec3::ZERO),
            (slope, Vec3::new(100.0, 10.0, 0.0)),
            (slope, Vec3::new(499.0, 49.9, 0.0)),
        ] {
            for cut in [0_u8, 1] {
                for dir in [0_u8, 64, 200] {
                    cases.push((height, base, 1.0_f32, TreeState::Down { cut, dir }));
                }
            }
        }
        cases.push((
            flat,
            Vec3::new(5.0, 0.0, 5.0),
            1.0,
            TreeState::Down { cut: 2, dir: 0 },
        ));
        cases.push((
            flat,
            Vec3::new(5.0, 0.0, 5.0),
            1.5,
            TreeState::Down { cut: 1, dir: 64 },
        ));
        for (height, base, scale, state) in cases {
            let spawn_pose = Transform {
                translation: base,
                rotation: yaw,
                scale: Vec3::splat(scale),
            };
            let (mut app, root) = derive_bed(fixture_grid(height), spawn_pose);
            *app.world_mut().get_mut::<TreeState>(root).unwrap() = state;
            app.update();
            let resting = *app.world().get::<Resting>(root).expect("the pose");
            let root_pose = Transform {
                scale: Vec3::ONE,
                ..spawn_pose
            };
            let sim: Vec<(Quat, Vec3)> = segments_of(&app, root)
                .iter()
                .map(|(local, _)| {
                    let world = root_pose.mul_transform(*local);
                    (world.rotation, world.translation)
                })
                .collect();
            let view = presented(root_pose, scale, state, resting, 1000);
            let TreeState::Down { cut, .. } = state else {
                unreachable!()
            };
            for k in usize::from(cut)..sim.len() {
                assert_eq!(
                    sim[k].0.to_array().map(f32::to_bits),
                    view[k].0.to_array().map(f32::to_bits),
                    "{state:?} at {base}, scale {scale}, segment {k}: rotation"
                );
                assert!(
                    sim[k].1.abs_diff_eq(view[k].1, 1e-4),
                    "{state:?} at {base}, scale {scale}, segment {k}: the sim at {}, the view at {}",
                    sim[k].1,
                    view[k].1
                );
            }
        }
    }

    /// THE FALL ENDS ON THE RESTING POSE, STAGE BY STAGE: an unchanged tree whose clock advances
    /// through its fall has its cut's trunk node, through stage A, at its bound translation with
    /// a partial turn, and at A's end exactly the hinge's turn; through stage B, between the
    /// bound translation and the rest with a turn between the hinge and the tilt; and from B's
    /// end, at exactly the rest translation and exactly the turn of the whole tilt — the bits the
    /// derive computes at rest, which the sim's lying piece shares. A seat no lower than the
    /// stump's top drops nothing, and B takes its 0.2 s floor. A presentation that stops
    /// redrawing once a phase reaches one, before writing that frame's pose, leaves the node
    /// short of the rest; one that moves the origin during A reds this, and one that ends B at
    /// `hinge + (tilt − hinge)` instead of the tilt itself reds the third case, whose pair —
    /// hinge 90°, tilt 0.3 rad, fixture data — is a DERIVED f32 counterexample: that sum reads
    /// one ulp above 0.3. Mid-B the origin drops with `phase²` and the turn runs linearly from
    /// the hinge to the tilt, read off the tick count independently of the presentation's own
    /// arithmetic; a linear drop, a quadratic turn or a reversed trajectory reds that frame.
    #[test]
    fn a_fall_ends_on_the_resting_pose() {
        let yaw = Quat::from_xyzw(0.0, libm::sinf(0.35), 0.0, libm::cosf(0.35));
        let body = TreeBody::new(fixture_kind(), 1.0);
        // The fixture's piece above cut 1 is 4 m, a one-second stage A: 64 ticks at 64 Hz
        // (DERIVED). Its butt drops 1.8 m from the stump's 2 m top to a seat 0.2 m up in
        // 0.606 s, 38.8 ticks (DERIVED: sqrt(2 × 1.8 / 9.81)); a seat 2.5 m up drops nothing,
        // 0.2 s, 12.8 ticks. The third case is the first with the rounding-sensitive tilt.
        for (seat_y, b_ticks, tilt) in [(0.2_f32, 39_u32, 1.6_f32), (2.5, 13, 1.6), (0.2, 39, 0.3)]
        {
            let resting = Resting {
                hinge: FRAC_PI_2,
                tilt,
                seat: Vec3::new(0.4, seat_y, 0.0),
                dx: 1.0,
                dz: 0.0,
            };
            let mut app = App::new();
            app.insert_resource(FallClock {
                tick: 0,
                overstep: 0.0,
            })
            .insert_resource(Time::<Fixed>::from_hz(64.0))
            .add_systems(Update, present_fall);
            let world = app.world_mut();
            let root = world
                .spawn((
                    TreeState::Down { cut: 1, dir: 0 },
                    Fell { tick: 0 },
                    resting,
                    body.clone(),
                    Transform::from_rotation(yaw),
                ))
                .id();
            let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
            let standing: Vec<Vec3> = (0..3)
                .map(|k| Vec3::new(0.0, 2.0 * k as f32, 0.0))
                .collect();
            let trunks: Vec<Entity> = standing
                .iter()
                .map(|translation| {
                    world
                        .spawn((
                            ChildOf(view),
                            Transform::from_translation(*translation),
                            Visibility::Inherited,
                        ))
                        .id()
                })
                .collect();
            world.entity_mut(view).insert(TreeViewNodes {
                trunks: trunks.iter().copied().map(Some).collect(),
                standing: standing.clone(),
                caps_top: vec![None; 3],
                caps_bottom: vec![None; 3],
            });
            let node = |app: &App| *app.world().get::<Transform>(trunks[1]).unwrap();
            let at = |app: &mut App, tick: u32| {
                app.world_mut().resource_mut::<FallClock>().tick = tick;
                app.update();
            };
            let bits = |rotation: Quat| rotation.to_array().map(f32::to_bits);
            let hinge_turn = fall_rotation(&resting, yaw, resting.hinge);
            let rest_turn = fall_rotation(&resting, yaw, resting.tilt);
            let rest =
                standing[1] + (seat_in_root(&resting, yaw) - Vec3::Y * body.prefix[1]) / body.scale;
            at(&mut app, 32);
            let mid_a = node(&app);
            assert_eq!(
                mid_a.translation, standing[1],
                "stage A: the node stands where it was bound"
            );
            assert!(
                mid_a.rotation != Quat::IDENTITY && mid_a.rotation != hinge_turn,
                "stage A: a partial turn"
            );
            at(&mut app, 32);
            assert_eq!(
                node(&app),
                mid_a,
                "a frame with no time passed redraws nothing"
            );
            at(&mut app, 64);
            assert_eq!(
                bits(node(&app).rotation),
                bits(hinge_turn),
                "A's end: the hinge"
            );
            assert_eq!(node(&app).translation, standing[1]);
            at(&mut app, 64 + b_ticks / 2);
            let mid_b = node(&app);
            let drop = (body.prefix[1] - seat_y).max(0.0);
            let slide = ((64 + b_ticks / 2) as f32 / 64.0 - 1.0) / slide_duration_s(drop);
            let dropped = standing[1] + (rest - standing[1]) * (slide * slide);
            let turning = fall_rotation(
                &resting,
                yaw,
                resting.hinge + (resting.tilt - resting.hinge) * slide,
            );
            assert!(
                mid_b.translation.abs_diff_eq(dropped, 1e-5),
                "seat {seat_y}: stage B drops the origin with phase²: {} for {dropped}",
                mid_b.translation
            );
            assert!(
                mid_b.rotation.abs_diff_eq(turning, 1e-5),
                "seat {seat_y}: stage B turns linearly from the hinge to the tilt: {} for {turning}",
                mid_b.rotation
            );
            at(&mut app, 64 + b_ticks - 1);
            assert_ne!(
                node(&app).translation,
                rest,
                "seat {seat_y}: a tick short of B's end is short of the rest"
            );
            at(&mut app, 64 + b_ticks);
            let landed = node(&app);
            assert_eq!(
                landed.translation.to_array().map(f32::to_bits),
                rest.to_array().map(f32::to_bits),
                "seat {seat_y}: the frame B completes writes the rest translation"
            );
            assert_eq!(
                bits(landed.rotation),
                bits(rest_turn),
                "seat {seat_y}: and the resting turn"
            );
            at(&mut app, 300);
            assert_eq!(node(&app), landed, "and the node stays there");
        }
    }

    /// THE GLB PRESENTS THE STATE AND NOTHING BEFORE IT: standing, every trunk node upright at
    /// its bound translation and every cap hidden; down at a cut, that trunk node alone turned
    /// to the resting tilt and moved to its seat (the fall long landed) and exactly the cap pair
    /// the cut reveals shown; back to standing, upright at the bound translation and hidden
    /// again; down at another cut, the old trunk upright and in place and the old caps hidden.
    /// A presentation that only reveals, only turns the current cut's node, leaves a node where
    /// the fall put it, or skips `Standing` reds this.
    #[test]
    fn the_glb_presents_every_state_from_the_state() {
        let mut app = App::new();
        app.insert_resource(FallClock {
            tick: 1000,
            overstep: 0.0,
        })
        .insert_resource(Time::<Fixed>::from_hz(64.0))
        .add_systems(Update, present_fall);
        let world = app.world_mut();
        let root = world
            .spawn((
                TreeState::Standing,
                TreeBody::new(fixture_kind(), 1.0),
                Transform::IDENTITY,
            ))
            .id();
        let view = world.spawn((ChildOf(root), Transform::IDENTITY)).id();
        let node = |world: &mut World, translation: Vec3| {
            world
                .spawn((
                    ChildOf(view),
                    Transform::from_translation(translation),
                    Visibility::Inherited,
                ))
                .id()
        };
        let standing: Vec<Vec3> = (0..3)
            .map(|k| Vec3::new(0.0, 2.0 * k as f32, 0.0))
            .collect();
        let trunks: Vec<Entity> = standing.iter().map(|at| node(world, *at)).collect();
        let caps_top: Vec<Entity> = (0..3).map(|_| node(world, Vec3::ZERO)).collect();
        let caps_bottom: Vec<Entity> = (0..3).map(|_| node(world, Vec3::ZERO)).collect();
        world.entity_mut(view).insert(TreeViewNodes {
            trunks: trunks.iter().copied().map(Some).collect(),
            standing: standing.clone(),
            caps_top: caps_top.iter().copied().map(Some).collect(),
            caps_bottom: caps_bottom.iter().copied().map(Some).collect(),
        });
        let shown = |app: &App, caps: &[Entity]| -> Vec<bool> {
            caps.iter()
                .map(|cap| app.world().get::<Visibility>(*cap) == Some(&Visibility::Inherited))
                .collect()
        };
        let turned = |app: &App| -> Vec<bool> {
            trunks
                .iter()
                .map(|trunk| {
                    app.world().get::<Transform>(*trunk).unwrap().rotation != Quat::IDENTITY
                })
                .collect()
        };
        let moved = |app: &App| -> Vec<bool> {
            trunks
                .iter()
                .zip(&standing)
                .map(|(trunk, at)| app.world().get::<Transform>(*trunk).unwrap().translation != *at)
                .collect()
        };
        app.update();
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);
        assert_eq!(turned(&app), [false; 3], "standing: upright");
        assert_eq!(moved(&app), [false; 3], "standing: in place");

        // Standing again leaves the last tick and pose on the root: the derive that drops them is
        // not mounted here, and the presentation reads the state first.
        let set = |app: &mut App, state: TreeState, resting: Option<Resting>| {
            let mut tree = app.world_mut().entity_mut(root);
            tree.insert(state);
            if let Some(resting) = resting {
                tree.insert((Fell { tick: 0 }, resting));
            }
            app.update();
        };
        let east = Resting {
            hinge: FRAC_PI_2,
            tilt: 1.2,
            seat: Vec3::new(0.4, 0.2, 0.0),
            dx: 1.0,
            dz: 0.0,
        };
        set(&mut app, TreeState::Down { cut: 1, dir: 0 }, Some(east));
        assert_eq!(turned(&app), [false, true, false], "cut 1: its trunk turns");
        assert_eq!(
            moved(&app),
            [false, true, false],
            "cut 1: its trunk alone leaves the stump"
        );
        let turn = app.world().get::<Transform>(trunks[1]).unwrap().rotation;
        assert!(
            (turn * Vec3::Y).abs_diff_eq(Vec3::new(libm::sinf(1.2), libm::cosf(1.2), 0.0), 1e-5),
            "the trunk's axis leans to +x by the tilt: {}",
            turn * Vec3::Y
        );
        assert_eq!(
            shown(&app, &caps_top),
            [true, false, false],
            "cap_top below the cut"
        );
        assert_eq!(
            shown(&app, &caps_bottom),
            [false, true, false],
            "cap_bottom of the cut"
        );

        set(&mut app, TreeState::Standing, None);
        assert_eq!(turned(&app), [false; 3], "standing again: upright");
        assert_eq!(
            moved(&app),
            [false; 3],
            "standing again: at the bound translation"
        );
        assert_eq!(shown(&app, &caps_top), [false; 3], "standing again: no cap");
        assert_eq!(shown(&app, &caps_bottom), [false; 3]);

        set(
            &mut app,
            TreeState::Down { cut: 0, dir: 64 },
            Some(Resting {
                hinge: 1.0,
                tilt: 1.0,
                seat: Vec3::ZERO,
                dx: 0.0,
                dz: 1.0,
            }),
        );
        assert_eq!(moved(&app), [false; 3], "cut 0: the butt stays at the base");
        assert_eq!(
            turned(&app),
            [true, false, false],
            "cut 0: the first trunk turns"
        );
        assert_eq!(
            shown(&app, &caps_top),
            [false; 3],
            "cut 0 reveals no cap_top"
        );
        assert_eq!(shown(&app, &caps_bottom), [true, false, false]);

        set(&mut app, TreeState::Down { cut: 2, dir: 0 }, Some(east));
        assert_eq!(
            turned(&app),
            [false, false, true],
            "cut 2: the old trunk stands again"
        );
        assert_eq!(shown(&app, &caps_top), [false, true, false]);
        assert_eq!(shown(&app, &caps_bottom), [false, false, true]);
    }

    /// One template node at `translation` under `parent`, with `part` and a drawn `mesh`.
    fn node(
        parent: Option<usize>,
        translation: Vec3,
        part: Option<(Part, usize)>,
        mesh: Option<(Handle<Mesh>, &Handle<StandardMaterial>)>,
    ) -> TemplateNode {
        TemplateNode {
            parent,
            transform: Transform::from_translation(translation),
            name: None,
            part,
            mesh: mesh.map(|(mesh, material)| (mesh, Some(material.clone()))),
        }
    }

    /// The fixture kind's scene as the template reads it (fixture data, the generator's nesting):
    /// a root node; under it the three trunk nodes chained bottom-up, each 2 m above its parent
    /// but the first; under each its trunk primitive (bark), its two cap nodes with a primitive
    /// each (cut wood), the top one at the segment's top, and above the base a canopy node with a
    /// primitive (needles). Every primitive's mesh differs from the others, so a merge that drops,
    /// repeats or reorders one reds a comparison. Returns the nodes; the bark and needle
    /// primitives in node order, each with its height above the root as the nesting states it;
    /// and the bark and needle materials.
    fn fixture_nodes(
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> (
        Vec<TemplateNode>,
        [Vec<(Handle<Mesh>, f32)>; 2],
        [Handle<StandardMaterial>; 2],
    ) {
        let [bark, needles, wood] =
            [0.1, 0.5, 0.9].map(|grey| materials.add(Color::srgb(grey, grey, grey)));
        let mut nodes = vec![node(None, Vec3::ZERO, None, None)];
        let (mut trunks, mut canopy) = (Vec::new(), Vec::new());
        let mut below = 0;
        for k in 0..3 {
            let lift = if k == 0 { 0.0 } else { 2.0 };
            let height = 2.0 * k as f32;
            nodes.push(node(
                Some(below),
                Vec3::Y * lift,
                Some((Part::Trunk, k)),
                None,
            ));
            let trunk = nodes.len() - 1;
            let mesh = meshes.add(Cylinder::new(0.3 - 0.1 * k as f32, 2.0));
            trunks.push((mesh.clone(), height));
            nodes.push(node(Some(trunk), Vec3::ZERO, None, Some((mesh, &bark))));
            for (part, at) in [(Part::CapBottom, 0.0), (Part::CapTop, 2.0)] {
                nodes.push(node(Some(trunk), Vec3::Y * at, Some((part, k)), None));
                let cap = nodes.len() - 1;
                let mesh = meshes.add(Circle::new(0.25 - 0.05 * k as f32));
                nodes.push(node(Some(cap), Vec3::ZERO, None, Some((mesh, &wood))));
            }
            if k > 0 {
                let mesh = meshes.add(Cone::new(1.0 + k as f32, 2.0));
                canopy.push((mesh.clone(), height));
                nodes.push(node(
                    Some(trunk),
                    Vec3::ZERO,
                    Some((Part::Other, k)),
                    Some((mesh, &needles)),
                ));
            }
            below = trunk;
        }
        (nodes, [trunks, canopy], [bark, needles])
    }

    fn float3(mesh: &Mesh, attribute: bevy::mesh::MeshVertexAttribute) -> Vec<[f32; 3]> {
        match mesh.attribute(attribute) {
            Some(VertexAttributeValues::Float32x3(values)) => values.clone(),
            other => panic!("{attribute:?}: {other:?}"),
        }
    }

    fn uvs(mesh: &Mesh) -> Vec<[f32; 2]> {
        match mesh.attribute(Mesh::ATTRIBUTE_UV_0) {
            Some(VertexAttributeValues::Float32x2(values)) => values.clone(),
            other => panic!("uv: {other:?}"),
        }
    }

    fn bits<const N: usize>(values: &[[f32; N]]) -> Vec<[u32; N]> {
        values.iter().map(|value| value.map(f32::to_bits)).collect()
    }

    /// THE STANDING MESHES ARE THE STANDING PRIMITIVES, BIT FOR BIT: the template's standing view
    /// is one mesh per material in first-use order — bark, then needles, no cut wood — and each is
    /// its material's primitives in node order, every position its own lifted by its node's
    /// height (stated by the fixture, not composed by the code under test), every normal and UV
    /// its own, every index its own offset past the vertices before it. A merge that skips a
    /// node's transform, takes in a cap, reorders the primitives or drops an attribute reds this,
    /// and the standing tree would not look as its segments do.
    #[test]
    fn the_standing_meshes_are_the_standing_primitives_bit_for_bit() {
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let (nodes, parts, [bark, needles]) = fixture_nodes(&mut meshes, &mut materials);
        let merged = standing_meshes(&nodes, &meshes).expect("the fixture merges");
        assert_eq!(
            merged
                .iter()
                .map(|(_, material)| material.clone())
                .collect::<Vec<_>>(),
            [Some(bark), Some(needles)],
            "one mesh per standing material"
        );
        for ((mesh, _), parts) in merged.iter().zip(&parts) {
            let (mut positions, mut normals, mut uv, mut indices) =
                (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for (handle, height) in parts {
                let part = meshes.get(handle).unwrap();
                let offset = positions.len() as u32;
                positions.extend(
                    float3(part, Mesh::ATTRIBUTE_POSITION)
                        .iter()
                        .map(|[x, y, z]| [*x, *y + height, *z]),
                );
                normals.extend(float3(part, Mesh::ATTRIBUTE_NORMAL));
                uv.extend(uvs(part));
                indices.extend(part.indices().unwrap().iter().map(|i| i as u32 + offset));
            }
            assert_eq!(
                bits(&float3(mesh, Mesh::ATTRIBUTE_POSITION)),
                bits(&positions)
            );
            assert_eq!(bits(&float3(mesh, Mesh::ATTRIBUTE_NORMAL)), bits(&normals));
            assert_eq!(bits(&uvs(mesh)), bits(&uv));
            assert_eq!(
                mesh.indices()
                    .unwrap()
                    .iter()
                    .map(|i| i as u32)
                    .collect::<Vec<_>>(),
                indices
            );
        }
    }

    /// The fixture view bed: the view plugin's own schedule over the fixture kind's template,
    /// read, at fall clock tick 1000. Returns the app, the template's standing meshes and its
    /// node count with a mesh.
    fn view_bed() -> (
        App,
        Vec<(Handle<Mesh>, Option<Handle<StandardMaterial>>)>,
        usize,
    ) {
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let (nodes, _, _) = fixture_nodes(&mut meshes, &mut materials);
        let drawn = nodes.iter().filter(|node| node.mesh.is_some()).count();
        let template = TreeTemplate::new("fixture", nodes, &mut meshes);
        let standing = template.standing.clone().expect("the fixture merges");
        let mut app = App::new();
        app.add_plugins(plugin)
            .insert_resource(FallClock {
                tick: 1000,
                overstep: 0.0,
            })
            .insert_resource(Time::<Fixed>::from_hz(64.0))
            .insert_resource(TreeTemplates(HashMap::from([(
                fixture_kind().id.clone(),
                Some(template),
            )])));
        (app, standing, drawn)
    }

    /// A standing fixture tree at `scale` with its view attached: its root and view.
    fn spawn_tree(app: &mut App, scale: f32) -> (Entity, Entity) {
        let world = app.world_mut();
        let root = world
            .spawn((
                TreeState::Standing,
                TreeBody::new(fixture_kind(), scale),
                Transform::IDENTITY,
            ))
            .id();
        let view = world
            .spawn((
                ChildOf(root),
                Transform::from_scale(Vec3::splat(scale)),
                TreeView,
            ))
            .id();
        (root, view)
    }

    /// Every mesh entity under `root`, as its mesh and material, in hierarchy order.
    fn meshes_under(
        app: &mut App,
        root: Entity,
    ) -> Vec<(Handle<Mesh>, Option<Handle<StandardMaterial>>)> {
        let world = app.world_mut();
        let mut drawn = world.query::<(&Mesh3d, Option<&MeshMaterial3d<StandardMaterial>>)>();
        // Breadth first from the root, each entity's children in their order.
        let mut below = vec![root];
        let mut k = 0;
        while k < below.len() {
            if let Some(children) = world.get::<Children>(below[k]) {
                below.extend(children.iter());
            }
            k += 1;
        }
        below
            .into_iter()
            .filter_map(|entity| drawn.get(world, entity).ok())
            .map(|(mesh, material)| (mesh.0.clone(), material.map(|material| material.0.clone())))
            .collect()
    }

    /// A STANDING TREE IS TWO MESH ENTITIES, SHARED BY ITS KIND: once its kind's template is read,
    /// a standing tree draws exactly one mesh entity per material of the kind's standing
    /// primitives — two for the fixture, trunk and canopy, as for the shipped fir — and every tree
    /// of the kind, whatever its scale, draws the same mesh and material handles. The forest's
    /// frame cost relies on the count (#157: a standing fir drew 27 of its glb's 57 primitives as
    /// entities), and the renderer's instancing on the sharing, which batches only equal handles.
    /// A view that draws the glb's segments while standing, shows a cap, or merges per tree reds
    /// this.
    #[test]
    fn a_standing_tree_is_two_mesh_entities_shared_by_its_kind() {
        let (mut app, standing, _) = view_bed();
        assert_eq!(standing.len(), 2, "trunk and canopy");
        let trees = [1.0, 1.5].map(|scale| spawn_tree(&mut app, scale));
        app.update();
        for (root, view) in trees {
            assert_eq!(meshes_under(&mut app, root), standing);
            assert!(app.world().get::<TreeViewNodes>(view).is_none());
        }
    }

    /// THE FIRST FALL BUILDS THE SEGMENTED VIEW IN ITS OWN FRAME: in the one update whose state
    /// leaves `Standing`, the standing meshes go and every node of the template is spawned under
    /// the view, its every primitive drawn, indexed and presented in that frame. With the clock at
    /// the transition's tick every trunk node stands upright at its bound translation, so that
    /// frame draws the standing geometry and nothing pops, while exactly the cut's cap pair shows.
    /// The fall then runs on those same entities, and the tree standing again draws its two
    /// standing meshes. A swap a frame late, a hierarchy rebuilt every frame, or a presentation
    /// before the build reds this.
    #[test]
    fn the_first_fall_builds_the_segmented_view_in_its_frame() {
        let (mut app, standing, drawn) = view_bed();
        let (root, view) = spawn_tree(&mut app, 1.0);
        app.update();
        assert_eq!(meshes_under(&mut app, root), standing);

        app.world_mut().entity_mut(root).insert((
            TreeState::Down { cut: 1, dir: 0 },
            Fell { tick: 1000 },
            Resting {
                hinge: FRAC_PI_2,
                tilt: 1.2,
                seat: Vec3::new(0.4, 0.2, 0.0),
                dx: 1.0,
                dz: 0.0,
            },
        ));
        app.update();
        let segments = meshes_under(&mut app, root);
        assert_eq!(segments.len(), drawn, "every primitive drawn");
        assert!(
            standing.iter().all(|mesh| !segments.contains(mesh)),
            "no standing mesh left"
        );
        let world = app.world();
        let index = world
            .get::<TreeViewNodes>(view)
            .expect("indexed in the frame");
        let trunks: Vec<Entity> = index.trunks.iter().map(|trunk| trunk.unwrap()).collect();
        for (k, trunk) in trunks.iter().enumerate() {
            let local = world.get::<Transform>(*trunk).unwrap();
            assert_eq!(local.rotation, Quat::IDENTITY, "trunk {k}: upright");
            assert_eq!(local.translation, index.standing[k], "trunk {k}: in place");
        }
        let shown = |caps: &[Option<Entity>]| -> Vec<bool> {
            caps.iter()
                .map(|cap| world.get::<Visibility>(cap.unwrap()) == Some(&Visibility::Inherited))
                .collect()
        };
        assert_eq!(
            shown(&index.caps_top),
            [true, false, false],
            "cap_top below the cut"
        );
        assert_eq!(
            shown(&index.caps_bottom),
            [false, true, false],
            "cap_bottom of the cut"
        );

        app.world_mut().resource_mut::<FallClock>().tick = 1032;
        app.update();
        let index = app.world().get::<TreeViewNodes>(view).unwrap();
        assert_eq!(
            index
                .trunks
                .iter()
                .map(|trunk| trunk.unwrap())
                .collect::<Vec<_>>(),
            trunks,
            "the fall runs on the hierarchy it built"
        );
        assert_ne!(
            app.world().get::<Transform>(trunks[1]).unwrap().rotation,
            Quat::IDENTITY,
            "the cut's trunk turns"
        );

        app.world_mut().entity_mut(root).insert(TreeState::Standing);
        app.update();
        assert_eq!(meshes_under(&mut app, root), standing, "standing again");
        assert!(app.world().get::<TreeViewNodes>(view).is_none());
    }
}
