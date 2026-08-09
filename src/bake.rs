//! Tank-geometry extraction and view-shadow verification.
//!
//! Invariant (ADR-0014): simulation construction uses synchronously extracted data, never a loaded
//! scene. The shadow verifier compares that data with instantiated view geometry.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::Collider;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::spec::{TankSpec, TankSpecHandle};
use crate::substances::SubstanceRegistry;
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
    /// Composed scale root→node, as the COMPONENTWISE product avian's `ColliderTransform`
    /// propagation forms (`parent.scale * child.scale`, `collider_transform/plugin.rs:133`). On a
    /// node holding a substance primitive it must be bit-exactly `1`; [`manifold_gate`] refuses
    /// anything else.
    pub root_scale: Vec3,
    /// Raw mesh buffers, captured only where the sim consumes them ([`classify`]): collision
    /// proxies (convex hull source) and ballistic volumes, the road wheels included (trimesh source).
    /// Vertices are **node-local** — exactly the bytes the glb holds, which is what the shadow
    /// compare needs to diff against the loaded `Mesh` assets. Consumers that build colliders hang
    /// them under the node entity and let avian compose [`transform`](Self::transform)'s scale down
    /// the hierarchy (`ColliderTransform`), so a collision proxy may be authored at any scale; a
    /// ballistic node may not.
    pub primitives: Vec<MeshGeometry>,
}

/// One glTF mesh primitive's sim-relevant buffers: what avian's `ConvexHullFromMesh` /
/// `TrimeshFromMesh` read (`extract_mesh_vertices_indices`: POSITION + the index buffer), plus the
/// primitive's [substance](MeshGeometry::substance) where it has one.
pub(crate) struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// The §12 membership declaration, resolved here once: `Some` iff this primitive's glTF
    /// material name is a key in the global substance registry (`assets/materials/materials.ron`).
    /// `None` = not a ballistic primitive — a collision-proxy hull, or a decor primitive that was
    /// never captured at all. Per PRIMITIVE, not per node, because glTF splits a mesh by material
    /// and §13.7 authors a mixed-substance part as one object holding one closed shell per
    /// substance region (the Tiger road wheel: `MildSteel` bodies + `Rubber` rims in one object).
    pub substance: Option<PrimitiveSubstance>,
    /// What [`manifold_gate`] proved about this primitive, and the identities it proved it with.
    /// `None` for a non-substance primitive, which is not armour and is never walked.
    pub certificate: Option<ShellCertificate>,
}

/// The manifold gate's findings, carried forward instead of discarded — the identities the §13.4
/// walk pairs on.
///
/// Without them a coincidence of two shells of one primitive and a duplicate claim of one shell are
/// the same three numbers at the collector's interface, and the walk has to guess which; with them,
/// pairing is a fact.
#[derive(Debug)]
pub(crate) struct ShellCertificate {
    /// Which closed shell each triangle belongs to, index-aligned with `indices.chunks_exact(3)` —
    /// the gate's own edge-connected components, dense and in first-triangle order.
    pub shells: Vec<u32>,
    /// The WELDED vertex id of each triangle corner. Two triangles meeting at a welded edge or
    /// vertex name it with the same two numbers however their own index buffers spell it, which is
    /// what lets the collector canonicalize a whole fan onto one contact.
    pub corners: Vec<[u32; 3]>,
}

/// A ballistic primitive's substance, resolved against the registry at extraction so the walk never
/// does a string lookup per query.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PrimitiveSubstance {
    /// The material datablock name — the join key to `materials.ron` (§12: identity, never parsed).
    pub name: String,
    /// The §13.2 field value: reference-mm of armour per metre of chord.
    pub factor: f32,
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
    /// Load-bearing roadwheel stations — `(node index, TrackSide)`, one per entry of the spec's
    /// `roadwheels` list, **in declaration order**. That order is load-bearing: it is the
    /// `WheelIndex` slot order both wire ends derive, so it comes from the authored file (one
    /// source of truth, ordered by construction) rather than from a name pattern plus a sort.
    /// These nodes carry the wheel mesh itself, so they are also ballistic volumes.
    pub roadwheels: Vec<(usize, TrackSide)>,
    /// Collision-proxy nodes — the spec's `colliders` list, in declaration order. No wire-shared
    /// index derives from it (each proxy just yields a convex hull); the order is fixed anyway so
    /// spawn is deterministic.
    pub collision_proxies: Vec<usize>,
    /// Every node holding at least one ballistic primitive — MEMBERSHIP IS THE MATERIAL (§12,
    /// classifier precedent 2026-08-07): a node is here iff one of its glTF primitives wears a
    /// registry substance. Name-sorted, so spawn order is deterministic without depending on the
    /// glTF node order. Nothing in the bind path scans names for this any more.
    pub ballistic_volumes: Vec<usize>,
}

