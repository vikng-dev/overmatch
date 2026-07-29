//! Tank-geometry extraction and view-shadow verification.
//!
//! Invariant (ADR-0014): simulation construction uses synchronously extracted data, never a loaded
//! scene. The shadow verifier compares that data with instantiated view geometry.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::Collider;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::spec::{TankSpec, TankSpecHandle};
use crate::tank::{SimParts, TrackSide, rig_world_pose};

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
    /// Raw mesh buffers, captured only where the sim consumes them ([`captures_mesh`]): collision
    /// proxies (convex hull source), ballistic volumes and roadwheel stations (trimesh source).
    /// Vertices are **node-local and unscaled** — exactly the bytes the glb holds, which is what the
    /// shadow compare needs to diff against the loaded `Mesh` assets. Consumers that build colliders
    /// hang them under the node entity and let avian compose [`transform`](Self::transform)'s scale
    /// down the hierarchy (`ColliderTransform`), so a node authored at scale ≠ 1 (the wheels, the
    /// coax MG volumes) is sized correctly without the extractor pre-baking anything.
    pub primitives: Vec<MeshGeometry>,
}

/// One glTF mesh primitive's sim-relevant buffers: what avian's `ConvexHullFromMesh` /
/// `TrimeshFromMesh` read (`extract_mesh_vertices_indices`: POSITION + the index buffer).
pub(crate) struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

impl MeshGeometry {
    /// Build the convex-hull collider consumed by both the game and the track sandbox.
    pub(crate) fn convex_hull_collider(&self) -> Option<Collider> {
        let points = self.positions.iter().copied().map(Vec3::from).collect();
        Collider::convex_hull(points)
    }
}

/// The whole model, extracted as data — the sim skeleton's construction source,
/// shadow-verified against every instantiated tank scene.
pub(crate) struct TankGeometry {
    pub nodes: Vec<NodeGeometry>,
    pub by_name: HashMap<String, usize>,
    /// Load-bearing roadwheel stations — `(node index, TrackSide)`, one per `Wheel_L/R_<n>` node
    /// ([`roadwheel_side`]), **sorted by node name** — a deterministic order every consumer
    /// (spawn, the track gear build) can rely on, instead of `HashMap`/extraction order. These
    /// nodes now carry the wheel mesh itself, so they are also ballistic volumes ([`captures_mesh`]).
    pub roadwheels: Vec<(usize, TrackSide)>,
    /// Collision-proxy nodes — `*_Collider` node indices in extraction order. No wire-shared index
    /// derives from this order (each proxy just yields a convex hull), so it is not sorted.
    pub collision_proxies: Vec<usize>,
}

/// Runtime spawn data for the shipped Tiger, assembled before network receive can run. The spec is
/// embedded and geometry is parsed synchronously from the GLB, so spawn does not depend on Bevy's
/// asset-server timing. Runtime GLB extraction and the missing content fingerprint remain the
/// transitional seam recorded in `ARCHITECTURE.md`.
#[derive(Resource, Clone)]
pub(crate) struct TankBlueprint {
    pub geometry: Arc<TankGeometry>,
    pub spec: Arc<TankSpec>,
}

const TIGER_SPEC_RON: &str = include_str!("../assets/tiger_1/tiger_1.tank.ron");

/// Which nodes' mesh buffers the sim consumes: collision proxies (`*_Collider` → convex hull,
/// Vehicle layer), ballistic volumes (`*_Ballistic` → trimesh, Armor layer), and the roadwheel
/// stations ([`roadwheel_side`]).
///
/// Wheels are the one node class that is BOTH. The Tiger's wheels were re-exported as one unified
/// watertight mesh per station (was: a `Wheel_L_0` empty parenting a `_Visual` + a `_Ballistic`
/// child), so the same node is the suspension station AND its own armour volume — and its buffers
/// have to be captured under the bare `Wheel_<side>_<n>` name or the spec's wheel volumes bind to
/// empty geometry. Nothing downstream required a station to be transform-only: the station role is
/// a marker component (`Roadwheel`) and the volume role a bundle + trimesh child, and they compose
/// on one entity (`tank::spawn::assemble_tank_body`).
///
/// Volumes are spec-keyed at bind, not name-matched — the golden test pins every declared volume to
/// this rule so a differently-suffixed volume can't silently dodge capture.
fn captures_mesh(name: &str) -> bool {
    name.ends_with("_Collider") || name.ends_with("_Ballistic") || roadwheel_side(name).is_some()
}

