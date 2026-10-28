//! A struck tank's component shapes, resolved by name from the tank asset: the record names a
//! component and this module answers with its real mesh.
//!
//! The source is the one the sim builds its trimesh colliders from: `bake`'s extraction of the
//! vehicle's sim artifact (`<id>.sim.glb`, beside `<id>.tank.ron`), whose ballistic primitives are
//! the component volumes, triangle for triangle. A shape is stated in the record's frame (the hull
//! node's, the rig contract's `Hull`), with every servo the tank file declares turned to the
//! record's angle for it: the node's local rotation becomes [`servo_pose`] of its authored rest,
//! exactly as the sim poses it, and every node under it follows. Generic over any tank file;
//! nothing here names a vehicle.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Mesh, PrimitiveTopology};
use bevy::prelude::*;

use avian3d::parry::query::{Ray, RayCast, RayCullingMode};
use avian3d::parry::shape::TriMesh;

use crate::bake::{TankBlueprint, TankGeometry, extract_tank_geometry};
use crate::spec::TankSpec;
use crate::substances::{Matter, SubstanceRegistry};
use crate::tank::{ServoRole, servo_pose};

/// The node every tank file's rig names its hull: the record's frame.
const HULL_NODE: &str = "Hull";

/// One component's real shape: its triangles in the record's frame.
#[derive(Clone, Debug)]
pub struct ComponentShape {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
}

impl ComponentShape {
    /// Its vertices' bounds in the record's frame, low and high corners.
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let (lo, hi) = self.positions.iter().fold(
            (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)),
            |(lo, hi), p| (lo.min(*p), hi.max(*p)),
        );
        if lo.is_finite() && hi.is_finite() {
            (lo, hi)
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        }
    }

    /// Its mean face (m²): the mean of the three axis-aligned face areas of its bounds
    /// ([`Self::bounds`]), so a face of the record's frame, which moves with its pose.
    pub fn mean_face(&self) -> f32 {
        let (lo, hi) = self.bounds();
        let e = hi - lo;
        (e.x * e.y + e.y * e.z + e.z * e.x) / 3.0
    }

    /// The centre of its vertices' bounds.
    pub fn center(&self) -> Vec3 {
        let (lo, hi) = self.bounds();
        0.5 * (lo + hi)
    }

    /// The shape as a render mesh, normals computed per face.
    pub fn mesh(&self) -> Mesh {
        let mut positions = Vec::with_capacity(self.indices.len());
        for index in &self.indices {
            positions.push(self.positions[*index as usize].to_array());
        }
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_computed_normals()
    }
}

/// A tank file and the geometry it names, read once.
pub struct TankAsset {
    geometry: Arc<TankGeometry>,
    spec: Arc<TankSpec>,
}

impl TankAsset {
    /// The tank file `vehicle` (relative to the asset root) and its sim artifact.
    pub fn load(vehicle: &str) -> Result<Self, String> {
        let root = crate::assets::asset_root();
        let path = root.join(vehicle);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec: TankSpec =
            ron::de::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let stem = vehicle.strip_suffix(".tank.ron").unwrap_or(vehicle);
        let glb = root.join(format!("{stem}.sim.glb"));
        let geometry = extract_tank_geometry(&glb, &spec, &SubstanceRegistry::shipped())
            .map_err(|findings| format!("{}: {} findings", glb.display(), findings.len()))?;
        Ok(Self {
            geometry: Arc::new(geometry),
            spec: Arc::new(spec),
        })
    }

    /// The tank the blueprint holds: its tank file and sim artifact, already read.
    pub(crate) fn of_blueprint(blueprint: &TankBlueprint) -> Self {
        Self {
            geometry: blueprint.geometry.clone(),
            spec: blueprint.spec.clone(),
        }
    }

    /// The extracted sim artifact.
    #[cfg(test)]
    pub(crate) fn geometry(&self) -> &TankGeometry {
        &self.geometry
    }

    /// The tank file.
    pub(crate) fn spec(&self) -> &TankSpec {
        &self.spec
    }

    /// Its crew stations' mean face (m², [`crew_face`]), whatever any servo's angle.
    pub fn crew_face(&self) -> Option<f32> {
        crew_face(&self.geometry, &self.spec)
    }

    /// The name a player reads for this vehicle.
    pub fn display_name(&self) -> &str {
        &self.spec.display_name
    }