impl TankGeometry {
    /// Whether a node name is a ballistic volume — the material verdict, published for the VIEW
    /// layer (which must hide the armour meshes it now cannot recognise by name). Consult this,
    /// never a suffix.
    pub fn is_ballistic(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|index| self.nodes[*index].is_ballistic())
    }

    /// Whether a node exists for the SIM only and must never render — the rule the view binder
    /// hides by, and the last thing the `*_Ballistic`/`*_Collider` suffixes used to answer.
    ///
    /// Collision proxies always. Ballistic volumes *unless they are also a roadwheel station*: the
    /// wheels are the one place where the visual and the ballistic mesh are the SAME object (the
    /// mesh-unification precedent), so hiding every volume would delete the running gear from the
    /// picture. The .blend states this with collections — `Ballistic` vs `RunningGear` — which glTF
    /// does not carry, so the roadwheel declaration is what carries it across: a station is by
    /// definition a rendered part of the vehicle. A future unified part joins that list explicitly
    /// rather than being guessed at from a material or a polygon count.
    pub fn is_physics_only(&self, name: &str) -> bool {
        let Some(&index) = self.by_name.get(name) else {
            return false;
        };
        if self.collision_proxies.contains(&index) {
            return true;
        }
        self.nodes[index].is_ballistic() && !self.roadwheels.iter().any(|&(w, _)| w == index)
    }
}

impl NodeGeometry {
    /// Whether this node holds any ballistic primitive.
    pub fn is_ballistic(&self) -> bool {
        self.primitives.iter().any(|p| p.substance.is_some())
    }

    /// This node's ballistic primitives grouped by substance, in name order — one group per
    /// substance region (§13.7). One group is the ordinary case; more than one is the authored
    /// mixed-substance part (the road wheels).
    pub fn substance_groups(&self) -> BTreeMap<&str, Vec<&MeshGeometry>> {
        let mut groups: BTreeMap<&str, Vec<&MeshGeometry>> = BTreeMap::new();
        for primitive in &self.primitives {
            if let Some(substance) = &primitive.substance {
                groups
                    .entry(substance.name.as_str())
                    .or_default()
                    .push(primitive);
            }
        }
        groups
    }
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

/// Which primitives' mesh buffers the sim consumes, and why (§12, classifier precedent 2026-08-07):
///
/// * **Ballistic volumes — MEMBERSHIP IS THE MATERIAL.** A primitive marches iff its glTF material
///   name is a key in the substance registry. Nothing is parsed out of a node name; the old
///   `*_Ballistic` suffix is retired and stripped from the source .blend, and with it the last way
///   a renamed volume could silently dodge capture. Wearing a registry material IS the declaration,
///   which is also why decor must never wear one.
/// * **Collision proxies — DECLARED.** The `colliders` list in `<tank>.tank.ron` names them; the
///   `*_Collider` suffix scan is retired with the rest. Every primitive of a declared proxy is
///   captured (a proxy is a hull source, not a substance).
///
/// Roadwheels are the one node class that is BOTH a rig station and a volume: the wheels ship as one
/// watertight mesh per station, so the same node is the suspension station AND its own armour. The
/// station role is a marker component (`Roadwheel`) and the volume role a bundle + trimesh children,
/// and they compose on one entity (`tank::spawn::assemble_tank_body`). Their capture needs no rule
/// of its own — the wheel meshes wear `MildSteel`/`Rubber`, so the material rule already takes them.
///
/// A node that mixes substance and non-substance primitives is REFUSED (see
/// [`extract_tank_geometry`]): half-armour has no honest meaning here, and the shadow compare's
/// whole-node mesh diff would quietly stop covering the uncaptured half.
fn classify(
    primitive: &gltf::Primitive,
    registry: &SubstanceRegistry,
) -> Option<PrimitiveSubstance> {
    // An unnamed material cannot be a registry key, so it cannot declare membership. (The coax MG's
    // physics-only meshes were exactly this case before the restructure.)
    let material = primitive.material().name()?;
    // Not a substance ⇒ ordinary art. The registry's `Unknown` arm IS the classifier; the error case
    // is a name that only LOOKS like a substance, which the bind test's near-miss lint catches (a
    // `.001` duplicate means the material-library link drifted from LINKED to appended).
    let substance = registry.get(material).ok()?;
    Some(PrimitiveSubstance {
        name: material.to_owned(),
        factor: substance.factor,
    })
}

/// The dynamic range a ballistic vertex coordinate is CERTIFIED in, as `(low, high)` magnitudes in
/// the frame the corridor kernel reads. That frame is the stored buffer itself: a ballistic node is
/// unit-scale ([`manifold_gate`]), so what parry multiplies into the vertices is `1`.
///
/// `collect::PROJECTION_SLACK` is a bound on the f32 projection's rounding, and it is a bound only
/// where the relative error model it is derived from holds. It is stated as a multiple of the
/// vertex's own magnitude, so on a vertex whose corridor-relative offset is subnormal it underflows
/// to zero and claims an exactness the arithmetic does not have — while a NEIGHBOURING vertex
/// megametres away multiplies that unclaimed error up through the edge area. Codex built exactly
/// that triangle and the kernel declined a crossing an exact reference accepts.
///
/// A tolerance cannot fix it, and neither can wider arithmetic in the hot path (§13 pays for the f64
/// reconsideration only inside the band; widening the band is the cost, not the cure). Per ADR-0034
/// the answer is generic and lives at the door: geometry outside the certified range does not enter
/// the sim, and the kernel's proof is written as a claim ABOUT that range.
///
/// * `2^16 m` (65 536 m) is the CEILING, and it is the half the proof needs: it is what keeps the
///   amplification of an unclaimed near-zero error by a neighbouring vertex inside
///   `collect::edge_area_slack`'s `f32::MIN_POSITIVE` term, with a factor of four to spare. The
///   derivation is written out at `collect::PROJECTION_SLACK`.
/// * `2^-64 m` (54 zeptometres) is the FLOOR, and it is coordinate hygiene rather than a term in
///   that derivation: it refuses geometry whose coordinates carry no relative accuracy at all — the
///   class the counterexample was drawn from, where the f32 rigid transform onto the collider has
///   already destroyed whatever the numbers meant. Exactly zero is legal and exact; it is the values
///   that are merely NEAR zero that mean nothing.
pub(crate) const CERTIFIED_RANGE: (f32, f32) = (
    // 2^-64 and 2^16, written as bit patterns so the constant cannot drift by a decimal digit.
    f32::from_bits(0x1F80_0000),
    f32::from_bits(0x4780_0000),
);

/// Whether one coordinate is inside [`CERTIFIED_RANGE`].
pub(crate) fn certified_coordinate(value: f32) -> bool {
    let magnitude = value.abs();
    magnitude == 0.0 || (magnitude >= CERTIFIED_RANGE.0 && magnitude <= CERTIFIED_RANGE.1)
}

/// The §13.6 per-primitive manifold gate: weld by position, then closed-manifold + consistent
/// outward orientation per connected shell.
///
/// Runs on every substance primitive, at extraction, before anything downstream can consume it. A
/// non-watertight ballistic mesh is SILENT ZERO ARMOUR — the walk pairs entry/exit per closed
/// surface, and a hole means the exit is never found — which is the exact defect class §13 exists to
/// kill, so a failure names the node, the primitive and the defect and refuses the whole extraction.
///
/// **Weld first.** glTF splits a vertex wherever the normal or UV differs, so every flat-shaded
/// plate arrives with each face's corners duplicated: a naive index-level edge-parity test
/// false-positives on literally every seam. Welding by exact position first is what makes the test
/// measure the surface instead of the export's vertex-splitting.
///
/// Per connected shell, two conditions:
/// * every undirected edge is shared by exactly two triangles, and each DIRECTED edge appears
///   exactly once — closed, and consistently wound (an inconsistent winding shows up as a directed
///   edge traversed twice the same way);
/// * the signed volume `Σ v₀·(v₁×v₂)/6` is positive — the winding is outward, not inward. An
///   inward-wound shell is closed but every normal points the wrong way, and the walk would read
///   its entry faces as exits.
///
/// Multiple closed shells in one primitive are legal and expected (§13.7: one shell per substance
/// region, and a substance region may be several islands — the wheel's two steel bodies plus axle).
/// Which is why the roots are the RETURN VALUE and not a local: they are the surface identity the
/// §13.4 walk pairs crossings on, dense and in first-triangle order, one per triangle.
///
/// UNIT SCALE IS THE CONTRACT, and `scale` — the composed authored scale
/// ([`NodeGeometry::root_scale`]) — is where it is enforced. A ballistic node reaches the world at
/// scale `1` or it does not reach it at all: everything below is judged on the stored buffer, and a
/// componentwise scale is injective over the reals but not over `f32`, so scaling can weld two
/// distinct coordinates into one (a positive-thickness shell becomes zero-thickness) or carry a
/// certified coordinate out of [`CERTIFIED_RANGE`]. Nothing here compensates for a scale; it names
/// the node and refuses. Apply the scale in the .blend and re-export.
fn manifold_gate(
    node: &str,
    index: usize,
    primitive: &MeshGeometry,
    scale: Vec3,
) -> Result<ShellCertificate, String> {
    let defect = |what: String| format!("node `{node}` primitive {index}: {what}");

    if primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
        return Err(defect(format!(
            "{} indices is not a non-empty multiple of 3 — a substance primitive must be an \
             indexed triangle soup",
            primitive.indices.len()
        )));
    }
    // Bit-exact, not near: there is no scale that is almost 1.
    if scale.to_array().map(f32::to_bits) != [1.0f32.to_bits(); 3] {
        return Err(defect(format!(
            "composed authored scale is {scale:?}, not 1 — a ballistic node must be authored at \
             unit scale (apply the object scale in the .blend and re-export)"
        )));
    }

