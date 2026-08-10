//! A synthetic vehicle, written to disk as the trio the door consumes: a `.glb` and its sibling
//! `.tank.ron`.
//!
//! Every L2 law is tested against geometry authored here rather than against a shipped model. A
//! shipped model can only ever witness the PASSING side of a law — its value is entirely in what it
//! refuses — and a test that pins a real vehicle's counts makes the second vehicle a gate edit.

use std::path::{Path, PathBuf};

/// A node of the synthetic model: its name, its parent, its local scale, and the mesh it holds.
pub(super) struct Node {
    pub name: String,
    pub parent: Option<String>,
    pub scale: [f32; 3],
    pub mesh: Vec<Primitive>,
}

/// One mesh primitive: the material name that declares (or does not declare) membership, the
/// positions, and the index buffer exactly as authored — including buffers no exporter would write,
/// which is the point.
pub(super) struct Primitive {
    pub material: String,
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

/// The unit tetrahedron's outward-wound index buffer.
pub(super) const CLOSED: [u32; 12] = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];

/// A closed, outward-wound, positive-volume solid at `origin` — the smallest thing that passes.
pub(super) fn solid(material: &str, origin: [f32; 3]) -> Primitive {
    let corner = |x: f32, y: f32, z: f32| [origin[0] + x, origin[1] + y, origin[2] + z];
    Primitive {
        material: material.to_owned(),
        positions: vec![
            corner(0.0, 0.0, 0.0),
            corner(1.0, 0.0, 0.0),
            corner(0.0, 1.0, 0.0),
            corner(0.0, 0.0, 1.0),
        ],
        indices: CLOSED.to_vec(),
    }
}

/// The unit box's outward-wound index buffer, over [`BOX_CORNERS`].
const BOX: [u32; 36] = [
    0, 3, 2, 0, 2, 1, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0, 7, 3, 1, 2,
    6, 1, 6, 5,
];

const BOX_CORNERS: [[f32; 3]; 8] = [
    [0.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
    [1.0, 1.0, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, 0.0, 1.0],
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 1.0, 1.0],
];

/// The unit box at `origin`, with the corner at index 6 wherever the caller puts it.
fn box_at(material: &str, origin: [f32; 3], sixth: [f32; 3]) -> Primitive {
    Primitive {
        material: material.to_owned(),
        positions: BOX_CORNERS
            .iter()
            .enumerate()
            .map(|(corner, position)| {
                let position = if corner == 6 { sixth } else { *position };
                [
                    origin[0] + position[0],
                    origin[1] + position[1],
                    origin[2] + position[2],
                ]
            })
            .collect(),
        indices: BOX.to_vec(),
    }
}

/// The unit box at `origin`: a second passing solid, and the control [`pierced`] is one corner away
/// from.
pub(super) fn boxed(material: &str, origin: [f32; 3]) -> Primitive {
    box_at(material, origin, BOX_CORNERS[6])
}

/// A closed, outward-wound, positive-volume solid that PASSES THROUGH ITSELF: [`boxed`] with the
/// corner at `(1,1,1)` dragged out through the opposite `x = 0` wall to `(-0.5, 0.5, 0.5)`.
///
/// Every earlier gate still holds — no corner is repeated and no face is flat, each directed welded
/// edge occurs once with its reverse once, and the one shell encloses `+1/6 m³` — so the embedding
/// certificate is the only law left to refuse it. The three faces the moved corner belongs to now
/// cross the `x = 0` wall: four of the six offending pairs share no welded corner at all, and two
/// share one and meet far past it.
pub(super) fn pierced(material: &str, origin: [f32; 3]) -> Primitive {
    box_at(material, origin, [-0.5, 0.5, 0.5])
}

impl Node {
    pub(super) fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            parent: None,
            scale: [1.0; 3],
            mesh: Vec::new(),
        }
    }

    pub(super) fn child_of(mut self, parent: &str) -> Self {
        self.parent = Some(parent.to_owned());
        self
    }

    pub(super) fn scaled(mut self, scale: [f32; 3]) -> Self {
        self.scale = scale;
        self
    }

    pub(super) fn holding(mut self, primitive: Primitive) -> Self {
        self.mesh.push(primitive);
        self
    }
}

