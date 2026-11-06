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
use crate::substances::{Matter, SubstanceRegistry};
use crate::tank::{SimParts, TrackSide, rig_world_pose};

mod embedding;
#[cfg(test)]
mod fixture;
/// The one finding shape every stage of the asset door emits.
mod report;

use report::sorted;
pub use report::{Check, Finding, Severity, Stage, Subject, has_error, render};

/// L2 consumer laws refuse sim construction; each row therefore has Error severity.
static L2_SPEC: Check = Check {
    id: "L2.SPEC",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "the tank RON parses, passes TankSpec::validate, every typed node reference resolves \
          to a model node, and every collider's substance is a registry row that states a friction",
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
    law: "a mesh is either wholly a substance solid or wholly art; a substance solid wears matter, \
          never an unworn substance; a declared collider is not armour and carries usable mesh data; \
          every declared component and roadwheel is a ballistic node",
};
static L2_WITNESS_ROLE: Check = Check {
    id: "L2.WITNESS_ROLE",
    stage: Stage::Consumer,
    severity: Severity::Error,
    law: "a volume declared crew wears only witness substances, and a witness substance is worn \
          only by a volume declared crew (ADR-0040)",
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
/// Door refusal, not an asset law: verification needs the revision's registry, not the binary's.
/// An unreadable registry is a mechanical failure to supply the contract, like an unpinned exporter.
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
    /// Node-local GLB buffers for collision proxies (convex hulls) and ballistic volumes, including
    /// road wheels (trimeshes); see [`classify`]. Exact bytes let the shadow compare loaded `Mesh`
    /// assets. Colliders attach under the node; avian's `ColliderTransform` composes
    /// [`transform`](Self::transform)'s scale, so proxies permit any scale; ballistic nodes do not.
    pub primitives: Vec<MeshGeometry>,
}

/// One glTF mesh primitive's sim-relevant buffers: what avian's `ConvexHullFromMesh` /
/// `TrimeshFromMesh` read (`extract_mesh_vertices_indices`: POSITION + the index buffer), plus the
/// primitive's [substance](MeshGeometry::substance) where it has one.
pub(crate) struct MeshGeometry {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    /// The substance this primitive wears, resolved here once: `Some` iff the primitive belongs to a
    /// declared armour node and its glTF material name is a key in the global substance registry
    /// (`assets/materials/materials.ron`). `None` = not a ballistic primitive — a collision-proxy
    /// hull, or a decor primitive that was never captured at all. Per PRIMITIVE, not per node, because glTF splits a mesh by material
    /// and §13.7 authors a mixed-substance part as one object holding one closed shell per
    /// substance region (the Tiger road wheel: `MildSteel` bodies + `Rubber` rims in one object).
    pub substance: Option<PrimitiveSubstance>,
    /// What [`manifold_gate`] proved about this primitive, and the identities it proved it with.
    /// `None` for a non-substance primitive, which is not armour and is never walked.
    pub certificate: Option<ShellCertificate>,
    /// Built on first use; later spawns clone the [`Collider`], sharing its shape.
    pub colliders: ColliderCache,
}

/// A primitive's colliders, built once from its buffers ([`MeshGeometry::trimesh_collider`],
/// [`MeshGeometry::convex_hull_colliders`]). The buffers never change after extraction, so a cached
/// shape is the shape a fresh build would give.
#[derive(Default)]
pub(crate) struct ColliderCache {
    trimesh: std::sync::OnceLock<Collider>,
    hulls: std::sync::OnceLock<Option<Vec<Collider>>>,
}

/// Shell identities for §13.4 pairing. Without them, coincident shells of one primitive and duplicate
/// claims of one shell present the same three numbers to the collector, leaving pairing ambiguous.
#[derive(Debug)]
pub(crate) struct ShellCertificate {
    /// Which closed shell each triangle belongs to, index-aligned with `indices.chunks_exact(3)` —
    /// the gate's own edge-connected components, dense and in first-triangle order. Shared with
    /// every spawned collider's [`crate::ballistics::BallisticSurfaces`].
    pub shells: Arc<[u32]>,
    /// The WELDED vertex id of each triangle corner. Two triangles meeting at a welded edge or
    /// vertex name it with the same two numbers however their own index buffers spell it, which is
    /// what lets the collector canonicalize a whole fan onto one contact.
    pub corners: Arc<[[u32; 3]]>,
}

/// A ballistic primitive's substance, resolved against the registry at extraction so the walk never
/// does a string lookup per query.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PrimitiveSubstance {
    /// The material datablock name — the join key to `materials.ron` (§12: identity, never parsed).
    pub name: String,
    /// The substance's identity in the registry it was resolved against.
    pub id: crate::substances::SubstanceId,
    /// Hard matter with its §13.2 field value (reference-mm per metre of chord), or a witness
    /// (ADR-0040), which states none.
    pub matter: crate::substances::Matter,
}

impl MeshGeometry {
    /// One convex hull per loose part of the primitive, consumed by both the game and the track
    /// sandbox. A loose part is a connected component of the triangle mesh after welding by exact
    /// position ([`weld`]): glTF splits a vertex wherever the normal or UV differs, so connectivity
    /// by index would cut a flat-shaded box into six quads. Vertices no triangle references belong
    /// to no part.
    ///
    /// `None` when the primitive has no part or any part has no hull (avian's `convex_hull`
    /// refuses a degenerate point set); the bake gate refuses a collision proxy with either, or
    /// with a triangle past its POSITION buffer (`declared_roles_resolve`). Built on the first call
    /// ([`ColliderCache`]).
    pub(crate) fn convex_hull_colliders(&self) -> Option<Vec<Collider>> {
        self.colliders
            .hulls
            .get_or_init(|| self.build_hulls())
            .clone()
    }

    /// The primitive as one query trimesh, `MERGE_DUPLICATE_VERTICES`: the exact parry construction
    /// avian's `TrimeshFromMesh` performs. Built on the first call ([`ColliderCache`]); panics,
    /// as avian does, on a buffer with no whole triangle.
    pub(crate) fn trimesh_collider(&self) -> Collider {
        self.colliders
            .trimesh
            .get_or_init(|| {
                Collider::trimesh_with_config(
                    self.positions.iter().copied().map(Vec3::from).collect(),
                    self.indices
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .map(|t| [t[0], t[1], t[2]])
                        .collect(),
                    avian3d::prelude::TrimeshFlags::MERGE_DUPLICATE_VERTICES,
                )
            })
            .clone()
    }

    fn build_hulls(&self) -> Option<Vec<Collider>> {
        let (welded, _) = weld(&self.positions);
        let mut parent: Vec<u32> = (0..welded.len() as u32).collect();
        fn find(parent: &mut [u32], mut id: u32) -> u32 {
            while parent[id as usize] != id {
                parent[id as usize] = parent[parent[id as usize] as usize];
                id = parent[id as usize];
            }
            id
        }
        // Union by welded id; the root of a set is its smallest id, so parts come out in
        // first-occurrence order of their vertices, deterministically.
        let mut referenced: HashSet<u32> = HashSet::new();
        for triangle in self.indices.as_chunks::<3>().0 {
            let ids = triangle
                .iter()
                .map(|&corner| {
                    *welded.get(corner as usize).expect(
                        "the bake gate refuses a collision proxy's triangle past its vertices",
                    )
                })
                .collect::<Vec<u32>>();
            referenced.extend(ids.iter().copied());
            let roots = [
                find(&mut parent, ids[0]),
                find(&mut parent, ids[1]),
                find(&mut parent, ids[2]),
            ];
            let low = roots[0].min(roots[1]).min(roots[2]);
            for root in roots {
                parent[root as usize] = low;
            }
        }
        let mut parts: BTreeMap<u32, Vec<Vec3>> = BTreeMap::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for (vertex, &id) in welded.iter().enumerate() {
            if !referenced.contains(&id) || !seen.insert(id) {
                continue;
            }
            let root = find(&mut parent, id);
            parts
                .entry(root)
                .or_default()
                .push(Vec3::from(self.positions[vertex]));
        }
        if parts.is_empty() {
            return None;
        }
        parts.into_values().map(Collider::convex_hull).collect()
    }
}

/// The whole model, extracted as data — the sim skeleton's construction source,
/// shadow-verified against every instantiated tank scene.
pub(crate) struct TankGeometry {
    pub nodes: Vec<NodeGeometry>,
    pub by_name: HashMap<String, usize>,
    /// One `(node index, TrackSide)` per spec `roadwheels` entry, in declaration order, never
    /// name-pattern sorted. A station's hard solids are declared on it or beneath its frame.
    pub roadwheels: Vec<(usize, TrackSide)>,
    /// Spec `colliders` in declaration order for deterministic spawn; no wire-shared index derives
    /// from this list (each proxy yields convex hulls).
    pub collision_proxies: Vec<usize>,
    /// Every declared armour node (the keys of [`TankSpec::volumes`]): MEMBERSHIP IS THE DECLARATION,
    /// never the material. A node is here iff the sheet names it, and a named node wears registry
    /// substances only. A node that wears one without being named is view geometry and holds no
    /// substance primitive. Name-sorted, so spawn order is deterministic without depending on the
    /// glTF node order.
    pub ballistic_volumes: Vec<usize>,
}

impl TankGeometry {
    /// Whether a node name is a ballistic volume — the sheet's declaration. Consult this, never a
    /// suffix or a material.
    pub fn is_ballistic(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|index| self.nodes[*index].is_ballistic())
    }

    /// Only declared collision proxies are sim-only and must never render. Ballistic plates form
    /// the rendered body; enclosed component volumes render behind the closed hull. The spec's
    /// declaration, not mesh appearance, separates a proxy from the picture.
    pub fn is_collision_proxy(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .is_some_and(|index| self.collision_proxies.contains(index))
    }
}

