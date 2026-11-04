//! The wall's cut, read from a building's bake (`assets/buildings/<id>/<id>.building.ron`): one
//! wall's chunks and joints, set down in the sandbox. The bake chose the cut; this only selects
//! and places it.

use bevy::math::{Quat, Vec3};

use super::model::{Block, Bond, Chunk, Cut, Joint, Masonry, Side, Structure, clip_polygon};
use crate::buildings::attachments::Attachments;
use crate::buildings::schema::BuildingAsset;

/// Where a wall is set down: the world x its centre stands at, the world z its back face stands
/// at; its foundation plane lies on world y = 0 and its outer face looks down +Z.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub centre_x: f32,
    pub back_z: f32,
}

/// The chunks and joints of the elements of `substance` in component `wall` (the bake's declared
/// membership, [`crate::buildings::schema::Element::component`]), as one [`Cut`] of `material`,
/// placed by `at`. The wall's outer face is its first
/// element's face across its thinnest span ([`Block::thinnest`]) away from the building's middle.
/// A joint to a chunk outside the selection (another wall, or another substance) is dropped.
pub(crate) fn wall_cut(
    asset: &BuildingAsset,
    wall: &str,
    substance: &str,
    material: Masonry,
    at: Placement,
) -> Result<Cut, String> {
    let component = asset
        .components
        .iter()
        .position(|name| name == wall)
        .ok_or_else(|| format!("no component {wall:?}"))?;
    let kept: Vec<usize> = asset
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            usize::from(e.component) == component
                && asset.materials[usize::from(e.material)] == substance
        })
        .map(|(i, _)| i)
        .collect();
    let first = kept
        .first()
        .ok_or_else(|| format!("no {substance} element in wall {wall:?}"))?;
    let element = Block::hull(asset.elements[*first].points());
    let (middle_low, middle_high) = asset
        .elements
        .iter()
        .flat_map(|element| element.points())
        .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), p| {
            (lo.min(p), hi.max(p))
        });
    let middle = (middle_low + middle_high) / 2.0;
    let (axis, ..) = element.thinnest();
    let outward = if (element.center() - middle).dot(axis) >= 0.0 {
        axis
    } else {
        -axis
    };
    let turn = Quat::from_rotation_arc(outward, Vec3::Z);
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
    for (index, j) in asset.joints.iter().enumerate() {
        let (Some(a), Some(b)) = (new_index[j.a as usize], new_index[j.b as usize]) else {
            continue;
        };
        let normal = turn * Vec3::from_array(j.normal);
        let centroid = place(Vec3::from_array(j.centroid));
        let (face_a, face_b) = (
            blocks[a].face_on(centroid, normal),
            blocks[b].face_on(centroid, normal),
        );
        let polygon = clip_polygon(&face_a, &face_b, normal);
        if face_a.len() < 3 || face_b.len() < 3 || polygon.len() < 3 {
            return Err(format!(
                "assets/buildings/{id}/{id}.building.ron joints[{index}].centroid/normal: \
                 no shared face reconstructs from chunks {} and {}; rebake the interface on both hulls",
                j.a,
                j.b,
                id = asset.id,
            ));
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
    let anchors = Attachments::shipped_static()
        .anchors(asset)
        .map_err(|err| {
            format!(
                "assets/buildings/{id}/{id}.building.ron: {err}",
                id = asset.id,
            )
        })?;
    for (index, (anchor, (_, bond))) in asset.anchors.iter().zip(anchors).enumerate() {
        let Some(a) = new_index[anchor.chunk as usize] else {
            continue;
        };
        let centroid = place(Vec3::from_array(anchor.centroid));
        let polygon = blocks[a].face_on(centroid, Vec3::Y);
        if polygon.len() < 3 {
            return Err(format!(
                "assets/buildings/{id}/{id}.building.ron anchors[{index}].centroid: \
                 no foundation face reconstructs from chunk {}; rebake the anchor on its hull",
                anchor.chunk,
                id = asset.id,
            ));
        }
        joints.push(Joint {
            a,
            b: Side::Ground,
            area: anchor.area_m2,
            centroid,
            normal: Vec3::Y,
            bond: match bond {
                Bond::AreaShare {
                    seam,
                    footprint_area,
                    footprint_centroid,
                } => Bond::AreaShare {
                    seam,
                    footprint_area,
                    footprint_centroid: place(footprint_centroid),
                },
                bond => bond,
            },
        });
        faces.push(polygon);
    }
    Ok(Cut {
        structure: Structure { chunks, joints },
        blocks,
        faces,
        element_of: chunks_of.iter().map(|&c| asset.chunks[c].element).collect(),
        elements: asset.elements.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::{BUILDING, PLACE, SUBSTANCE, WALL, brick};
    use super::*;

    fn cut(asset: &BuildingAsset) -> Result<Cut, String> {
        wall_cut(asset, WALL, SUBSTANCE, brick(), PLACE)
    }

    fn selected(asset: &BuildingAsset, chunk: u32) -> bool {
        let element = &asset.elements[usize::from(asset.chunks[chunk as usize].element)];
        asset.components[usize::from(element.component)] == WALL
            && asset.materials[usize::from(element.material)] == SUBSTANCE
    }

    fn foundation_fixture(name: &str) -> BuildingAsset {
        let mut asset = BuildingAsset::from_ron(BUILDING).unwrap();
        // Fixture: only the inspected chunks carry ground interfaces, so their placed
        // centroids independently determine each complete footprint's centroid.
        asset.anchors = asset
            .anchors
            .iter()
            .filter(|anchor| selected(&asset, anchor.chunk))
            .cloned()
            .collect();
        let attachment = asset
            .attachments
            .iter()
            .position(|kind| kind == name)
            .unwrap_or_else(|| {
                asset.attachments.push(name.into());
                asset.attachments.len() - 1
            });
        for anchor in &mut asset.anchors {
            anchor.attachment = u8::try_from(attachment).unwrap();
        }
        asset.check().expect("the fixture meets schema admission");
        asset
    }

    /// The game's environment destruction inspector must retain the declared foundation's
    /// strength and whole-footprint geometry. Fixture kinds exercise unequal seams and a root
    /// plate; replacing any of them with the wall material's seam changes the construction.
    #[test]
    fn foundations_keep_their_declared_attachment() {
        let attachments = Attachments::shipped_static();
        for name in ["mortared", "nailed", "norway_spruce_root_plate"] {
            let asset = foundation_fixture(name);
            let expected = attachments.anchors(&asset).unwrap();
            let cut = cut(&asset).unwrap();
            let ground: Vec<_> = cut
                .structure
                .joints
                .iter()
                .filter(|joint| joint.b == Side::Ground)
                .collect();
            let mut checked = 0;
            for (anchor, (_, bond)) in asset.anchors.iter().zip(expected) {
                if !selected(&asset, anchor.chunk) {
                    continue;
                }
                let actual = ground[checked].bond;
                match (bond, actual) {
                    (
                        Bond::AreaShare {
                            seam,
                            footprint_area,
                            ..
                        },
                        Bond::AreaShare {
                            seam: actual_seam,
                            footprint_area: actual_area,
                            footprint_centroid,
                        },
                    ) => {
                        assert_eq!(actual_seam, seam);
                        assert_eq!(actual_area, footprint_area);
                        let component = asset.elements
                            [usize::from(asset.chunks[anchor.chunk as usize].element)]
                        .component;
                        let footprint: Vec<_> = ground
                            .iter()
                            .filter(|joint| {
                                asset.elements[usize::from(cut.element_of[joint.a])].component
                                    == component
                            })
                            .collect();
                        let area: f32 = footprint.iter().map(|joint| joint.area).sum();
                        let centre = footprint
                            .iter()
                            .map(|joint| joint.centroid * joint.area)
                            .sum::<Vec3>()
                            / area;
                        assert!((footprint_centroid - centre).length() < 1e-4);
                    }
                    _ => assert_eq!(actual, bond, "fixture attachment {name}"),
                }
                checked += 1;
            }
            assert!(checked > 0, "the fixture exercises a foundation");
            assert_eq!(checked, ground.len());
        }
    }

    /// The game's environment destruction model refuses ground rests without a friction pair;
    /// the inspector must report that same admission failure instead of inventing a seam.
    #[test]
    fn a_resting_foundation_is_refused_by_the_inspector() {
        let asset = foundation_fixture("resting");
        let error = cut(&asset).unwrap_err();
        assert!(error.contains("ground states no friction pair"), "{error}");
        assert!(error.contains(&asset.id), "{error}");
    }

    /// The sandbox must draw real shared interfaces. Fixture edits move either the declared
    /// interface off both chunks or only its second chunk away. Inventing a square, or clipping
    /// against an absent second face as if it imposed no bounds, must refuse the bake.
    #[test]
    fn an_interface_requires_both_chunk_faces() {
        let original = BuildingAsset::from_ron(BUILDING).unwrap();
        cut(&original).expect("the unmodified fixture reconstructs");
        let joint = original
            .joints
            .iter()
            .find(|j| selected(&original, j.a) && selected(&original, j.b))
            .expect("the fixture has a selected interface")
            .clone();
        let normal = Vec3::from_array(joint.normal);
        let (low, high) = [joint.a, joint.b]
            .into_iter()
            .flat_map(|c| &original.chunks[c as usize].hull)
            .map(|p| normal.dot(Vec3::from_array(*p)))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), p| {
                (lo.min(p), hi.max(p))
            });
        // Fixture: one metre beyond the pair's full span cannot meet either original face.
        let shift = normal * (high - low + 1.0);
        for move_chunk in [false, true] {
            let mut asset = original.clone();
            asset.joints = vec![joint.clone()];
            if move_chunk {
                for p in &mut asset.chunks[joint.b as usize].hull {
                    *p = (Vec3::from_array(*p) + shift).to_array();
                }
            } else {
                asset.joints[0].centroid = (Vec3::from_array(joint.centroid) + shift).to_array();
            }
            asset
                .check()
                .expect("the fixture still meets schema admission");
            let Err(error) = cut(&asset) else {
                panic!("an absent interface must refuse the bake");
            };
            assert!(
                error.contains(&format!("{}/{}.building.ron", asset.id, asset.id))
                    && error.contains("joints[0]")
                    && error.contains("rebake"),
                "{error}"
            );
        }
    }

    /// The sandbox's ground interface must come from the anchored chunk. A square fallback
    /// hides this fixture's chunk translated above its still-admitted foundation declaration.
    #[test]
    fn an_anchor_requires_its_chunk_face() {
        let mut asset = BuildingAsset::from_ron(BUILDING).unwrap();
        cut(&asset).expect("the unmodified fixture reconstructs");
        let anchor = asset
            .anchors
            .iter()
            .find(|a| selected(&asset, a.chunk))
            .expect("the fixture has a selected anchor")
            .clone();
        asset.joints.clear();
        asset.anchors = vec![anchor.clone()];
        let hull = &mut asset.chunks[anchor.chunk as usize].hull;
        let low = hull.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        // Fixture: lift the whole chunk at least one metre above its declared foundation.
        let lift = asset.foundation_y_m - low + 1.0;
        for p in hull {
            p[1] += lift;
        }
        asset
            .check()
            .expect("the fixture still meets schema admission");
        let Err(error) = cut(&asset) else {
            panic!("an absent anchor face must refuse the bake");
        };
        assert!(
            error.contains(&format!("{}/{}.building.ron", asset.id, asset.id))
                && error.contains("anchors[0]")
                && error.contains("rebake"),
            "{error}"
        );
    }
}
