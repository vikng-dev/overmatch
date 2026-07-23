//! The marker-driven track model: reads the sharp sources of truth out of the glb (the `Pin_*` /
//! `*_Surface` marker empties, the `Sprocket_*` / `Idler_*` rig meshes, and the wheel/idler mesh
//! radii) and derives the geometry the editor renders. This is the "new model" — nothing here reads
//! the RON's geometry fields (pitch, thickness, plane_x, sprocket/idler center, wheel_radius); those
//! all come from the model now. Only the authored non-geometry (`link_count`, `mass`; `teeth` is an
//! editor knob) still comes from the spec.
//!
//! It reads the glb DIRECTLY via the `gltf` crate, composing the FULL `T·R·S` world transform down
//! the node tree. The bake's `root_position` is not usable here: it omits scale (the markers sit
//! under a scaled ancestor) and the sprocket/idler are meshes with their geometry baked into the
//! vertices (zero node translation), so their centre is the mesh centroid, not the node origin.
//! When the model graduates into the game this measurement moves into the bake.

use std::collections::HashMap;

use bevy::math::{Mat4, Quat, Vec2, Vec3};
use bevy::prelude::Resource;

use crate::bake::TankBlueprint;

/// Everything the editor's geometry needs, derived from the model (all lengths in metres, side-plane
/// centres as `(z, y)` in the glTF frame the blueprint's wheel positions also use).
#[derive(Resource, Clone, Copy)]
pub struct DerivedModel {
    /// Link pitch = `|Pin_End − Pin_Start|`.
    pub pitch: f32,
    /// Measured pin-centre → inner-face offset (stands the track off the wheels/sprocket).
    pub pin_to_inner: f32,
    /// Measured pin-centre → outer-face offset (the ground-contact side).
    pub pin_to_outer: f32,
    /// The track's lateral median plane (|x| of the pin markers).
    pub plane_x: f32,
    /// Sprocket centre, side-plane `(z, y)`, from the `Sprocket_R` mesh centroid.
    pub sprocket_center: Vec2,
    /// Idler centre, side-plane `(z, y)`, from the `Idler_R` mesh centroid.
    pub idler_center: Vec2,
    /// Idler rim (track-contact) radius, from the `Idler_R` mesh.
    pub idler_radius: f32,
    /// Road-wheel tread radius, from a `Wheel_R_*_Visual` mesh.
    pub wheel_tread: f32,
    /// Authored count (RON) — the assembled loop length is `link_count · pitch`.
    pub link_count: usize,
    /// Hull mass (RON) — for the spring-rate readout.
    pub mass: f32,
    /// True when the fields came from the glb markers; false on the RON fallback (old glb).
    pub marker_driven: bool,
}

impl DerivedModel {
    /// Derive from the model markers/nodes; falls back to the RON if the glb read fails (old glb).
    pub fn build(blueprint: &TankBlueprint, glb_path: &std::path::Path) -> Self {
        Self::from_markers(blueprint, glb_path).unwrap_or_else(|| Self::from_spec(blueprint))
    }

    fn from_markers(blueprint: &TankBlueprint, glb_path: &std::path::Path) -> Option<Self> {
        let g = GlbNodes::read(
            glb_path,
            &["Pin_Start", "Pin_End", "Inner_Surface", "Outer_Surface"],
            &["Sprocket_R", "Idler_R", "Wheel_R_0_Visual"],
        )?;

        let p0 = *g.empties.get("Pin_Start")?;
        let p1 = *g.empties.get("Pin_End")?;
        let inner = *g.empties.get("Inner_Surface")?;
        let outer = *g.empties.get("Outer_Surface")?;

        let pitch = super::derive::pitch_from_pins(p0, p1);
        let pin_mid = (p0 + p1) * 0.5;
        let normal = (outer - inner).normalize_or_zero();
        let pin_to_inner = (pin_mid - inner).dot(normal);
        let pin_to_outer = (outer - pin_mid).dot(normal);
        let plane_x = ((p0.x + p1.x) * 0.5).abs();

        let (sprocket_c, _) = *g.meshes.get("Sprocket_R")?;
        let (idler_c, idler_radius) = *g.meshes.get("Idler_R")?;
        let (_, wheel_tread) = *g.meshes.get("Wheel_R_0_Visual")?;

        Some(Self {
            pitch,
            pin_to_inner,
            pin_to_outer,
            plane_x,
            // side-plane (z, y): glTF x is lateral, y is height, z is longitudinal.
            sprocket_center: Vec2::new(sprocket_c.z, sprocket_c.y),
            idler_center: Vec2::new(idler_c.z, idler_c.y),
            idler_radius,
            wheel_tread,
            link_count: blueprint.spec.track.link_count,
            mass: blueprint.spec.mass,
            marker_driven: true,
        })
    }

