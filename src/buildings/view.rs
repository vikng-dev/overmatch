//! A building's view: one mesh per building asset, built from its RON's elements, merged and
//! shared by every instance, drawn with the buildings' one material ([`crate::building_surface`]).
//!
//! VIEW ONLY (ADR-0014). The mesh is every element's prism, face by face: the drawn silhouette is
//! exactly the colliders' union, and an opening, which is no element, is a hole the camera sees
//! through. Faces two elements share stay in the mesh; they lie inside the wall and are never seen.
//!
//! # Metres on every face, timber by the mesh's word
//!
//! Each face carries the frame [`crate::building_surface::unit_box`] uses: outward normal `n`,
//! tangent `t`, image-up `b = n × t`, `uv = (p·t, −p·b)` at every corner `p`, here in metres of
//! the building frame. The material's vertex stage multiplies them by the instance's own scale
//! along `t` and `b`, so a scaled house keeps real-size bricks. A wall face stands its image
//! upright (`t = Y × n`); a timber face runs its boards along its element's grain, the extrusion
//! axis, where that lies in the face. The second UV channel's `x` says which pack a face wears:
//! 1 on timber, 0 on masonry, so a sill's top stays brick and a board's edge stays timber.

use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::prelude::*;

use super::schema::BuildingAsset;

/// The substance key whose faces wear the timber pack; every other wears the masonry pack.
const TIMBER: &str = "Wood";

/// The tangent frame of a face with outward normal `n` on an element extruded along `grain`:
/// the grain where it lies in the face and the face is timber, else `Y × n` (the image upright),
/// else `X` on a face that is level.
fn tangent(n: Vec3, grain: Vec3, timber: bool) -> Vec3 {
    if timber && grain.dot(n).abs() < 1e-3 {
        return grain.normalize();
    }
    let upright = Vec3::Y.cross(n);
    if upright.length_squared() > 1e-6 {
        upright.normalize()
    } else {
        Vec3::X
    }
}

