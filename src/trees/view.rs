//! The tree's view, under the tree's root (ADR-0014: the sim never waits on it; ADR-0016: it
//! derives from the state).
//!
//! TWO REPRESENTATIONS. A standing tree draws its kind's standing meshes: every glb primitive
//! outside a cut cap, merged in the view root's frame, one mesh per material (trunk and canopy for
//! the shipped kinds) and shared by every tree of the kind, so the renderer instances them. A tree
//! that is down draws the glb's segmented hierarchy, cut caps included, rebuilt from the kind's
//! [`TreeTemplate`] in the frame its state leaves `Standing` ([`draw_trees`]), and the fall is
//! presented on it from the authoritative body pose ([`present_fall`]). A tree standing
//! again draws its standing meshes again; a settled tree keeps the segmented hierarchy.
//!
//! THE TEMPLATE. Read once per kind off one hidden, unparented instance of the kind's glb scene
//! ([`read_template`]), bound to the trunk by the generator's node extras (`tree_part`,
//! `tree_segment`), then despawned. A view drawn before its kind's template is read draws nothing
//! until it is.

use std::collections::{BTreeSet, HashMap};

use bevy::app::SceneSpawnerSystems;
use bevy::asset::LoadState;
use bevy::camera::visibility::VisibilityRange;
use bevy::gltf::GltfExtras;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy::world_serialization::{WorldAssetRoot, WorldInstanceReady};

use super::{TreeBody, TreeKind, TreePiece, TreeState};
use crate::scatter::MapInstance;

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
    let (id, segments, assets) = (kind.id.clone(), kind.segments.len(), assets.clone());
    commands.queue(move |world: &mut World| request_template(world, id, segments, &assets));
}

/// Every tree kind's view template by kind id: `None` while its glb loads.
#[derive(Resource, Default)]
pub(crate) struct TreeTemplates(HashMap<String, Option<TreeTemplate>>);

/// The hidden glb instance a kind's template is read from, by kind id.
#[derive(Component)]
struct TreeViewSource {
    id: String,
    segments: usize,
}

/// Start reading kind `id`'s template unless it is read or loading: one hidden, unparented instance
/// of the kind's glb scene, read and despawned once it is ready ([`read_template`]).
fn request_template(world: &mut World, id: String, segments: usize, assets: &AssetServer) {
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
            TreeViewSource { id, segments },
        ))
        .observe(read_template);
}

/// A kind's glb is a required, shipped asset: one that fails to load would leave its template
/// pending and every tree of the kind undrawn while its colliders stand. Refused by name.
fn refuse_failed_templates(
    sources: Query<(&TreeViewSource, &WorldAssetRoot)>,
    server: Res<AssetServer>,
) {
    for (TreeViewSource { id, .. }, root) in &sources {
        if let LoadState::Failed(err) = server.load_state(&root.0) {
            panic!("trees: the view of kind {id} (trees/{id}/{id}.glb) failed to load: {err}");
        }
    }
}

/// Which part of the trunk a glb node presents, by the generator's `tree_part`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Trunk,
    CapTop,
    CapBottom,
    /// The canopy: drawn standing and down, never indexed.
    Canopy,
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
/// materials.
pub(crate) struct TreeTemplate {
    nodes: Vec<TemplateNode>,
    standing: Vec<(Handle<Mesh>, Option<Handle<StandardMaterial>>)>,
}