    // Weld by EXACT position. `-0.0` is canonicalized so a coordinate that is zero has one bit
    // pattern (otherwise two vertices at the same point can fail to weld and open a phantom seam);
    // a coordinate outside [`CERTIFIED_RANGE`] cannot be welded, integrated, or projected by a
    // watertight kernel that has a bound to offer.
    let mut canonical: HashMap<[u32; 3], u32> = HashMap::new();
    let mut welded: Vec<u32> = Vec::with_capacity(primitive.positions.len());
    for position in &primitive.positions {
        let mut key = [0u32; 3];
        for (slot, value) in key.iter_mut().zip(position) {
            if !certified_coordinate(*value) {
                return Err(defect(format!(
                    "vertex coordinate {position:?} is outside the certified range — a ballistic \
                     coordinate must reach the corridor kernel as 0 or {low:e}..={high:e} m in \
                     magnitude, which is the domain `collect`'s watertight projection carries a \
                     proven error bound over",
                    low = CERTIFIED_RANGE.0,
                    high = CERTIFIED_RANGE.1,
                )));
            }
            *slot = if *value == 0.0 { 0.0f32 } else { *value }.to_bits();
        }
        let next = canonical.len() as u32;
        welded.push(*canonical.entry(key).or_insert(next));
    }

    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(primitive.indices.len() / 3);
    for (triangle, corners) in primitive.indices.chunks_exact(3).enumerate() {
        let mut welded_corners = [0u32; 3];
        for (slot, &corner) in welded_corners.iter_mut().zip(corners) {
            *slot = *welded.get(corner as usize).ok_or_else(|| {
                defect(format!(
                    "triangle {triangle} indexes vertex {corner}, out of range"
                ))
            })?;
        }
        let [a, b, c] = welded_corners;
        if a == b || b == c || a == c {
            return Err(defect(format!(
                "triangle {triangle} is degenerate after welding (corners {welded_corners:?}) — a \
                 zero-area face has no orientation and breaks edge parity"
            )));
        }
        triangles.push(welded_corners);
    }

