//! Tank-geometry extraction and view-shadow verification.
//!
//! Invariant (ADR-0014): simulation construction uses synchronously extracted data, never a loaded
//! scene. The shadow verifier compares that data with instantiated view geometry.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use avian3d::prelude::Collider;
use bevy::mesh::VertexAttributeValues;
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;

use crate::spec::{NodeReference, NodeRole, TankSpec, TankSpecHandle};
use crate::substances::SubstanceRegistry;
use crate::tank::{SimParts, TrackSide, rig_world_pose};

mod embedding;
#[cfg(test)]
mod fixture;
/// The one finding shape every stage of the asset door emits.
mod report;

use report::sorted;
pub use report::{Check, Finding, Severity, Stage, Subject, has_error, render};

/// The L2 consumer contract, one static row per law. Severity is compiled in beside the id: every
/// row is an error, because every row is a refusal to build the sim from this asset.
static L2_SPEC: Check = Check {
    id: "L2.SPEC",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "the tank RON parses, passes TankSpec::validate, and every typed node reference resolves \
          to a model node",
};
/// Not a law about the model: the condition under which the laws below can be evaluated at all.
static L2_DOCUMENT: Check = Check {
    id: "L2.DOCUMENT",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "the GLB opens, holds a scene, resolves every buffer, and names every node uniquely",
};
static L2_ROLE_COHERENCE: Check = Check {
    id: "L2.ROLE_COHERENCE",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "a mesh is either wholly a substance solid or wholly art; a declared collider is not \
          armour and carries usable mesh data; every declared component and roadwheel is a \
          ballistic node",
};
static L2_PRIMITIVE_FORM: Check = Check {
    id: "L2.PRIMITIVE_FORM",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "a consumed primitive has POSITION data and a non-empty index buffer whose length is a \
          multiple of three, every index in range",
};
static L2_UNIT_SCALE: Check = Check {
    id: "L2.UNIT_SCALE",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "the composed authored scale of a ballistic node, and the authored scale of every node \
          the rigid pose composition traverses, is bit-exactly (1, 1, 1)",
};
static L2_CERTIFIED_RANGE: Check = Check {
    id: "L2.CERTIFIED_RANGE",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "every ballistic coordinate is exactly zero or inside CERTIFIED_RANGE in magnitude",
};
static L2_EXACT_DEGENERACY: Check = Check {
    id: "L2.EXACT_DEGENERACY",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "after the exact-position weld, every triangle has three distinct welded ids and encloses \
          exactly non-zero area",
};
static L2_MANIFOLD_WINDING: Check = Check {
    id: "L2.MANIFOLD_WINDING",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "each directed welded edge occurs exactly once and its reverse exactly once",
};
static L2_POSITIVE_SHELL_VOLUME: Check = Check {
    id: "L2.POSITIVE_SHELL_VOLUME",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "every edge-connected shell has finite, strictly positive signed volume",
};
/// The door's own row, not a law of the asset: the substance registry is DATA, and a lane
/// verifying a REVISION must evaluate that revision's registry rather than whatever the running
/// binary was compiled against. The door hands the file over; a file it cannot hand over is the
/// door failing to feed the contract, which is a mechanical refusal of the same kind as an
/// unpinned exporter — never a finding about the model.
static DOOR_REGISTRY: Check = Check {
    id: "door.registry",
    stage: Stage::Door,
    severity: Severity::Error,
    law: "the substance registry the door supplied with --registry is readable and parses",
};
static L2_SHELL_EMBEDDING: Check = Check {
    id: "L2.SHELL_EMBEDDING",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "two triangles of one shell meet only inside the welded feature they declare",
};

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
    /// Whether a node name is a ballistic volume — the material verdict. Consult this, never a
    /// suffix.
    pub fn is_ballistic(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|index| self.nodes[*index].is_ballistic())
    }

    /// Whether a node is a declared collision proxy — the only nodes that exist for the SIM alone
    /// and must never render (RULED 2026-08-10). Since mesh unification the ballistic plates ARE
    /// the vehicle's rendered body, and the component volumes they enclose render behind a closed
    /// hull, so declaration in the spec is the one thing that separates a proxy from the picture.
    pub fn is_collision_proxy(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|index| self.collision_proxies.contains(index))
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
    // is a name that only LOOKS like a substance, which `L1.SUBSTANCE_IDENTITY` refuses at the
    // source (a `.001` duplicate means the material-library link drifted from LINKED to appended,
    // and only the .blend can tell a linked datablock from a copy of one).
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
/// megametres away multiplies that unclaimed error up through the edge area, and the kernel then
/// declines a crossing an exact reference accepts.
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