    /// Every component's shape, each servo at its angle in `servo_deg` (degrees, by node name, a
    /// record's [`crate::damage_cam::PenetrationReport::servo_deg`]); a servo it does not name is at
    /// rest.
    pub fn shapes(&self, servo_deg: &BTreeMap<String, f32>) -> TankShapes {
        TankShapes::from_geometry(&self.geometry, &self.spec, servo_deg)
    }
}

/// Every component of a tank by name, posed for one record, the whole tank's bounds, and its hard
/// bodies.
#[derive(Clone, Debug, Default)]
pub struct TankShapes {
    by_name: HashMap<String, ComponentShape>,
    /// The low and high corners, in the record's frame, of every ballistic primitive of every node
    /// (armour and components alike) as posed; `None` when the geometry holds none.
    bounds: Option<(Vec3, Vec3)>,
    hard: Option<Arc<HardBodies>>,
    /// Its crew stations' mean face at rest ([`crew_face`]).
    crew_face: Option<f32>,
}

/// Every hard ballistic primitive of a tank, posed for one record, as one triangle mesh: the
/// bodies a field's direction is stopped by, at the first it enters
/// (`ballistics::resolve::field_direction`), which a witness never is. Each primitive is one body, a closed shell wound outward (the manifold
/// gate's certificate), so a line enters a body through a face it meets head-on
/// (`normal · direction < 0`) and leaves it through one it meets from behind. The mesh's bounding
/// volume hierarchy is built on the first cast, so shapes that are never cast against cost none.
pub struct HardBodies {
    vertices: Vec<Vec3>,
    triangles: Vec<[u32; 3]>,
    mesh: OnceLock<Option<TriMesh>>,
}

impl HardBodies {
    /// The bodies of `triangles` over `vertices` (record frame).
    pub fn new(vertices: Vec<Vec3>, triangles: Vec<[u32; 3]>) -> Self {
        Self {
            vertices,
            triangles,
            mesh: OnceLock::new(),
        }
    }

    fn mesh(&self) -> Option<&TriMesh> {
        self.mesh
            .get_or_init(|| TriMesh::new(self.vertices.clone(), self.triangles.clone()).ok())
            .as_ref()
    }

    /// Every body's vertices (record frame).
    pub fn vertices(&self) -> &[Vec3] {
        &self.vertices
    }

    /// Every body's triangles, wound outward, over [`Self::vertices`].
    pub fn triangles(&self) -> &[[u32; 3]] {
        &self.triangles
    }

    /// Where the line from `origin` along unit `direction` first meets a face within `reach` (m
    /// along it): of the faces `culling` keeps, every face for `None`.
    pub fn cast(
        &self,
        origin: Vec3,
        direction: Vec3,
        reach: f32,
        culling: Option<RayCullingMode>,
    ) -> Option<f32> {
        let mesh = self.mesh()?;
        let ray = Ray::new(origin, direction);
        match culling {
            Some(culling) => mesh
                .cast_local_ray_with_culling(&ray, reach, culling)
                .map(|hit| hit.time_of_impact),
            None => mesh.cast_local_ray(&ray, reach, false),
        }
    }
}

impl std::fmt::Debug for HardBodies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HardBodies")
            .field("triangles", &self.triangles.len())
            .finish_non_exhaustive()
    }
}

impl TankShapes {
    pub(crate) fn from_geometry(
        geometry: &TankGeometry,
        spec: &TankSpec,
        servo_deg: &BTreeMap<String, f32>,
    ) -> Self {
        let components = spec
            .volumes
            .iter()
            .filter(|(_, volume)| volume.component().is_some())
            .map(|(name, _)| name.as_str());
        let posed = Self::posed(geometry, components, |name| {
            let servo = spec.servos.get(name)?;
            let angle = servo_deg.get(name).copied().unwrap_or(0.0).to_radians();
            Some((servo.role(), angle))
        });
        Self {
            crew_face: crew_face(geometry, spec),
            ..posed
        }
    }