impl NodeGeometry {
    /// Whether this node holds any ballistic primitive: it is declared armour, since only a
    /// declared node's substance primitives are captured.
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

/// Spawn data assembled before network receive: an embedded spec and synchronous GLB extraction
/// remove asset-server timing from spawn. Runtime extraction remains the transitional seam in
/// `ARCHITECTURE.md`; [`construction_digest`] supplies the handshake's construction fingerprint.
#[derive(Resource, Clone)]
pub(crate) struct TankBlueprint {
    /// The tank file, relative to the asset root: the name a record gives the vehicle.
    pub file: String,
    pub geometry: Arc<TankGeometry>,
    pub spec: Arc<TankSpec>,
}

impl TankBlueprint {
    /// A tank kind's synchronous spawn data, shared by every instance.
    pub(crate) fn new(file: String, geometry: TankGeometry, spec: TankSpec) -> Self {
        // DERIVED from the mark wire's unsigned part index and signed millimetre coordinates.
        assert!(
            geometry.nodes.len() <= usize::from(u16::MAX) + 1,
            "{file}: hull_marks.part cannot index this node table — widen the mark wire format"
        );
        let mark_bound_m = f32::from(i16::MAX) * 0.001 - 0.001;
        for node in &geometry.nodes {
            for primitive in node
                .primitives
                .iter()
                .filter(|primitive| primitive.substance.is_some())
            {
                assert!(
                    primitive
                        .positions
                        .iter()
                        .flatten()
                        .all(|value| value.is_finite() && value.abs() <= mark_bound_m),
                    "{file}: hull_marks.at cannot pack {}'s local surface — recenter its mesh in the model or widen the mark wire coordinates",
                    node.name
                );
            }
        }
        Self {
            file,
            geometry: Arc::new(geometry),
            spec: Arc::new(spec),
        }
    }
}

/// The game's damage model: detachable construction is declared independently of optics.
/// A missing node is reported by the reference gate; admission refuses an absent declaration.
pub(crate) fn turret_node(geometry: &TankGeometry, spec: &TankSpec) -> Option<usize> {
    geometry.by_name.get(spec.turret.node.as_ref()?).copied()
}

/// Embedded sheet selected with the model and certificate by `assets::tank_file!`, keeping
/// geometry and construction paired.
const TANK_SPEC_RON: &str = include_str!(concat!(
    "../assets/",
    crate::assets::tank_file!(".tank.ron")
));

/// [`TANK_SPEC_RON`] parsed once, for a check that holds before any blueprint exists (the sfx load
/// gate's `required_clips`). The bake certifies the same text at startup, so a sheet that fails to
/// parse here is a broken build.
pub(crate) fn selected_spec() -> &'static TankSpec {
    static SPEC: std::sync::LazyLock<TankSpec> = std::sync::LazyLock::new(|| {
        crate::provenance::from_shipped_str(crate::assets::tank_file!(".tank.ron"), TANK_SPEC_RON)
            .expect("the selected tank's embedded sheet parses")
    });
    &SPEC
}

/// The selected tank's CONSTRUCTION digest: the embedded sheet and the sim artifact the bake reads
/// (`tank::TANK_SIM_GLB_PATH` under `root`), each length-prefixed. Every peer builds each tank's
/// construction and registry keys from these two sources. Armament and substance registries supply
/// the keyed facts. `net::protocol::protocol_id` folds all these inputs into the handshake: peers
/// over different game data refuse at connect instead of resolving facts against another model. A sim
/// artifact that cannot be read panics naming its path, as the bake would at startup.
pub(crate) fn construction_digest(root: &Path) -> u64 {
    use crate::fingerprint::{FNV_OFFSET_BASIS, fnv1a_64};
    let path = root.join(crate::tank::TANK_SIM_GLB_PATH);
    let sim = std::fs::read(&path)
        .unwrap_or_else(|err| panic!("bake: {} cannot be read: {err}", path.display()));
    [TANK_SPEC_RON.as_bytes(), sim.as_slice()].iter().fold(
        fnv1a_64(FNV_OFFSET_BASIS, b"overmatch-tank-construction-v1"),
        |hash, bytes| fnv1a_64(fnv1a_64(hash, &(bytes.len() as u64).to_le_bytes()), bytes),
    )
}

/// Which primitives' mesh buffers the sim consumes, and why (§12):
///
/// * **Ballistic volumes — DECLARED.** A node marches iff the sheet names it
///   (a key of [`TankSpec::volumes`]). Every primitive of a declared node wears a registry
///   substance, whose resistance the walk reads; a registry material on an undeclared node is view
///   geometry and captures nothing. Nothing is parsed out
///   of a node name.
/// * **Collision proxies — DECLARED.** The `colliders` list in `<tank>.tank.ron`, never a suffix,
///   names them. Every primitive is captured as a hull source, not a substance.
///
/// Roadwheels are the one node class that is BOTH a rig station and a volume: the wheels ship as one
/// watertight mesh per station, so the same node is the suspension station AND its own armour. The
/// station role is a marker component (`Roadwheel`) and the volume role a bundle + trimesh children,
/// and they compose on one entity (`tank::spawn::assemble_tank_body`). Their capture needs no rule
/// of its own — the wheel nodes are keyed in `volumes` and wear `MildSteel`/`Rubber`.
///
/// A declared node that mixes substance and non-substance primitives is REFUSED (see
/// [`extract_tank_geometry`]): half-armour has no honest meaning here, and the shadow compare's
/// whole-node mesh diff would quietly stop covering the uncaptured half.
///
/// This lookup does not decide armour membership; the sheet does. `Some(Err(material))` names an
/// unworn substance ([`crate::substances::Substance::matter`] is `None`): the walk can read nothing
/// from it, so capture refuses a declared node wearing it rather than treating it as art.
fn classify<'p>(
    primitive: &gltf::Primitive<'p>,
    registry: &SubstanceRegistry,
) -> Option<Result<PrimitiveSubstance, &'p str>> {
    // An unnamed material is no registry key, so it names no substance.
    let material = primitive.material().name()?;
    // Not a substance ⇒ ordinary art. The registry's `Unknown` arm IS the classifier; the error case
    // is a name that only LOOKS like a substance, which `L1.SUBSTANCE_IDENTITY` refuses at the
    // source (a `.001` duplicate means the material-library link drifted from LINKED to appended,
    // and only the .blend can tell a linked datablock from a copy of one).
    let substance = registry.get(material).ok()?;
    let Some(matter) = substance.matter() else {
        return Some(Err(material));
    };
    Some(Ok(PrimitiveSubstance {
        name: material.to_owned(),
        id: registry
            .id(material)
            .expect("a key the registry just resolved has an id in it"),
        matter,
    }))
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

pub(crate) fn certified_coordinate(value: f32) -> bool {
    let magnitude = value.abs();
    magnitude == 0.0 || (magnitude >= CERTIFIED_RANGE.0 && magnitude <= CERTIFIED_RANGE.1)
}

/// One finding per subject: first offender in the check's deterministic order, plus a count,
/// keeping a primitive's thousands of failing triangles from overwhelming the report.
fn first_of(offenders: &[String]) -> Option<String> {
    offenders.first().map(|first| match offenders.len() {
        1 => first.clone(),
        count => format!("{first} (and {} more)", count - 1),
    })
}

/// The §13.6 gate runs at extraction on every substance primitive, before any consumer. Failure
/// refuses extraction and names the node, primitive and defect. A hole leaves the walk without
/// an exit to pair, silently producing zero armour (§13).
///
/// Weld by exact position first: glTF splits vertices at normal/UV seams, so index-level edge
/// parity would falsely reject every seam of a flat-shaded plate instead of measuring its surface.
/// Each edge-connected shell must have:
/// * two triangles per undirected edge, each directed edge appearing once (closure and consistent
///   winding; repeated traversal in the same direction signals inconsistent winding);
/// * positive signed volume `Σ v₀·(v₁×v₂)/6` (outward winding). Inward shells are closed but their
///   normals make the walk read entries as exits.
///
/// Multiple shells per primitive are legal (§13.7): a substance region may have several islands,
/// e.g. a wheel's two steel bodies and axle. Return their surface identities for §13.4 pairing:
/// one id per triangle, dense in first-triangle order.
///
/// `scale` is the composed authored scale ([`NodeGeometry::root_scale`]) and must be exactly `1`:
/// the gate judges stored buffers. Componentwise scaling is injective over reals but not `f32`;
/// it can weld distinct coordinates (collapsing shell thickness) or move certified coordinates out
/// of [`CERTIFIED_RANGE`]. Refuse rather than compensate; apply scale in the .blend and re-export.
pub(crate) fn manifold_gate(
    node: &str,
    index: usize,
    primitive: &MeshGeometry,
    scale: Vec3,
) -> Result<ShellCertificate, Vec<Finding>> {
    let at = || Subject::node(node).element(format!("primitive {index}"));

    let findings = form_refusals(&at, primitive, scale);
    if !findings.is_empty() {
        return Err(sorted(findings));
    }

    let (welded, points) = weld(&primitive.positions);
    let (triangles, findings) = welded_triangles(&at, &primitive.indices, &welded, &points);
    if !findings.is_empty() {
        return Err(sorted(findings));
    }

    let findings = directed_edge_census(&at, &triangles);
    if !findings.is_empty() {
        return Err(sorted(findings));
    }

    let shells =
        outward_shells(&at, primitive, &triangles).map_err(|hollow| sorted(vec![hollow]))?;

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
        shells: shells.into(),
        corners: triangles.into(),
    })
}

/// The [`manifold_gate`] preconditions every later stage reads the buffers under: a non-empty index
/// buffer of whole triangles, a bit-exactly unit composed `scale`, and every coordinate inside
/// [`CERTIFIED_RANGE`]. All three are judged, so one report names every form defect at once.
fn form_refusals(at: &impl Fn() -> Subject, primitive: &MeshGeometry, scale: Vec3) -> Vec<Finding> {
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
    findings
}

