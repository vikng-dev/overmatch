//! The sim/view split's data source (design `sim-view-split-and-tank-bake.md` §8): the
//! tank-geometry extractor and its shadow harness.
//!
//! `extract_tank_geometry` parses the tank's `.glb` **as data** — the `gltf` crate against the
//! file, no Bevy scene, no asset dependency — into [`TankGeometry`]: every node's name, parent,
//! local transform, root-relative pose, (for sim-consumed meshes) raw vertex/index buffers, and
//! the typed part lists the sim classifies by naming convention (roadwheel stations tagged with
//! their track side, `*_Collider` collision proxies) — so the runtime never re-parses a node name
//! for sim meaning (design §8 step 3). Since step 1 this is what the sim skeleton spawns from
//! (`tank::spawn_tank_sim`) — the scene walk it replaced was step-0-shadow-proven to read exactly
//! these values. The same function is phase 2's offline compiler core (one parser, two mounting
//! points — design §6A).
//!
//! The shadow harness stays on: the extractor runs at startup and a shadow observer compares its
//! output against every instantiated tank scene — names, hierarchy, local transforms (bit-exact),
//! composed root poses (bit-exact, in `rig_world_pose`'s own operation order), and
//! collider/ballistic mesh bytes against `Assets<Mesh>`. Post-step-1 that direction reverses
//! meaning: it proves the *view* the player sees matches the data the sim runs on (and still
//! catches a bevy_gltf coordinate-conversion default flip — design §7.2). Mismatches panic in
//! debug builds and log errors in release; a clean pass logs one grep-able `SHADOW-BAKE ok` line
//! (harness evidence).
//!
//! The startup parse re-reads the glb the asset server also loads (~65 MB, once) — scaffolding
//! that phase 2 deletes along with the runtime glb dependency.

use std::collections::HashMap;
use std::path::Path;

use bevy::asset::io::file::FileAssetReader;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::spec::TankSpecHandle;
use crate::tank::{SimParts, TrackSide};

/// One glTF node, extracted. `name` follows bevy_gltf's rule exactly (authored name, else
/// `GltfNode{index}` — `bevy_gltf::loader::gltf_ext::scene::node_name`), so scene entities and
/// extracted nodes join on identical strings.
pub(crate) struct NodeGeometry {
    pub name: String,
    /// Index into [`TankGeometry::nodes`] of the parent node; `None` only for the scene-root
    /// wrapper (nodes[0], mirroring the loader's `Scene{i}` wrapper entity).
    pub parent: Option<usize>,
    /// The node's local TRS, converted exactly as bevy_gltf's `node_transform` converts it.
    pub transform: Transform,
    /// Root-relative pose, composed root→node in `rig_world_pose`'s exact operation order
    /// (`pos += rot * t; rot *= r`) so equal inputs give bit-equal outputs.
    pub root_position: Vec3,
    pub root_rotation: Quat,
    /// Raw mesh buffers, captured only where the sim consumes them ([`captures_mesh`]):
    /// collision proxies (convex hull source) and ballistic volumes (trimesh source).
    pub primitives: Vec<MeshGeometry>,
}

/// One glTF mesh primitive's sim-relevant buffers: what avian's `ConvexHullFromMesh` /
/// `TrimeshFromMesh` read (`extract_mesh_vertices_indices`: POSITION + the index buffer).
pub(crate) struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// The whole model, extracted as data — the sim skeleton's spawn source (`tank::spawn_tank_sim`),
/// shadow-verified against every instantiated tank scene.
pub(crate) struct TankGeometry {
    pub nodes: Vec<NodeGeometry>,
    pub by_name: HashMap<String, usize>,
    /// Load-bearing roadwheel stations — `(node index, TrackSide)`, one per `Wheel_L/R_<n>` empty
    /// ([`roadwheel_side`]), **sorted by node name**. That order is the wheels' [`WheelIndex`]
    /// (`crate::tank::WheelIndex`) slot order into `TankSim::anchors` — an index both wire ends
    /// derive, so the sort is load-bearing determinism, not cosmetics: a `HashMap`- or
    /// extraction-order list would let client and server disagree on which anchor is which wheel.
    pub roadwheels: Vec<(usize, TrackSide)>,
    /// Collision-proxy nodes — `*_Collider` node indices in extraction order. No wire-shared index
    /// derives from this order (each proxy just yields a convex hull), so it is not sorted.
    pub collision_proxies: Vec<usize>,
}