impl TreeTemplate {
    /// The template of `nodes`, its standing meshes merged ([`standing_meshes`]) and added to
    /// `meshes`. Primitives that cannot merge are a broken ship, refused against kind `id`.
    fn new(id: &str, segments: usize, nodes: Vec<TemplateNode>, meshes: &mut Assets<Mesh>) -> Self {
        check_bindings(id, segments, &nodes);
        let merged = standing_meshes(&nodes, meshes).unwrap_or_else(|why| {
            panic!(
                "trees: {id}.glb's standing primitives do not merge ({why}); export every \
                 primitive of one material with the same vertex attributes and indexing"
            )
        });
        info!(
            "trees: {id}'s view read — a standing tree draws {} merged meshes, a fallen one {} \
             entities of which {} draw",
            merged.len(),
            nodes.len(),
            nodes.iter().filter(|node| node.mesh.is_some()).count()
        );
        let standing = merged
            .into_iter()
            .map(|(mesh, material)| (meshes.add(mesh), material))
            .collect();
        Self { nodes, standing }
    }
}

/// Refuse a broken view at template load, before its standing meshes can be drawn. The
/// segmented view needs exactly one trunk and both caps per declared segment, even if the
/// first instance never falls, and each of them draws: some mesh's nearest binding (its own or an
/// ancestor's) is that one. A binding with no mesh leaves its segment's collider undrawn.
fn check_bindings(id: &str, segments: usize, nodes: &[TemplateNode]) {
    let mut bound = vec![[false; 3]; segments];
    let mut drawn = vec![[false; 3]; segments];
    // Each node's nearest binding, its own first; a parent precedes its children.
    let mut nearest: Vec<Option<(Part, usize)>> = Vec::with_capacity(nodes.len());
    for node in nodes {
        let binding = node.part.or_else(|| node.parent.and_then(|p| nearest[p]));
        nearest.push(binding);
        if node.mesh.is_some()
            && let Some((part, segment)) = binding
            && let Some(slot) = slot_of(part)
            && segment < segments
        {
            drawn[segment][slot] = true;
        }
        let Some((part, segment)) = node.part else {
            continue; // Unmarked grouping and primitive nodes bind no part.
        };
        let Some(slot) = slot_of(part) else {
            continue; // Canopy nodes carry no segmented binding.
        };
        assert!(
            segment < segments,
            "trees: trees/{id}/{id}.glb tree_segment {segment} for tree_part {part:?} is outside {segments} segments; re-export against its tree asset"
        );
        assert!(
            !bound[segment][slot],
            "trees: trees/{id}/{id}.glb repeats tree_part {part:?} at tree_segment {segment}; export each trunk and cap binding once"
        );
        bound[segment][slot] = true;
    }
    for (segment, parts) in bound.iter().enumerate() {
        for (slot, part) in [Part::Trunk, Part::CapTop, Part::CapBottom]
            .iter()
            .enumerate()
        {
            assert!(
                parts[slot],
                "trees: trees/{id}/{id}.glb is missing tree_part {part:?} at tree_segment {segment}; export one trunk and both caps for every segment"
            );
            assert!(
                drawn[segment][slot],
                "trees: trees/{id}/{id}.glb binds tree_part {part:?} at tree_segment {segment} to no mesh; export its primitives under the binding node"
            );
        }
    }
}

/// A segmented binding's slot (trunk, top cap, bottom cap); the canopy binds no segment.
fn slot_of(part: Part) -> Option<usize> {
    match part {
        Part::Trunk => Some(0),
        Part::CapTop => Some(1),
        Part::CapBottom => Some(2),
        Part::Canopy => None,
    }
}

/// The per-node extras the generator writes.
#[derive(serde::Deserialize)]
struct PartExtras {
    tree_part: String,
    tree_segment: usize,
}