/// A written asset trio, removed when the test that built it ends.
pub(super) struct Asset {
    directory: PathBuf,
    pub glb: PathBuf,
}

impl Drop for Asset {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// Write one synthetic asset: the nodes as a glb, and `spec` as its sibling sheet.
///
/// `id` names the directory and both files, so the door's mechanical `<id>.glb` →
/// `<id>.tank.ron` sibling rule is what the tests exercise.
pub(super) fn write(id: &str, nodes: &[Node], spec: &str) -> Asset {
    static WRITTEN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let directory = std::env::temp_dir().join(format!(
        "overmatch-asset-{}-{}-{id}",
        std::process::id(),
        WRITTEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).expect("the fixture directory is writable");
    let glb = directory.join(format!("{id}.glb"));
    std::fs::write(directory.join(format!("{id}.tank.ron")), spec).expect("the sheet is writable");
    std::fs::write(&glb, glb_bytes(nodes)).expect("the model is writable");
    Asset { directory, glb }
}

/// The spec sheet of a vehicle whose only declared structure is what a test needs: the domain
/// values are the smallest ones `TankSpec::validate` accepts, so nothing here is a vehicle datum
/// anybody could mistake for balance.
pub(super) fn spec(colliders: &[&str], roadwheels: &[(&str, &str)], volumes: &[&str]) -> String {
    spec_with(colliders, roadwheels, volumes, "servos: {},")
}

/// The same sheet with `declarations` standing in for the empty `servos` block — where a test that
/// needs the roles a bare vehicle does not declare (servos, weapons, views) authors them.
pub(super) fn spec_with(
    colliders: &[&str],
    roadwheels: &[(&str, &str)],
    volumes: &[&str],
    declarations: &str,
) -> String {
    let colliders = colliders
        .iter()
        .map(|node| format!("\"{node}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let roadwheels = roadwheels
        .iter()
        .map(|(node, side)| format!("(node: \"{node}\", side: {side})"))
        .collect::<Vec<_>>()
        .join(", ");
    let volumes = volumes
        .iter()
        .map(|node| format!("\"{node}\": (hp: 1.0)"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "#![enable(implicit_some)]
TankSpec(
    mass: 1.0,
    inertia_extents: (1.0, 1.0, 1.0),
    track: (
        link_count: 3,
        link_mass: 1.0,
        hinge_torque: 1.0,
        link_angle: (inward_deg: 40.0, outward_deg: 18.0),
        sprocket: (teeth: 1),
        powertrain: (
            max_speed: 1.0,
            power: 1.0,
            force: 1.0,
            governor_gain: 1.0,
            inertia: 1.0,
            transmission: (architecture: Governor),
        ),
        suspension: (
            ride_frequency: 1.0,
            damping_ratio: 1.0,
            bump_stop: 1.0,
            engage: 1.0,
        ),
    ),
    {declarations}
    volumes: {{{volumes}}},
    colliders: [{colliders}],
    roadwheels: [{roadwheels}],
)
"
    )
}

/// Serialize the nodes as a binary glTF: one buffer holding every accessor, one mesh per node that
/// has primitives, one material per distinct material name.
fn glb_bytes(nodes: &[Node]) -> Vec<u8> {
    let mut binary: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut materials: Vec<String> = Vec::new();
    let mut meshes = Vec::new();
    let mut json_nodes = Vec::new();

    for node in nodes {
        let mut primitives = Vec::new();
        for primitive in &node.mesh {
            let material = materials
                .iter()
                .position(|name| *name == primitive.material)
                .unwrap_or_else(|| {
                    materials.push(primitive.material.clone());
                    materials.len() - 1
                });
            // No positions at all means no POSITION accessor — the primitive the consumer contract
            // must refuse, not a zero-length accessor no exporter can write.
            let mut attributes = serde_json::Map::new();
            if !primitive.positions.is_empty() {
                attributes.insert("POSITION".to_string(), serde_json::json!(accessors.len()));
                let (offset, length) = (binary.len(), primitive.positions.len() * 12);
                for position in &primitive.positions {
                    for coordinate in position {
                        binary.extend_from_slice(&coordinate.to_le_bytes());
                    }
                }
                let bounds = |slot: usize, pick: fn(f32, f32) -> f32| {
                    primitive
                        .positions
                        .iter()
                        .map(|position| position[slot])
                        .fold(f32::NAN, pick)
                };
                views.push(
                    serde_json::json!({ "buffer": 0, "byteOffset": offset, "byteLength": length }),
                );
                accessors.push(serde_json::json!({
                    "bufferView": views.len() - 1,
                    "componentType": 5126,
                    "count": primitive.positions.len(),
                    "type": "VEC3",
                    "min": [bounds(0, f32::min), bounds(1, f32::min), bounds(2, f32::min)],
                    "max": [bounds(0, f32::max), bounds(1, f32::max), bounds(2, f32::max)],
                }));
            }
            let mut json = serde_json::json!({
                "attributes": attributes,
                "material": material,
            });
            if !primitive.indices.is_empty() {
                let (offset, length) = (binary.len(), primitive.indices.len() * 4);
                for index in &primitive.indices {
                    binary.extend_from_slice(&index.to_le_bytes());
                }
                views.push(
                    serde_json::json!({ "buffer": 0, "byteOffset": offset, "byteLength": length }),
                );
                accessors.push(serde_json::json!({
                    "bufferView": views.len() - 1,
                    "componentType": 5125,
                    "count": primitive.indices.len(),
                    "type": "SCALAR",
                }));
                json["indices"] = serde_json::json!(accessors.len() - 1);
            }
            primitives.push(json);
        }
        let mesh = (!primitives.is_empty()).then(|| {
            meshes.push(serde_json::json!({ "primitives": primitives }));
            meshes.len() - 1
        });
        let children: Vec<usize> = nodes
            .iter()
            .enumerate()
            .filter(|(_, child)| child.parent.as_deref() == Some(node.name.as_str()))
            .map(|(index, _)| index)
            .collect();
        let mut json = serde_json::json!({ "name": node.name, "scale": node.scale });
        if let Some(mesh) = mesh {
            json["mesh"] = serde_json::json!(mesh);
        }
        if !children.is_empty() {
            json["children"] = serde_json::json!(children);
        }
        json_nodes.push(json);
    }

    let roots: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.parent.is_none())
        .map(|(index, _)| index)
        .collect();
    let document = serde_json::json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": roots }],
        "nodes": json_nodes,
        "meshes": meshes,
        "materials": materials
            .iter()
            .map(|name| serde_json::json!({ "name": name }))
            .collect::<Vec<_>>(),
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{ "byteLength": binary.len() }],
    });
    container(
        &serde_json::to_vec(&document).expect("the document serializes"),
        &binary,
    )
}

/// The glb container: a 12-byte header and the JSON and BIN chunks, each padded to four bytes.
fn container(json: &[u8], binary: &[u8]) -> Vec<u8> {
    let pad = |chunk: &[u8], filler: u8| {
        let mut padded = chunk.to_vec();
        padded.resize(chunk.len().div_ceil(4) * 4, filler);
        padded
    };
    let (json, binary) = (pad(json, b' '), pad(binary, 0));
    let mut glb = Vec::with_capacity(28 + json.len() + binary.len());
    glb.extend_from_slice(&0x4654_6c67u32.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&((28 + json.len() + binary.len()) as u32).to_le_bytes());
    glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4e4f_534au32.to_le_bytes());
    glb.extend_from_slice(&json);
    glb.extend_from_slice(&(binary.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004e_4942u32.to_le_bytes());
    glb.extend_from_slice(&binary);
    glb
}

/// Every asset trio the repository ships: a directory under `assets/` holding `<id>.glb` beside
/// `<id>.tank.ron`. Discovery, never a list — adding a vehicle adds no line of test code.
pub(super) fn shipped_assets() -> Vec<PathBuf> {
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let mut models: Vec<PathBuf> = std::fs::read_dir(&assets)
        .expect("the repository ships an assets directory")
        .filter_map(|entry| {
            let directory = entry.ok()?.path();
            let id = directory.file_name()?.to_str()?.to_owned();
            let model = directory.join(format!("{id}.glb"));
            (model.is_file() && directory.join(format!("{id}.tank.ron")).is_file()).then_some(model)
        })
        .collect();
    models.sort();
    models
}