#[derive(Resource)]
pub(crate) struct ExtractedTankGeometry(pub TankGeometry);

/// Which nodes' mesh buffers the sim consumes: collision proxies (`*_Collider` → convex hull,
/// Vehicle layer) and ballistic volumes (`*_Ballistic` → trimesh, Armor layer). Volumes are
/// spec-keyed at bind, not name-matched — the golden test pins every declared volume to this
/// rule so a differently-suffixed volume can't silently dodge capture.
fn captures_mesh(name: &str) -> bool {
    name.ends_with("_Collider") || name.ends_with("_Ballistic")
}

/// The track side of a roadwheel *rig empty* — `Wheel_L_<n>` / `Wheel_R_<n>` with a purely numeric
/// index, and nothing else. The numeric check is load-bearing: it excludes the wheel's typed
/// children (`Wheel_L_0_Ballistic`, `Wheel_L_0_Visual`), which also begin with `Wheel_` but are a
/// volume / render mesh, not a suspension station. Lives here (not in the sim) because classifying
/// a node name into sim meaning is the extractor's job (design §8 step 3); [`TrackSide`] itself is
/// sim vocabulary, imported from `crate::tank`.
fn roadwheel_side(name: &str) -> Option<TrackSide> {
    for (prefix, side) in [
        ("Wheel_L_", TrackSide::Left),
        ("Wheel_R_", TrackSide::Right),
    ] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
            && rest.bytes().all(|b| b.is_ascii_digit())
        {
            return Some(side);
        }
    }
    None
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Startup, extract_at_startup);
    // Global observer, not per-spawn `.observe()`: fires for every world instance and self-gates
    // on `TankSpecHandle`, so no spawn path can forget to arm the shadow.
    app.add_observer(shadow_compare_on_instance_ready);
}

fn extract_at_startup(mut commands: Commands) {
    // Same base-path rule the asset server uses (BEVY_ASSET_ROOT → CARGO_MANIFEST_DIR → exe dir),
    // so the extractor and the loader always read the same file.
    let path = FileAssetReader::get_base_path()
        .join("assets")
        .join(crate::tank::TIGER_GLB_PATH);
    let geometry = extract_tank_geometry(&path)
        .unwrap_or_else(|err| panic!("bake: extracting {} failed: {err}", path.display()));
    let mesh_nodes = geometry
        .nodes
        .iter()
        .filter(|n| !n.primitives.is_empty())
        .count();
    info!(
        "bake: extracted tank geometry — {} nodes, {} mesh-captured",
        geometry.nodes.len(),
        mesh_nodes
    );
    commands.insert_resource(ExtractedTankGeometry(geometry));
}