    /// The shapes of `components`, each node named by `servo` (its role and angle, radians) turned.
    fn posed<'a>(
        geometry: &TankGeometry,
        components: impl IntoIterator<Item = &'a str>,
        servo: impl Fn(&str) -> Option<(ServoRole, f32)>,
    ) -> Self {
        let poses = posed_nodes(geometry, servo);
        let hull_from_root = geometry
            .by_name
            .get(HULL_NODE)
            .map_or(Affine3A::IDENTITY, |&hull| poses[hull].inverse());
        let mut bounds: Option<(Vec3, Vec3)> = None;
        let (mut vertices, mut triangles) = (Vec::new(), Vec::new());
        for (pose, node) in poses.iter().zip(&geometry.nodes) {
            let to_record = hull_from_root * *pose;
            for primitive in &node.primitives {
                let Some(substance) = &primitive.substance else {
                    continue;
                };
                let posed: Vec<Vec3> = primitive
                    .positions
                    .iter()
                    .map(|p| to_record.transform_point3(Vec3::from(*p)))
                    .collect();
                for &p in &posed {
                    bounds = Some(bounds.map_or((p, p), |(lo, hi)| (lo.min(p), hi.max(p))));
                }
                if matches!(substance.matter, Matter::Witness) {
                    continue;
                }
                let base = vertices.len() as u32;
                vertices.extend(posed);
                for t in primitive.indices.as_chunks::<3>().0 {
                    triangles.push([base + t[0], base + t[1], base + t[2]]);
                }
            }
        }
        let hard = (!triangles.is_empty()).then(|| Arc::new(HardBodies::new(vertices, triangles)));
        let mut by_name = HashMap::new();
        for name in components {
            let Some(&index) = geometry.by_name.get(name) else {
                continue;
            };
            let to_record = hull_from_root * poses[index];
            let mut shape = ComponentShape {
                positions: Vec::new(),
                indices: Vec::new(),
            };
            for primitive in geometry.nodes[index]
                .primitives
                .iter()
                .filter(|primitive| primitive.substance.is_some())
            {
                let base = shape.positions.len() as u32;
                shape.positions.extend(
                    primitive
                        .positions
                        .iter()
                        .map(|p| to_record.transform_point3(Vec3::from(*p))),
                );
                shape
                    .indices
                    .extend(primitive.indices.iter().map(|i| base + i));
            }
            by_name.insert(name.to_owned(), shape);
        }
        Self {
            by_name,
            bounds,
            hard,
            crew_face: None,
        }
    }

    /// The tank's hard bodies as posed; `None` when the geometry holds none.
    pub fn hard(&self) -> Option<&Arc<HardBodies>> {
        self.hard.as_ref()
    }

    /// The whole tank's bounds in the record's frame, low and high corners: every ballistic
    /// primitive, posed.
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        self.bounds
    }

    /// Its crew stations' mean face (m², [`crew_face`]): read at rest, so the same at every pose;
    /// `None` for a tank with no crew station, and for shapes read from no tank.
    pub fn crew_face(&self) -> Option<f32> {
        self.crew_face
    }

    /// The shape of the component the record names `name`.
    pub fn get(&self, name: &str) -> Option<&ComponentShape> {
        self.by_name.get(name)
    }

    /// Every component's name and shape, in no particular order.
    #[cfg(test)]
    pub fn iter(&self) -> impl Iterator<Item = (&str, &ComponentShape)> {
        self.by_name
            .iter()
            .map(|(name, shape)| (name.as_str(), shape))
    }
}

/// The mean face (m²) of the crew stations of the tank `spec` names over `geometry`, the face the
/// pen cam's potency bands for that tank are read from (`pen_cam::potency`): each crew station's
/// box with every servo at rest, its mean face ([`ComponentShape::mean_face`]) in the record's
/// frame, averaged over the stations. The game's damage model authors one shared crew box size
/// within a tank, so the mean represents each station. `None` without a crew station whose box
/// resolves to a positive face.
fn crew_face(geometry: &TankGeometry, spec: &TankSpec) -> Option<f32> {
    let poses = posed_nodes(geometry, |_| None);
    let hull_from_root = geometry
        .by_name
        .get(HULL_NODE)
        .map_or(Affine3A::IDENTITY, |&hull| poses[hull].inverse());
    let faces: Vec<f32> = spec
        .volumes
        .iter()
        .filter(|(_, volume)| volume.component().is_some_and(|c| c.crew.is_some()))
        .filter_map(|(name, _)| {
            let &index = geometry.by_name.get(name)?;
            let to_record = hull_from_root * poses[index];
            let positions = geometry.nodes[index]
                .primitives
                .iter()
                .filter(|primitive| primitive.substance.is_some())
                .flat_map(|primitive| &primitive.positions)
                .map(|p| to_record.transform_point3(Vec3::from(*p)))
                .collect();
            let shape = ComponentShape {
                positions,
                indices: Vec::new(),
            };
            Some(shape.mean_face())
        })
        .collect();
    let face = faces.iter().sum::<f32>() / faces.len() as f32;
    (face.is_finite() && face > 0.0).then_some(face)
}