/// Weld by EXACT position: the welded id of each vertex, index-aligned with `positions` and dense
/// in first-occurrence order, and the canonical position of each id. `-0.0` is canonicalized so a
/// coordinate that is zero has one bit pattern; otherwise two vertices at the same point can fail
/// to weld and open a phantom seam.
fn weld(positions: &[[f32; 3]]) -> (Vec<u32>, Vec<[f32; 3]>) {
    let mut canonical: HashMap<[u32; 3], u32> = HashMap::new();
    let mut welded: Vec<u32> = Vec::with_capacity(positions.len());
    // One canonical point per welded id makes shared corners coincide by construction for embedding.
    let mut points: Vec<[f32; 3]> = Vec::new();
    for position in positions {
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
    (welded, points)
}

/// Every triangle of `indices` in welded ids, in index-buffer order, with the refusals of the
/// triangle pass: a corner that indexes past the POSITION accessor (that triangle is dropped), and
/// a triangle that encloses exactly zero area (it is kept, and refused).
fn welded_triangles(
    at: &impl Fn() -> Subject,
    indices: &[u32],
    welded: &[u32],
    points: &[[f32; 3]],
) -> (Vec<[u32; 3]>, Vec<Finding>) {
    let mut findings: Vec<Finding> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(indices.len() / 3);
    let mut out_of_range: Vec<String> = Vec::new();
    let mut degenerate: Vec<String> = Vec::new();
    for (triangle, corners) in indices.as_chunks::<3>().0.iter().enumerate() {
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
        // Zero area: repeated welded ids, or distinct collinear points. Use the exact integer
        // plane normal, not a thresholded float cross product: an unoriented face breaks edge
        // parity and gives embedding no plane to intersect.
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
    (triangles, findings)
}

/// Directed-edge census: closure AND winding consistency over the welded triangles, refused unless
/// each directed welded edge occurs exactly once and its reverse exactly once.
///
/// `BTreeMap` and ordered passes keep diagnostics reproducible and searchable. Winding comes first:
/// duplicate or same-wound faces also look like closure failures, which would suggest the wrong fix.
fn directed_edge_census(at: &impl Fn() -> Subject, triangles: &[[u32; 3]]) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let mut directed: BTreeMap<(u32, u32), u32> = BTreeMap::new();
    for &[a, b, c] in triangles {
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
    findings
}

/// The closed shell each welded triangle belongs to, index-aligned with `triangles` — refused unless
/// every edge-connected shell encloses finite, strictly positive signed volume over the primitive's
/// stored positions.
fn outward_shells(
    at: &impl Fn() -> Subject,
    primitive: &MeshGeometry,
    triangles: &[[u32; 3]],
) -> Result<Vec<u32>, Finding> {
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
        return Err(Finding::new(
            &L2_POSITIVE_SHELL_VOLUME,
            at(),
            evidence,
            "recompute the shell's outward normals in the .blend — a negative volume is an \
             inside-out shell whose entry faces the walk reads as exits, and a zero one is flat",
        ));
    }

    // Dense surface ids follow shipped triangle order, independent of union-find root selection.
    let mut ids: HashMap<usize, u32> = HashMap::new();
    let mut shells: Vec<u32> = Vec::with_capacity(triangles.len());
    for triangle in 0..triangles.len() {
        let root = find(&mut parent, triangle);
        let next = ids.len() as u32;
        shells.push(*ids.entry(root).or_insert(next));
    }
    Ok(shells)
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
    // Use `AssetPlugin`'s `asset_root()` so bake and view open the same file; it already ends in
    // `assets` (`Contents/Resources/assets` in a macOS .app). Bevy's raw `get_base_path()` resolves
    // `Contents/MacOS` in a double-clicked bundle; appending `assets` there crashes. See `crate::assets`.
    let root = crate::assets::asset_root();
    // THE SIM ARTIFACT (ADR-0035), not the view glb: a byte-strip of the certified view glb holding
    // LOD0 geometry and material names — the walk's substance lookup
    // travels with it. The server opens this file and no other; the client opens it too, so both
    // sides walk identical accessor bytes by construction. `geometry_lod` has already fingerprinted
    // it against the certificate by the time this runs.
    let path = root.join(crate::tank::TANK_SIM_GLB_PATH);
    let asset = crate::provenance::with_shipped_policy(|| {
        certify_asset(&path, TANK_SPEC_RON, &SubstanceRegistry::shipped())
    })
        .unwrap_or_else(|findings| {
        panic!(
            "bake: {} did not pass the consumer contract:\n{}\n\
             bake resolves the glb via crate::assets::asset_root(). \
             Resolved assets root: {}. If this path is wrong, the packaging layout and asset_root() \
             disagree — see crate::assets.",
            path.display(),
            render(&findings),
            root.display(),
        )
    });
    let blueprint = TankBlueprint::new(
        crate::tank::TANK_SPEC_PATH.to_owned(),
        asset.geometry,
        asset.spec,
    );
    // The rigid track colliders ride the blueprint's flush: every body built from it gets them at
    // spawn. They are cut from the running gear measured off this same sim artifact.
    commands.insert_resource(crate::track::bump_stop::TrackColliders::of_blueprint(
        &blueprint, &path,
    ));
    commands.insert_resource(blueprint);
}

/// A GLB/spec pair that passed the whole L2 contract: the spec sheet and the certified geometry.
pub(crate) struct CertifiedAsset {
    pub spec: TankSpec,
    pub geometry: TankGeometry,
}

/// The single `L2.*` contract: certify a spec/model pair or return deterministic, ordered refusals.
/// Runtime calls it at startup; [`verify_asset`] serves CLI export candidates before texture encode
/// and CI's committed pairs, keeping all three on the same laws.
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
    SubstanceRegistry::from_ron_at(path, &text).map_err(|err| refused(err.to_string()))
}

/// The sibling spec sheet of a model, by the mechanical `<id>.glb` → `<id>.tank.ron` rule.
fn spec_ron_name(glb: &Path) -> String {
    glb.with_extension("").to_string_lossy().into_owned() + ".tank.ron"
}

/// Name a tracked model's sibling sheet, or a derived artifact itself. Tracked stems are asset ids
/// (directory names without dots); `<id>.sim.glb` (ADR-0035) uses an embedded sheet, so deriving
/// `<id>.sim.tank.ron` would direct repairs to a nonexistent file. The artifact is the pair's disk half.
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
    crate::provenance::with_shipped_policy(|| verify_asset_in_policy(glb, registry))
}

fn verify_asset_in_policy(glb: &Path, registry: Option<&Path>) -> Vec<Finding> {
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
/// off the canonical Rust definitions and handed across the language boundary as one JSON document,
/// with the sheet's [`TankSpec::validate`] verdict:
///
/// ```json
/// {"node_references": [{"field": "volumes", "node": "Hull_Front"}], "spec_validates": true,
///  "substance_keys": ["Cast"]}
/// ```
///
/// `node_references` is [`TankSpec::node_references`] — so a spec field added later reaches
/// `L1.SPEC_REFERENCES` without Python learning its name — and `substance_keys` is the registry
/// [`classify`] looks substances up in. Both are already ordered by their producers.
///
/// The document is also everything the tank build's source pass reads of the sheet, in place of
/// the sheet's bytes: a field it omits reaches no artifact byte, so its edits never move what
/// `scripts/tank/build.py verify` re-exports and compares. `spec_validates` is the verdict alone,
/// not its message.
///
/// Parse failures use the consumer contract's `L2.SPEC` report, keeping one refusal vocabulary.
pub fn canon_lists(spec_path: &Path, registry: Option<&Path>) -> Result<String, Vec<Finding>> {
    crate::provenance::with_shipped_policy(|| canon_lists_in_policy(spec_path, registry))
}

fn canon_lists_in_policy(
    spec_path: &Path,
    registry: Option<&Path>,
) -> Result<String, Vec<Finding>> {
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
        "spec_validates": spec.validate().is_ok(),
        "substance_keys": registry_at(registry)?.keys(),
    })
    .to_string())
}

/// Parse GLB data into [`TankGeometry`] using only `gltf`, independent of app state, for both
/// runtime and offline compilation.
pub(crate) fn extract_tank_geometry(
    path: &Path,
    spec: &TankSpec,
    registry: &SubstanceRegistry,
) -> Result<TankGeometry, Vec<Finding>> {
    // Validate before capture: duplicate or overlapping declarations (e.g. collider + armour)
    // would make capture unsound, regardless of caller.
    spec.validate().map_err(|err| {
        vec![Finding::new(
            &L2_SPEC,
            spec_subject_for(path),
            err.to_string(),
            "author a value inside the domain the rejection names — a spec that parses but cannot \
             yield a working vehicle never reaches the sim",
        )]
    })?;
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
    let declared_colliders: HashSet<&str> = spec
        .colliders
        .iter()
        .map(|collider| collider.node.as_str())
        .collect();
    let declared_armour: HashSet<&str> = spec.volumes.keys().map(String::as_str).collect();

    // Only the BIN chunk belongs to the fingerprinted GLB. Refuse `Source::Uri` (sidecars and
    // data: URIs), as `track::marker_model` does, so geometry stays within the named artifact.
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
                return Err(refused(
                    format!("buffer `{uri}` lives outside the file"),
                    "re-export the model as a self-contained binary glb",
                ));
            }
        }
    }

    let capture = MeshCapture {
        registry,
        declared_colliders: &declared_colliders,
        declared_armour: &declared_armour,
        buffers: &buffers,
    };

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

    // Depth-first traversal mirrors the loader's spawn recursion.
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

        let primitives = match node.mesh() {
            Some(mesh) => capture.primitives(&mesh, &name, root_scale, &mut findings),
            None => Vec::new(),
        };

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

    // Resolve typed lists from spec declarations, never node-name patterns.
    let mut collision_proxies: Vec<usize> = Vec::new();
    for collider in &spec.colliders {
        collision_proxies.extend(by_name.get(&collider.node).copied());
    }
    let mut roadwheels: Vec<(usize, TrackSide)> = Vec::new();
    for wheel in &spec.roadwheels {
        roadwheels.extend(by_name.get(&wheel.node).map(|&index| (index, wheel.side)));
    }
    // Name-sorted so spawn order never depends on the glTF node order.
    let mut ballistic_volumes: Vec<usize> = spec
        .volumes
        .keys()
        .filter_map(|name| by_name.get(name).copied())
        .collect();
    ballistic_volumes.sort_by(|a, b| nodes[*a].name.cmp(&nodes[*b].name));

    let geometry = TankGeometry {
        nodes,
        by_name,
        roadwheels,
        collision_proxies,
        ballistic_volumes,
    };
    let refused_at_capture: BTreeSet<String> = findings
        .iter()
        .map(|finding| finding.subject.name.clone())
        .collect();
    findings.extend(declared_roles_resolve(&geometry, spec, &refused_at_capture));
    findings.extend(aim_chain_contract(path, &geometry, spec));
    findings.extend(collider_substances(spec, registry));
    findings.extend(witness_roles(&geometry, spec));
    if report::has_error(&findings) {
        return Err(sorted(findings));
    }
    Ok(geometry)
}