/// Parse the glb as data into [`TankGeometry`]. Pure with respect to the app: `gltf` crate only,
/// usable identically from the runtime (step 0/phase 1) and the offline compiler (phase 2).
pub(crate) fn extract_tank_geometry(path: &Path) -> Result<TankGeometry, String> {
    let gltf::Gltf { document, mut blob } =
        gltf::Gltf::open(path).map_err(|e| format!("open: {e}"))?;

    // Resolve buffer data: a .glb's buffers are the BIN chunk (`Source::Bin`); external `.bin`
    // URIs are read relative to the glb (not used by our assets, supported for completeness).
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in document.buffers() {
        match buffer.source() {
            gltf::buffer::Source::Bin => buffers.push(
                blob.take()
                    .ok_or_else(|| "glb has a Bin buffer but no blob".to_string())?,
            ),
            gltf::buffer::Source::Uri(uri) => {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                buffers.push(
                    std::fs::read(parent.join(uri)).map_err(|e| format!("buffer {uri}: {e}"))?,
                );
            }
        }
    }

    // The loader instantiates `GltfAssetLabel::Scene(0)` under a wrapper entity named after the
    // scene (`Scene{i}` fallback) whose transform is the coordinate-conversion transform —
    // IDENTITY while bevy_gltf's opt-in glTF→Bevy conversion stays off (the repo never enables
    // it; the shadow compare is exactly what catches a future default flip — design §7.2).
    let scene = document
        .scenes()
        .next()
        .ok_or_else(|| "glb has no scene".to_string())?;
    let mut nodes = vec![NodeGeometry {
        name: scene
            .name()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("Scene{}", scene.index())),
        parent: None,
        transform: Transform::IDENTITY,
        root_position: Vec3::ZERO,
        root_rotation: Quat::IDENTITY,
        primitives: Vec::new(),
    }];
    let mut by_name: HashMap<String, usize> = HashMap::new();
    by_name.insert(nodes[0].name.clone(), 0);

    // Depth-first over the node tree, mirroring the loader's spawn recursion.
    let mut stack: Vec<(gltf::Node, usize)> = scene.nodes().map(|n| (n, 0usize)).collect();
    while let Some((node, parent)) = stack.pop() {
        // bevy_gltf's `node_name` rule, verbatim: every node ends up named.
        let name = node
            .name()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("GltfNode{}", node.index()));
        // bevy_gltf's `node_transform` conversion, verbatim.
        let transform = match node.transform() {
            gltf::scene::Transform::Matrix { matrix } => {
                Transform::from_matrix(Mat4::from_cols_array_2d(&matrix))
            }
            gltf::scene::Transform::Decomposed {
                translation,
                rotation,
                scale,
            } => Transform {
                translation: Vec3::from(translation),
                rotation: Quat::from_array(rotation),
                scale: Vec3::from(scale),
            },
        };
        // `rig_world_pose`'s composition, verbatim (root at identity): bit-equal by construction.
        let root_position =
            nodes[parent].root_position + nodes[parent].root_rotation * transform.translation;
        let root_rotation = nodes[parent].root_rotation * transform.rotation;

        let mut primitives = Vec::new();
        if captures_mesh(&name)
            && let Some(mesh) = node.mesh()
        {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .ok_or_else(|| format!("node `{name}`: primitive has no POSITION"))?
                    .collect();
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_default();
                primitives.push(MeshGeometry { positions, indices });
            }
        }

        let index = nodes.len();
        // Blender enforces unique object names and the fallback names are unique by index; a
        // collision would make the name-keyed join ambiguous, so it is fatal at extract time.
        if by_name.insert(name.clone(), index).is_some() {
            return Err(format!("duplicate node name `{name}`"));
        }
        nodes.push(NodeGeometry {
            name,
            parent: Some(parent),
            transform,
            root_position,
            root_rotation,
            primitives,
        });
        for child in node.children() {
            stack.push((child, index));
        }
    }

    // Classify the sim's name-convention parts once, here — the runtime consumes these typed lists
    // and never re-scans node names for sim meaning (design §8 step 3). glTF nodes only: the
    // extractor never captures the loader's per-material render leaves, so these lists can't be
    // polluted by mesh names the way the old runtime scene walk could. Index 0 is the scene-root
    // wrapper, skipped.
    let mut roadwheels: Vec<(usize, TrackSide)> = Vec::new();
    let mut collision_proxies: Vec<usize> = Vec::new();
    for (index, node) in nodes.iter().enumerate().skip(1) {
        if let Some(side) = roadwheel_side(&node.name) {
            roadwheels.push((index, side));
        } else if node.name.ends_with("_Collider") {
            collision_proxies.push(index);
        }
    }
    // Name-sorted: this is the `WheelIndex` slot order both wire ends derive (see the field doc).
    roadwheels.sort_by(|a, b| nodes[a.0].name.cmp(&nodes[b.0].name));

    Ok(TankGeometry {
        nodes,
        by_name,
        roadwheels,
        collision_proxies,
    })
}