/// Every node's root-relative pose with each servo `servo` names turned to its angle: the
/// extraction's own composition (`pos += rot * t; rot *= r`, scale the node's composed one), a
/// servo node's local rotation replaced by [`servo_pose`] of its authored rotation. Relies on the
/// extraction's order: a node's parent precedes it in [`TankGeometry::nodes`].
fn posed_nodes(
    geometry: &TankGeometry,
    servo: impl Fn(&str) -> Option<(ServoRole, f32)>,
) -> Vec<Affine3A> {
    let mut placed: Vec<(Vec3, Quat)> = Vec::with_capacity(geometry.nodes.len());
    for (index, node) in geometry.nodes.iter().enumerate() {
        let local = match servo(&node.name) {
            Some((role, angle)) => servo_pose(role, node.transform.rotation, angle),
            None => node.transform.rotation,
        };
        let (position, rotation) = match node.parent {
            Some(parent) => {
                assert!(parent < index, "{}: a parent follows its child", node.name);
                let (position, rotation) = placed[parent];
                (
                    position + rotation * node.transform.translation,
                    rotation * local,
                )
            }
            None => (node.transform.translation, local),
        };
        placed.push((position, rotation));
    }
    placed
        .into_iter()
        .zip(&geometry.nodes)
        .map(|((position, rotation), node)| {
            Affine3A::from_scale_rotation_translation(node.root_scale, rotation, position)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{MeshGeometry, NodeGeometry, PrimitiveSubstance};

    /// A record angle (deg) for posing one servo alone: fixture data, large enough that every
    /// vertex under the servo moves visibly.
    const POSE_DEG: f32 = 25.0;
    /// Vertex tolerance (m): DERIVED, f32 rounding of a few affine products over a tank's few-metre
    /// extent is under 1e-5 m.
    const VERTEX_TOL_M: f32 = 1.0e-4;

    /// The servo's own axis, stated here independently of [`ServoRole`]'s.
    fn axis(role: ServoRole) -> Vec3 {
        match role {
            ServoRole::Yaw => Vec3::Y,
            ServoRole::Pitch => Vec3::X,
        }
    }

    /// Whether node `index` is `ancestor` or lies under it.
    fn under(geometry: &TankGeometry, mut index: usize, ancestor: usize) -> bool {
        loop {
            if index == ancestor {
                return true;
            }
            match geometry.nodes[index].parent {
                Some(parent) => index = parent,
                None => return false,
            }
        }
    }

    /// The independent expectation: a vertex under a servo turned `angle` about its axis is the rest
    /// vertex carried into the servo's rest frame, turned there, and carried back, all in the
    /// record's frame. The rest frame is the extraction's own composed pose of the servo node.
    fn expected(
        geometry: &TankGeometry,
        servo: usize,
        role: ServoRole,
        angle: f32,
        rest: Vec3,
    ) -> Vec3 {
        let hull = geometry
            .by_name
            .get(HULL_NODE)
            .map_or(Affine3A::IDENTITY, |&hull| {
                let node = &geometry.nodes[hull];
                Affine3A::from_rotation_translation(node.root_rotation, node.root_position)
            });
        let node = &geometry.nodes[servo];
        let frame = hull.inverse()
            * Affine3A::from_rotation_translation(node.root_rotation, node.root_position);
        let turn = Affine3A::from_quat(Quat::from_axis_angle(axis(role), angle));
        (frame * turn * frame.inverse()).transform_point3(rest)
    }

    /// Every component the tank file declares resolves to a real triangle mesh. Class invariants
    /// over the tank file's own entries.
    #[test]
    fn every_component_resolves_to_its_real_mesh() {
        let asset = TankAsset::load(crate::tank::TIGER_SPEC_PATH).unwrap();
        let rest = asset.shapes(&BTreeMap::new());
        for (name, volume) in &asset.spec.volumes {
            if volume.component().is_none() {
                continue;
            }
            let shape = rest.get(name).unwrap_or_else(|| panic!("{name} resolves"));
            assert!(shape.indices.len() >= 12, "{name}: a closed solid");
            assert_eq!(shape.indices.len() % 3, 0, "{name}");
        }
    }

    /// Each servo the tank file declares, turned alone, carries every vertex of every component
    /// under it to where the independent expectation puts it, and leaves every other component
    /// where it was; at least one servo carries a component.
    #[test]
    fn each_servo_turns_the_components_under_it_about_its_own_rest_frame() {
        let asset = TankAsset::load(crate::tank::TIGER_SPEC_PATH).unwrap();
        let geometry = &asset.geometry;
        let rest = asset.shapes(&BTreeMap::new());
        let mut carried = 0;
        for (servo_name, servo) in &asset.spec.servos {
            let servo_index = geometry.by_name[servo_name];
            let posed = asset.shapes(&BTreeMap::from([(servo_name.clone(), POSE_DEG)]));
            for (name, volume) in &asset.spec.volumes {
                if volume.component().is_none() {
                    continue;
                }
                let index = geometry.by_name[name];
                let carried_here = under(geometry, index, servo_index);
                carried += usize::from(carried_here);
                let (before, after) = (&rest.get(name).unwrap(), &posed.get(name).unwrap());
                for (a, b) in before.positions.iter().zip(&after.positions) {
                    let want = if carried_here {
                        expected(
                            geometry,
                            servo_index,
                            servo.role(),
                            POSE_DEG.to_radians(),
                            *a,
                        )
                    } else {
                        *a
                    };
                    assert!(
                        b.distance(want) < VERTEX_TOL_M,
                        "{servo_name} at {POSE_DEG} deg: {name} vertex {a} drawn at {b}, \
                         expected {want}"
                    );
                }
            }
        }
        assert!(carried > 0, "a servo carries a component");
    }

    fn node(name: &str, parent: usize, translation: Vec3, rotation: Quat) -> NodeGeometry {
        NodeGeometry {
            name: name.to_owned(),
            parent: Some(parent),
            transform: Transform::from_translation(translation).with_rotation(rotation),
            root_position: Vec3::ZERO,
            root_rotation: Quat::IDENTITY,
            root_scale: Vec3::ONE,
            primitives: Vec::new(),
        }
    }

    /// A fixture tank whose servo has a tilted authored rest and sits off the hull's origin, with
    /// one cube component under it: the case that tells `rest * turn` from `turn * rest`, a negated
    /// angle, and a pivot at the parent's origin from the servo's own. Composed poses are filled the
    /// extraction's way. Every number is fixture data.
    fn tilted_fixture() -> TankGeometry {
        let mut nodes = vec![NodeGeometry {
            name: "Scene0".to_owned(),
            parent: None,
            transform: Transform::IDENTITY,
            root_position: Vec3::ZERO,
            root_rotation: Quat::IDENTITY,
            root_scale: Vec3::ONE,
            primitives: Vec::new(),
        }];
        nodes.push(node(
            HULL_NODE,
            0,
            Vec3::new(0.1, 1.0, 0.2),
            Quat::from_rotation_z(0.05),
        ));
        nodes.push(node(
            "Servo",
            1,
            Vec3::new(0.3, 0.5, -0.2),
            Quat::from_rotation_x(0.35) * Quat::from_rotation_z(0.2),
        ));
        let mut cube = node("Box", 2, Vec3::new(0.0, 0.1, -1.0), Quat::IDENTITY);
        let corners: Vec<[f32; 3]> = (0..8)
            .map(|i| {
                [
                    if i & 1 == 0 { -0.2 } else { 0.2 },
                    if i & 2 == 0 { -0.1 } else { 0.1 },
                    if i & 4 == 0 { -0.3 } else { 0.3 },
                ]
            })
            .collect();
        cube.primitives.push(MeshGeometry {
            positions: corners,
            indices: vec![0, 1, 2, 1, 3, 2],
            substance: Some(PrimitiveSubstance {
                name: "fixture".to_owned(),
                id: crate::substances::SubstanceId::fixture(1.0),
                matter: crate::substances::Matter::Hard {
                    factor: 1.0,
                    debris_ratio: 2.0,
                },
            }),
            certificate: None,
        });
        nodes.push(cube);
        for index in 1..nodes.len() {
            let parent = nodes[index].parent.unwrap();
            let (position, rotation) = (nodes[parent].root_position, nodes[parent].root_rotation);
            nodes[index].root_position = position + rotation * nodes[index].transform.translation;
            nodes[index].root_rotation = rotation * nodes[index].transform.rotation;
        }
        let by_name = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.name.clone(), index))
            .collect();
        TankGeometry {
            nodes,
            by_name,
            roadwheels: Vec::new(),
            collision_proxies: Vec::new(),
            ballistic_volumes: vec![3],
        }
    }

    /// A fixture chain: a yaw servo with a tilted rest off the hull's origin, a pitch servo under it
    /// with its own tilted rest off the yaw servo's origin, and a cube component under the pitch
    /// servo. Composed poses are filled the extraction's way. Every number is fixture data.
    fn nested_fixture() -> TankGeometry {
        let mut geometry = tilted_fixture();
        let cube = geometry.nodes.pop().expect("the cube");
        geometry.nodes.push(node(
            "Child",
            2,
            Vec3::new(-0.2, 0.4, -0.6),
            Quat::from_rotation_y(0.3) * Quat::from_rotation_z(-0.15),
        ));
        geometry.nodes.push(NodeGeometry {
            parent: Some(3),
            ..cube
        });
        for index in 1..geometry.nodes.len() {
            let parent = geometry.nodes[index].parent.unwrap();
            let (position, rotation) = (
                geometry.nodes[parent].root_position,
                geometry.nodes[parent].root_rotation,
            );
            let local = geometry.nodes[index].transform;
            geometry.nodes[index].root_position = position + rotation * local.translation;
            geometry.nodes[index].root_rotation = rotation * local.rotation;
        }
        geometry.by_name = geometry
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.name.clone(), index))
            .collect();
        geometry.ballistic_volumes = vec![4];
        geometry
    }

    /// A parent yaw servo and a child pitch servo turned together, both with tilted rests: each
    /// vertex is the child's turn about its rest frame, then the parent's about its own, so a
    /// posing that drops either turn, or composes them in the wrong order, misses.
    #[test]
    fn nested_servos_compose_parent_yaw_and_child_pitch() {
        let geometry = nested_fixture();
        let (yaw, pitch) = (0.5_f32, -0.35_f32);
        let rest = TankShapes::posed(&geometry, ["Box"], |_| None);
        let posed = TankShapes::posed(&geometry, ["Box"], |name| match name {
            "Servo" => Some((ServoRole::Yaw, yaw)),
            "Child" => Some((ServoRole::Pitch, pitch)),
            _ => None,
        });
        let (before, after) = (rest.get("Box").unwrap(), posed.get("Box").unwrap());
        for (a, b) in before.positions.iter().zip(&after.positions) {
            let child = expected(&geometry, 3, ServoRole::Pitch, pitch, *a);
            let want = expected(&geometry, 2, ServoRole::Yaw, yaw, child);
            assert!(
                b.distance(want) < VERTEX_TOL_M,
                "{a} drawn at {b}, expected {want}"
            );
        }
    }

    /// On a servo with a tilted rest, off its parent's origin, every role at a positive and a
    /// negative angle draws each vertex where the independent expectation puts it.
    #[test]
    fn a_tilted_servo_turns_about_its_own_axis_and_origin() {
        let geometry = tilted_fixture();
        let rest = TankShapes::posed(&geometry, ["Box"], |_| None);
        for role in [ServoRole::Yaw, ServoRole::Pitch] {
            for angle in [0.6_f32, -0.4] {
                let posed = TankShapes::posed(&geometry, ["Box"], |name| {
                    (name == "Servo").then_some((role, angle))
                });
                let (before, after) = (rest.get("Box").unwrap(), posed.get("Box").unwrap());
                for (a, b) in before.positions.iter().zip(&after.positions) {
                    let want = expected(&geometry, 2, role, angle, *a);
                    assert!(
                        b.distance(want) < VERTEX_TOL_M,
                        "{angle} rad: {a} drawn at {b}, expected {want}"
                    );
                    assert!(b.distance(*a) > 0.01, "{angle} rad: {a} did not move");
                }
            }
        }
    }
}