/// One check emits one finding per subject: the first offender in the check's own deterministic
/// order, and how many there are. A primitive that fails a law fails it thousands of times, and a
/// report with one row per triangle is a report nobody reads.
fn first_of(offenders: &[String]) -> Option<String> {
    offenders.first().map(|first| match offenders.len() {
        1 => first.clone(),
        count => format!("{first} (and {} more)", count - 1),
    })
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
pub(crate) fn manifold_gate(
    node: &str,
    index: usize,
    primitive: &MeshGeometry,
    scale: Vec3,
) -> Result<ShellCertificate, Vec<Finding>> {
    let at = || Subject::node(node).element(format!("primitive {index}"));
    let mut findings: Vec<Finding> = Vec::new();

    if primitive.indices.is_empty() || !primitive.indices.len().is_multiple_of(3) {
        findings.push(Finding::new(
            &L2_PRIMITIVE_FORM,
            at(),
            format!("{} indices", primitive.indices.len()),
            "export the primitive as an indexed triangle soup — triangulate the mesh and keep the \
             exporter's index buffer",
        ));
    }
    // Bit-exact, not near: there is no scale that is almost 1.
    if scale.to_array().map(f32::to_bits) != [1.0f32.to_bits(); 3] {
        findings.push(Finding::new(
            &L2_UNIT_SCALE,
            at(),
            format!("composed authored scale {scale:?}"),
            "apply the object scale in the .blend (Ctrl+A → Scale, on the node and every parent) \
             and re-export",
        ));
    }
    // A coordinate outside [`CERTIFIED_RANGE`] cannot be welded, integrated, or projected by a
    // watertight kernel that has a bound to offer.
    let uncertified: Vec<String> = primitive
        .positions
        .iter()
        .enumerate()
        .filter(|(_, position)| !position.iter().copied().all(certified_coordinate))
        .map(|(vertex, position)| format!("vertex {vertex} at {position:?}"))
        .collect();
    if let Some(evidence) = first_of(&uncertified) {
        findings.push(Finding::new(
            &L2_CERTIFIED_RANGE,
            at(),
            evidence,
            format!(
                "move the geometry into 0 or {low:e}..={high:e} m in magnitude — the domain \
                 `collect`'s watertight projection carries a proven error bound over",
                low = CERTIFIED_RANGE.0,
                high = CERTIFIED_RANGE.1,
            ),
        ));
    }
    if !findings.is_empty() {
        return Err(sorted(findings));
    }

    // Weld by EXACT position. `-0.0` is canonicalized so a coordinate that is zero has one bit
    // pattern; otherwise two vertices at the same point can fail to weld and open a phantom seam.
    let mut canonical: HashMap<[u32; 3], u32> = HashMap::new();
    let mut welded: Vec<u32> = Vec::with_capacity(primitive.positions.len());
    // The canonical position of each welded id, which is what the embedding certificate measures:
    // one point per id, so two triangles naming a corner the same way are AT the same point by
    // construction rather than by comparison.
    let mut points: Vec<[f32; 3]> = Vec::new();
    for position in &primitive.positions {
        let mut key = [0u32; 3];
        for (slot, value) in key.iter_mut().zip(position) {
            *slot = if *value == 0.0 { 0.0f32 } else { *value }.to_bits();
        }
        let next = canonical.len() as u32;
        let id = *canonical.entry(key).or_insert(next);
        if id as usize == points.len() {
            points.push(key.map(f32::from_bits));
        }
        welded.push(id);
    }

    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(primitive.indices.len() / 3);
    let mut out_of_range: Vec<String> = Vec::new();
    let mut degenerate: Vec<String> = Vec::new();
    for (triangle, corners) in primitive.indices.chunks_exact(3).enumerate() {
        let mut welded_corners = [0u32; 3];
        let mut resolved = true;
        for (slot, &corner) in welded_corners.iter_mut().zip(corners) {
            match welded.get(corner as usize) {
                Some(id) => *slot = *id,
                None => {
                    out_of_range.push(format!(
                        "triangle {triangle} indexes vertex {corner} of {}",
                        welded.len()
                    ));
                    resolved = false;
                }
            }
        }
        if !resolved {
            continue;
        }
        // ZERO AREA, in both its spellings. Repeated welded ids is the cheap one; three DISTINCT
        // ids on one line is the other, and it is exact — the integer plane normal of the welded
        // positions, not a float cross product with a threshold under it. A face with no
        // orientation breaks edge parity and gives the embedding certificate no plane to intersect.
        let [a, b, c] = welded_corners;
        if a == b || b == c || a == c {
            degenerate.push(format!(
                "triangle {triangle} welds to corners {welded_corners:?}"
            ));
        } else if embedding::encloses_zero_area(&welded_corners.map(|id| points[id as usize])) {
            degenerate.push(format!(
                "triangle {triangle} welds to corners {welded_corners:?}, which are collinear — it \
                 encloses exactly zero area"
            ));
        }
        triangles.push(welded_corners);
    }
    if let Some(evidence) = first_of(&out_of_range) {
        findings.push(Finding::new(
            &L2_PRIMITIVE_FORM,
            at(),
            evidence,
            "re-export the primitive — its index buffer addresses vertices its POSITION accessor \
             does not hold",
        ));
    }
    if let Some(evidence) = first_of(&degenerate) {
        findings.push(Finding::new(
            &L2_EXACT_DEGENERACY,
            at(),
            evidence,
            "dissolve the zero-area face in the .blend (Mesh → Clean Up → Degenerate Dissolve at \
             zero distance finds them) and re-export — a face with no orientation breaks edge \
             parity and has no plane for the walk to cross",
        ));
    }
    if !findings.is_empty() {
        return Err(sorted(findings));
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
    let rewound: Vec<String> = directed
        .iter()
        .filter(|&(_, &count)| count != 1)
        .map(|(&(a, b), &count)| format!("directed edge {a}→{b} is traversed {count} times"))
        .collect();
    if let Some(evidence) = first_of(&rewound) {
        findings.push(Finding::new(
            &L2_MANIFOLD_WINDING,
            at(),
            evidence,
            "flip the inconsistently wound face, or delete the duplicated one — two triangles wind \
             the same face the same way",
        ));
    }
    let unpaired: Vec<String> = directed
        .iter()
        .filter(|&(&(a, b), _)| directed.get(&(b, a)).copied().unwrap_or(0) != 1)
        .map(|(&(a, b), &count)| {
            format!(
                "edge {a}—{b} is shared by {} triangle(s), not 2",
                count + directed.get(&(b, a)).copied().unwrap_or(0)
            )
        })
        .collect();
    if let Some(evidence) = first_of(&unpaired) {
        findings.push(Finding::new(
            &L2_MANIFOLD_WINDING,
            at(),
            evidence,
            "close the shell in the .blend — a hole, a boundary edge or a non-manifold fin leaves \
             the walk without the exit face, which is silently zero armour",
        ));
    }
    if !findings.is_empty() {
        return Err(sorted(findings));
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
    let hollow: Vec<String> = volumes
        .iter()
        .filter(|(_, volume)| !volume.is_finite() || **volume <= 0.0)
        .map(|(shell, volume)| format!("the shell rooted at triangle {shell} encloses {volume} m³"))
        .collect();
    if let Some(evidence) = first_of(&hollow) {
        return Err(sorted(vec![Finding::new(
            &L2_POSITIVE_SHELL_VOLUME,
            at(),
            evidence,
            "recompute the shell's outward normals in the .blend — a negative volume is an \
             inside-out shell whose entry faces the walk reads as exits, and a zero one is flat",
        )]));
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

    // EMBEDDING, over the surface. Closure and outward winding give a shell an inside; this is
    // what says a ray that enters it leaves it. Unconditional — no flag and no allowlist, because
    // a shell that passes through itself charges a fraction of the plate it crossed and the walk's
    // one-dimensional evidence for that can cancel before it is read.
    if let Err(evidence) = embedding::certify_embedding(&triangles, &shells, &points) {
        return Err(sorted(vec![Finding::new(
            &L2_SHELL_EMBEDDING,
            at(),
            evidence,
            "separate the two faces in the .blend — a shell that passes through itself charges a \
             fraction of the plate the walk crossed, and the one-dimensional evidence for that can \
             cancel before it is read",
        )]));
    }

    Ok(ShellCertificate {
        shells,
        corners: triangles,
    })
}

pub(crate) fn plugin(app: &mut App) {
    // After the certificate: a trio whose bytes do not match their recorded hashes must refuse
    // before anything is extracted from them.
    app.add_systems(
        Startup,
        extract_at_startup.after(crate::geometry_lod::load_certificate),
    );
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
    // THE SIM ARTIFACT (ADR-0035), not the view glb: a byte-strip of the certified view glb holding
    // LOD0 geometry and material names — membership is the material, so the walk's substance lookup
    // travels with it. The server opens this file and no other; the client opens it too, so both
    // sides walk identical accessor bytes by construction. `geometry_lod` has already fingerprinted
    // it against the certificate by the time this runs.
    let path = root.join(crate::tank::TIGER_SIM_GLB_PATH);
    let asset = certify_asset(&path, TIGER_SPEC_RON, &SubstanceRegistry::shipped())
        .unwrap_or_else(|findings| {
        panic!(
            "bake: {} did not pass the consumer contract:\n{}\n\
             bake resolves the glb via asset_root() (BEVY_ASSET_ROOT → CARGO_MANIFEST_DIR → exe dir; \
             a macOS `.app` exe in Contents/MacOS resolves to Contents/Resources/assets). \
             Resolved assets root: {}. If this path is wrong, the packaging layout and asset_root() \
             disagree — see crate::assets.",
            path.display(),
            render(&findings),
            root.display(),
        )
    });
    let mesh_nodes = asset
        .geometry
        .nodes
        .iter()
        .filter(|n| !n.primitives.is_empty())
        .count();
    info!(
        "bake: extracted tank geometry — {} nodes, {} mesh-captured, {} ballistic volumes",
        asset.geometry.nodes.len(),
        mesh_nodes,
        asset.geometry.ballistic_volumes.len(),
    );

    commands.insert_resource(TankBlueprint {
        geometry: Arc::new(asset.geometry),
        spec: Arc::new(asset.spec),
    });
}

/// A GLB/spec pair that passed the whole L2 contract: the spec sheet and the certified geometry.
pub(crate) struct CertifiedAsset {
    pub spec: TankSpec,
    pub geometry: TankGeometry,
}

/// **The consumer contract.** Parse a spec sheet and the model it names, and either certify the
/// pair or say — in one report shape, deterministically ordered — exactly what refused it.
///
/// This is the single implementation of every `L2.*` law. The runtime bake calls it at startup on
/// the shipped asset; [`verify_asset`] calls it from the CLI on an export candidate before the
/// texture encode, and from CI on every committed pair. There is no second copy to drift from.
pub(crate) fn certify_asset(
    glb: &Path,
    spec_ron: &str,
    registry: &SubstanceRegistry,
) -> Result<CertifiedAsset, Vec<Finding>> {
    let spec_subject = || spec_subject_for(glb);
    let spec: TankSpec = ron::de::from_str(spec_ron).map_err(|err| {
        vec![Finding::new(
            &L2_SPEC,
            spec_subject(),
            err.to_string(),
            "fix the RON so it deserializes into TankSpec — the parser names the offending \
             position and field",
        )]
    })?;
    spec.validate().map_err(|err| {
        vec![Finding::new(
            &L2_SPEC,
            spec_subject(),
            err.to_string(),
            "author a value inside the domain the rejection names — a spec that parses but cannot \
             yield a working vehicle never reaches the sim",
        )]
    })?;
    let geometry = extract_tank_geometry(glb, &spec, registry)?;
    Ok(CertifiedAsset { spec, geometry })
}

/// The substance registry one verification runs against: the file the door named, or — for the
/// game, which ships its own registry inside the binary that reads it — the compiled-in one.
fn registry_at(path: Option<&Path>) -> Result<SubstanceRegistry, Vec<Finding>> {
    let Some(path) = path else {
        return Ok(SubstanceRegistry::shipped());
    };
    let refused = |evidence: String| {
        vec![Finding::new(
            &DOOR_REGISTRY,
            Subject::door("registry"),
            evidence,
            format!(
                "hand `--registry` the `assets/materials/materials.ron` of the revision this model \
                 came from — {} is the file the substance names in the model are read against, and \
                 a gate that cannot read it knows no substances at all",
                path.display()
            ),
        )]
    };
    let text = std::fs::read_to_string(path).map_err(|err| refused(err.to_string()))?;
    SubstanceRegistry::from_ron(&text).map_err(|err| refused(err.to_string()))
}

/// The sibling spec sheet of a model, by the mechanical `<id>.glb` → `<id>.tank.ron` rule.
fn spec_ron_name(glb: &Path) -> String {
    glb.with_extension("").to_string_lossy().into_owned() + ".tank.ron"
}

/// What a spec finding NAMES, given the model the sheet was certified against.
///
/// The sibling rule only holds for the TRACKED model, `assets/<id>/<id>.glb`, whose stem is the
/// asset id — and an id is a directory name, so it carries no dot. A derived artifact does:
/// the runtime bake certifies `<id>.sim.glb` (ADR-0035) against a sheet compiled into the binary,
/// and stripping one extension off that path yields `<id>.sim.tank.ron`, a file that does not exist
/// and never will, offered to the reader as the thing to go and fix. So a model whose stem is not
/// an id names ITSELF: the finding points at the pair that refused, and the sim artifact is the
/// half of that pair which is on disk.
fn spec_subject_for(glb: &Path) -> Subject {
    let derived = glb
        .file_stem()
        .is_some_and(|stem| stem.to_string_lossy().contains('.'));
    if derived {
        Subject::document(glb)
    } else {
        Subject::spec(Path::new(&spec_ron_name(glb)))
    }
}

/// Verify one asset pair and return the whole report, in order. Empty means certified.
///
/// The pair is named by the model alone: the spec sheet is its sibling `<id>.tank.ron`.
pub fn verify_asset(glb: &Path, registry: Option<&Path>) -> Vec<Finding> {
    let registry = match registry_at(registry) {
        Ok(registry) => registry,
        Err(findings) => return findings,
    };
    let spec_path = spec_ron_name(glb);
    let spec_ron = match std::fs::read_to_string(&spec_path) {
        Ok(text) => text,
        Err(err) => {
            return vec![Finding::new(
                &L2_SPEC,
                Subject::spec(Path::new(&spec_path)),
                err.to_string(),
                "every model ships beside its spec sheet — an asset is the trio `<id>.blend`, \
                 `<id>.tank.ron`, `<id>.glb` in `assets/<id>/`",
            )];
        }
    };
    match certify_asset(glb, &spec_ron, &registry) {
        Ok(_) => Vec::new(),
        Err(findings) => findings,
    }
}

/// **The canon file.** The two lists the `.blend` source pass may not maintain for itself, read
/// off the canonical Rust definitions and handed across the language boundary as one JSON document:
///
/// ```json
/// {"node_references": [{"field": "volumes", "node": "Hull_Front"}], "substance_keys": ["Cast"]}
/// ```
///
/// `node_references` is [`TankSpec::node_references`] — so a spec field added later reaches
/// `L1.SPEC_REFERENCES` without Python learning its name — and `substance_keys` is the registry
/// [`classify`] decides membership by. Both are already ordered by their producers.
///
/// A sheet that does not parse is the same `L2.SPEC` refusal the consumer contract gives it: there
/// is one report shape, and a canon generator that invented a second vocabulary would be a second
/// door.
pub fn canon_lists(spec_path: &Path, registry: Option<&Path>) -> Result<String, Vec<Finding>> {
    let refused = |evidence: String, repair: &str| {
        vec![Finding::new(
            &L2_SPEC,
            Subject::spec(spec_path),
            evidence,
            repair.to_owned(),
        )]
    };
    let spec_ron = std::fs::read_to_string(spec_path)
        .map_err(|err| refused(err.to_string(), "name the tank's `<id>.tank.ron`"))?;
    let spec: TankSpec = ron::de::from_str(&spec_ron).map_err(|err| {
        refused(
            err.to_string(),
            "fix the RON so it deserializes into TankSpec — the parser names the offending \
             position and field",
        )
    })?;
    let references: Vec<serde_json::Value> = spec
        .node_references()
        .into_iter()
        .map(|reference| serde_json::json!({"field": reference.field, "node": reference.node}))
        .collect();
    Ok(serde_json::json!({
        "node_references": references,
        "substance_keys": registry_at(registry)?.keys(),
    })
    .to_string())
}

/// Parse the glb as data into [`TankGeometry`]. Pure with respect to the app: `gltf` crate only,
/// usable identically from the runtime (step 0/phase 1) and the offline compiler (phase 2).
pub(crate) fn extract_tank_geometry(
    path: &Path,
    spec: &TankSpec,
    registry: &SubstanceRegistry,
) -> Result<TankGeometry, Vec<Finding>> {
    let refused = |evidence: String, repair: &str| {
        vec![Finding::new(
            &L2_DOCUMENT,
            Subject::document(path),
            evidence,
            repair.to_owned(),
        )]
    };
    let gltf::Gltf { document, mut blob } = gltf::Gltf::open(path).map_err(|err| {
        refused(
            err.to_string(),
            "re-export the model — this file is not a glb",
        )
    })?;
    let declared_colliders: HashSet<&str> = spec.colliders.iter().map(String::as_str).collect();

    // Resolve buffer data: a .glb's buffers are the BIN chunk (`Source::Bin`); external `.bin`
    // URIs are read relative to the glb (not used by our assets, supported for completeness).
    let mut buffers: Vec<Vec<u8>> = Vec::new();
    for buffer in document.buffers() {
        match buffer.source() {
            gltf::buffer::Source::Bin => buffers.push(blob.take().ok_or_else(|| {
                refused(
                    "the document declares a Bin buffer and carries no BIN chunk".to_string(),
                    "re-export the model as a self-contained binary glb",
                )
            })?),
            gltf::buffer::Source::Uri(uri) => {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                buffers.push(std::fs::read(parent.join(uri)).map_err(|err| {
                    refused(
                        format!("buffer `{uri}`: {err}"),
                        "re-export the model as a self-contained binary glb",
                    )
                })?);
            }
        }
    }

    // The loader instantiates `GltfAssetLabel::Scene(0)` under a wrapper entity named after the
    // scene (`Scene{i}` fallback) whose transform is the coordinate-conversion transform —
    // IDENTITY while bevy_gltf's opt-in glTF→Bevy conversion stays off (the repo never enables
    // it; the shadow compare is exactly what catches a future default flip — design §7.2).
    let scene = document.scenes().next().ok_or_else(|| {
        refused(
            "the document holds no scene".to_string(),
            "export with an active scene — the loader instantiates scene 0",
        )
    })?;
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
    let mut findings: Vec<Finding> = Vec::new();

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
                match classify(&primitive, registry) {
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
                findings.push(Finding::new(
                    &L2_ROLE_COHERENCE,
                    Subject::node(&name),
                    format!(
                        "{ballistic} substance primitive(s) and {plain} non-substance one(s) in \
                         one mesh"
                    ),
                    "split the object, or give every primitive a registry substance — membership \
                     is the material (§12)",
                ));
            }
            if is_collider && ballistic > 0 {
                findings.push(Finding::new(
                    &L2_ROLE_COHERENCE,
                    Subject::node(&name),
                    format!(
                        "declared a collision proxy and wearing {ballistic} registry substance(s)"
                    ),
                    "strip the substance material, or drop the node from the spec's `colliders` — \
                     a convex hull that is also armour is charged twice by the march",
                ));
            }
            for (index, substance) in captured {
                let Some(primitive) = mesh.primitives().nth(index) else {
                    continue;
                };
                let reader = primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                let Some(positions) = reader.read_positions() else {
                    findings.push(Finding::new(
                        &L2_PRIMITIVE_FORM,
                        Subject::node(&name).element(format!("primitive {index}")),
                        "no POSITION accessor",
                        "re-export the mesh — a primitive the sim consumes must carry its vertex \
                         positions",
                    ));
                    continue;
                };
                let indices: Vec<u32> = reader
                    .read_indices()
                    .map(|i| i.into_u32().collect())
                    .unwrap_or_default();
                let mut geometry = MeshGeometry {
                    positions: positions.collect(),
                    indices,
                    substance,
                    certificate: None,
                };
                if geometry.substance.is_some() {
                    match manifold_gate(&name, index, &geometry, root_scale) {
                        Ok(certificate) => geometry.certificate = Some(certificate),
                        Err(refusals) => findings.extend(refusals),
                    }
                }
                primitives.push(geometry);
            }
        }

        let index = nodes.len();
        // Blender enforces unique object names and the fallback names are unique by index; a
        // collision would make the name-keyed join ambiguous, so it is fatal at extract time.
        if by_name.insert(name.clone(), index).is_some() {
            findings.push(Finding::new(
                &L2_DOCUMENT,
                Subject::node(&name),
                "two nodes carry this name",
                "rename one of them in the .blend — every consumer joins the model to the scene by \
                 node name",
            ));
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
    let mut collision_proxies: Vec<usize> = Vec::new();
    for name in &spec.colliders {
        collision_proxies.extend(by_name.get(name).copied());
    }
    let mut roadwheels: Vec<(usize, TrackSide)> = Vec::new();
    for wheel in &spec.roadwheels {
        roadwheels.extend(by_name.get(&wheel.node).map(|&index| (index, wheel.side)));
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

    let geometry = TankGeometry {
        nodes,
        by_name,
        roadwheels,
        collision_proxies,
        ballistic_volumes,
    };
    findings.extend(declared_roles_resolve(&geometry, spec));
    if report::has_error(&findings) {
        return Err(sorted(findings));
    }
    Ok(geometry)
}

/// What the spec DECLARED, met with what the model turned out to hold: every typed reference
/// resolves (`L2.SPEC`), every declared role is one the resolved node can actually play
/// (`L2.ROLE_COHERENCE`), and every node the rigid pose composition walks through is authored at
/// unit scale (`L2.UNIT_SCALE`).
///
/// The reference vocabulary is `TankSpec::node_references`, so a spec field added later is checked
/// here without this function learning its name.
fn declared_roles_resolve(geometry: &TankGeometry, spec: &TankSpec) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut scaled: BTreeSet<&str> = BTreeSet::new();
    for NodeReference {
        role, node: name, ..
    } in spec.node_references()
    {
        let Some(&index) = geometry.by_name.get(name) else {
            findings.push(Finding::new(
                &L2_SPEC,
                Subject::node(name).element(format!("declared {role:?}")),
                "no node of the model carries this name",
                "name a node the model holds, or export the object the spec expects — nothing \
                 infers a node from a name pattern",
            ));
            continue;
        };
        let node = &geometry.nodes[index];
        let ballistic = node.is_ballistic();
        let refusal = match role {
            // A facet on a node the march never meets is inert; a station that is not its own
            // armour is a wheel shells pass through.
            NodeRole::Volume | NodeRole::Roadwheel if !ballistic => {
                Some("no primitive of it wears a registry substance".to_string())
            }
            NodeRole::Collider if node.primitives.iter().any(|p| p.indices.is_empty()) => {
                Some("its mesh data is unindexed".to_string())
            }
            NodeRole::Collider if node.primitives.is_empty() => {
                Some("it holds no mesh data at all".to_string())
            }
            _ => None,
        };
        if let Some(evidence) = refusal {
            findings.push(Finding::new(
                &L2_ROLE_COHERENCE,
                Subject::node(&node.name).element(format!("declared {role:?}")),
                evidence,
                "give the node the geometry its declared role needs, or declare the role on the \
                 node that has it",
            ));
        }
        // `rig_world_pose` composes rigidly — position and rotation only — so a scale anywhere on
        // the chain is silently dropped by the sim and kept by the view.
        if role.rigid_pose() {
            let mut walk = Some(index);
            while let Some(current) = walk {
                let node = &geometry.nodes[current];
                if node.transform.scale.to_array().map(f32::to_bits) != [1.0f32.to_bits(); 3] {
                    scaled.insert(node.name.as_str());
                }
                walk = node.parent;
            }
        }
    }
    for name in scaled {
        let index = geometry.by_name[name];
        findings.push(Finding::new(
            &L2_UNIT_SCALE,
            Subject::node(name),
            format!("authored scale {:?}", geometry.nodes[index].transform.scale),
            "apply the object scale in the .blend and re-export — the sim composes this chain \
             rigidly and drops the scale the view keeps",
        ));
    }
    findings
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
    use fixture::{CLOSED, Node, Primitive, pierced, solid};

    /// A registry substance, and a material name that is not one. Which two does not matter — what
    /// matters is that membership is decided by the registry and by nothing else.
    const SUBSTANCE: &str = "RHA";
    const ART: &str = "Mat_Decor";

    /// The gate at scale 1 — the case every synthetic fixture below is about.
    fn gate(node: &str, primitive: &MeshGeometry) -> Result<Vec<u32>, Vec<Finding>> {
        manifold_gate(node, 0, primitive, Vec3::ONE).map(|certificate| certificate.shells)
    }

    /// The check ids a report names, in report order.
    fn refusals(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|finding| finding.check.id).collect()
    }

    /// A substance primitive built from a raw index buffer.
    fn ballistic(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> MeshGeometry {
        MeshGeometry {
            positions,
            indices,
            substance: Some(PrimitiveSubstance {
                name: SUBSTANCE.into(),
                factor: 1000.0,
            }),
            certificate: None,
        }
    }

    /// The unit tetrahedron: the smallest closed, outward-wound, positive-volume shell.
    fn tetrahedron(indices: Vec<u32>) -> MeshGeometry {
        ballistic(
            vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
            ],
            indices,
        )
    }

    /// A vehicle that certifies: a hull, a plate, a collision proxy, and one station per side.
    fn sound_vehicle() -> (Vec<Node>, String) {
        let nodes = vec![
            Node::new("Hull"),
            Node::new("Plate")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [0.0, 0.0, 0.0])),
            Node::new("Proxy")
                .child_of("Hull")
                .holding(solid(ART, [0.0, 0.0, 0.0])),
            Node::new("Station_L")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [3.0, 0.0, 0.0])),
            Node::new("Station_R")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [6.0, 0.0, 0.0])),
        ];
        let spec = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &[],
        );
        (nodes, spec)
    }

    /// Write a synthetic trio and put it through the whole contract, exactly as the CLI and CI do.
    fn verify(id: &str, nodes: &[Node], spec: &str) -> Vec<Finding> {
        let asset = fixture::write(id, nodes, spec);
        verify_asset(&asset.glb, None)
    }

    /// THE REGISTRY IS AN INPUT, because it is DATA. A lane verifying a REVISION hydrates that
    /// revision's trio and its material library, and the substance numbers those bytes were
    /// authored against travel with them; a gate reading whatever registry the running binary was
    /// compiled with would certify a pair that never existed. So the door names the file, and a
    /// file it cannot name is a refusal of the door's own — never silence, and never a fallback to
    /// the compiled-in one, which is exactly how a wrong verdict would look like a right one.
    #[test]
    fn the_registry_the_door_supplies_is_the_one_the_contract_reads() {
        let (nodes, spec) = sound_vehicle();
        let asset = fixture::write("supplied-registry", &nodes, &spec);
        let elsewhere = asset.glb.with_file_name("materials.ron");

        // The same substances the model names, from a file rather than from this binary.
        std::fs::write(
            &elsewhere,
            std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/materials/materials.ron"),
            )
            .expect("the shipped registry is readable"),
        )
        .expect("the fixture registry is writable");
        let findings = verify_asset(&asset.glb, Some(&elsewhere));
        assert!(findings.is_empty(), "{}", render(&findings));

        // A registry that declares OTHER substances is a different verdict about the same bytes,
        // which is the whole reason the file has to come from the right revision.
        std::fs::write(
            &elsewhere,
            "SubstanceRegistry(substances: {\"Cheese\": (factor: 1.0, paintable: false)})",
        )
        .expect("the fixture registry is writable");
        let findings = verify_asset(&asset.glb, Some(&elsewhere));
        assert_eq!(
            refusals(&findings)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            ["L2.ROLE_COHERENCE"].into_iter().collect(),
            "{}",
            render(&findings)
        );

        // And a registry the door could not hand over refuses mechanically, under the door's own
        // id — not as a finding about the model, and not by falling back to the embedded one.
        std::fs::remove_file(&elsewhere).expect("the fixture registry is removable");
        let findings = verify_asset(&asset.glb, Some(&elsewhere));
        assert_eq!(
            refusals(&findings),
            ["door.registry"],
            "{}",
            render(&findings)
        );
        assert!(has_error(&findings));
        assert_eq!(findings[0].check.stage, Stage::Door);

        let refused = canon_lists(Path::new(&spec_ron_name(&asset.glb)), Some(&elsewhere))
            .expect_err("the canon lists are stated in the registry too");
        assert_eq!(
            refusals(&refused),
            ["door.registry"],
            "{}",
            render(&refused)
        );
    }

    /// A SPEC FINDING NAMES SOMETHING THAT EXISTS. The sibling `<id>.glb` → `<id>.tank.ron` rule
    /// holds for the tracked model and for nothing else, and the runtime bake certifies the SIM
    /// artifact (ADR-0035) against a sheet compiled into the binary — so deriving a path from
    /// `tiger_1.sim.glb` invents `tiger_1.sim.tank.ron` and hands the reader a file to go and fix
    /// that has never existed. The pair that refused is what a finding may name.
    #[test]
    fn a_finding_against_the_sim_artifact_names_the_sim_artifact() {
        let tracked = Path::new("assets/tiger_1/tiger_1.glb");
        assert_eq!(
            spec_subject_for(tracked),
            Subject::spec(Path::new("assets/tiger_1/tiger_1.tank.ron")),
            "the tracked model's sheet really is its sibling, and must still be named as one",
        );
        let sim = Path::new("assets/tiger_1/tiger_1.sim.glb");
        assert_eq!(
            spec_subject_for(sim),
            Subject::document(sim),
            "a derived artifact has no sibling sheet, so the finding names the artifact itself",
        );
        assert!(
            !format!("{}", spec_subject_for(sim)).contains("tank.ron"),
            "no finding may name a spec sheet that was never a file",
        );
    }

    /// The door's own shape: a sound pair certifies, and the report it produces is empty.
    #[test]
    fn a_sound_pair_certifies_with_an_empty_report() {
        let (nodes, spec) = sound_vehicle();
        let findings = verify("sound", &nodes, &spec);
        assert!(findings.is_empty(), "{}", render(&findings));
        assert!(!has_error(&findings));
    }

    /// EVERY SHIPPED ASSET, THROUGH THE REAL GATE. Discovery, not a list: a second vehicle is a
    /// directory, never a line of test code.
    /// THE WALK IS THE SAME WALK ON BOTH ENDS, and it is the same walk it was before the split.
    ///
    /// The dedicated server opens `<id>.sim.glb` and nothing else; the client opens it too, for the
    /// ballistic/armour walk, while its render path keeps `<id>.glb`. Two claims follow, and this
    /// is where they are checked on the shipped bytes rather than argued:
    ///
    /// 1. SERVER == CLIENT. Both ends call [`extract_at_startup`] on the SAME path constant, so the
    ///    geometry is one file read twice. What that leaves to prove is that the read is a pure
    ///    function of the bytes: extracting the sim artifact twice must produce bit-identical
    ///    positions, indices, substances and composed poses.
    /// 2. NOTHING MOVED. The sim artifact is a byte-strip of the certified view glb, so extracting
    ///    the view glb — what the walk used to read — must produce the identical geometry. A
    ///    ballistics change hiding inside a packaging change would show up here as a moved vertex.
    #[test]
    fn the_sim_artifact_walks_the_bytes_the_view_glb_carried() {
        let root = crate::assets::asset_root();
        let registry = SubstanceRegistry::shipped();
        let spec: TankSpec = ron::de::from_str(TIGER_SPEC_RON).expect("the shipped sheet parses");
        let extract = |path: &Path| {
            extract_tank_geometry(path, &spec, &registry)
                .unwrap_or_else(|f| panic!("{} extracts: {}", path.display(), render(&f)))
        };
        // Bitwise, on everything the walk consumes: the node's identity and composed pose, and
        // every captured primitive's positions, indices and substance verdict.
        let fingerprint = |geometry: &TankGeometry| -> Vec<String> {
            geometry
                .nodes
                .iter()
                .map(|node| {
                    let mut row = format!(
                        "{}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                        node.name,
                        // The PARENT, by name rather than by index: the sim skeleton is spawned as
                        // a hierarchy and a re-parented node moves everything under it. By name so
                        // a reordered node list is not a false positive.
                        node.parent.map(|p| geometry.nodes[p].name.as_str()),
                        // The LOCAL transform, which is what the skeleton's entities are spawned
                        // with — the composed pose below can agree while the chain that produced it
                        // does not.
                        node.transform.translation.to_array().map(f32::to_bits),
                        node.transform.rotation.to_array().map(f32::to_bits),
                        node.transform.scale.to_array().map(f32::to_bits),
                        node.root_position.to_array().map(f32::to_bits),
                        node.root_rotation.to_array().map(f32::to_bits),
                        node.root_scale.to_array().map(f32::to_bits),
                    );
                    for primitive in &node.primitives {
                        use std::fmt::Write as _;
                        let _ = write!(
                            row,
                            "|{:?}|{:?}|{:?}",
                            primitive
                                .positions
                                .iter()
                                .flatten()
                                .copied()
                                .map(f32::to_bits)
                                .collect::<Vec<_>>(),
                            primitive.indices,
                            primitive
                                .substance
                                .as_ref()
                                .map(|s| (s.name.as_str(), s.factor.to_bits())),
                        );
                    }
                    row
                })
                .collect()
        };

        let sim = root.join(crate::tank::TIGER_SIM_GLB_PATH);
        let view = root.join(crate::tank::TIGER_GLB_PATH);
        assert_eq!(
            fingerprint(&extract(&sim)),
            fingerprint(&extract(&sim)),
            "extraction is not a pure function of the sim artifact's bytes, so the server and the \
             client cannot be relied on to walk the same geometry from the same file",
        );
        assert_eq!(
            fingerprint(&extract(&sim)),
            fingerprint(&extract(&view)),
            "the sim artifact and the view glb do not carry the same walked geometry — the sim \
             artifact is a byte-strip of the certified view glb (ADR-0035), so a difference here \
             is a ballistics change wearing a packaging change's clothes",
        );
    }

    #[test]
    fn every_shipped_asset_passes_the_consumer_contract() {
        let shipped = fixture::shipped_assets();
        assert!(
            !shipped.is_empty(),
            "no `assets/<id>/<id>.glb` beside an `<id>.tank.ron` was discovered at all"
        );
        for model in shipped {
            let findings = verify_asset(&model, None);
            assert!(
                findings.is_empty(),
                "{} does not pass the consumer contract:\n{}",
                model.display(),
                render(&findings)
            );
            let spec_ron =
                std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
            let asset = certify_asset(&model, &spec_ron, &SubstanceRegistry::shipped())
                .expect("the pair certifies");
            let geometry = &asset.geometry;

            // What certification means, held to at the data: every volume the material rule found
            // carries usable armour, and the spawn order is the name order rather than the glTF
            // node order.
            let names: Vec<&str> = geometry
                .ballistic_volumes
                .iter()
                .map(|&index| geometry.nodes[index].name.as_str())
                .collect();
            let mut sorted_names = names.clone();
            sorted_names.sort_unstable();
            assert_eq!(names, sorted_names, "volumes must be extracted name-sorted");
            for &index in &geometry.ballistic_volumes {
                let volume = &geometry.nodes[index];
                for primitive in &volume.primitives {
                    let substance = primitive
                        .substance
                        .as_ref()
                        .expect("a volume holds only substance primitives");
                    assert!(
                        substance.factor > 0.0,
                        "`{}` wears `{}`, whose factor is not positive",
                        volume.name,
                        substance.name
                    );
                    assert!(
                        primitive.certificate.is_some(),
                        "`{}` passed the gate without a certificate",
                        volume.name
                    );
                }
            }
            // A station driven by a local rotation spins about its own origin. One left at the
            // model root orbits the tank instead, and reads as `(0, 0, 0)` to anything that takes
            // the origin for the axle centre.
            for &(index, _) in &geometry.roadwheels {
                let station = &geometry.nodes[index];
                assert!(
                    station.root_position.length() > 1.0e-4,
                    "roadwheel `{}` has no authored origin — set the object origin to the axle",
                    station.name
                );
            }
        }
    }

    /// THE MATERIAL-IMPLIED LINTS, over every shipped asset. A substance whose whole meaning is a
    /// consequence must have the facet that states it: Flesh that no one can kill, an Ammunition
    /// rack that cannot cook off, and an EngineBlock with no hp are each a plate pretending to be
    /// a module. The declared collision proxies and the structural singletons the assembler
    /// resolves by name are the same kind of claim — about the class "tank", asserted here rather
    /// than at spawn, where a bad asset would only surface as a panic.
    ///
    /// The substance names are the GLOBAL registry's, shared by every vehicle, and the loop is
    /// discovery: nothing here is one model's count, and a second tank adds no line.
    #[test]
    fn every_shipped_asset_states_the_consequences_its_materials_imply() {
        for model in fixture::shipped_assets() {
            let spec_ron =
                std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
            let asset = certify_asset(&model, &spec_ron, &SubstanceRegistry::shipped())
                .expect("the pair certifies");
            let (geometry, spec) = (&asset.geometry, &asset.spec);
            let named = |what: &str| format!("{}: `{what}`", model.display());

            assert!(
                !geometry.ballistic_volumes.is_empty(),
                "{}: no node wears a registry substance — the material library link is broken, \
                 and every plate on this vehicle is decor to the march",
                model.display()
            );
            // The assembler resolves these by name and nothing declares them, so nothing else can
            // notice they are gone until a tank is spawned.
            for singleton in ["Hull", "Center_Of_Mass"] {
                assert!(
                    geometry.by_name.contains_key(singleton),
                    "{} is absent — the tank assembler resolves it by name",
                    named(singleton)
                );
            }
            for &index in &geometry.collision_proxies {
                let proxy = &geometry.nodes[index];
                assert!(
                    !proxy.is_ballistic(),
                    "{} is a declared collision proxy wearing a substance — it would be charged as \
                     armour AND stand in for the body, counting the hull twice",
                    named(&proxy.name)
                );
            }
            for &index in &geometry.ballistic_volumes {
                let node = &geometry.nodes[index];
                let facets = spec.volumes.get(&node.name);
                let worn = node.substance_groups();
                if worn.contains_key("Flesh") {
                    assert!(
                        facets.is_some_and(|facets| facets.crew.is_some()),
                        "{} is made of Flesh and declares no crew facet — a crewman nobody can \
                         kill, and a seat nothing can knock out",
                        named(&node.name)
                    );
                }
                if worn.contains_key("Ammunition") {
                    assert!(
                        facets.is_some_and(|facets| facets.ammo),
                        "{} is made of Ammunition and declares no ammo facet — a rack that cannot \
                         cook off",
                        named(&node.name)
                    );
                }
                if worn.contains_key("EngineBlock") {
                    assert!(
                        facets.is_some(),
                        "{} is an EngineBlock with no component entry — a powerplant with no hp is \
                         armour pretending to be a module",
                        named(&node.name)
                    );
                }
            }
        }
    }

    /// L2.SPEC — the sheet parses, validates, and every typed reference it makes resolves.
    #[test]
    fn an_unreadable_sheet_or_an_unresolved_reference_is_refused() {
        let (nodes, _) = sound_vehicle();
        let findings = verify("unparsable", &nodes, "TankSpec(mass: ");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));

        // Parses, but declares a vehicle that cannot drive: no station on one side.
        let refused = fixture::spec(&["Proxy"], &[("Station_L", "Left")], &[]);
        let findings = verify("half-tracked", &nodes, &refused);
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));

        // Parses and validates, and names a node the model does not hold.
        let absent = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Absent", "Right")],
            &[],
        );
        let findings = verify("absent-node", &nodes, &absent);
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
        assert!(
            findings[0].subject.name == "Absent"
                && findings[0].evidence.contains("no node of the model"),
            "{}",
            render(&findings)
        );

        // The missing sibling sheet is the same refusal: an asset is a trio.
        let asset = fixture::write("orphan", &nodes, "");
        std::fs::remove_file(spec_ron_name(&asset.glb)).expect("the sheet is removable");
        assert_eq!(refusals(&verify_asset(&asset.glb, None)), ["L2.SPEC"]);
    }

    /// The canon file — the document shape the Blender source pass parses, and the two canonical
    /// lists inside it. Python maintains no vocabulary of RON field names or substance keys, so
    /// this is the only place either is pinned.
    #[test]
    fn the_canon_file_carries_the_reference_list_and_the_registry_keys() {
        let (nodes, _) = sound_vehicle();
        // EVERY role, and both of a weapon's two node fields: the RON path is what the Blender
        // pass prints as the line to go to, and a role does not determine one.
        let declared = fixture::spec_with(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &["Plate"],
            r#"servos: {"Turret_Yaw": (role: Yaw, max_speed: 1.0, accel: 1.0, travel: Continuous)},
    weapons: {"Main": (
        trigger: Primary,
        muzzle: "Muzzle",
        barrel: "Recoil",
        speed: 1.0, caliber: 0.1, mass: 1.0,
        fire_mode: Single(reload_secs: 1.0),
        recoil: (kick: 1.0, stiffness: 1.0, damping: 1.0),
    )},
    views: {Gunner: (node: "Sight", fov: 0.5)},"#,
        );
        let asset = fixture::write("canon", &nodes, &declared);
        let json =
            canon_lists(Path::new(&spec_ron_name(&asset.glb)), None).expect("the sheet parses");
        let document: serde_json::Value = serde_json::from_str(&json).expect("one JSON document");

        let references: Vec<(&str, &str)> = document["node_references"]
            .as_array()
            .expect("node_references is an array")
            .iter()
            .map(|row| {
                (
                    row["field"].as_str().expect("field is a string"),
                    row["node"].as_str().expect("node is a string"),
                )
            })
            .collect();
        // Role order, then node, then the authored path — `TankSpec::node_references`' own
        // ordering, carried through. Every field here is pinned nowhere else: Python maintains no
        // vocabulary of RON paths and renders whatever arrives in this document.
        assert_eq!(
            references,
            [
                ("servos", "Turret_Yaw"),
                ("volumes", "Plate"),
                ("colliders", "Proxy"),
                ("roadwheels[0].node", "Station_L"),
                ("roadwheels[1].node", "Station_R"),
                ("weapons[\"Main\"].muzzle", "Muzzle"),
                ("weapons[\"Main\"].barrel", "Recoil"),
                ("views[Gunner].node", "Sight"),
            ]
        );

        let keys: Vec<&str> = document["substance_keys"]
            .as_array()
            .expect("substance_keys is an array")
            .iter()
            .map(|key| key.as_str().expect("a key is a string"))
            .collect();
        assert_eq!(keys, SubstanceRegistry::shipped().keys());
        assert!(
            keys.contains(&SUBSTANCE),
            "the vocabulary is the registry's"
        );

        // A sheet that does not parse is the consumer contract's own refusal, not a second one.
        let broken = fixture::write("canon-unparsable", &nodes, "TankSpec(mass: ");
        let findings = canon_lists(Path::new(&spec_ron_name(&broken.glb)), None)
            .expect_err("an unparsable sheet has no canon");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
    }

    /// L2.ROLE_COHERENCE — a declared role the resolved node cannot play is refused by name.
    #[test]
    fn a_node_that_cannot_play_its_declared_role_is_refused() {
        let (mut nodes, spec) = sound_vehicle();
        // A station that wears no substance is a wheel the march passes straight through.
        nodes[3] = Node::new("Station_L")
            .child_of("Hull")
            .holding(solid(ART, [3.0, 0.0, 0.0]));
        let findings = verify("inert-station", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.ROLE_COHERENCE"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings[0].subject.name, "Station_L");

        // A collision proxy wearing armour would be charged twice by the march.
        let (mut nodes, spec) = sound_vehicle();
        nodes[2] = Node::new("Proxy")
            .child_of("Hull")
            .holding(solid(SUBSTANCE, [0.0, 3.0, 0.0]));
        let findings = verify("armoured-proxy", &nodes, &spec);
        assert_eq!(refusals(&findings), ["L2.ROLE_COHERENCE"]);

        // Half armour and half art in one mesh: the shadow compare diffs a node's bytes whole, so
        // a partly-captured node quietly stops being covered.
        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate")
            .child_of("Hull")
            .holding(solid(SUBSTANCE, [0.0, 0.0, 0.0]))
            .holding(solid(ART, [0.0, 0.0, 0.0]));
        let findings = verify("half-armour", &nodes, &spec);
        assert_eq!(refusals(&findings), ["L2.ROLE_COHERENCE"]);
        assert_eq!(findings[0].subject.name, "Plate");

        // A hull source avian cannot read: `extract_mesh_vertices_indices` bails on an unindexed
        // mesh even for the convex hull.
        let (mut nodes, spec) = sound_vehicle();
        nodes[2] = Node::new("Proxy").child_of("Hull").holding(Primitive {
            material: ART.into(),
            positions: solid(ART, [0.0, 0.0, 0.0]).positions,
            indices: Vec::new(),
        });
        let findings = verify("unindexed-proxy", &nodes, &spec);
        assert_eq!(refusals(&findings), ["L2.ROLE_COHERENCE"]);

        // A component facet on a node the march never meets is inert.
        let (nodes, _) = sound_vehicle();
        let facets = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &["Proxy"],
        );
        let findings = verify("inert-facet", &nodes, &facets);
        assert_eq!(
            refusals(&findings),
            ["L2.ROLE_COHERENCE"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings[0].subject.name, "Proxy");
    }

    /// L2.PRIMITIVE_FORM — what the sim reads has to be there, and has to be triangles.
    #[test]
    fn a_primitive_the_sim_cannot_read_is_refused() {
        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
            material: SUBSTANCE.into(),
            positions: Vec::new(),
            indices: Vec::new(),
        });
        // The glTF reader refuses a primitive with no POSITION accessor before the contract can
        // ask anything about it, so the document law is what names this one.
        let findings = verify("no-position", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.DOCUMENT"],
            "{}",
            render(&findings)
        );
        assert!(
            findings[0].evidence.contains("POSITION"),
            "{}",
            render(&findings)
        );

        // An index buffer that is not whole triangles, and one that addresses vertices the
        // POSITION accessor does not hold.
        let positions = solid(SUBSTANCE, [0.0, 0.0, 0.0]).positions;
        for indices in [vec![0u32, 1, 2, 3], vec![0u32, 1, 9]] {
            let (mut nodes, spec) = sound_vehicle();
            nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
                material: SUBSTANCE.into(),
                positions: positions.clone(),
                indices,
            });
            let findings = verify("malformed-indices", &nodes, &spec);
            assert_eq!(
                refusals(&findings),
                ["L2.PRIMITIVE_FORM"],
                "{}",
                render(&findings)
            );
        }
    }

    /// L2.UNIT_SCALE — the composed scale on armour, and the authored scale on every node the
    /// rigid pose composition walks through.
    #[test]
    fn a_scaled_node_the_sim_composes_through_is_refused() {
        // The armour half: a node whose COMPOSED scale is not one, anywhere up the chain.
        let (mut nodes, spec) = sound_vehicle();
        nodes[0] = Node::new("Hull").scaled([2.0, 2.0, 2.0]);
        let findings = verify("scaled-hull", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            [
                "L2.UNIT_SCALE",
                "L2.UNIT_SCALE",
                "L2.UNIT_SCALE",
                "L2.UNIT_SCALE"
            ],
            "{}",
            render(&findings)
        );
        assert!(
            findings.iter().any(|f| f.subject.name == "Hull"),
            "the ANCESTOR that carries the scale must be named: {}",
            render(&findings)
        );

        // The rig half: a station is a rig node, and `rig_world_pose` composes rigidly, so a scale
        // on it is dropped by the sim and kept by the view.
        let (mut nodes, spec) = sound_vehicle();
        nodes[3] = Node::new("Station_L")
            .child_of("Hull")
            .scaled([1.0, 1.0, f32::from_bits(1.0f32.to_bits() + 1)])
            .holding(solid(SUBSTANCE, [3.0, 0.0, 0.0]));
        let findings = verify("scaled-station", &nodes, &spec);
        assert!(
            refusals(&findings).iter().all(|id| *id == "L2.UNIT_SCALE"),
            "{}",
            render(&findings)
        );
        assert!(findings.iter().any(|f| f.subject.name == "Station_L"));
    }

    /// A BALLISTIC NODE IS UNIT-SCALE OR IT IS REFUSED, BY NAME.
    ///
    /// The same geometry passes at `1` and fails at every other scale, including one a millionth of
    /// an ULP away — nothing here compensates, rescales, or tolerates. The message names the node,
    /// because the fix is in the .blend and the artist has to be told which object.
    #[test]
    fn the_gate_refuses_every_composed_scale_that_is_not_exactly_one() {
        let tetra = tetrahedron(CLOSED.to_vec());
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
            let Err(findings) = manifold_gate("Scaled_Plate", 0, &tetra, scale) else {
                panic!("a ballistic node at {scale:?} must be refused");
            };
            assert_eq!(refusals(&findings), ["L2.UNIT_SCALE"]);
            assert_eq!(findings[0].subject.name, "Scaled_Plate");
        }
    }

    /// L2.CERTIFIED_RANGE — a coordinate the watertight projection has no proven bound over never
    /// reaches the corridor kernel.
    #[test]
    fn a_coordinate_outside_the_certified_range_is_refused() {
        let (low, high) = CERTIFIED_RANGE;
        for outside in [
            f32::from_bits(low.to_bits() - 1),
            f32::from_bits(high.to_bits() + 1),
            f32::INFINITY,
            f32::NAN,
        ] {
            let mut positions = tetrahedron(CLOSED.to_vec()).positions;
            positions[3] = [0.0, 0.0, outside];
            let findings = gate("Fringe", &ballistic(positions, CLOSED.to_vec()))
                .expect_err("an uncertified coordinate is refused");
            assert_eq!(refusals(&findings), ["L2.CERTIFIED_RANGE"]);
        }
        // And through the door, so the law's refusal is one the report carries rather than one only
        // a caller of the gate can see.
        let (mut nodes, spec) = sound_vehicle();
        let mut positions = solid(SUBSTANCE, [0.0, 0.0, 0.0]).positions;
        positions[3] = [0.0, 0.0, f32::from_bits(high.to_bits() + 1)];
        nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
            material: SUBSTANCE.into(),
            positions,
            indices: CLOSED.to_vec(),
        });
        let findings = verify("uncertified", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.CERTIFIED_RANGE"],
            "{}",
            render(&findings)
        );
        assert!(has_error(&findings));

        // Exact zero is legal and exact; the floor refuses what is merely NEAR zero.
        for inside in [0.0f32, -0.0, low, high] {
            let mut positions = tetrahedron(CLOSED.to_vec()).positions;
            positions[3] = [0.0, 0.0, inside];
            let primitive = ballistic(positions, CLOSED.to_vec());
            let refused = gate("Fringe", &primitive)
                .err()
                .map(|findings| refusals(&findings).contains(&"L2.CERTIFIED_RANGE"));
            assert_ne!(
                refused,
                Some(true),
                "{inside:e} is inside the certified range"
            );
        }
    }

    /// L2.EXACT_DEGENERACY — a face with no orientation, in BOTH its spellings: welded ids that
    /// repeat, and three DISTINCT welded ids on one line. One law, because they are one defect —
    /// a triangle that encloses exactly zero area breaks edge parity and leaves the embedding
    /// certificate no plane to intersect.
    #[test]
    fn a_zero_area_face_is_refused() {
        // Two corners at one position: the welded ids repeat.
        let mut repeated = CLOSED.to_vec();
        repeated.extend_from_slice(&[0, 1, 1]);
        let findings =
            gate("Repeated", &tetrahedron(repeated)).expect_err("a repeated corner is refused");
        assert_eq!(refusals(&findings)[0], "L2.EXACT_DEGENERACY");

        // Three DISTINCT welded ids, on one line: a fourth vertex collinear with an existing edge.
        let collinear = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, 0.0, 0.0],
        ];
        let mut indices = CLOSED.to_vec();
        indices.extend_from_slice(&[0, 1, 4]);
        let findings = gate("Collinear", &ballistic(collinear, indices))
            .expect_err("three distinct ids on one line are refused");
        assert_eq!(refusals(&findings)[0], "L2.EXACT_DEGENERACY");
        assert!(
            findings[0].evidence.contains("collinear"),
            "{}",
            render(&findings)
        );

        // And through the door, both spellings.
        let (mut nodes, spec) = sound_vehicle();
        let mut repeated = CLOSED.to_vec();
        repeated.extend_from_slice(&[0, 1, 1]);
        nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
            material: SUBSTANCE.into(),
            positions: solid(SUBSTANCE, [0.0, 0.0, 0.0]).positions,
            indices: repeated,
        });
        let findings = verify("repeated-corner", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.EXACT_DEGENERACY"],
            "{}",
            render(&findings)
        );
        assert!(has_error(&findings));

        let (mut nodes, spec) = sound_vehicle();
        let plate = solid(SUBSTANCE, [0.0, 0.0, 0.0]);
        let mut positions = plate.positions.clone();
        positions.push([2.0, 0.0, 0.0]);
        let mut indices = plate.indices.clone();
        indices.extend_from_slice(&[0, 1, (positions.len() - 1) as u32]);
        nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
            material: SUBSTANCE.into(),
            positions,
            indices,
        });
        let findings = verify("collinear-corners", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.EXACT_DEGENERACY"],
            "{}",
            render(&findings)
        );
        assert!(has_error(&findings));

        // One ulp off the line is a triangle, and the weld must not call it degenerate.
        let sliver = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [2.0, f32::from_bits(1), 0.0],
        ];
        let mut indices = CLOSED.to_vec();
        indices.extend_from_slice(&[0, 1, 4]);
        let findings = gate("Sliver", &ballistic(sliver, indices))
            .expect_err("the sliver is open, not degenerate");
        assert!(
            !refusals(&findings).contains(&"L2.EXACT_DEGENERACY"),
            "{}",
            render(&findings)
        );
    }

    /// L2.MANIFOLD_WINDING and L2.POSITIVE_SHELL_VOLUME — what silently-zero armour is made of.
    /// Driven on synthetic primitives, because a shipped asset must (and does) pass: the gate's
    /// value is entirely in what it rejects.
    #[test]
    fn open_inverted_and_doubled_shells_are_refused() {
        let closed = CLOSED.to_vec();
        let shells =
            gate("Good", &tetrahedron(closed.clone())).expect("a closed tetrahedron passes");
        assert_eq!(shells, vec![0; 4], "one tetrahedron is one shell");

        // One face removed: an open shell. This is the defect that makes armour worth ZERO — the
        // walk never finds the exit face and the volume is never charged.
        let mut open = closed.clone();
        open.truncate(9);
        let findings = gate("Open", &tetrahedron(open)).expect_err("an open shell is refused");
        assert_eq!(refusals(&findings), ["L2.MANIFOLD_WINDING"]);
        assert!(
            findings[0].evidence.contains("not 2"),
            "{}",
            render(&findings)
        );

        // A duplicated face: closed by undirected count, but one directed edge is traversed twice.
        let mut doubled = closed.clone();
        doubled.extend_from_slice(&[0, 2, 1]);
        let findings =
            gate("Doubled", &tetrahedron(doubled)).expect_err("a duplicated face is refused");
        assert!(
            findings[0].evidence.contains("directed edge"),
            "{}",
            render(&findings)
        );

        // Every face flipped: closed, but wound inward — the walk would read its entries as exits.
        let inverted: Vec<u32> = closed
            .chunks_exact(3)
            .flat_map(|face| [face[0], face[2], face[1]])
            .collect();
        let findings = gate("Inverted", &tetrahedron(inverted.clone()))
            .expect_err("an inside-out shell is refused");
        assert_eq!(refusals(&findings), ["L2.POSITIVE_SHELL_VOLUME"]);
        assert_eq!(findings[0].subject.name, "Inverted");

        // Both laws through the door, so each one's refusal is a row of the one report.
        for (id, indices) in [
            ("L2.MANIFOLD_WINDING", closed[..9].to_vec()),
            ("L2.POSITIVE_SHELL_VOLUME", inverted),
        ] {
            let (mut nodes, spec) = sound_vehicle();
            nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
                material: SUBSTANCE.into(),
                positions: solid(SUBSTANCE, [0.0, 0.0, 0.0]).positions,
                indices,
            });
            let findings = verify("hollow", &nodes, &spec);
            assert_eq!(refusals(&findings), [id], "{}", render(&findings));
            assert_eq!(findings[0].subject.name, "Plate");
            assert!(has_error(&findings));
        }
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
        let findings = gate("Abutting", &ballistic(positions, indices))
            .expect_err("face-to-face contact inside one primitive is refused");
        assert!(
            refusals(&findings)
                .iter()
                .all(|id| *id == "L2.MANIFOLD_WINDING"),
            "{}",
            render(&findings)
        );
        assert!(
            findings[0].evidence.contains("directed edge"),
            "{}",
            render(&findings)
        );
    }

    /// L2.SHELL_EMBEDDING — A SHELL THAT PASSES THROUGH ITSELF IS REFUSED, THROUGH THE WHOLE
    /// CONTRACT.
    ///
    /// `fixture::pierced` is built to reach this law and no earlier one: it is closed, consistently
    /// wound and encloses a positive volume, so the census and the volume gate both pass it and the
    /// embedding certificate is the only thing between it and the walk. That is what the pair-level
    /// witnesses in `embedding::tests` cannot say — they are handed a triangle list, not an asset —
    /// and it is the whole reason this case is driven from the glb.
    #[test]
    fn a_shell_that_passes_through_itself_is_refused() {
        // The gate's own three products first: the defect is not a census or a volume defect.
        let solid = pierced(SUBSTANCE, [0.0, 0.0, 0.0]);
        let geometry = ballistic(solid.positions.clone(), solid.indices.clone());
        let findings = gate("Pierced", &geometry).expect_err("a pierced shell is refused");
        assert_eq!(refusals(&findings), ["L2.SHELL_EMBEDDING"]);
        assert!(
            findings[0].evidence.contains("triangles 2 and 8")
                && findings[0].evidence.contains("welded vertex 4"),
            "{}",
            render(&findings)
        );

        // And through the door: one glb, one spec sheet, one report.
        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate").child_of("Hull").holding(solid);
        let findings = verify("pierced-shell", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.SHELL_EMBEDDING"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings[0].subject.name, "Plate");
        assert_eq!(findings[0].check.severity, Severity::Error);
        assert!(has_error(&findings), "the door exits non-zero on it");

        // The same box with its corner where a box has one certifies: nothing about the mutation
        // beyond the pierced corner is what refuses it.
        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate")
            .child_of("Hull")
            .holding(fixture::boxed(SUBSTANCE, [0.0, 0.0, 0.0]));
        let findings = verify("intact-box", &nodes, &spec);
        assert!(findings.is_empty(), "{}", render(&findings));
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
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for corner in CLOSED {
            indices.push(positions.len() as u32);
            positions.push(corners[corner as usize]);
        }
        gate("Split", &ballistic(positions, indices))
            .expect("welding by position closes the split shell");
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
        let shells = gate("Pair", &ballistic(positions, indices))
            .expect("two closed outward tetrahedra pass");
        assert_eq!(
            shells,
            vec![0, 0, 0, 0, 1, 1, 1, 1],
            "one key per edge-connected shell, dense and in triangle order",
        );
    }

    /// The report is one shape, in one order: stage, severity, check id, subject, element. Two
    /// runs of a defective asset produce the same rows in the same order, or nobody can diff them.
    #[test]
    fn the_report_is_deterministic_and_carries_every_field() {
        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate").child_of("Hull").holding(Primitive {
            material: SUBSTANCE.into(),
            positions: solid(SUBSTANCE, [0.0, 0.0, 0.0]).positions,
            indices: vec![0, 1, 2],
        });
        nodes[4] = Node::new("Station_R")
            .child_of("Hull")
            .scaled([2.0, 1.0, 1.0])
            .holding(solid(SUBSTANCE, [6.0, 0.0, 0.0]));
        let findings = verify("many-defects", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.MANIFOLD_WINDING", "L2.UNIT_SCALE", "L2.UNIT_SCALE"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings, verify("many-defects", &nodes, &spec));
        for finding in &findings {
            assert_eq!(finding.check.stage, Stage::Consumer);
            assert_eq!(finding.check.severity, Severity::Error);
            assert!(!finding.check.law.is_empty());
            assert!(!finding.evidence.is_empty());
            assert!(!finding.repair.is_empty());
            let rendering = finding.to_string();
            assert!(rendering.contains(finding.check.id));
            assert!(rendering.contains(&finding.subject.name));
            assert!(rendering.contains(&finding.evidence));
            assert!(rendering.contains(&finding.repair));
        }
        assert!(
            has_error(&findings),
            "an error makes the exit status non-zero"
        );
    }
}