    // Directed-edge census: closure AND winding consistency. A `BTreeMap` and two ORDERED passes,
    // so the diagnostic is the same sentence every run — a defect that reports differently each time
    // is one nobody can grep for. Winding is checked FIRST, because a duplicated or same-wound face
    // also looks like a closure failure, and naming it that way sends the fix the wrong way.
    let mut directed: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for &[a, b, c] in &triangles {
        for edge in [(a, b), (b, c), (c, a)] {
            *directed.entry(edge).or_insert(0) += 1;
        }
    }
    if let Some((&(a, b), &count)) = directed.iter().find(|&(_, &count)| count != 1) {
        return Err(defect(format!(
            "directed edge {a}→{b} is traversed {count} times — two triangles wind the same face \
             the same way (inconsistent orientation, or a duplicated face)"
        )));
    }
    if let Some((&(a, b), _)) = directed
        .iter()
        .find(|&(&(a, b), _)| directed.get(&(b, a)).copied().unwrap_or(0) != 1)
    {
        let opposite = directed.get(&(b, a)).copied().unwrap_or(0);
        return Err(defect(format!(
            "edge {a}—{b} is shared by {} triangle(s), not 2 — the shell is not closed (a hole, a \
             boundary edge, or a non-manifold fin)",
            1 + opposite
        )));
    }