    /// Fallback: reproduce the old RON-authored geometry so the editor still runs on a glb that
    /// predates the markers (with the symmetric mid-plate assumption baked back in).
    fn from_spec(blueprint: &TankBlueprint) -> Self {
        let t = &blueprint.spec.track;
        Self {
            pitch: t.pitch,
            pin_to_inner: t.thickness * 0.5,
            pin_to_outer: t.thickness * 0.5,
            plane_x: t.plane_x,
            sprocket_center: Vec2::new(t.sprocket.center.0, t.sprocket.center.1),
            idler_center: Vec2::new(t.idler.center.0, t.idler.center.1),
            idler_radius: t.idler.radius,
            wheel_tread: t.wheel_radius,
            link_count: t.link_count,
            mass: blueprint.spec.mass,
            marker_driven: false,
        }
    }
}

/// World positions of named empties + (centroid, disc-radius) of named meshes, read from the glb
/// with the full node transform chain.
struct GlbNodes {
    empties: HashMap<String, Vec3>,
    meshes: HashMap<String, (Vec3, f32)>,
}

impl GlbNodes {
    fn read(glb_path: &std::path::Path, empty_names: &[&str], mesh_names: &[&str]) -> Option<Self> {
        let gltf::Gltf { document, mut blob } = gltf::Gltf::open(glb_path).ok()?;
        let mut buffers: Vec<Vec<u8>> = Vec::new();
        for buffer in document.buffers() {
            match buffer.source() {
                gltf::buffer::Source::Bin => buffers.push(blob.take()?),
                gltf::buffer::Source::Uri(_) => return None,
            }
        }

        let mut empties = HashMap::new();
        let mut meshes = HashMap::new();
        let scene = document.scenes().next()?;
        // DFS composing the world matrix (parent · local) down the tree.
        let mut stack: Vec<(gltf::Node, Mat4)> =
            scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        while let Some((node, parent)) = stack.pop() {
            let world = parent * node_matrix(&node);
            if let Some(name) = node.name() {
                if empty_names.contains(&name) {
                    empties.insert(name.to_string(), world.transform_point3(Vec3::ZERO));
                }
                if mesh_names.contains(&name)
                    && let Some(mesh) = node.mesh()
                {
                    let mut verts: Vec<Vec3> = Vec::new();
                    for primitive in mesh.primitives() {
                        let reader =
                            primitive.reader(|b| buffers.get(b.index()).map(Vec::as_slice));
                        if let Some(positions) = reader.read_positions() {
                            verts.extend(positions.map(|p| world.transform_point3(Vec3::from(p))));
                        }
                    }
                    if let Some(cr) = centroid_disc_radius(&verts) {
                        meshes.insert(name.to_string(), cr);
                    }
                }
            }
            for child in node.children() {
                stack.push((child, world));
            }
        }
        Some(Self { empties, meshes })
    }
}

/// glTF node local transform → matrix (Matrix form, or composed from the decomposed TRS).
fn node_matrix(node: &gltf::Node) -> Mat4 {
    match node.transform() {
        gltf::scene::Transform::Matrix { matrix } => Mat4::from_cols_array_2d(&matrix),
        gltf::scene::Transform::Decomposed {
            translation,
            rotation,
            scale,
        } => Mat4::from_scale_rotation_translation(
            Vec3::from(scale),
            Quat::from_array(rotation),
            Vec3::from(translation),
        ),
    }
}

/// Centroid + disc radius (max in-plane distance from the centroid, perpendicular to the thinnest
/// axis = the axle) of a world-space vertex cloud.
fn centroid_disc_radius(verts: &[Vec3]) -> Option<(Vec3, f32)> {
    if verts.len() < 3 {
        return None;
    }
    let centroid = verts.iter().fold(Vec3::ZERO, |a, &b| a + b) / verts.len() as f32;
    let extent = |axis: usize| {
        let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
        for v in verts {
            lo = lo.min(v[axis]);
            hi = hi.max(v[axis]);
        }
        hi - lo
    };
    let extents = [extent(0), extent(1), extent(2)];
    let axle = (0..3)
        .min_by(|&a, &b| extents[a].total_cmp(&extents[b]))
        .unwrap();
    let (p, q) = match axle {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let r = verts
        .iter()
        .map(|v| ((v[p] - centroid[p]).powi(2) + (v[q] - centroid[q]).powi(2)).sqrt())
        .fold(0.0_f32, f32::max);
    Some((centroid, r))
}
