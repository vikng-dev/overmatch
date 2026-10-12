//! A struck tank's component shapes, resolved by name from the tank asset: the record names a
//! component and this module answers with its real mesh.
//!
//! The source is the one the sim builds its trimesh colliders from: `bake`'s extraction of the
//! vehicle's sim artifact (`<id>.sim.glb`, beside `<id>.tank.ron`), whose ballistic primitives are
//! the component volumes, triangle for triangle. A shape is stated in the record's frame (the hull
//! node's, the rig contract's `Hull`), with the turret traversed to the record's pose: the nodes
//! under the yaw servo above the gunner view's node turn about that servo's origin, as
//! [`crate::tank`] binds it. Generic over any tank file; nothing here names a vehicle.

use std::collections::HashMap;

use bevy::asset::RenderAssetUsages;
use bevy::math::Affine3A;
use bevy::mesh::{Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::bake::{TankGeometry, extract_tank_geometry};
use crate::spec::{TankSpec, ViewKind};
use crate::substances::SubstanceRegistry;
use crate::tank::ServoRole;

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

    /// Every component's shape, the turret traversed `turret_yaw_deg` from its spawn pose.
    pub fn shapes(&self, turret_yaw_deg: f32) -> TankShapes {
        TankShapes::from_geometry(&self.geometry, &self.spec, turret_yaw_deg)
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
        turret_yaw_deg: f32,
    ) -> Self {
        let pose = |index: usize| {
            let node = &geometry.nodes[index];
            Affine3A::from_scale_rotation_translation(
                node.root_scale,
                node.root_rotation,
                node.root_position,
            )
        };
        let hull_from_root = geometry
            .by_name
            .get(HULL_NODE)
            .map_or(Affine3A::IDENTITY, |&hull| pose(hull).inverse());
        let turret = turret_node(geometry, spec);
        let turn = turret.map(|turret| {
            let parent = geometry.nodes[turret]
                .parent
                .map_or(Quat::IDENTITY, |p| geometry.nodes[p].root_rotation);
            let pivot =
                Affine3A::from_rotation_translation(parent, geometry.nodes[turret].root_position);
            pivot
                * Affine3A::from_quat(Quat::from_rotation_y(turret_yaw_deg.to_radians()))
                * pivot.inverse()
        });
        let under_turret = |mut index: usize| loop {
            if Some(index) == turret {
                return true;
            }
            match geometry.nodes[index].parent {
                Some(parent) => index = parent,
                None => return false,
            }
        };
        let mut by_name = HashMap::new();
        for (name, volume) in &spec.volumes {
            if volume.component().is_none() {
                continue;
            }
            let Some(&index) = geometry.by_name.get(name) else {
                continue;
            };
            let mut to_record = hull_from_root * pose(index);
            if under_turret(index)
                && let Some(turn) = turn
            {
                to_record = hull_from_root * turn * pose(index);
            }
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
            by_name.insert(name.clone(), shape);
        }
        Self { by_name }
    }

    /// The shape of the component the record names `name`.
    pub fn get(&self, name: &str) -> Option<&ComponentShape> {
        self.by_name.get(name)
    }
}

/// The yaw servo above the gunner view's node: the turret, as the rig binder resolves it.
fn turret_node(geometry: &TankGeometry, spec: &TankSpec) -> Option<usize> {
    let gunner = spec.views.get(&ViewKind::Gunner)?;
    let mut index = *geometry.by_name.get(&gunner.node)?;
    loop {
        let node = &geometry.nodes[index];
        if spec
            .servos
            .get(&node.name)
            .is_some_and(|servo| servo.role() == ServoRole::Yaw)
        {
            return Some(index);
        }
        index = node.parent?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every component the tank file declares resolves to a real triangle mesh, and a component
    /// under the turret turns with the record's pose while a hull component does not. Class
    /// invariants over the tank file's own entries.
    #[test]
    fn every_component_resolves_to_its_real_mesh() {
        let asset = TankAsset::load(crate::tank::TIGER_SPEC_PATH).unwrap();
        let rest = asset.shapes(0.0);
        let turned = asset.shapes(90.0);
        let turret = turret_node(&asset.geometry, &asset.spec).expect("a turret");
        let mut moved = 0;
        for (name, volume) in &asset.spec.volumes {
            if volume.component().is_none() {
                continue;
            }
            let shape = rest.get(name).unwrap_or_else(|| panic!("{name} resolves"));
            assert!(shape.indices.len() >= 12, "{name}: a closed solid");
            assert_eq!(shape.indices.len() % 3, 0, "{name}");
            let index = asset.geometry.by_name[name];
            let mut under = false;
            let mut at = Some(index);
            while let Some(i) = at {
                under |= i == turret;
                at = asset.geometry.nodes[i].parent;
            }
            let shift = rest
                .get(name)
                .unwrap()
                .center()
                .distance(turned.get(name).unwrap().center());
            if under {
                moved += usize::from(shift > 1.0e-3);
            } else {
                assert!(shift < 1.0e-5, "{name}: a hull component does not turn");
            }
        }
        assert!(moved > 0, "a turret component turns with the turret");
    }
}