/// Playable handles must resolve before spawn. The structural assembly is independent of the
/// Gunner chain; yaw must be above pitch so a parent-local yaw actually turns that sight.
fn aim_chain_contract(path: &Path, geometry: &TankGeometry, spec: &TankSpec) -> Vec<Finding> {
    let Some(view) = spec.views.get(&crate::spec::ViewKind::Gunner) else {
        return Vec::new(); // Unarmed tooling sheets have no player rig.
    };
    let refuse = |field: &str, evidence: String, repair: &str| {
        Finding::new(
            &L2_SPEC,
            spec_subject_for(path).element(field),
            evidence,
            repair,
        )
    };
    let mut findings = Vec::new();
    for name in ["Hull", "Center_Of_Mass"] {
        if !geometry.by_name.contains_key(name) {
            findings.push(refuse(
                name,
                "required rig node is absent".into(),
                "export the named rig node before spawning this playable sheet",
            ));
        }
    }
    let yaw = spec
        .gunner_yaw
        .as_ref()
        .expect("TankSpec admits a Gunner yaw declaration");
    if let (Some(&yaw), Some(&pitch)) =
        (geometry.by_name.get(yaw), geometry.by_name.get(&view.node))
    {
        let mut walk = geometry.nodes[pitch].parent;
        let mut found = false;
        while let Some(index) = walk {
            if index == yaw {
                found = true;
                break;
            }
            walk = geometry.nodes[index].parent;
        }
        if !found {
            findings.push(refuse(
                "gunner_yaw / views[Gunner].node",
                "the declared yaw is not an ancestor of the Gunner pitch servo".into(),
                "parent the Gunner pitch under its declared yaw, or select its actual yaw servo",
            ));
        }
    }
    findings
}

/// Each declared collider whose substance the registry cannot give a contact friction: a name it
/// lacks, a substance that is not hard, or a row that states no friction. Every collision proxy's
/// hulls carry their substance's friction (`tank::proxy_friction`), so a sheet naming one of these
/// is refused here, at the door and at the runtime bake, instead of at a tank's first spawn. The
/// track's substance is refused on the same terms: every track contact carries its friction
/// ([`crate::spec::TrackSpec::friction`]).
fn collider_substances(spec: &TankSpec, registry: &SubstanceRegistry) -> Vec<Finding> {
    let fix = "name a hard substance of `assets/materials/materials.ron` whose row states a \
               `friction`, or give that row one";
    let track = registry
        .contact_friction(&spec.track.substance)
        .err()
        .map(|error| {
            Finding::new(
                &L2_SPEC,
                Subject::door("track").element("substance"),
                error,
                fix,
            )
        });
    spec.colliders
        .iter()
        .filter_map(|collider| {
            let error = registry.contact_friction(&collider.substance).err()?;
            Some(Finding::new(
                &L2_SPEC,
                Subject::node(&collider.node).element("declared Collider substance"),
                error,
                fix,
            ))
        })
        .chain(track)
        .collect()
}

/// Capture inputs: the substance registry ([`classify`]), declared proxies/armour and resolved buffers.
struct MeshCapture<'a> {
    registry: &'a SubstanceRegistry,
    declared_colliders: &'a HashSet<&'a str>,
    declared_armour: &'a HashSet<&'a str>,
    buffers: &'a [Vec<u8>],
}

impl MeshCapture<'_> {
    /// The primitives of node `name`'s mesh the sim consumes, in primitive order: for a declared
    /// armour node every primitive, gated by [`manifold_gate`] at the node's composed
    /// `root_scale`; for any other node none of its substance primitives (they are view
    /// geometry), and every primitive of a declared collision proxy. Refusals are appended to `findings`: the node's role-coherence
    /// refusals first, then each captured primitive's, in primitive order.
    fn primitives(
        &self,
        mesh: &gltf::Mesh,
        name: &str,
        root_scale: Vec3,
        findings: &mut Vec<Finding>,
    ) -> Vec<MeshGeometry> {
        let is_collider = self.declared_colliders.contains(name);
        let is_armour = self.declared_armour.contains(name);
        let mut captured: Vec<(usize, Option<PrimitiveSubstance>)> = Vec::new();
        let mut ballistic = 0usize;
        let mut plain = 0usize;
        for primitive in mesh.primitives() {
            match classify(&primitive, self.registry) {
                Some(Ok(substance)) if is_armour => {
                    ballistic += 1;
                    captured.push((primitive.index(), Some(substance)));
                }
                Some(Err(material)) if is_armour => findings.push(Finding::new(
                    &L2_ROLE_COHERENCE,
                    Subject::node(name).element(format!("primitive {}", primitive.index())),
                    format!(
                        "wears `{material}`, a substance no volume wears: a registry key that \
                         states no resistance and no witness role"
                    ),
                    "give the primitive a hard or witness substance — an unworn substance's facts \
                     are read by a round, never by the walk",
                )),
                _ => {
                    plain += 1;
                    if is_collider {
                        captured.push((primitive.index(), None));
                    }
                }
            }
        }
        // Refuse partial capture: see [`classify`]'s whole-node shadow-coverage invariant.
        if ballistic > 0 && plain > 0 {
            findings.push(Finding::new(
                &L2_ROLE_COHERENCE,
                Subject::node(name),
                format!(
                    "declared armour, and {plain} of its primitive(s) wear no registry \
                     substance ({ballistic} do)"
                ),
                "give every primitive of the node a registry substance, or drop the node from \
                 the sheet's `volumes`",
            ));
        }
        if is_collider && is_armour {
            findings.push(Finding::new(
                &L2_ROLE_COHERENCE,
                Subject::node(name),
                "declared a collision proxy and declared armour",
                "drop the node from the spec's `colliders` or from its armour declaration — \
                 a convex hull that is also armour is charged twice by the march",
            ));
        }
        let mut primitives = Vec::new();
        for (index, substance) in captured {
            let primitive = mesh
                .primitives()
                .nth(index)
                .expect("`captured` indexes this mesh's own primitives");
            let reader = primitive.reader(|b| self.buffers.get(b.index()).map(Vec::as_slice));
            let Some(positions) = reader.read_positions() else {
                findings.push(Finding::new(
                    &L2_PRIMITIVE_FORM,
                    Subject::node(name).element(format!("primitive {index}")),
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
                colliders: ColliderCache::default(),
            };
            if geometry.substance.is_some() {
                match manifold_gate(name, index, &geometry, root_scale) {
                    Ok(certificate) => geometry.certificate = Some(certificate),
                    Err(refusals) => findings.extend(refusals),
                }
            }
            primitives.push(geometry);
        }
        primitives
    }
}

/// The two classes stay apart (ADR-0040): a crew volume is a witness, and only a crew volume is.
///
/// A crew box wearing a hard substance would resist the shell and shadow the field as matter; a
/// witness substance on anything else would be a volume no law wounds and no walk sees. Both are
/// refused here, naming the node, before anything spawns.
fn witness_roles(geometry: &TankGeometry, spec: &TankSpec) -> Vec<Finding> {
    let mut findings = Vec::new();
    for &index in &geometry.ballistic_volumes {
        let node = &geometry.nodes[index];
        let crew = spec
            .volumes
            .get(&node.name)
            .and_then(|volume| volume.component())
            .and_then(|component| component.crew);
        let (mut witness, mut hard) = (Vec::new(), Vec::new());
        for substance in node.primitives.iter().filter_map(|p| p.substance.as_ref()) {
            match substance.matter {
                crate::substances::Matter::Witness => witness.push(substance.name.as_str()),
                crate::substances::Matter::Hard { .. } => hard.push(substance.name.as_str()),
            }
        }
        let refusal = match (crew, witness.is_empty(), hard.is_empty()) {
            (Some(station), _, false) => Some((
                format!("declared crew ({station:?}) and wearing hard substance(s) {hard:?}"),
                "give every primitive of the crew box a witness substance — a crewman \
                 neither resists the shell nor shadows the spall",
            )),
            (None, false, _) => Some((
                format!("wearing witness substance(s) {witness:?} without a crew declaration"),
                "declare the volume `crew: <station>` in the tank file, or give it a hard \
                 substance — only crew are witnesses",
            )),
            _ => None,
        };
        if let Some((evidence, repair)) = refusal {
            findings.push(Finding::new(
                &L2_WITNESS_ROLE,
                Subject::node(&node.name),
                evidence,
                repair,
            ));
        }
    }
    findings
}

/// Resolve typed references (`L2.SPEC`), check their roles (`L2.ROLE_COHERENCE`) and require unit
/// authored scale throughout rigid-pose chains (`L2.UNIT_SCALE`). `TankSpec::node_references` owns
/// the vocabulary, so new spec fields reach this gate without another field-name list.
fn declared_roles_resolve(
    geometry: &TankGeometry,
    spec: &TankSpec,
    refused_at_capture: &BTreeSet<String>,
) -> Vec<Finding> {
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
            // A declared volume without captured matter is inert in the game's damage model.
            NodeRole::Volume if !ballistic => Some(
                "it is declared armour, and no primitive of it wears a registry substance"
                    .to_string(),
            ),
            // Every volumes key has a Volume reference, which owns missing-matter diagnostics.
            NodeRole::Roadwheel if !ballistic && spec.volumes.contains_key(name) => None,
            NodeRole::Roadwheel if !station_has_solid(geometry, index) => {
                Some("neither the station nor its descendants hold declared hard volumes; declare their `volumes` keys under this station frame".to_string())
            }
            NodeRole::Collider if node.primitives.iter().any(|p| p.indices.is_empty()) => {
                Some("its mesh data is unindexed".to_string())
            }
            NodeRole::Collider if node.primitives.is_empty() => {
                Some("it holds no mesh data at all".to_string())
            }
            NodeRole::Collider
                if node.primitives.iter().any(|p| {
                    p.indices.len() % 3 != 0
                        || p.indices.iter().any(|&i| i as usize >= p.positions.len())
                }) =>
            {
                Some("its mesh data holds a triangle past its vertices".to_string())
            }
            // The spawn builds one convex hull per loose part from these buffers; a part with no
            // hull (a flat or degenerate point set) is a proxy that cannot collide.
            NodeRole::Collider
                if node
                    .primitives
                    .iter()
                    .any(|p| p.convex_hull_colliders().is_none()) =>
            {
                Some("a loose part of its mesh has no convex hull (flat or degenerate)".to_string())
            }
            // Armour is drawn geometry that casts its own shadow; a node that is both would be
            // silenced and stood in for by itself.
            NodeRole::ShadowProxy if ballistic => Some("it is declared armour".to_string()),
            _ => None,
        };
        if let Some(evidence) = refusal.filter(|_| !refused_at_capture.contains(name)) {
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

/// The game's damage model: declared hard volumes belong to a station through the authored
/// parent chain. The station may be a bare frame; mesh shape and names never infer membership.
fn station_has_solid(geometry: &TankGeometry, station: usize) -> bool {
    geometry.ballistic_volumes.iter().any(|&solid| {
        if !geometry.nodes[solid].primitives.iter().any(|primitive| {
            primitive
                .substance
                .as_ref()
                .is_some_and(|substance| matches!(substance.matter, Matter::Hard { .. }))
        }) {
            return false;
        }
        let mut ancestor = Some(solid);
        while let Some(index) = ancestor {
            if index == station {
                return true;
            }
            ancestor = geometry.nodes[index].parent;
        }
        false
    })
}

/// Compare every instantiated tank view with extracted sim data. Read-only and global, hence
/// order-independent of `tank::bind_tank_view`, the entity-scoped observer of the same event.
#[expect(
    clippy::too_many_arguments,
    reason = "an observer comparing the blueprint with an instantiated scene: hierarchy, names, transforms and meshes are separate reads"
)]
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
    };
    let geometry = &blueprint.geometry;
    // Sim and view trees share names; compare only the view, since sim transforms are extracted
    // values by construction.
    let skeleton: std::collections::HashSet<Entity> = sim_parts
        .get(ready.entity)
        .expect("a tank root holds its SimParts from spawn (tank::spawn), before its scene loads")
        .0
        .values()
        .copied()
        .collect();
    let mut mismatches: Vec<String> = Vec::new();

    // Compare named glTF nodes, excluding `Mesh3d` primitive leaves. `GltfMaterialName` cannot
    // distinguish leaves with unnamed materials (e.g. physics-only coax MG volumes); the view
    // binder likewise uses `Mesh3d`.
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

    for node in &geometry.nodes {
        if !seen.contains_key(node.name.as_str()) {
            mismatches.push(format!("extracted node `{}` not in scene", node.name));
        }
    }

    if !mismatches.is_empty() {
        fail(mismatches);
    }
}