/// The building's mesh in its own frame at scale 1 (module doc).
pub(crate) fn mesh(asset: &BuildingAsset) -> Mesh {
    let (mut positions, mut normals, mut tangents, mut uvs, mut packs, mut indices) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for element in &asset.elements {
        let timber = asset.materials[usize::from(element.material)] == TIMBER;
        let grain = element.frame.w();
        for (n, corners) in element.faces() {
            let t = tangent(n, grain, timber);
            let b = n.cross(t);
            let first = u32::try_from(positions.len()).expect("a building's mesh fits u32");
            for p in &corners {
                positions.push(p.to_array());
                normals.push(n.to_array());
                tangents.push(t.extend(1.0).to_array());
                uvs.push([p.dot(t), -p.dot(b)]);
                packs.push([if timber { 1.0 } else { 0.0 }, 0.0]);
            }
            // Corners run counter-clockwise seen from outside: a fan from the first.
            for k in 1..corners.len() as u32 - 1 {
                indices.extend([first, first + k, first + k + 1]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, tangents)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, packs)
        .with_inserted_indices(Indices::U32(indices))
}

#[cfg(test)]
mod tests {
    use bevy::mesh::VertexAttributeValues;

    use super::*;
    use crate::buildings::tests::shipped_house;

    /// Every triangle of `mesh`, as its three corners.
    fn triangles(mesh: &Mesh) -> Vec<[Vec3; 3]> {
        let Some(VertexAttributeValues::Float32x3(positions)) =
            mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        else {
            panic!("positions");
        };
        let Some(Indices::U32(indices)) = mesh.indices() else {
            panic!("u32 indices");
        };
        indices
            .chunks(3)
            .map(|t| [0, 1, 2].map(|k| Vec3::from_array(positions[t[k] as usize])))
            .collect()
    }

    /// Whether the segment `from → to` crosses a triangle (Möller–Trumbore, either side).
    fn crosses(triangles: &[[Vec3; 3]], from: Vec3, to: Vec3) -> bool {
        let dir = to - from;
        triangles.iter().any(|&[a, b, c]| {
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-9 {
                return false;
            }
            let s = from - a;
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = dir.dot(q) / det;
            let t = e2.dot(q) / det;
            (0.0..=1.0).contains(&u) && v >= 0.0 && u + v <= 1.0 && (0.0..=1.0).contains(&t)
        })
    }

    /// AN OPENING IS A HOLE THE CAMERA SEES THROUGH, A PIER IS NOT: across the shipped house's
    /// east wall (outer face x = 2 m, inner 1.75 m), a sight line through its first window's
    /// middle (z 1.5 m, 1.5 m up; house_1's spec) meets no triangle, and one through the pier
    /// beside it (z 2.5 m) does. A view that drew the wall as its graybox box, or filled the
    /// openings, fails the first; one that dropped an element fails the second.
    #[test]
    fn a_window_is_a_hole_and_a_pier_is_solid() {
        let triangles = triangles(&mesh(&shipped_house().asset));
        let across = |z: f32| crosses(&triangles, Vec3::new(2.1, 1.5, z), Vec3::new(1.6, 1.5, z));
        assert!(!across(1.5), "the window is open");
        assert!(across(2.5), "the pier is drawn");
    }

    /// EVERY FACE IS FRAMED IN METRES AND WEARS ITS ELEMENT'S PACK: each corner's UV is its
    /// position on the face's tangent and image-up axes (the frame the material's vertex stage
    /// rescales by the instance's scale), wall faces stand their image upright, and the second
    /// UV marks exactly the timber elements' faces. A UV off the frame stretches the bricks; a
    /// mark on the wrong faces puts boards on a wall or bricks on the roof.
    #[test]
    fn every_face_is_framed_in_metres_and_wears_its_element_s_pack() {
        let asset = shipped_house().asset.clone();
        let mesh = mesh(&asset);
        let attribute = |id| match mesh.attribute(id) {
            Some(VertexAttributeValues::Float32x3(values)) => values
                .iter()
                .copied()
                .map(Vec3::from_array)
                .collect::<Vec<_>>(),
            _ => panic!("Float32x3"),
        };
        let (positions, normals) = (
            attribute(Mesh::ATTRIBUTE_POSITION),
            attribute(Mesh::ATTRIBUTE_NORMAL),
        );
        let Some(VertexAttributeValues::Float32x4(tangents)) =
            mesh.attribute(Mesh::ATTRIBUTE_TANGENT)
        else {
            panic!("tangents");
        };
        let Some(VertexAttributeValues::Float32x2(uvs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_0)
        else {
            panic!("uvs");
        };
        let Some(VertexAttributeValues::Float32x2(packs)) = mesh.attribute(Mesh::ATTRIBUTE_UV_1)
        else {
            panic!("packs");
        };
        let mut timber_vertices = 0;
        for k in 0..positions.len() {
            let (p, n, t) = (positions[k], normals[k], Vec4::from_array(tangents[k]));
            let b = t.w * n.cross(t.truncate());
            let uv = Vec2::from_array(uvs[k]);
            assert!((uv - Vec2::new(p.dot(t.truncate()), -p.dot(b))).length() < 1e-4);
            assert!(
                t.truncate().dot(n).abs() < 1e-4,
                "the tangent lies in the face"
            );
            if packs[k][0] == 1.0 {
                timber_vertices += 1;
            } else {
                assert_eq!(packs[k][0], 0.0);
                if n.y.abs() < 1e-3 {
                    assert!(
                        (b - Vec3::Y).length() < 1e-4,
                        "a wall stands its bricks upright"
                    );
                }
            }
        }
        let expected: usize = asset
            .elements
            .iter()
            .filter(|element| asset.materials[usize::from(element.material)] == TIMBER)
            .map(|element| element.faces().iter().map(|(_, c)| c.len()).sum::<usize>())
            .sum();
        assert!(expected > 0);
        assert_eq!(timber_vertices, expected, "exactly the timber faces");
    }
}