/// The track side of a roadwheel station — `Wheel_L_<n>` / `Wheel_R_<n>` with a purely numeric
/// index, and nothing else. The numeric check is load-bearing: it is what keeps a *typed* sibling
/// (`Wheel_L_0_Visual`, a spare `Wheel_L_0_Collider`) from being classified as a second station on
/// the same axle, which would double-count the wheel in the name-sorted slot order both wire ends
/// derive. Lives here (not in the sim) because classifying a node name into sim meaning is the
/// extractor's job (design §8 step 3); [`TrackSide`] itself is sim vocabulary, imported from
/// `crate::tank`.
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
    // Resolve the glb through the SAME `asset_root()` that `AssetPlugin`'s `file_path` uses, so the
    // bake and the asset server always open the same file. `asset_root()` already returns the
    // `…/assets` directory (on a macOS `.app`, `Contents/Resources/assets` — not the exe dir), so we
    // join the glb path straight onto it with NO extra `assets` segment. Using Bevy's raw
    // `get_base_path()` here instead was the v0.3.0-alpha.2 macOS crash: it resolved the exe dir
    // (`Contents/MacOS`) in a double-clicked bundle, `+ "assets"` → a directory that does not exist,
    // while the asset server was reading `Contents/Resources/assets`. See `crate::assets`.
    let root = crate::assets::asset_root();
    let path = root.join(crate::tank::TIGER_GLB_PATH);
    let geometry = extract_tank_geometry(&path).unwrap_or_else(|err| {
        panic!(
            "bake: extracting {} failed: {err}\n\
             bake resolves the glb via asset_root() (BEVY_ASSET_ROOT → CARGO_MANIFEST_DIR → exe dir; \
             a macOS `.app` exe in Contents/MacOS resolves to Contents/Resources/assets). \
             Resolved assets root: {}. If this path is wrong, the packaging layout and asset_root() \
             disagree — see crate::assets.",
            path.display(),
            root.display(),
        )
    });
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
    let spec: TankSpec = ron::de::from_str(TIGER_SPEC_RON)
        .unwrap_or_else(|err| panic!("bake: embedded Tiger spec failed to parse: {err}"));
    spec.validate()
        .unwrap_or_else(|err| panic!("bake: embedded Tiger spec failed validation: {err}"));

    commands.insert_resource(TankBlueprint {
        geometry: Arc::new(geometry),
        spec: Arc::new(spec),
    });
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
    // Name-sorted: the deterministic consumer order (see the field doc).
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
    blueprint: Option<Res<TankBlueprint>>,
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
    let Some(blueprint) = blueprint.as_deref() else {
        // Startup extraction precedes any instantiation; absence is a wiring bug, not a race.
        fail(vec!["TankBlueprint resource missing at bind".into()]);
        return;
    };
    let geometry = &blueprint.geometry;
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
        let scene_parent =
            nearest_extracted_ancestor(entity, ready.entity, geometry, &parents, &names);
        let extracted_parent = node.parent.map(|p| geometry.nodes[p].name.as_str());
        if scene_parent != extracted_parent {
            mismatches.push(format!(
                "`{name}` parent: scene {scene_parent:?} vs extracted {extracted_parent:?}"
            ));
        }

        // Composed root pose, bit-exact: catches any wrapper/intermediate divergence that local
        // comparisons can't see — this is the quantity `rig_world_pose` actually feeds the sim.
        if let Some((position, rotation)) = rig_world_pose(
            entity,
            ready.entity,
            Vec3::ZERO,
            Quat::IDENTITY,
            &parents,
            &transforms,
        ) {
            if position.to_array().map(f32::to_bits)
                != node.root_position.to_array().map(f32::to_bits)
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
                        p.positions
                            .iter()
                            .flatten()
                            .copied()
                            .map(f32::to_bits)
                            .collect(),
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
        assert_eq!(
            wheel_names, sorted,
            "roadwheels must be extracted name-sorted"
        );
        // Station AND armour in one node: the unified wheel mesh means every station is also a
        // volume, so it must be declared (the volume loop above then proves its geometry captured).
        // Re-split the asset into `Wheel_L_0` + a `_Ballistic` child and this fails at CI time
        // instead of leaving the wheels silently invisible to the penetration march.
        for &name in &wheel_names {
            assert!(
                spec.volumes.contains_key(name),
                "roadwheel `{name}` carries the wheel mesh but has no volume declaration"
            );
        }

        // Collision proxies: present (extraction order), captured, indexed — via the typed list.
        assert!(
            !geometry.collision_proxies.is_empty(),
            "no *_Collider proxies extracted"
        );
        for &index in &geometry.collision_proxies {
            let collider = &geometry.nodes[index];
            assert!(!collider.primitives.is_empty());
            for p in &collider.primitives {
                assert!(
                    p.positions.len() >= 4,
                    "`{}`: degenerate hull source",
                    collider.name
                );
                assert!(!p.indices.is_empty(), "`{}`: unindexed mesh", collider.name);
            }
        }

        // Rig chains are authored scale-1 (`rig_world_pose` composes rigidly) — pin it for every
        // node the sim's pose chains traverse.
        for name in spec
            .servos
            .keys()
            .map(String::as_str)
            .chain(
                spec.weapons
                    .values()
                    .flat_map(|w| std::iter::once(w.muzzle.as_str()).chain(w.barrel.as_deref())),
            )
            .chain(["Hull", "Center_Of_Mass"])
        {
            assert_eq!(
                node(name).transform.scale,
                Vec3::ONE,
                "rig node `{name}` is not scale-1"
            );
        }
    }

    /// Every node that SPINS must carry its axle in its own origin.
    ///
    /// A rotating part is driven by writing a local rotation onto its node — which turns the mesh
    /// about that node's origin. If the origin sits at the model root (geometry baked into the
    /// vertices, zero node translation — Blender's default for a mesh that was never re-origined),
    /// the part orbits the tank instead of spinning in place, and any consumer that reads the
    /// origin as the axle centre reads `(0, 0, 0)`.
    ///
    /// The check is deliberately blunt — *at the root* is the failure mode that actually happens on
    /// export, and it is invisible in Blender unless you look at the gizmo. It is not a proof that
    /// the origin is on the true axle (only the mesh centroid can say that; see
    /// `track::marker_model`), just that an origin was authored at all.
    #[test]
    fn rotating_nodes_carry_their_own_axle_origin() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(crate::tank::TIGER_GLB_PATH);
        let geometry = extract_tank_geometry(&path).expect("tiger_1.glb must extract");

        // Roadwheels come from the extractor's typed list; the drive/idler hubs are named directly
        // (they are rig meshes, not `Wheel_<side>_<n>` empties, so no typed list carries them).
        let spinning: Vec<&NodeGeometry> = geometry
            .roadwheels
            .iter()
            .map(|&(index, _)| &geometry.nodes[index])
            .chain(
                ["Sprocket_L", "Sprocket_R", "Idler_L", "Idler_R"]
                    .into_iter()
                    .map(|name| {
                        let index = geometry.by_name.get(name).unwrap_or_else(|| {
                            panic!("extracted geometry is missing drive hub `{name}`")
                        });
                        &geometry.nodes[*index]
                    }),
            )
            .collect();

        let at_root: Vec<&str> = spinning
            .iter()
            .filter(|n| n.root_position.length() < 1.0e-4)
            .map(|n| n.name.as_str())
            .collect();
        assert!(
            at_root.is_empty(),
            "rotating nodes whose origin is at the model root (set the object origin to the axle \
             in Blender and re-export): {at_root:?}"
        );
    }
}