/// The part a node's glTF `extras` name, or `None` for extras that name none (the root's
/// generator record: species, seed, height). Extras that carry a part key but no valid part are a
/// broken ship: the prop pipeline's node contract (`scripts/prop/build.py`) certifies every
/// part node's, so the kind's glb is refused by name.
fn part_of(id: &str, extras: &str) -> Option<(Part, usize)> {
    let value: serde_json::Value = serde_json::from_str(extras).unwrap_or_else(|err| {
        panic!("trees: {id}.glb carries node extras that are no JSON ({err})")
    });
    if value.get("tree_part").is_none() && value.get("tree_segment").is_none() {
        return None;
    }
    let extras: PartExtras = serde_json::from_value(value).unwrap_or_else(|err| {
        panic!("trees: {id}.glb carries malformed tree part extras {extras} ({err}); re-export it")
    });
    let part = match extras.tree_part.as_str() {
        "trunk" => Part::Trunk,
        "cap_top" => Part::CapTop,
        "cap_bottom" => Part::CapBottom,
        "canopy" => Part::Canopy,
        other => panic!(
            "trees: {id}.glb names tree_part {other:?}; the parts are trunk, cap_top, cap_bottom \
             and canopy"
        ),
    };
    Some((part, extras.tree_segment))
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
    let source = sources
        .get(ready.entity)
        .expect("the template observer is mounted only on its source instance");
    let id = &source.id;
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
            let part = extras.and_then(|extras| part_of(id, &extras.value));
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
    let template = TreeTemplate::new(id, source.segments, nodes, &mut meshes);
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
        let want = if *state == TreeState::Standing {
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
        for (mesh, material) in &template.standing {
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
        // The prop pipeline's node contract binds every (part, segment) once, segments numbered
        // as the kind's asset numbers them.
        if let Some((part, k)) = node.part
            && part != Part::Canopy
        {
            assert!(
                k < segments,
                "a glb's part segment {k} lies within its asset's {segments}"
            );
        }
        let bind = |slot: &mut Option<Entity>| {
            assert!(slot.is_none(), "a glb binds each trunk part once");
            *slot = Some(id);
        };
        match node.part {
            Some((Part::Trunk, k)) => {
                bind(&mut index.trunks[k]);
                index.standing[k] = node.transform.translation;
            }
            // Every cap is hidden until a cut reveals it.
            Some((Part::CapTop, k)) => {
                bind(&mut index.caps_top[k]);
                entity.insert(Visibility::Hidden);
            }
            Some((Part::CapBottom, k)) => {
                bind(&mut index.caps_bottom[k]);
                entity.insert(Visibility::Hidden);
            }
            Some((Part::Canopy, _)) | None => {}
        }
        spawned.push(id);
    }
    assert!(
        [&index.trunks, &index.caps_top, &index.caps_bottom]
            .iter()
            .all(|bound| bound.iter().all(Option::is_some)),
        "the glb binds a trunk node and two caps to every segment of its asset"
    );
    commands.entity(view).insert(index);
}

/// The game's environment destruction view: the cut node follows the freed body's world pose.
/// Its ancestors retain the stump's placement and uniform scale. A missing carrier is the
/// receive interval before the authoritative pose stream arrives; the view stays upright then.
fn present_fall(
    trees: Query<(
        Ref<TreeState>,
        &TreeBody,
        &MapInstance,
        &Transform,
        &Children,
    )>,
    pieces: Query<(
        &TreePiece,
        &Transform,
        Option<&crate::world::WorldGeneration>,
    )>,
    generation: Option<Res<crate::world::CurrentWorld>>,
    views: Query<Ref<TreeViewNodes>>,
    mut nodes: Query<(&mut Transform, &mut Visibility), (Without<TreeBody>, Without<TreePiece>)>,
) {
    for (state, body, identity, root, children) in &trees {
        let Some(view) = children.iter().find_map(|child| views.get(child).ok()) else {
            continue; // The standing instanced representation has no segmented hierarchy.
        };
        let turned = match *state {
            TreeState::Standing => None,
            TreeState::Down { cut, .. } => pieces
                .iter()
                .find(|(piece, _, belongs)| {
                    // Offline pieces are local bodies, without a streamed generation tag.
                    belongs.is_none_or(|belongs| {
                        generation.as_deref().and_then(|current| current.0.as_ref())
                            == Some(belongs)
                    }) && piece.tree == identity.ordinal
                        && piece.cut == cut
                })
                .map(|(_, pose, _)| {
                    let turn = root.rotation.inverse() * pose.rotation;
                    let pivot = root.rotation.inverse() * (pose.translation - root.translation)
                        + turn
                            * (Vec3::Y * body.prefix[usize::from(cut)] - body.piece_centroid(cut));
                    let cut = usize::from(cut);
                    let translation =
                        view.standing[cut] + (pivot - Vec3::Y * body.prefix[cut]) / body.scale;
                    (cut, turn, translation)
                }),
        };
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

/// The view half: each tree's representation and the streamed pose on its segmented hierarchy.
pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TreeTemplates>()
        // A composition with no asset server requests no glb, so it has none to refuse.
        .add_systems(
            Update,
            refuse_failed_templates.run_if(resource_exists::<AssetServer>),
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
    use crate::trees::tests::fixture_kind;
    use bevy::mesh::VertexAttributeValues;

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
                    Some((Part::Canopy, k)),
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

    /// A NODE'S EXTRAS NAME ITS PART OR NONE: the root's generator record (the shipped fir's
    /// shape, fixture values) names no part and binds nothing, while extras that carry a part key
    /// but a malformed or unknown part refuse the glb. Reading those as "no part" would leave a
    /// trunk segment unbound and a felled tree's geometry standing.
    #[test]
    fn node_extras_name_their_part_or_none_and_malformed_parts_refuse() {
        let record = r#"{"tree_species": "norway_spruce", "tree_seed": 1, "tree_height_m": 27.0}"#;
        assert_eq!(part_of("fixture", record), None);
        assert_eq!(
            part_of("fixture", r#"{"tree_part": "cap_top", "tree_segment": 2}"#),
            Some((Part::CapTop, 2))
        );
        for broken in [
            r#"{"tree_part": "trunk"}"#,
            r#"{"tree_part": "trunk", "tree_segment": -1}"#,
            r#"{"tree_part": "bough", "tree_segment": 0}"#,
            "not json",
        ] {
            let refused = std::panic::catch_unwind(|| part_of("fixture", broken));
            assert!(refused.is_err(), "{broken} was read as a part or as none");
        }
    }

    /// A standing view must refuse bad part bindings at load: waiting until a fall would
    /// accept invisible or unposeable segments into play. ASSUMED fixture nodes and segment count.
    #[test]
    fn template_load_refuses_missing_duplicate_and_outside_part_bindings() {
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let (nodes, _, _) = fixture_nodes(&mut meshes, &mut materials);
        check_bindings("fixture", 3, &nodes);
        for mutation in 0..3 {
            let mut broken = nodes.clone();
            let index = broken
                .iter()
                .position(|node| node.part == Some((Part::Trunk, 1)))
                .unwrap();
            match mutation {
                0 => broken[index].part = None,
                1 => broken.push(broken[index].clone()),
                _ => broken[index].part = Some((Part::Trunk, 3)),
            }
            let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                TreeTemplate::new("fixture", 3, broken, &mut meshes);
            }))
            .expect_err("bad bindings must fail at template construction");
            let reason = refused
                .downcast_ref::<String>()
                .expect("binding refusal names its repair");
            for required in [
                "trees/fixture/fixture.glb",
                "tree_part",
                "tree_segment",
                "export",
            ] {
                assert!(reason.contains(required), "{reason}");
            }
        }
    }

    /// EVERY BINDING DRAWS: complete trunk and cap bindings whose meshes are gone, all of them or
    /// one trunk's alone, leave a segment's collider undrawn, so the template is refused when it
    /// is read. A check of the merged standing view alone passes the second. ASSUMED fixture nodes.
    #[test]
    fn a_binding_without_its_mesh_is_refused() {
        let mut meshes = Assets::<Mesh>::default();
        let mut materials = Assets::<StandardMaterial>::default();
        let (nodes, _, _) = fixture_nodes(&mut meshes, &mut materials);
        let trunk = nodes
            .iter()
            .position(|node| node.part == Some((Part::Trunk, 1)))
            .expect("the fixture binds trunk 1");
        let one_trunk: Vec<TemplateNode> = nodes
            .iter()
            .map(|node| TemplateNode {
                mesh: if node.parent == Some(trunk) && node.part.is_none() {
                    None
                } else {
                    node.mesh.clone()
                },
                ..node.clone()
            })
            .collect();
        let every: Vec<TemplateNode> = nodes
            .iter()
            .map(|node| TemplateNode {
                mesh: None,
                ..node.clone()
            })
            .collect();
        for (broken, segment) in [(every, 0), (one_trunk, 1)] {
            let refused = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                TreeTemplate::new("fixture", 3, broken, &mut meshes);
            }))
            .expect_err("a binding without its mesh must fail at template construction");
            let reason = refused
                .downcast_ref::<String>()
                .expect("the refusal names its repair");
            assert!(
                reason.contains(&format!("Trunk at tree_segment {segment} to no mesh")),
                "{reason}"
            );
        }
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
    /// read. Returns the app, the template's standing meshes and its
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
        let template = TreeTemplate::new("fixture", 3, nodes, &mut meshes);
        let standing = template.standing.clone();
        let mut app = App::new();
        app.add_plugins(plugin)
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
                MapInstance {
                    id: "fixture".to_owned(),
                    ordinal: 0,
                },
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

    /// The streamed body is the view's sole motion source. Renderer users rely on the cut node
    /// moving in the same frame the segmented view is built, even after a late template load.
    /// ASSUMED fixture pose, scale and cut; a local fall integrator or an ignored carrier fails.
    #[test]
    fn the_segmented_view_follows_the_authoritative_body_pose() {
        let (mut app, standing, drawn) = view_bed();
        let (root, view) = spawn_tree(&mut app, 1.5);
        app.update();
        assert_eq!(meshes_under(&mut app, root), standing);
        let body = app.world().get::<TreeBody>(root).unwrap().clone();
        let pose = Transform::from_xyz(3.0, 2.0, -1.0).with_rotation(Quat::from_rotation_z(-1.0));
        let piece = app
            .world_mut()
            .spawn((TreePiece { tree: 0, cut: 1 }, pose))
            .id();
        app.world_mut()
            .entity_mut(root)
            .insert(TreeState::Down { cut: 1, dir: 0 });
        app.update();
        assert_eq!(meshes_under(&mut app, root).len(), drawn);
        let index = app.world().get::<TreeViewNodes>(view).unwrap();
        let cut = index.trunks[1].unwrap();
        let local = *app.world().get::<Transform>(cut).unwrap();
        let parent = Transform::from_scale(Vec3::splat(body.scale))
            .mul_transform(Transform::from_translation(index.standing[0]));
        let world = parent.mul_transform(local);
        let expected =
            pose.translation + pose.rotation * (Vec3::Y * body.prefix[1] - body.piece_centroid(1));
        assert!(world.translation.abs_diff_eq(expected, 1e-5));
        assert!(world.rotation.abs_diff_eq(pose.rotation, 1e-5));
        assert_eq!(
            app.world().get::<Visibility>(index.caps_top[0].unwrap()),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().get::<Visibility>(index.caps_bottom[1].unwrap()),
            Some(&Visibility::Inherited)
        );
        let mut next = pose;
        next.translation += Vec3::X;
        app.world_mut().entity_mut(piece).insert(next);
        app.update();
        assert!(
            (app.world().get::<Transform>(cut).unwrap().translation.x
                - local.translation.x
                - 1.0 / body.scale)
                .abs()
                < 1e-5
        );
        app.world_mut().entity_mut(root).insert(TreeState::Standing);
        app.update();
        assert_eq!(meshes_under(&mut app, root), standing);
    }
}