/// The shadow harness: on every tank scene instantiation, verify the extracted geometry against
/// what the scene actually contains — the step-0 equivalence proof, kept as the sim-data-vs-view
/// guard (module doc). Read-only, so it is order-independent with respect to the view binder
/// (`tank::bind_tank_view`, an entity-scoped observer of this same event; this one is global and
/// writes nothing).
fn shadow_compare_on_instance_ready(
    ready: On<WorldInstanceReady>,
    geometry: Option<Res<ExtractedTankGeometry>>,
    tanks: Query<(), With<TankSpecHandle>>,
    sim_parts: Query<&SimParts>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    transforms: Query<&Transform>,
    primitives: Query<&Mesh3d>,
    meshes: Res<Assets<Mesh>>,
) {
    if !tanks.contains(ready.entity) {
        return;
    }
    let Some(geometry) = geometry.as_deref() else {
        // Startup extraction precedes any instantiation; absence is a wiring bug, not a race.
        fail(vec!["ExtractedTankGeometry resource missing at bind".into()]);
        return;
    };
    let geometry = &geometry.0;
    // The tank root's descendants hold TWO same-named trees since step 1: the data-spawned sim
    // skeleton and the instantiated glb scene (the view). The shadow's subject is the VIEW — skip
    // the sim parts, whose transforms are the extracted values by construction.
    let skeleton: std::collections::HashSet<Entity> = sim_parts
        .get(ready.entity)
        .map(|parts| parts.0.values().copied().collect())
        .unwrap_or_default();
    let mut mismatches: Vec<String> = Vec::new();

    // Scene side: every named descendant that is a glTF NODE. Mesh data always spawns as child
    // entities carrying `Mesh3d` — that presence is the primitive-leaf discriminator, NOT
    // `GltfMaterialName`: a primitive with an UNNAMED material never gets that marker (the coax
    // MG volumes' physics-only meshes), which the shadow's first run caught. (The binder's walk
    // still uses the marker and so silently indexes those mesh names — latent fragility, dies
    // with the walk in phase 1.)
    let mut seen: HashMap<&str, Entity> = HashMap::new();
    for entity in children.iter_descendants(ready.entity) {
        if skeleton.contains(&entity) || primitives.contains(entity) {
            continue;
        }
        let Ok(name) = names.get(entity) else {
            continue;
        };
        seen.insert(name.as_str(), entity);

        let Some(&index) = geometry.by_name.get(name.as_str()) else {
            mismatches.push(format!("scene node `{name}` not extracted"));
            continue;
        };
        let node = &geometry.nodes[index];

        // Local transform, bit-exact.
        if let Ok(local) = transforms.get(entity)
            && !transform_bits_eq(local, &node.transform)
        {
            mismatches.push(format!(
                "`{name}` local transform: scene {local:?} vs extracted {:?}",
                node.transform
            ));
        }

        // Parent node, by name. The scene-root wrapper's parent chain holds no extracted node.
        let scene_parent = nearest_extracted_ancestor(entity, ready.entity, geometry, &parents, &names);
        let extracted_parent = node.parent.map(|p| geometry.nodes[p].name.as_str());
        if scene_parent != extracted_parent {
            mismatches.push(format!(
                "`{name}` parent: scene {scene_parent:?} vs extracted {extracted_parent:?}"
            ));
        }

        // Composed root pose, bit-exact: catches any wrapper/intermediate divergence that local
        // comparisons can't see — this is the quantity `rig_world_pose` actually feeds the sim.
        if let Some((position, rotation)) =
            compose_scene_pose(entity, ready.entity, &parents, &transforms)
        {
            if position.to_array().map(f32::to_bits) != node.root_position.to_array().map(f32::to_bits)
                || rotation.to_array().map(f32::to_bits)
                    != node.root_rotation.to_array().map(f32::to_bits)
            {
                mismatches.push(format!(
                    "`{name}` root pose: scene ({position:?}, {rotation:?}) vs extracted ({:?}, {:?})",
                    node.root_position, node.root_rotation
                ));
            }
        } else {
            mismatches.push(format!("`{name}`: broken parent chain to the tank root"));
        }

        // Mesh bytes, where the sim consumes them: the node's primitive children vs the captured
        // buffers, compared as order-insensitive multisets of exact bits.
        if captures_mesh(name.as_str()) {
            let mut scene_prims: Vec<(Vec<u32>, Vec<u32>)> = Vec::new();
            if let Ok(node_children) = children.get(entity) {
                for &child in node_children {
                    let Ok(mesh3d) = primitives.get(child) else {
                        continue;
                    };
                    let Some(mesh) = meshes.get(&mesh3d.0) else {
                        mismatches.push(format!("`{name}`: primitive mesh asset missing"));
                        continue;
                    };
                    let positions = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
                        Some(VertexAttributeValues::Float32x3(p)) => {
                            p.iter().flatten().copied().map(f32::to_bits).collect()
                        }
                        _ => {
                            mismatches.push(format!("`{name}`: primitive has no f32x3 POSITION"));
                            continue;
                        }
                    };
                    let indices: Vec<u32> = match mesh.indices() {
                        Some(idx) => idx.iter().map(|i| i as u32).collect(),
                        None => Vec::new(),
                    };
                    scene_prims.push((positions, indices));
                }
            }
            let mut extracted_prims: Vec<(Vec<u32>, Vec<u32>)> = node
                .primitives
                .iter()
                .map(|p| {
                    (
                        p.positions.iter().flatten().copied().map(f32::to_bits).collect(),
                        p.indices.clone(),
                    )
                })
                .collect();
            scene_prims.sort();
            extracted_prims.sort();
            if scene_prims != extracted_prims {
                mismatches.push(format!(
                    "`{name}` mesh data: scene {} primitives ({} verts) vs extracted {} ({} verts)",
                    scene_prims.len(),
                    scene_prims.iter().map(|p| p.0.len() / 3).sum::<usize>(),
                    extracted_prims.len(),
                    extracted_prims.iter().map(|p| p.0.len() / 3).sum::<usize>(),
                ));
            }
        }
    }

    // Reverse direction: every extracted node must exist in the scene.
    for node in &geometry.nodes {
        if !seen.contains_key(node.name.as_str()) {
            mismatches.push(format!("extracted node `{}` not in scene", node.name));
        }
    }

    if mismatches.is_empty() {
        let verts: usize = geometry
            .nodes
            .iter()
            .flat_map(|n| &n.primitives)
            .map(|p| p.positions.len())
            .sum();
        info!(
            "bake: SHADOW-BAKE ok — {} nodes matched, {} captured verts",
            geometry.nodes.len(),
            verts
        );
    } else {
        fail(mismatches);
    }
}

