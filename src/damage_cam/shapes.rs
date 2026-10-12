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

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::bake::{TankGeometry, extract_tank_geometry};
use crate::spec::TankSpec;
use crate::substances::SubstanceRegistry;
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
    geometry: TankGeometry,
    spec: TankSpec,
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
        Ok(Self { geometry, spec })
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

/// Every component of a tank by name, posed for one record.
#[derive(Clone, Debug, Default)]
pub struct TankShapes {
    by_name: HashMap<String, ComponentShape>,
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
        Self::posed(geometry, components, |name| {
            let servo = spec.servos.get(name)?;
            let angle = servo_deg.get(name).copied().unwrap_or(0.0).to_radians();
            Some((servo.role(), angle))
        })
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
        Self { by_name }
    }

    /// The shape of the component the record names `name`.
    pub fn get(&self, name: &str) -> Option<&ComponentShape> {
        self.by_name.get(name)
    }
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
                factor: 1.0,
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