/// Shadow verdict: fatal in every build. The sim was built from the extracted geometry, so a scene
/// that disagrees with it is a tank whose view and simulation are two different vehicles.
fn fail(mismatches: Vec<String>) -> ! {
    for m in &mismatches {
        error!("bake: SHADOW-BAKE mismatch: {m}");
    }
    panic!(
        "bake: shadow compare failed with {} mismatches (see log)",
        mismatches.len()
    );
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
    use crate::spec::ViewKind;
    use fixture::{CLOSED, Node, Primitive, pierced, solid};

    /// A registry substance, and a material name that is not one. Which two does not matter — what
    /// matters is that the registry names a substance and nothing else.
    const SUBSTANCE: &str = "RHA";
    const ART: &str = "Mat_Decor";

    const REGISTRY_FIXTURE: &str = r#"SubstanceRegistry(substances: {"Cheese": Hard(resistance: Reference, visual: "fixture", feedback: (exposed_linear_rgb: ((0.5, 0.51, 0.53), Fixture("fixture")), powder_linear_rgb: ((0.47, 0.47, 0.47), Fixture("fixture")), metallic: (1.0, Fixture("fixture")), roughness_gain: (1.0, Fixture("fixture")), sparks: (true, Fixture("fixture"))),
                debris_half_angle_deg: (26.2, Fixture("fixture")),
                facts: Some((density: (1000.0, Fixture("fixture")),
                    flow_strength: (1.0e6, Fixture("fixture")),
                    youngs_modulus: (1.0e9, Fixture("fixture")),
                    poisson_ratio: (0.4, Fixture("fixture")))))})"#;

    /// The gate at scale 1 — the case every synthetic fixture below is about.
    fn gate(node: &str, primitive: &MeshGeometry) -> Result<Vec<u32>, Vec<Finding>> {
        manifold_gate(node, 0, primitive, Vec3::ONE).map(|certificate| certificate.shells.to_vec())
    }

    fn refusals(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|finding| finding.check.id).collect()
    }

    fn ballistic(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> MeshGeometry {
        MeshGeometry {
            positions,
            indices,
            substance: Some(PrimitiveSubstance {
                name: SUBSTANCE.into(),
                id: crate::substances::SubstanceId::fixture(1000.0),
                matter: crate::substances::Matter::Hard {
                    factor: 1000.0,
                    debris_ratio: 2.0,
                },
            }),
            certificate: None,
            colliders: ColliderCache::default(),
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

    /// The game's damage model: adding an independently traversing sight must not select the
    /// body which receives assembly mass. Structural data is the sole owner of that identity.
    #[test]
    fn assembly_root_does_not_follow_optics() {
        let (nodes, sheet) = sound_vehicle();
        let asset = fixture::write("assembly-root", &nodes, &sheet);
        let mut spec: TankSpec = ron::from_str(&sheet).unwrap();
        let geometry =
            extract_tank_geometry(&asset.glb, &spec, &SubstanceRegistry::shipped()).unwrap();
        let expected = geometry.by_name["Hull"];
        assert_eq!(turret_node(&geometry, &spec), Some(expected));
        // Fixture sight is elsewhere and need not have a servo ancestor.
        spec.views.insert(
            ViewKind::Gunner,
            crate::spec::ViewSpec {
                node: "Plate".into(),
                optics: crate::spec::Optics::Unaided(60.0),
                requires: Vec::new(),
            },
        );
        spec.servos.insert(
            "Plate".into(),
            crate::tank::ServoSpec::test_continuous(crate::tank::ServoRole::Yaw, 1.0),
        );
        assert_eq!(turret_node(&geometry, &spec), Some(expected));
    }

    /// The certified sheet selects assembly and aim handles separately. A two-part turret and
    /// an independent sight yaw are admitted; wrong roles, ancestry and absent assemblies stop
    /// at L2.SPEC instead of a complete spawn or the player's first optic input.
    #[test]
    fn certification_separates_assembly_from_the_gunner_chain() {
        let (mut nodes, sheet) = sound_vehicle();
        nodes.extend([
            Node::new("Center_Of_Mass").child_of("Hull"),
            Node::new("Assembly").child_of("Hull"),
            Node::new("BodyYaw").child_of("Assembly"),
            Node::new("SightYaw").child_of("BodyYaw"),
            Node::new("SightPitch").child_of("SightYaw"),
        ]);
        let sheet = sheet.replace("node: Some(\"Hull\"), mass:", "node: Some(\"Assembly\"), mass:")
            .replace("gunner_yaw: None", "gunner_yaw: Some(\"SightYaw\")")
            .replace("servos: {}, weapons: {}, views: {},", r#"
                servos: {
                    "BodyYaw": (role: Yaw, drive: Hand(hand_rate: (1.0, Fixture("rate"))), travel: Continuous, requires: []),
                    "SightYaw": (role: Yaw, drive: Hand(hand_rate: (2.0, Fixture("rate"))), travel: Continuous, requires: []),
                    "SightPitch": (role: Pitch, drive: Hand(hand_rate: (3.0, Fixture("rate"))), travel: Continuous, requires: []),
                }, weapons: {}, views: {
                    Gunner: (node: "SightPitch", optics: Unaided(60.0), requires: []),
                },"#);
        let asset = fixture::write("independent-sight", &nodes, &sheet);
        let registry = SubstanceRegistry::shipped();
        let certified = certify_asset(&asset.glb, &sheet, &registry)
            .expect("independent construction certifies");
        let geometry = &certified.geometry;
        assert_eq!(
            turret_node(geometry, &certified.spec),
            Some(geometry.by_name["Assembly"])
        );
        for (old, new, field) in [
            (
                "node: Some(\"Assembly\"), mass:",
                "node: None, mass:",
                "turret.node",
            ),
            (
                "gunner_yaw: Some(\"SightYaw\")",
                "gunner_yaw: None",
                "gunner_yaw",
            ),
            (
                "gunner_yaw: Some(\"SightYaw\")",
                "gunner_yaw: Some(\"SightPitch\")",
                "Yaw servo",
            ),
            (
                "gunner_yaw: Some(\"SightYaw\")",
                "gunner_yaw: Some(\"Hull\")",
                "Yaw servo",
            ),
        ] {
            let report = render(
                &certify_asset(&asset.glb, &sheet.replace(old, new), &registry)
                    .err()
                    .expect("invalid rig sheet is refused"),
            );
            assert!(
                report.contains(".tank.ron")
                    && report.contains("L2.SPEC")
                    && report.contains(field),
                "{report}"
            );
        }
        let unrelated = sheet.replace(
            "gunner_yaw: Some(\"SightYaw\")",
            "gunner_yaw: Some(\"BodyYaw\")",
        );
        let index = nodes
            .iter()
            .position(|node| node.name == "SightPitch")
            .unwrap();
        nodes[index].parent = Some("Hull".into());
        let asset = fixture::write("unrelated-sight", &nodes, &unrelated);
        let report = render(
            &certify_asset(&asset.glb, &unrelated, &registry)
                .err()
                .expect("yaw must turn its declared sight"),
        );
        assert!(
            report.contains("gunner_yaw") && report.contains("ancestor"),
            "{report}"
        );
    }

    /// `spec` with `node` also declared main armour.
    fn declaring(spec: &str, node: &str) -> String {
        spec.replacen(
            "volumes: {",
            &format!("volumes: {{\"{node}\": (main: true), "),
            1,
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
            Node::new("Fixture_Engine")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [0.0, 12.0, 0.0])),
            Node::new("Fixture_Transmission")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [0.0, 15.0, 0.0])),
            // The game's damage model: fixture construction seats the two-crew floor.
            Node::new("Fixture_SeatA")
                .child_of("Hull")
                .holding(solid("Flesh", [0.0, 6.0, 0.0])),
            Node::new("Fixture_SeatB")
                .child_of("Hull")
                .holding(solid("Flesh", [0.0, 9.0, 0.0])),
        ];
        let spec = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &[],
            &["Plate", "Station_L", "Station_R"],
        );
        (nodes, spec)
    }

    /// Synthetic trios use the shared L2 reader with inline-fixture admission; shipped readers refuse it.
    fn verify(id: &str, nodes: &[Node], spec: &str) -> Vec<Finding> {
        let asset = fixture::write(id, nodes, spec);
        verify_asset_in_policy(&asset.glb, None)
    }

    /// CLI verification and Blender canon must refuse Fixture provenance in every build. Inline
    /// synthetic readers stay separate; public refusals must restore their caller's admission policy.
    #[test]
    fn shipped_file_readers_refuse_fixture_provenance() {
        let (nodes, spec) = sound_vehicle();
        let asset = fixture::write("shipped-provenance", &nodes, &spec);
        let sheet = spec_ron_name(&asset.glb);
        let sheet_path = Path::new(&sheet);

        let findings = verify_asset(&asset.glb, None);
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
        let report = render(&findings);
        assert!(report.contains(&sheet), "{report}");
        assert!(
            report.contains("Fixture provenance is test-only"),
            "{report}"
        );
        assert!(report.contains("replace this field's label"), "{report}");
        let findings = canon_lists(sheet_path, None).expect_err("canon reads a shipped sheet");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
        assert!(render(&findings).contains("Fixture provenance is test-only"));

        assert!(verify_asset_in_policy(&asset.glb, None).is_empty());
        canon_lists_in_policy(sheet_path, None).expect("the inline fixture policy is restored");

        let registry = asset.glb.with_file_name("materials.ron");
        std::fs::write(&registry, REGISTRY_FIXTURE).expect("the fixture registry is writable");
        let findings = verify_asset(&asset.glb, Some(&registry));
        assert_eq!(
            refusals(&findings),
            ["door.registry"],
            "{}",
            render(&findings)
        );
        let report = render(&findings);
        assert!(report.contains(&*registry.to_string_lossy()), "{report}");
        assert!(
            report.contains("Fixture provenance is test-only"),
            "{report}"
        );
        std::fs::write(sheet_path, TANK_SPEC_RON).expect("the fixture sheet is writable");
        canon_lists(sheet_path, None).expect("a shipped sheet still produces canon");
        let findings = canon_lists(sheet_path, Some(&registry))
            .expect_err("canon must refuse Fixture provenance in its supplied registry");
        assert_eq!(
            refusals(&findings),
            ["door.registry"],
            "{}",
            render(&findings)
        );
    }

    /// Verification reads the revision's registry with its trio: the binary's registry could
    /// certify a pair that never existed. Missing registry files must produce door refusals,
    /// never silence or fallback to the compiled-in registry.
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
        let findings = verify_asset_in_policy(&asset.glb, Some(&elsewhere));
        assert!(findings.is_empty(), "{}", render(&findings));

        // Different registry keys leave both plate and collider substances unresolved.
        std::fs::write(&elsewhere, REGISTRY_FIXTURE).expect("the fixture registry is writable");
        let findings = verify_asset_in_policy(&asset.glb, Some(&elsewhere));
        assert_eq!(
            refusals(&findings)
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            ["L2.ROLE_COHERENCE", "L2.SPEC"].into_iter().collect(),
            "{}",
            render(&findings)
        );

        // A missing registry is a door refusal, not a model finding or embedded-registry fallback.
        std::fs::remove_file(&elsewhere).expect("the fixture registry is removable");
        let findings = verify_asset_in_policy(&asset.glb, Some(&elsewhere));
        assert_eq!(
            refusals(&findings),
            ["door.registry"],
            "{}",
            render(&findings)
        );
        assert!(has_error(&findings));
        assert_eq!(findings[0].check.stage, Stage::Door);

        let refused =
            canon_lists_in_policy(Path::new(&spec_ron_name(&asset.glb)), Some(&elsewhere))
                .expect_err("the canon lists are stated in the registry too");
        assert_eq!(
            refusals(&refused),
            ["door.registry"],
            "{}",
            render(&refused)
        );
    }

    /// Tracked spec findings name the sibling sheet, even when missing. The sim artifact embeds
    /// its sheet (ADR-0035), so its finding names the artifact, never an invented `.sim.tank.ron`.
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

    /// The selected tank's server and client walk `<id>.sim.glb`; rendering reads `<id>.glb`.
    /// Both ends call [`extract_at_startup`] with the same path constant. On shipped bytes, repeated
    /// sim extraction must give bit-identical positions, indices, substances and composed poses;
    /// extracting the view must agree too, since the sim artifact is its byte-strip (ADR-0035).
    /// A moved vertex would expose a geometry change hidden in packaging.
    #[test]
    fn the_sim_artifact_walks_the_bytes_the_view_glb_carried() {
        let root = crate::assets::asset_root();
        let registry = SubstanceRegistry::shipped();
        let spec: TankSpec = crate::provenance::from_shipped_str(
            crate::assets::tank_file!(".tank.ron"),
            TANK_SPEC_RON,
        )
        .expect("the shipped sheet parses");
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
                        // Reparenting moves descendants; compare names so list reordering stays legal.
                        node.parent.map(|p| geometry.nodes[p].name.as_str()),
                        // Spawn uses local transforms; equal composed poses can hide different chains.
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
                                .map(|s| (s.name.as_str(), s.matter)),
                        );
                    }
                    row
                })
                .collect()
        };

        let sim = root.join(crate::tank::TANK_SIM_GLB_PATH);
        let view = root.join(crate::tank::TANK_GLB_PATH);
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

    /// Discover every shipped asset through the real gate; a synthetic origin-axle vehicle shares
    /// the assertions so they cannot impose a coordinate-origin restriction on future construction.
    #[test]
    fn every_shipped_asset_passes_the_consumer_contract() {
        let mut shipped = fixture::shipped_assets();
        assert!(
            !shipped.is_empty(),
            "no `assets/<id>/<id>.glb` beside an `<id>.tank.ron` was discovered at all"
        );
        for model in &shipped {
            let findings = verify_asset(model, None);
            assert!(
                findings.is_empty(),
                "{}: {}",
                model.display(),
                render(&findings)
            );
        }
        // Fixture: authored station frames may be at the model origin; mesh shape cannot establish
        // axle alignment. Certification already checks every declared rigid frame's unit scale.
        let (nodes, spec) = sound_vehicle();
        let at_origin = fixture::write("origin-axles", &nodes, &spec);
        let asset = certify_asset(&at_origin.glb, &spec, &SubstanceRegistry::shipped())
            .expect("the origin-axle fixture certifies");
        assert!(asset.geometry.roadwheels.iter().all(|&(index, _)| {
            asset.geometry.nodes[index].root_position.to_array() == [0.0; 3]
        }));
        shipped.push(at_origin.glb.clone());
        for model in shipped {
            let spec_ron =
                std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
            let asset = certify_asset(&model, &spec_ron, &SubstanceRegistry::shipped())
                .expect("the pair certifies");
            let geometry = &asset.geometry;

            // Declared volumes carry usable armour; spawn follows name order, not glTF node order.
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
                        match substance.matter {
                            crate::substances::Matter::Hard { factor, .. } => factor > 0.0,
                            crate::substances::Matter::Witness => true,
                        },
                        "`{}` wears `{}`, a hard substance whose factor is not positive",
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
        }
    }

    /// The bake captures exactly the declared armour, over every shipped asset. Invariant: a node
    /// holds substance primitives iff the sheet's `volumes` keys it, and `ballistic_volumes` is
    /// that set. A mutant that captures a registry-material node the sheet does not key, or drops a
    /// keyed one from the walk, fails here; the march and the spawn rely on the two agreeing.
    #[test]
    fn the_bake_captures_exactly_the_declared_armour() {
        let registry = SubstanceRegistry::shipped();
        for model in fixture::shipped_assets() {
            let spec_ron =
                std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
            let asset = certify_asset(&model, &spec_ron, &registry).expect("the pair certifies");
            let geometry = &asset.geometry;
            let declared: BTreeSet<&str> = asset.spec.volumes.keys().map(String::as_str).collect();
            let volumes: BTreeSet<&str> = geometry
                .ballistic_volumes
                .iter()
                .map(|&index| geometry.nodes[index].name.as_str())
                .collect();
            let captured: BTreeSet<&str> = geometry
                .nodes
                .iter()
                .filter(|node| node.is_ballistic())
                .map(|node| node.name.as_str())
                .collect();
            assert_eq!(
                volumes,
                declared,
                "{}: volumes differ from the sheet",
                model.display()
            );
            assert_eq!(
                captured,
                declared,
                "{}: substance primitives captured on a node the sheet does not declare, or none \
                 on one it does",
                model.display()
            );
        }
    }

    /// THE CLASS "TANK", over every shipped asset: some node wears a registry substance, the
    /// structural singletons the assembler resolves by name exist, and no declared collision proxy
    /// is armour — asserted here rather than at spawn, where a bad asset would only surface as a
    /// panic. A volume's consequences are its declared facets, never its material: the sheet's
    /// gates must close over the volumes it declares (`TankSpec::validate`), and a witness without
    /// a crew facet is refused at load (`L2.WITNESS_ROLE`).
    ///
    /// The loop is discovery: nothing here is one model's count, and a second tank adds no line.
    #[test]
    fn every_shipped_asset_is_a_tank_the_assembler_can_build() {
        for model in fixture::shipped_assets() {
            let spec_ron =
                std::fs::read_to_string(spec_ron_name(&model)).expect("the sheet is readable");
            let asset = certify_asset(&model, &spec_ron, &SubstanceRegistry::shipped())
                .expect("the pair certifies");
            let geometry = &asset.geometry;
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
        }
    }

    /// L2.SPEC — the sheet parses, validates, and every typed reference it makes resolves.
    #[test]
    fn an_unreadable_sheet_or_an_unresolved_reference_is_refused() {
        let (nodes, _) = sound_vehicle();
        let findings = verify("unparsable", &nodes, "TankSpec(mass: ");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));

        // Parses, but declares a vehicle that cannot drive: no station on one side.
        let refused = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left")],
            &[],
            &["Station_L", "Station_R"],
        );
        let findings = verify("half-tracked", &nodes, &refused);
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));

        // Parses and validates, and names a node the model does not hold.
        let absent = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Absent", "Right")],
            &[],
            &["Station_L", "Station_R"],
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
        assert_eq!(
            refusals(&verify_asset_in_policy(&asset.glb, None)),
            ["L2.SPEC"]
        );
    }

    /// The game's damage model: a tank file below the crew floor refuses at the asset door,
    /// naming the file, the crew field and its repair before spawn can turn it into a loss.
    #[test]
    fn the_asset_door_refuses_a_file_below_the_crew_floor() {
        for count in [0, 1] {
            let (nodes, sheet) = sound_vehicle();
            let mut sheet = sheet.replace("\"Fixture_SeatB\": (crew: BowGunner)", "");
            if count == 0 {
                sheet = sheet.replace("\"Fixture_SeatA\": (crew: Commander)", "");
            }
            // A removed map entry leaves its comma; an empty entry is not RON.
            sheet = sheet.replace(", ,", ",").replace(", }", " }");
            let asset = fixture::write(&format!("crew-floor-{count}"), &nodes, &sheet);
            let findings = verify_asset_in_policy(&asset.glb, None);
            let report = render(&findings);
            assert_eq!(refusals(&findings), ["L2.SPEC"], "{report}");
            assert!(
                report.contains(".tank.ron")
                    && report.contains("volumes.*.crew")
                    && report.contains("at least two")
                    && report.contains("add"),
                "{report}"
            );
        }
    }

    /// Pin Blender's canon document and both lists: Python owns no RON-field or substance vocabulary.
    #[test]
    fn the_canon_file_carries_the_reference_list_and_the_registry_keys() {
        let (nodes, _) = sound_vehicle();
        // EVERY role, and both of a weapon's two node fields: the RON path is what the Blender
        // pass prints as the line to go to, and a role does not determine one.
        let declared = fixture::spec_with(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &["Plate"],
            &[],
            r#"servos: {"Turret_Yaw": (role: Yaw, drive: Hand(hand_rate: (1.0, Fixture("fixture hand rate"))), travel: Continuous,
        requires: []), "Sight": (role: Pitch, drive: Hand(hand_rate: (1.0, Fixture("fixture pitch rate"))), travel: Continuous, requires: [])},
    weapons: {"Main": (
        trigger: Primary,
        muzzle: "Muzzle",
        bore_length: (1.0, Fixture("fixture construction")),
        barrel: "Recoil",
        gun: "kwk36_l56",
        stowage: [(round: "88mm_AP", count: (1, Fixture("fixture")))],
        fire_mode: Single(reload_secs: 1.0),
        recoil: (kick: 1.0, stiffness: 1.0, damping: 1.0, max_length: (1.0, Fixture("fixture"))),
        fire: [],
        load: [],
    )},
    views: {Gunner: (node: "Sight", optics: Magnified(magnification: 2.0, field_deg: 30.0),
        requires: []), Commander: (node: "Sight", optics: Unaided(60.0), requires: [])},"#,
        );
        let declared = declared.replace("gunner_yaw: None", "gunner_yaw: Some(\"Turret_Yaw\")");
        let asset = fixture::write("canon", &nodes, &declared);
        let json = canon_lists_in_policy(Path::new(&spec_ron_name(&asset.glb)), None)
            .expect("the sheet parses");
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
        // Preserve `TankSpec::node_references` order: role, node, authored path, as Python renders it.
        assert_eq!(
            references,
            [
                ("servos", "Sight"),
                ("gunner_yaw", "Turret_Yaw"),
                ("servos", "Turret_Yaw"),
                ("volumes", "Fixture_Engine"),
                ("volumes", "Fixture_SeatA"),
                ("volumes", "Fixture_SeatB"),
                ("volumes", "Fixture_Transmission"),
                ("volumes", "Plate"),
                ("colliders", "Proxy"),
                ("roadwheels[0].node", "Station_L"),
                ("roadwheels[1].node", "Station_R"),
                ("weapons[\"Main\"].muzzle", "Muzzle"),
                ("weapons[\"Main\"].barrel", "Recoil"),
                ("views[Commander].node", "Sight"),
                ("views[Gunner].node", "Sight"),
                ("turret.node", "Hull"),
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

        assert_eq!(document["spec_validates"], serde_json::json!(true));

        // A sheet that does not parse is the consumer contract's own refusal, not a second one.
        let broken = fixture::write("canon-unparsable", &nodes, "TankSpec(mass: ");
        let findings = canon_lists_in_policy(Path::new(&spec_ron_name(&broken.glb)), None)
            .expect_err("an unparsable sheet has no canon");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
    }

    /// The canon document is the build's projection of the sheet: a field the build does not read
    /// leaves it unchanged, and a renamed node reference or a lost `validate` verdict moves it.
    #[test]
    fn the_canon_file_moves_exactly_when_what_the_build_reads_does() {
        let (nodes, spec) = sound_vehicle();
        let canon = |id: &str, sheet: &str| {
            let asset = fixture::write(id, &nodes, sheet);
            canon_lists_in_policy(Path::new(&spec_ron_name(&asset.glb)), None)
                .expect("the sheet parses")
        };
        let base = canon("projection-base", &spec);
        let renamed = spec.replace("display_name: \"Fixture\"", "display_name: \"Renamed\"");
        assert_ne!(renamed, spec, "the fixture sheet carries a display name");
        assert_eq!(
            canon("projection-name", &renamed),
            base,
            "a gameplay-only field is not read by the build"
        );
        assert_ne!(
            canon(
                "projection-node",
                &spec.replace("\"Proxy\"", "\"Proxy_Renamed\"")
            ),
            base,
            "a renamed node reference is read by the build"
        );
        let invalid = fixture::spec(
            &[],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &[],
            &[],
        );
        let document: serde_json::Value =
            serde_json::from_str(&canon("projection-invalid", &invalid)).expect("one document");
        assert_eq!(document["spec_validates"], serde_json::json!(false));
    }

    /// Extraction validates the sheet itself. Invariant: no caller reaches capture with an
    /// unvalidated sheet, so a repeated declaration is refused as `L2.SPEC` and never yields two
    /// volume indices for one node; `damage_cam::shapes::TankAsset::load` relies on this, since it
    /// parses and extracts without the certification door.
    #[test]
    fn extraction_refuses_a_sheet_that_does_not_validate() {
        let (nodes, spec) = sound_vehicle();
        let spec = spec.replacen(
            "colliders: [(node: \"Proxy\", substance: \"RHA\")]",
            "colliders: [(node: \"Proxy\", substance: \"RHA\"), (node: \"Proxy\", substance: \"RHA\")]",
            1,
        );
        let asset = fixture::write("unvalidated-extraction", &nodes, &spec);
        let parsed: TankSpec = ron::de::from_str(&spec).expect("it parses");
        let findings = extract_tank_geometry(&asset.glb, &parsed, &SubstanceRegistry::shipped())
            .err()
            .expect("an invalid sheet is refused at extraction");
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
    }

    /// L2.SPEC — A COLLIDER'S SUBSTANCE IS CHECKED AT LOAD: every collision proxy's hulls carry
    /// their substance's friction (`tank::proxy_friction`), so a sheet naming a substance the
    /// registry cannot give a friction for is refused at the door and the runtime bake, naming the
    /// node, not at the first spawn of that tank on a server. Mutant: dropping
    /// `collider_substances` certifies this sheet. A row that states no friction takes the same
    /// path (`SubstanceRegistry::contact_friction`'s own test).
    #[test]
    fn a_collider_whose_substance_states_no_friction_is_refused_at_load() {
        let (nodes, spec) = sound_vehicle();
        let spec = spec.replacen(
            "colliders: [(node: \"Proxy\", substance: \"RHA\")]",
            "colliders: [(node: \"Proxy\", substance: \"Unobtainium\")]",
            1,
        );
        let findings = verify("collider-substance", &nodes, &spec);
        assert_eq!(refusals(&findings), ["L2.SPEC"], "{}", render(&findings));
        assert_eq!(findings[0].subject.name, "Proxy", "{}", render(&findings));
    }

    /// The game's damage model admits a bare station frame carrying separately declared solids.
    /// Bake consumers rely on authored hierarchy, not a requirement that the frame own the mesh.
    #[test]
    fn a_station_can_carry_its_declared_solids_on_children() {
        let (mut nodes, spec) = sound_vehicle();
        nodes[3] = Node::new("Station_L").child_of("Hull");
        nodes.push(
            Node::new("DeclaredSolid")
                .child_of("Station_L")
                .holding(solid(SUBSTANCE, [3.0, 0.0, 0.0])),
        );
        let spec = spec.replace(
            "\"Station_L\": (main: true)",
            "\"DeclaredSolid\": (main: true)",
        );
        let findings = verify("station-child-solid", &nodes, &spec);
        assert!(!has_error(&findings), "{}", render(&findings));

        nodes.last_mut().unwrap().parent = Some("Hull".into());
        let findings = verify("station-unrelated-solid", &nodes, &spec);
        assert!(
            findings.iter().any(|finding| {
                finding.check.id == "L2.ROLE_COHERENCE" && finding.subject.name == "Station_L"
            }),
            "unrelated hard matter cannot supply the station's construction: {}",
            render(&findings)
        );
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

        // A collision proxy declared armour would be charged twice by the march.
        let (mut nodes, spec) = sound_vehicle();
        nodes[2] = Node::new("Proxy")
            .child_of("Hull")
            .holding(solid(SUBSTANCE, [0.0, 3.0, 0.0]));
        let findings = verify("armoured-proxy", &nodes, &declaring(&spec, "Proxy"));
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

        // A proxy the spawn cannot build its hulls from: a triangle past the vertices, and a part
        // whose points are one point (no hull). Both are refused here, so `spawn_proxy_parts`
        // never meets them.
        for (case, positions, indices) in [
            (
                "proxy-past-vertices",
                solid(ART, [0.0, 0.0, 0.0]).positions,
                vec![0, 1, 9],
            ),
            ("proxy-no-hull", vec![[0.0, 0.0, 0.0]; 3], vec![0, 1, 2]),
        ] {
            let (mut nodes, spec) = sound_vehicle();
            nodes[2] = Node::new("Proxy").child_of("Hull").holding(Primitive {
                material: ART.into(),
                positions,
                indices,
            });
            let findings = verify(case, &nodes, &spec);
            assert_eq!(
                refusals(&findings),
                ["L2.ROLE_COHERENCE"],
                "{case}: {}",
                render(&findings)
            );
        }

        // A component facet on a node the march never meets is inert.
        let (nodes, _) = sound_vehicle();
        let facets = fixture::spec(
            &["Proxy"],
            &[("Station_L", "Left"), ("Station_R", "Right")],
            &["Proxy"],
            &["Station_L", "Station_R"],
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

    /// L2.WITNESS_ROLE — crew and witness substances name one class: each without the other is
    /// refused by node, and the pair together certifies.
    #[test]
    fn crew_and_witness_substance_come_together_or_not_at_all() {
        let registry = SubstanceRegistry::shipped();
        let witness = registry
            .keys()
            .into_iter()
            .find(|name| registry.get(name) == Ok(crate::substances::Substance::Witness))
            .expect("the shipped registry declares a witness substance");
        let seat = |material: &str| {
            Node::new("Seat")
                .child_of("Hull")
                .holding(solid(material, [0.0, 3.0, 0.0]))
        };
        let wheels = [("Station_L", "Left"), ("Station_R", "Right")];
        let declared = |entry: &str| {
            fixture::spec(&["Proxy"], &wheels, &[], &["Station_L", "Station_R"]).replacen(
                "volumes: {",
                &format!("volumes: {{\"Seat\": {entry}, "),
                1,
            )
        };
        let crew = declared("(crew: Driver)");
        let with = |extra: Node| {
            let (mut nodes, _) = sound_vehicle();
            nodes.push(extra);
            nodes
        };

        let findings = verify("witness-crew", &with(seat(witness)), &crew);
        assert!(findings.is_empty(), "{}", render(&findings));

        let cases = [
            ("hard-crew", seat(SUBSTANCE), crew.clone()),
            (
                "mixed-crew",
                seat(witness).holding(solid(SUBSTANCE, [0.0, 3.0, 0.0])),
                crew,
            ),
            ("witness-module", seat(witness), declared("(hp: 10)")),
            (
                "witness-plate",
                seat(witness),
                fixture::spec(
                    &["Proxy"],
                    &wheels,
                    &[],
                    &["Station_L", "Station_R", "Seat"],
                ),
            ),
        ];
        for (id, node, spec) in cases {
            let findings = verify(id, &with(node), &spec);
            assert_eq!(
                refusals(&findings),
                ["L2.WITNESS_ROLE"],
                "{id}: {}",
                render(&findings)
            );
            assert_eq!(findings[0].subject.name, "Seat", "{id}");
        }
    }

    /// L2.ROLE_COHERENCE — a primitive wearing an unworn substance's key (the shipped burst core)
    /// declares a volume the walk has nothing to read for, so the capture refuses it by node. A
    /// mutant that reads it as art (a plate that silently stops being armour) or hands it the walk
    /// as matter of any factor certifies the vehicle and fails here.
    #[test]
    fn a_volume_wearing_an_unworn_substance_is_refused() {
        let registry = SubstanceRegistry::shipped();
        let core = registry
            .keys()
            .into_iter()
            .find(|name| registry.get(name).is_ok_and(|s| s.matter().is_none()))
            .expect("the shipped registry declares an unworn substance");
        let (mut nodes, spec) = sound_vehicle();
        nodes.push(
            Node::new("Shield")
                .child_of("Hull")
                .holding(solid(core, [0.0, 3.0, 0.0])),
        );
        let findings = verify("burst-core-volume", &nodes, &declaring(&spec, "Shield"));
        assert_eq!(
            refusals(&findings),
            ["L2.ROLE_COHERENCE"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings[0].subject.name, "Shield");
    }

    /// Armour membership is the sheet's declaration, never a material. Invariant: an undeclared
    /// node wearing a registry substance is view geometry (no volume, no captured primitive, no
    /// finding), and a declared node wearing a non-registry material is refused by name. A mutant
    /// that reads membership off the material makes the first half fail; one that accepts a
    /// declared node of art makes the second fail. The shadow-proxy and collision-proxy checks
    /// and the spawn consumers read the same set.
    #[test]
    fn armour_is_what_the_sheet_declares_not_what_a_material_implies() {
        let (mut nodes, spec) = sound_vehicle();
        nodes.push(
            Node::new("Decor")
                .child_of("Hull")
                .holding(solid(SUBSTANCE, [0.0, 3.0, 0.0])),
        );
        let asset = fixture::write("undeclared-decor", &nodes, &spec);
        let certified = certify_asset(&asset.glb, &spec, &SubstanceRegistry::shipped())
            .expect("an undeclared registry-material node certifies");
        let geometry = &certified.geometry;
        assert!(!geometry.is_ballistic("Decor"), "undeclared is no volume");
        let decor = &geometry.nodes[geometry.by_name["Decor"]];
        assert!(
            decor.primitives.is_empty(),
            "an undeclared node's substance primitives are not captured"
        );
        assert!(geometry.is_ballistic("Plate"), "declared is a volume");

        let (mut nodes, spec) = sound_vehicle();
        nodes[1] = Node::new("Plate")
            .child_of("Hull")
            .holding(solid(ART, [0.0, 0.0, 0.0]));
        let findings = verify("declared-art", &nodes, &spec);
        assert_eq!(
            refusals(&findings),
            ["L2.ROLE_COHERENCE"],
            "{}",
            render(&findings)
        );
        assert_eq!(findings[0].subject.name, "Plate");
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
            vec!["L2.UNIT_SCALE"; 1 + ron::de::from_str::<TankSpec>(&spec).unwrap().volumes.len()],
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
    /// Geometry passes at `1` and fails at other scales, including one ULP away; no compensation
    /// or tolerance. The message names the node so the artist can fix the object in the .blend.
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
        let mut indices = plate.indices;
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
            .as_chunks::<3>()
            .0
            .iter()
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

    /// Same-primitive face contact is refused (§13.7): shells sharing a welded edge present four
    /// triangles there, failing the directed-edge census before union-find. Guessing shell boundaries
    /// would make pairing, restart seeding and contact ownership ambiguous. Face-to-face plates
    /// must be separate primitives; vertex contact and interior intersection remain legal within one.
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

    /// L2.SHELL_EMBEDDING must refuse self-intersection through the whole asset contract.
    /// `fixture::pierced` is closed, consistently wound and positive-volume, isolating embedding
    /// refusal. Unlike `embedding::tests`' triangle lists, this GLB also proves asset-level wiring.
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

    /// Welding must ignore glTF's normal/UV vertex splits: index-level edge tests would falsely
    /// reject every seam of a flat-shaded plate.
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

    /// §13.4 pairing needs one key per edge-connected shell. Legal shells may touch at a vertex
    /// (§13.7's islands); vertex connectivity would ambiguously merge two surfaces into one key.
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

    /// Reports sort by stage, severity, check id, subject, element, making repeated runs diffable.
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

    /// An axis-aligned box `[lo, hi]` as a flat-shaded glTF primitive does: 24 vertices, four per
    /// face, each face its own quad, so no two faces share a vertex index.
    fn flat_box(lo: Vec3, hi: Vec3) -> (Vec<[f32; 3]>, Vec<u32>) {
        let c = |x: bool, y: bool, z: bool| {
            [
                if x { hi.x } else { lo.x },
                if y { hi.y } else { lo.y },
                if z { hi.z } else { lo.z },
            ]
        };
        let faces = [
            [
                c(false, false, false),
                c(false, true, false),
                c(false, true, true),
                c(false, false, true),
            ],
            [
                c(true, false, false),
                c(true, false, true),
                c(true, true, true),
                c(true, true, false),
            ],
            [
                c(false, false, false),
                c(false, false, true),
                c(true, false, true),
                c(true, false, false),
            ],
            [
                c(false, true, false),
                c(true, true, false),
                c(true, true, true),
                c(false, true, true),
            ],
            [
                c(false, false, false),
                c(true, false, false),
                c(true, true, false),
                c(false, true, false),
            ],
            [
                c(false, false, true),
                c(false, true, true),
                c(true, true, true),
                c(true, false, true),
            ],
        ];
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for face in faces {
            let base = positions.len() as u32;
            positions.extend(face);
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        (positions, indices)
    }

    fn proxy(boxes: &[(Vec3, Vec3)]) -> MeshGeometry {
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for &(lo, hi) in boxes {
            let (p, i) = flat_box(lo, hi);
            let base = positions.len() as u32;
            positions.extend(p);
            indices.extend(i.into_iter().map(|index| index + base));
        }
        MeshGeometry {
            positions,
            indices,
            substance: None,
            certificate: None,
            colliders: ColliderCache::default(),
        }
    }

    /// Invariant: a collision proxy is one hull per loose part, so the collision shape of a part
    /// pair keeps the space between them (the notch a single hull would fill). Relied on by the
    /// tank body's collision proxies, which author a notched silhouette as two convex parts.
    #[test]
    fn two_disjoint_boxes_make_two_hulls_and_the_notch_is_outside_both() {
        let primitive = proxy(&[
            (Vec3::ZERO, Vec3::ONE),
            (Vec3::new(2.0, 0.0, 0.0), Vec3::new(3.0, 1.0, 1.0)),
        ]);
        let hulls = primitive.convex_hull_colliders().expect("two valid parts");
        assert_eq!(hulls.len(), 2);
        let notch = Vec3::new(1.5, 0.5, 0.5);
        assert!(
            hulls
                .iter()
                .all(|hull| !hull.contains_point(Vec3::ZERO, Quat::IDENTITY, notch))
        );
        assert!(hulls[0].contains_point(Vec3::ZERO, Quat::IDENTITY, Vec3::splat(0.5)));
        assert!(hulls[1].contains_point(Vec3::ZERO, Quat::IDENTITY, Vec3::new(2.5, 0.5, 0.5)));
    }

    /// Invariant: connectivity is by welded position, not by index. glTF splits a vertex wherever
    /// the normal differs, so a flat-shaded box is six index-disconnected quads and must still be
    /// exactly one part.
    #[test]
    fn one_flat_shaded_box_is_one_hull() {
        let primitive = proxy(&[(Vec3::ZERO, Vec3::ONE)]);
        assert_eq!(primitive.positions.len(), 24);
        assert_eq!(primitive.convex_hull_colliders().expect("a box").len(), 1);
    }

    /// Invariant: a primitive builds each of its colliders once, and every later call shares that
    /// shape; another primitive's shape is its own. Relied on by every tank spawn after the first
    /// (the server's respawns), which builds no trimesh and no hull. A cache that rebuilt on each
    /// call, or handed one primitive's shape to another, fails here.
    #[test]
    fn a_primitive_builds_its_colliders_once() {
        use avian3d::prelude::SimpleCollider;
        let unit = proxy(&[(Vec3::ZERO, Vec3::ONE)]);
        let double = proxy(&[(Vec3::ZERO, Vec3::splat(2.0))]);
        let shared = |a: &Collider, b: &Collider| Arc::ptr_eq(&a.shape().0, &b.shape().0);
        let hull = |p: &MeshGeometry| p.convex_hull_colliders().expect("a box").remove(0);
        assert!(shared(&hull(&unit), &hull(&unit)));
        assert!(shared(&unit.trimesh_collider(), &unit.trimesh_collider()));
        let top = |c: &Collider| c.aabb(Vec3::ZERO, Quat::IDENTITY).max;
        assert_eq!(top(&hull(&unit)), Vec3::ONE);
        assert_eq!(top(&hull(&double)), Vec3::splat(2.0));
        assert_eq!(top(&double.trimesh_collider()), Vec3::splat(2.0));
    }
}
