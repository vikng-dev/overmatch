//! The wall's cut, read from a building's bake (`assets/buildings/<id>/<id>.building.ron`): one
//! wall's chunks and joints, set down in the sandbox. The bake chose the cut; this only selects
//! and places it.

use bevy::math::{Quat, Vec3};

use super::model::{
    Block, Bond, Chunk, Cut, Joint, Masonry, Side, Structure, clip_polygon, plane_axes,
};
use crate::buildings::schema::BuildingAsset;

/// Where a wall is set down: the world x its centre stands at, the world z its back face stands
/// at; its foundation plane lies on world y = 0 and its outer face looks down +Z.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub centre_x: f32,
    pub back_z: f32,
}

/// The chunks and joints of `wall`'s elements of `substance` (an element's name begins
/// `<wall>/`), as one [`Cut`] of `material`, placed by `at`. The wall's outer face is the far side
/// of its first element's extrusion. A joint to a chunk outside the selection (another wall, or
/// another substance) is dropped.
pub(crate) fn wall_cut(
    asset: &BuildingAsset,
    wall: &str,
    substance: &str,
    material: Masonry,
    at: Placement,
) -> Result<Cut, String> {
    let prefix = format!("{wall}/");
    let kept: Vec<usize> = asset
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.name.starts_with(&prefix) && asset.materials[usize::from(e.material)] == substance
        })
        .map(|(i, _)| i)
        .collect();
    let first = kept
        .first()
        .ok_or_else(|| format!("no {substance} element in wall {wall:?}"))?;
    let turn = Quat::from_rotation_arc(asset.elements[*first].frame.w().normalize(), Vec3::Z);
    let mut chunks_of: Vec<usize> = Vec::new();
    for &e in &kept {
        let [from, to] = asset.elements[e].chunks;
        chunks_of.extend(from as usize..to as usize);
    }
    let mut new_index = vec![None; asset.chunks.len()];
    for (new, &old) in chunks_of.iter().enumerate() {
        new_index[old] = Some(new);
    }
    let rotated = |chunk: usize| -> Vec<Vec3> {
        asset.chunks[chunk]
            .hull
            .iter()
            .map(|p| turn * Vec3::from_array(*p))
            .collect()
    };
    let (mut low, mut high) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
    for &old in &chunks_of {
        for p in rotated(old) {
            low = low.min(p);
            high = high.max(p);
        }
    }
    let shift = Vec3::new(
        at.centre_x - (low.x + high.x) / 2.0,
        -asset.foundation_y_m,
        at.back_z - low.z,
    );
    let place = |p: Vec3| turn * p + shift;
    let blocks: Vec<Block> = chunks_of
        .iter()
        .map(|&old| {
            Block::hull(
                asset.chunks[old]
                    .hull
                    .iter()
                    .map(|p| place(Vec3::from_array(*p)))
                    .collect(),
            )
        })
        .collect();
    let chunks = chunks_of
        .iter()
        .map(|&old| {
            let c = &asset.chunks[old];
            Chunk {
                volume: c.volume_m3,
                centroid: place(Vec3::from_array(c.centroid)),
                mass: c.mass_kg,
                material,
                // The range's wall stands on its foundation's bottom: nothing lies below grade.
                buried: false,
            }
        })
        .collect();
    let mut joints = Vec::new();
    let mut faces = Vec::new();
    for j in &asset.joints {
        let (Some(a), Some(b)) = (new_index[j.a as usize], new_index[j.b as usize]) else {
            continue;
        };
        let normal = turn * Vec3::from_array(j.normal);
        let centroid = place(Vec3::from_array(j.centroid));
        let (face_a, face_b) = (
            blocks[a].face_on(centroid, normal),
            blocks[b].face_on(centroid, normal),
        );
        let mut polygon = clip_polygon(&face_a, &face_b, normal);
        if polygon.len() < 3 {
            polygon = square(centroid, normal, j.area_m2);
        }
        joints.push(Joint {
            a,
            b: Side::Chunk(b),
            area: j.area_m2,
            centroid,
            normal,
            bond: Bond::Bonded(material.seam),
        });
        faces.push(polygon);
    }
    for anchor in &asset.anchors {
        let Some(a) = new_index[anchor.chunk as usize] else {
            continue;
        };
        let centroid = place(Vec3::from_array(anchor.centroid));
        let mut polygon = blocks[a].face_on(centroid, Vec3::Y);
        if polygon.len() < 3 {
            polygon = square(centroid, Vec3::Y, anchor.area_m2);
        }
        joints.push(Joint {
            a,
            b: Side::Ground,
            area: anchor.area_m2,
            centroid,
            normal: Vec3::Y,
            bond: Bond::Bonded(material.seam),
        });
        faces.push(polygon);
    }
    Ok(Cut {
        structure: Structure { chunks, joints },
        blocks,
        faces,
    })
}

/// A square of `area` m² about `centroid` in the plane of unit `normal`: what a joint draws as
/// when its corners are not on both chunks' faces.
fn square(centroid: Vec3, normal: Vec3, area: f32) -> Vec<Vec3> {
    let (u, v) = plane_axes(normal);
    let h = area.sqrt() / 2.0;
    vec![
        centroid - u * h - v * h,
        centroid + u * h - v * h,
        centroid + u * h + v * h,
        centroid - u * h + v * h,
    ]
}