/// Shadow verdict: fatal in debug (the equivalence proof failed — phase 1 must not build on it),
/// loud-but-alive in release.
fn fail(mismatches: Vec<String>) {
    for m in &mismatches {
        error!("bake: SHADOW-BAKE mismatch: {m}");
    }
    if cfg!(debug_assertions) {
        panic!(
            "bake: shadow compare failed with {} mismatches (see log)",
            mismatches.len()
        );
    }
}

/// Nearest ancestor of `entity` (below `root`) that is an extracted node, by name — tolerant of
/// loader wrapper entities that aren't glTF nodes.
fn nearest_extracted_ancestor<'a>(
    entity: Entity,
    root: Entity,
    geometry: &'a TankGeometry,
    parents: &Query<&ChildOf>,
    names: &'a Query<&Name>,
) -> Option<&'a str> {
    let mut current = parents.get(entity).ok()?.parent();
    while current != root {
        if let Ok(name) = names.get(current)
            && let Some(&index) = geometry.by_name.get(name.as_str())
        {
            return Some(geometry.nodes[index].name.as_str());
        }
        current = parents.get(current).ok()?.parent();
    }
    None
}

/// `rig_world_pose` with an identity root, over the full entity chain (loader wrappers included —
/// identity transforms are bit-exact no-ops in this composition).
fn compose_scene_pose(
    entity: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Option<(Vec3, Quat)> {
    let mut chain = Vec::new();
    let mut current = entity;
    while current != root {
        chain.push(current);
        current = parents.get(current).ok()?.parent();
    }
    let mut position = Vec3::ZERO;
    let mut rotation = Quat::IDENTITY;
    for &link in chain.iter().rev() {
        let local = transforms.get(link).ok()?;
        position += rotation * local.translation;
        rotation *= local.rotation;
    }
    Some((position, rotation))
}

fn transform_bits_eq(a: &Transform, b: &Transform) -> bool {
    a.translation.to_array().map(f32::to_bits) == b.translation.to_array().map(f32::to_bits)
        && a.rotation.to_array().map(f32::to_bits) == b.rotation.to_array().map(f32::to_bits)
        && a.scale.to_array().map(f32::to_bits) == b.scale.to_array().map(f32::to_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spec::TankSpec;

    /// The extractor's golden test: extract the Tiger and hold it to the same contract the
    /// binder enforces at runtime — every spec-declared node present, the structural singletons,
    /// wheels per side, and sim-consumed mesh data captured with the buffers avian requires
    /// (indices are mandatory for BOTH collider paths: avian's `extract_mesh_vertices_indices`
    /// bails on unindexed meshes even for the hull).
    #[test]
    fn tiger_1_extracts_to_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(crate::tank::TIGER_GLB_PATH);
        let geometry = extract_tank_geometry(&path).expect("tiger_1.glb must extract");
        let spec: TankSpec = ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
            .expect("tiger_1.tank.ron must parse");

        let node = |name: &str| -> &NodeGeometry {
            let index = geometry
                .by_name
                .get(name)
                .unwrap_or_else(|| panic!("extracted geometry is missing node `{name}`"));
            &geometry.nodes[*index]
        };

        // Every spec-declared node resolves (the bind contract, from data alone).
        for servo in spec.servos.keys() {
            node(servo);
        }
        for weapon in spec.weapons.values() {
            node(&weapon.muzzle);
            if let Some(barrel) = &weapon.barrel {
                node(barrel);
            }
        }
        for volume in spec.volumes.keys() {
            // Every declared volume must fall under the mesh-capture rule AND carry an indexed
            // mesh — a differently-suffixed or unindexed volume would silently break phase 1.
            assert!(
                captures_mesh(volume),
                "volume `{volume}` dodges the mesh-capture rule"
            );
            let n = node(volume);
            assert!(
                !n.primitives.is_empty(),
                "volume `{volume}` captured no mesh data"
            );
            for p in &n.primitives {
                assert!(p.positions.len() >= 3, "volume `{volume}`: degenerate mesh");
                assert!(!p.indices.is_empty(), "volume `{volume}`: unindexed mesh");
            }
        }
        for view in spec.views.values() {
            node(&view.node);
        }
        node("Hull");
        node("Center_Of_Mass");

        // Wheels: 8 per side on the Tiger (snapshot; SIM-EVIDENCE's 16/16), via the extractor's
        // typed list.
        let per_side = |want| {
            geometry
                .roadwheels
                .iter()
                .filter(|&&(_, side)| side == want)
                .count()
        };
        assert_eq!(per_side(crate::tank::TrackSide::Left), 8);
        assert_eq!(per_side(crate::tank::TrackSide::Right), 8);
        // The wheel list is name-sorted — the load-bearing `WheelIndex` slot order both wire ends
        // derive — so pin that too, not just the per-side counts.
        let wheel_names: Vec<&str> = geometry
            .roadwheels
            .iter()
            .map(|&(index, _)| geometry.nodes[index].name.as_str())
            .collect();
        let mut sorted = wheel_names.clone();
        sorted.sort_unstable();
        assert_eq!(wheel_names, sorted, "roadwheels must be extracted name-sorted");

        // Collision proxies: present (extraction order), captured, indexed — via the typed list.
        assert!(
            !geometry.collision_proxies.is_empty(),
            "no *_Collider proxies extracted"
        );
        for &index in &geometry.collision_proxies {
            let collider = &geometry.nodes[index];
            assert!(!collider.primitives.is_empty());
            for p in &collider.primitives {
                assert!(p.positions.len() >= 4, "`{}`: degenerate hull source", collider.name);
                assert!(!p.indices.is_empty(), "`{}`: unindexed mesh", collider.name);
            }
        }

        // Rig chains are authored scale-1 (`rig_world_pose` composes rigidly) — pin it for every
        // node the sim's pose chains traverse.
        for name in spec
            .servos
            .keys()
            .map(String::as_str)
            .chain(spec.weapons.values().flat_map(|w| {
                std::iter::once(w.muzzle.as_str()).chain(w.barrel.as_deref())
            }))
            .chain(["Hull", "Center_Of_Mass"])
        {
            assert_eq!(
                node(name).transform.scale,
                Vec3::ONE,
                "rig node `{name}` is not scale-1"
            );
        }
    }
}