    // Connected shells, by shared welded edge. Union-find over triangles.
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    let mut owner: HashMap<(u32, u32), usize> = HashMap::new();
    for (triangle, &[a, b, c]) in triangles.iter().enumerate() {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let edge = if u < v { (u, v) } else { (v, u) };
            match owner.get(&edge) {
                Some(&other) => {
                    let (x, y) = (find(&mut parent, triangle), find(&mut parent, other));
                    parent[x] = y;
                }
                None => {
                    owner.insert(edge, triangle);
                }
            }
        }
    }

    // Signed volume per shell. f64 because the divergence-theorem sum of a thin plate far from the
    // origin cancels hard.
    let mut volumes: BTreeMap<usize, f64> = BTreeMap::new();
    for triangle in 0..triangles.len() {
        let shell = find(&mut parent, triangle);
        let corner = |slot: usize| -> [f64; 3] {
            let position = primitive.positions[primitive.indices[triangle * 3 + slot] as usize];
            [position[0] as f64, position[1] as f64, position[2] as f64]
        };
        let (a, b, c) = (corner(0), corner(1), corner(2));
        let cross = [
            b[1] * c[2] - b[2] * c[1],
            b[2] * c[0] - b[0] * c[2],
            b[0] * c[1] - b[1] * c[0],
        ];
        *volumes.entry(shell).or_insert(0.0) +=
            (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0;
    }
    for (shell, volume) in &volumes {
        if !volume.is_finite() || *volume <= 0.0 {
            return Err(defect(format!(
                "shell rooted at triangle {shell} has signed volume {volume} — a closed shell must \
                 enclose positive volume with outward winding (a negative one is inside-out; zero \
                 is a degenerate/flat shell)"
            )));
        }
    }

    // Publish the roots as DENSE ids in first-triangle order. The root is a union-find artefact —
    // it moves when path compression moves — and the id is the walk's surface name, so it is
    // derived from the triangle order the mesh actually ships with and nothing else.
    let mut ids: HashMap<usize, u32> = HashMap::new();
    let mut shells: Vec<u32> = Vec::with_capacity(triangles.len());
    for triangle in 0..triangles.len() {
        let root = find(&mut parent, triangle);
        let next = ids.len() as u32;
        shells.push(*ids.entry(root).or_insert(next));
    }
    Ok(ShellCertificate {
        shells,
        corners: triangles,
    })
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
    // The spec is parsed BEFORE the glb because extraction now consumes it: `colliders` and
    // `roadwheels` are explicit declarations there, not name patterns here (§12 identity rule).
    let spec: TankSpec = ron::de::from_str(TIGER_SPEC_RON)
        .unwrap_or_else(|err| panic!("bake: embedded Tiger spec failed to parse: {err}"));
    spec.validate()
        .unwrap_or_else(|err| panic!("bake: embedded Tiger spec failed validation: {err}"));
    let geometry = extract_tank_geometry(&path, &spec).unwrap_or_else(|err| {
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
        "bake: extracted tank geometry — {} nodes, {} mesh-captured, {} ballistic volumes",
        geometry.nodes.len(),
        mesh_nodes,
        geometry.ballistic_volumes.len(),
    );

    commands.insert_resource(TankBlueprint {
        geometry: Arc::new(geometry),
        spec: Arc::new(spec),
    });
}

/// Parse the glb as data into [`TankGeometry`]. Pure with respect to the app: `gltf` crate only,
/// usable identically from the runtime (step 0/phase 1) and the offline compiler (phase 2).
pub(crate) fn extract_tank_geometry(path: &Path, spec: &TankSpec) -> Result<TankGeometry, String> {
    let gltf::Gltf { document, mut blob } =
        gltf::Gltf::open(path).map_err(|e| format!("open: {e}"))?;
    // The one substance vocabulary, shared with `materials.blend`. A malformed registry panics
    // inside `shipped()` — a broken authored contract is a build defect, not a runtime condition.
    let registry = SubstanceRegistry::shipped();
    let declared_colliders: HashSet<&str> = spec.colliders.iter().map(String::as_str).collect();

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
        root_scale: Vec3::ONE,
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
        // Avian's own composition, verbatim (`collider_transform/plugin.rs:133`).
        let root_scale = nodes[parent].root_scale * transform.scale;

        let mut primitives = Vec::new();
        if let Some(mesh) = node.mesh() {
            let is_collider = declared_colliders.contains(name.as_str());
            let mut captured: Vec<(usize, Option<PrimitiveSubstance>)> = Vec::new();
            let mut ballistic = 0usize;
            let mut plain = 0usize;
            for primitive in mesh.primitives() {
                match classify(&primitive, &registry) {
                    Some(substance) => {
                        ballistic += 1;
                        captured.push((primitive.index(), Some(substance)));
                    }
                    None => {
                        plain += 1;
                        if is_collider {
                            captured.push((primitive.index(), None));
                        }
                    }
                }
            }
            // Half-armour has no honest meaning: the shadow compare diffs a node's mesh bytes as a
            // whole, so a partially-captured node would quietly stop being covered, and the walk
            // would meet a solid whose other half it cannot see.
            if ballistic > 0 && plain > 0 {
                return Err(format!(
                    "node `{name}` mixes {ballistic} substance primitive(s) with {plain} \
                     non-substance one(s) — a mesh is either a ballistic solid or it is art. \
                     Membership is the material (§12): split the object, or give every primitive a \
                     registry substance"
                ));
            }
            if is_collider && ballistic > 0 {
                return Err(format!(
                    "node `{name}` is declared a collision proxy but wears a registry substance — a \
                     convex hull that is ALSO armour would be charged twice by the march"
                ));
            }
            for (index, substance) in captured {
                let primitive = mesh
                    .primitives()
                    .nth(index)
                    .ok_or_else(|| format!("node `{name}`: primitive {index} vanished"))?;
                let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let positions: Vec<[f32; 3]> = reader
                    .read_positions()
                    .ok_or_else(|| format!("node `{name}`: primitive has no POSITION"))?
                    .collect();
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_default();
                let mut geometry = MeshGeometry {
                    positions,
                    indices,
                    substance,
                    certificate: None,
                };
                if geometry.substance.is_some() {
                    geometry.certificate =
                        Some(manifold_gate(&name, index, &geometry, root_scale)?);
                }
                primitives.push(geometry);
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
            root_scale,
            primitives,
        });
        for child in node.children() {
            stack.push((child, index));
        }
    }

    // The sim's typed lists, resolved once here (design §8 step 3): two from the spec's explicit
    // declarations, one from the material verdict. Nothing scans a node name for sim meaning.
    let resolve = |name: &str, what: &str| -> Result<usize, String> {
        by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("spec declares {what} `{name}`, which is absent from the model"))
    };
    let mut collision_proxies: Vec<usize> = Vec::new();
    for name in &spec.colliders {
        collision_proxies.push(resolve(name, "collision proxy")?);
    }
    let mut roadwheels: Vec<(usize, TrackSide)> = Vec::new();
    for wheel in &spec.roadwheels {
        roadwheels.push((resolve(&wheel.node, "roadwheel")?, wheel.side));
    }
    // Name-sorted so spawn order never depends on the glTF node order.
    let mut ballistic_volumes: Vec<usize> = nodes
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, node)| node.is_ballistic())
        .map(|(index, _)| index)
        .collect();
    ballistic_volumes.sort_by(|a, b| nodes[*a].name.cmp(&nodes[*b].name));

    Ok(TankGeometry {
        nodes,
        by_name,
        roadwheels,
        collision_proxies,
        ballistic_volumes,
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
        // buffers, compared as order-insensitive multisets of exact bits. Extraction refuses a node
        // that mixes substance and non-substance primitives, so "captured anything" means "captured
        // the whole node" and the multiset compare stays total.
        if !node.primitives.is_empty() {
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

    fn tiger_spec() -> TankSpec {
        ron::de::from_str(include_str!("../assets/tiger_1/tiger_1.tank.ron"))
            .expect("tiger_1.tank.ron must parse")
    }

    fn tiger_glb() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(crate::tank::TIGER_GLB_PATH)
    }

    fn tiger_geometry(spec: &TankSpec) -> TankGeometry {
        extract_tank_geometry(&tiger_glb(), spec).expect("tiger_1.glb must extract")
    }

    /// The extractor's golden test: extract the Tiger and hold it to the same contract the binder
    /// enforces at runtime — every spec-declared node present, the structural singletons, the
    /// declared proxies and stations, and sim-consumed mesh data captured with the buffers avian
    /// requires (indices are mandatory for BOTH collider paths: avian's
    /// `extract_mesh_vertices_indices` bails on unindexed meshes even for the hull).
    #[test]
    fn tiger_1_extracts_to_contract() {
        let spec = tiger_spec();
        let geometry = tiger_geometry(&spec);

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
        for view in spec.views.values() {
            node(&view.node);
        }
        node("Hull");
        node("Center_Of_Mass");

        // Every ballistic volume the MATERIAL rule found carries usable geometry: a substance on
        // every primitive, a positive factor, and an indexed mesh. (Watertightness is not asserted
        // here — extraction REFUSES a non-manifold substance primitive outright, so reaching this
        // line already proves every shell closed and outward-wound.)
        assert!(
            !geometry.ballistic_volumes.is_empty(),
            "the material rule found no ballistic volume at all"
        );
        for &index in &geometry.ballistic_volumes {
            let volume = &geometry.nodes[index];
            let name = &volume.name;
            assert!(
                !volume.primitives.is_empty(),
                "volume `{name}` captured no mesh data"
            );
            for primitive in &volume.primitives {
                let substance = primitive.substance.as_ref().unwrap_or_else(|| {
                    panic!("volume `{name}` holds a primitive with no substance")
                });
                assert!(
                    substance.factor > 0.0,
                    "volume `{name}` is `{}`, whose factor is not positive",
                    substance.name
                );
                assert!(
                    primitive.positions.len() >= 3,
                    "volume `{name}`: degenerate"
                );
                assert!(!primitive.indices.is_empty(), "volume `{name}`: unindexed");
            }
        }
        // Name-sorted: the deterministic spawn order.
        let volume_names: Vec<&str> = geometry
            .ballistic_volumes
            .iter()
            .map(|&index| geometry.nodes[index].name.as_str())
            .collect();
        let mut sorted = volume_names.clone();
        sorted.sort_unstable();
        assert_eq!(
            volume_names, sorted,
            "volumes must be extracted name-sorted"
        );

        // Every declared component names a ballistic volume — a facet on a node the march never
        // meets is inert, and the binder refuses it, so the shipped pair must agree here first.
        for component in spec.volumes.keys() {
            assert!(
                geometry.is_ballistic(component),
                "component `{component}` is not a ballistic volume (no primitive of its node wears \
                 a registry substance)"
            );
        }

        // Wheels: 8 per side on the Tiger (snapshot; SIM-EVIDENCE's 16/16), via the extractor's
        // typed list, in the DECLARED order — the load-bearing `WheelIndex` slot order both wire
        // ends derive.
        let per_side = |want| {
            geometry
                .roadwheels
                .iter()
                .filter(|&&(_, side)| side == want)
                .count()
        };
        assert_eq!(per_side(crate::tank::TrackSide::Left), 8);
        assert_eq!(per_side(crate::tank::TrackSide::Right), 8);
        let wheel_names: Vec<&str> = geometry
            .roadwheels
            .iter()
            .map(|&(index, _)| geometry.nodes[index].name.as_str())
            .collect();
        assert_eq!(
            wheel_names,
            spec.roadwheels
                .iter()
                .map(|wheel| wheel.node.as_str())
                .collect::<Vec<_>>()
        );
        // Station AND armour in one node: the wheels ship as one object per station, so every
        // station must also have been classified a volume by its materials. Re-split the asset, or
        // strip a wheel's substance, and this fails at CI time instead of leaving the wheels
        // silently invisible to the penetration march.
        for &name in &wheel_names {
            assert!(
                geometry.is_ballistic(name),
                "roadwheel `{name}` carries the wheel mesh but wears no substance"
            );
        }

        // Collision proxies: the declared list, captured, indexed, and NOT armour.
        assert_eq!(geometry.collision_proxies.len(), spec.colliders.len());
        for &index in &geometry.collision_proxies {
            let collider = &geometry.nodes[index];
            assert!(!collider.primitives.is_empty());
            assert!(
                !collider.is_ballistic(),
                "`{}` is a collision proxy AND armour",
                collider.name
            );
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

    /// The SUBSTANCE CENSUS — the pin that makes "decor must never wear a substance material" a
    /// mechanical fact rather than an eyeball rule.
    ///
    /// Membership is the material now, so a decor object that acquires a registry material silently
    /// becomes armour: it would march, it would be charged, and nothing else in the pipeline would
    /// object. Nothing can detect that from the material alone (wearing one IS the declaration), so
    /// the guard is this snapshot: the exact number of volumes, and the exact number of PRIMITIVES
    /// per substance. A deliberate model change updates these numbers in the same commit; an
    /// accidental one fails CI naming the substance whose count moved.
    #[test]
    fn tiger_1_substance_census_is_pinned() {
        let geometry = tiger_geometry(&tiger_spec());
        let mut census: BTreeMap<&str, usize> = BTreeMap::new();
        for node in &geometry.nodes {
            for primitive in &node.primitives {
                if let Some(substance) = &primitive.substance {
                    *census.entry(substance.name.as_str()).or_default() += 1;
                }
            }
        }
        assert_eq!(
            census,
            BTreeMap::from([
                ("Ammunition", 4),
                // 4, was 5: `Turret_Cupola` is GONE — the commander's cupola was rebuilt
                // 2026-08-08 as `Commander_Cupola` + `Commander_Hatch`, and both are RHA below.
                // The substance moved with the modelling: the old single object was cast
                // (`Panzerguß`) because it was one lump; the rebuilt cupola is a rolled-plate ring
                // with a separate hatch, which is what the real Tiger's later cupola is.
                ("Cast", 4),
                ("EngineBlock", 2),
                ("Flesh", 5),
                ("GunSteel", 6),
                // 26, was 22: the running gear joined the ballistics 2026-08-07 — `Sprocket_L/R`
                // and `Idler_L/R`, one primitive each, `Mat_RunningGear_Paint` -> `MildSteel`.
                // Machinery `Stahlguß`, not `Panzerguß`, so MildSteel and not Cast.
                ("MildSteel", 26),
                // 19, was 17: `Commander_Cupola` (832 tris) and `Commander_Hatch` (190), the two
                // halves of the rebuilt cupola, both rolled plate. Net armour primitives +1, not
                // +2, because `Turret_Cupola` left `Cast` in the same edit.
                ("RHA", 19),
                ("Rubber", 16),
            ]),
            "the Tiger's substance census moved — see the doc above before re-pinning"
        );
        // 66, was 65: one volume out (`Turret_Cupola`), two in (`Commander_Cupola`,
        // `Commander_Hatch`). Total primitives 89, was 88, by the same arithmetic — this IS new
        // geometry, unlike the 2026-08-07 running-gear change, which was a re-classification.
        assert_eq!(geometry.ballistic_volumes.len(), 66);

        // Closed shells, the grain the walk pairs on. Pinned alongside the primitives because a
        // remodel that splits or merges an island changes what the walk can tell apart, and
        // nothing else in the pipeline would notice.
        let shells: usize = geometry
            .nodes
            .iter()
            .flat_map(|node| &node.primitives)
            .map(|primitive| {
                primitive.certificate.as_ref().map_or(0, |certificate| {
                    certificate
                        .shells
                        .iter()
                        .copied()
                        .max()
                        .map_or(0, |max| max as usize + 1)
                })
            })
            .sum();
        // 131 shells over 89 primitives: multi-shell primitives are the ordinary case here, not a
        // wheel-only curiosity.
        assert_eq!(shells, 131, "the Tiger's shell census moved");
    }

    /// The gate at scale 1 — the case every synthetic fixture below is about.
    fn gate(node: &str, primitive: &MeshGeometry) -> Result<Vec<u32>, String> {
        manifold_gate(node, 0, primitive, Vec3::ONE).map(|certificate| certificate.shells)
    }

    /// SAME-PRIMITIVE FACE-TO-FACE CONTACT IS REFUSED, AND THAT IS THE CONTRACT (§13.7).
    ///
    /// Two closed shells of one primitive that share a welded edge present four triangles on it, so
    /// the directed-edge census rejects them before union-find can even be asked whether they are
    /// one shell or two. That refusal is deliberate and it is the answer: a geometric guess at where
    /// one shell ends and the next begins is exactly the ambiguity surface identity exists to
    /// remove, and the walk's pairing, restart seeding and contact ownership all depend on the
    /// distinction. Plates authored face to face stay separate PRIMITIVES; only vertex contact and
    /// interior intersection are legal inside one.
    #[test]
    fn same_primitive_face_to_face_contact_is_refused() {
        // Two unit cubes sharing the whole `x = 1` face, in one primitive.
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for offset in [0.0f32, 1.0] {
            let base = positions.len() as u32;
            for (x, y, z) in [
                (0.0f32, 0.0f32, 0.0f32),
                (1.0, 0.0, 0.0),
                (1.0, 1.0, 0.0),
                (0.0, 1.0, 0.0),
                (0.0, 0.0, 1.0),
                (1.0, 0.0, 1.0),
                (1.0, 1.0, 1.0),
                (0.0, 1.0, 1.0),
            ] {
                positions.push([x + offset, y, z]);
            }
            for face in [
                [0u32, 3, 2],
                [0, 2, 1],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [3, 7, 6],
                [3, 6, 2],
                [0, 4, 7],
                [0, 7, 3],
                [1, 2, 6],
                [1, 6, 5],
            ] {
                indices.extend(face.map(|corner| base + corner));
            }
        }
        let abutting = MeshGeometry {
            positions,
            indices,
            substance: Some(PrimitiveSubstance {
                name: "RHA".into(),
                factor: 1000.0,
            }),
            certificate: None,
        };
        let err = gate("Abutting", &abutting)
            .expect_err("face-to-face contact inside one primitive must be refused");
        assert!(err.contains("directed edge"), "{err}");
    }

    /// The manifold gate refuses what silently-zero armour is made of, and names it. Driven on
    /// synthetic primitives, because the shipped asset must (and does) pass — the gate's value is
    /// entirely in what it rejects.
    #[test]
    fn the_manifold_gate_refuses_open_inverted_and_degenerate_shells() {
        // A unit tetrahedron, outward-wound: the smallest closed positive-volume shell.
        let tetra = |indices: Vec<u32>| MeshGeometry {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices,
            substance: Some(PrimitiveSubstance {
                name: "RHA".into(),
                factor: 1000.0,
            }),
            certificate: None,
        };
        let closed = vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
        let shells = gate("Good", &tetra(closed.clone())).expect("a closed tetrahedron passes");
        assert_eq!(shells, vec![0; 4], "one tetrahedron is one shell");

        // One face removed: an open shell. This is the defect that makes armour worth ZERO — the
        // walk never finds the exit face and the volume is never charged.
        let mut open = closed.clone();
        open.truncate(9);
        let err = gate("Open", &tetra(open)).expect_err("an open shell must be refused");
        assert!(err.contains("Open") && err.contains("not closed"), "{err}");

        // Every face flipped: closed, but wound inward — the walk would read entries as exits.
        let inverted: Vec<u32> = closed
            .chunks_exact(3)
            .flat_map(|t| [t[0], t[2], t[1]])
            .collect();
        let err =
            gate("Inverted", &tetra(inverted)).expect_err("an inside-out shell must be refused");
        assert!(err.contains("Inverted") && err.contains("volume"), "{err}");

        // A duplicated face: closed by undirected count, but one directed edge is traversed twice.
        let mut doubled = closed.clone();
        doubled.extend_from_slice(&[0, 2, 1]);
        let err = gate("Doubled", &tetra(doubled)).expect_err("a duplicated face must be refused");
        assert!(
            err.contains("Doubled") && err.contains("directed edge"),
            "{err}"
        );

        // A degenerate (zero-area) triangle has no orientation to check.
        let mut degenerate = closed.clone();
        degenerate.extend_from_slice(&[0, 1, 1]);
        let err =
            gate("Degenerate", &tetra(degenerate)).expect_err("a zero-area face must be refused");
        assert!(
            err.contains("Degenerate") && err.contains("degenerate"),
            "{err}"
        );
    }

    /// Weld-by-position is what makes the gate measure the SURFACE instead of the export's vertex
    /// splitting: glTF duplicates a vertex wherever the normal or UV differs, so a flat-shaded
    /// plate arrives with every corner split and a naive index-level edge test false-positives on
    /// every seam of every volume in the model.
    #[test]
    fn the_gate_welds_before_it_measures() {
        // The same tetrahedron with EVERY corner split per face — 12 vertices, 4 faces, no shared
        // index anywhere. Index-level, it is four disconnected triangles; welded, it is closed.
        let corners = [
            [0.0f32, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let faces = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for face in faces {
            for corner in face {
                indices.push(positions.len() as u32);
                positions.push(corners[corner]);
            }
        }
        let split = MeshGeometry {
            positions,
            indices,
            substance: Some(PrimitiveSubstance {
                name: "RHA".into(),
                factor: 1000.0,
            }),
            certificate: None,
        };
        gate("Split", &split).expect("welding by position closes the split shell");
    }

    /// TWO SHELLS IN ONE PRIMITIVE ARE TWO KEYS, AND ONE SHARED VERTEX DOES NOT MERGE THEM.
    ///
    /// This is what the §13.4 walk pairs on. Edge-connected components are the right grain and
    /// vertex-connected ones are not: two legal shells may legally touch at a point (§13.7's
    /// islands), and merging them there would put one key on two surfaces — exactly the ambiguity
    /// the identity exists to remove.
    #[test]
    fn two_shells_in_one_primitive_are_two_keys() {
        // Two tetrahedra sharing the single vertex at the origin, one either side of it.
        let mut positions = vec![[0.0f32, 0.0, 0.0]];
        let mut indices = Vec::new();
        for sign in [1.0f32, -1.0] {
            let base = positions.len() as u32;
            positions.extend([[sign, 0.0, 0.0], [0.0, sign, 0.0], [0.0, 0.0, sign]]);
            // `0` is the shared apex; the outward winding flips with the octant.
            let faces: [[u32; 3]; 4] = if sign > 0.0 {
                [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]]
            } else {
                [[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]]
            };
            for face in faces {
                indices.extend(face.map(|corner| if corner == 0 { 0 } else { base + corner - 1 }));
            }
        }
        let pair = MeshGeometry {
            positions,
            indices,
            substance: Some(PrimitiveSubstance {
                name: "RHA".into(),
                factor: 1000.0,
            }),
            certificate: None,
        };
        let shells = gate("Pair", &pair).expect("two closed outward tetrahedra pass");
        assert_eq!(
            shells,
            vec![0, 0, 0, 0, 1, 1, 1, 1],
            "one key per edge-connected shell, dense and in triangle order",
        );
    }

    /// A BALLISTIC NODE IS UNIT-SCALE OR IT IS REFUSED, BY NAME.
    ///
    /// The same geometry passes at `1` and fails at every other scale, including one a millionth of
    /// an ULP away — nothing here compensates, rescales, or tolerates. The message names the node,
    /// because the fix is in the .blend and the artist has to be told which object.
    #[test]
    fn a_ballistic_node_that_is_not_unit_scale_is_refused_by_name() {
        let tetra = MeshGeometry {
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3],
            substance: Some(PrimitiveSubstance {
                name: "RHA".into(),
                factor: 1000.0,
            }),
            certificate: None,
        };
        gate("Unscaled", &tetra).expect("at scale 1 it is ordinary geometry");

        // A uniform shrink, a single stretched axis, a mirror, a collapse, and one ULP: all the
        // same verdict, because the contract is bit-exact equality with 1 and nothing weaker.
        for scale in [
            Vec3::splat(0.9312),
            Vec3::new(1.0, 2.0, 1.0),
            Vec3::new(1.0, 1.0, -1.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, f32::from_bits(1.0f32.to_bits() + 1)),
        ] {
            let Err(err) = manifold_gate("Turret_Bottom", 0, &tetra, scale) else {
                panic!("a ballistic node at {scale:?} must be refused");
            };
            assert!(
                err.contains("Turret_Bottom") && err.contains("unit scale"),
                "{err}"
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
        let geometry = tiger_geometry(&tiger_spec());

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
