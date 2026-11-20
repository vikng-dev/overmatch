//! The struck construction at a ram capture's end, in the game's building-debris model (game state
//! only): the struck panel's opening, how much of the construction stands in place, and the
//! largest loose piece. Read in the construction frame and keyed by chunk, never by entity.
//!
//! IN PLACE. A chunk's displacement is the farthest any corner of its hull lies from where it was
//! built, so a chunk turned about its centroid has moved. A chunk is in place while a body carries
//! it (its fate there is standing) with its displacement within the recipe's `in_place_m`, one
//! `Unit` for every chunk whose note says why that scale. A standing chunk and a roof resting on
//! its walls count, a heap does not. The share counts volume built above grade ([`GRADE_M`], the
//! centroid's height); chunks built below it are counted apart. The volume-weighted displacement of
//! what a body still carries above grade is read beside it, for a reader judging the limit.
//!
//! PANEL. The construction component the approaching tank meets first: the element that a ray
//! along the recipe's heading from any point of the tank's collision construction enters first.
//! Chosen before play from the heading and the two geometries, never by the construction's shape.
//! Its face is the plane that ray enters through, clipped at grade; the opening is that face's
//! area over the panel's chunks not in place.

use bevy::prelude::*;
use serde::Serialize;

use crate::construction::{
    ConstructionKind,
    breakage::{Breakage, GRADE_M, Piece},
    fracture::{Block, Fate},
};

/// The panel the approach meets.
#[derive(Serialize)]
pub(super) struct Panel {
    #[serde(skip)]
    component: u16,
    /// The component's authored name.
    name: String,
    /// Where the approach ray enters it, and its face's outward normal, construction frame.
    at: [f32; 3],
    normal: [f32; 3],
    /// The face's area above grade, m².
    face_m2: f32,
}

impl Panel {
    /// The component whose element a ray from one of `origins` along unit `heading` (construction
    /// frame) enters first, the first origin's lowest element on a tie. Refused, naming the recipe
    /// field, for a construction with no breakable matter above grade and for rays that meet no
    /// element or a face with no area above grade.
    pub(super) fn met(
        kind: &ConstructionKind,
        origins: impl IntoIterator<Item = Vec3>,
        heading: Vec3,
    ) -> Result<Self, &'static str> {
        if !(kind.layout.chunks.iter())
            .any(|&chunk| kind.asset.chunks[chunk as usize].centroid[1] >= GRADE_M)
        {
            return Err("an instance with breakable matter above grade");
        }
        let missed =
            "a heading_deg whose approach meets a panel of the instance with face area above grade";
        let blocks = &kind.layout.element_blocks;
        let (element, _, at, normal) = (origins.into_iter())
            .flat_map(|origin| {
                (blocks.iter().enumerate()).filter_map(move |(k, block)| {
                    let (d, n) = block.entry_face(origin, heading)?;
                    Some((k, d, origin + heading * d, n))
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .ok_or(missed)?;
        let component = kind.asset.elements[element].component;
        let face_m2 = (kind.asset.elements.iter().zip(blocks))
            .filter(|(element, _)| element.component == component)
            .map(|(_, block)| face_area(block, at, normal))
            .sum();
        if face_m2 <= 0.0 {
            return Err(missed);
        }
        Ok(Self {
            component,
            name: kind.asset.components[usize::from(component)].clone(),
            at: at.to_array(),
            normal: normal.to_array(),
            face_m2,
        })
    }
}

/// The area of `block`'s face on the plane through `at` of unit `normal`, above grade, m².
fn face_area(block: &Block, at: Vec3, normal: Vec3) -> f32 {
    let face = block.face_on(at, normal);
    // Each edge crossing grade is cut there; corners below it drop out.
    let mut above = Vec::with_capacity(face.len() + 1);
    for (k, &a) in face.iter().enumerate() {
        let b = face[(k + 1) % face.len()];
        if a.y >= GRADE_M {
            above.push(a);
        }
        if (a.y >= GRADE_M) != (b.y >= GRADE_M) {
            above.push(a + (b - a) * ((GRADE_M - a.y) / (b.y - a.y)));
        }
    }
    let twice: Vec3 = (1..above.len().saturating_sub(1))
        .map(|k| (above[k] - above[0]).cross(above[k + 1] - above[0]))
        .sum();
    0.5 * twice.dot(normal).abs()
}

/// The construction at a capture's end.
#[derive(Serialize, Debug, PartialEq)]
pub(super) struct Reading {
    /// The construction's ordinal and its chunks not in place, ascending: the key two runs
    /// compare by (`scripts/debris-lab`), never an entity.
    construction: u16,
    displaced: Vec<u16>,
    /// Volume built above grade, m³, and the part of it in place.
    above_grade_m3: f32,
    in_place_m3: f32,
    in_place_share: f32,
    /// Volume built above grade that a body still carries, m³, and its displacement weighted by
    /// volume, m; none while a body carries none of it.
    carried_m3: f32,
    displacement_m: Option<f32>,
    /// Chunks built below grade, outside the share.
    below_grade_chunks: usize,
    /// The panel's face area over its chunks not in place, m².
    opening_m2: f32,
    /// The loose piece carrying the most chunks, its lowest chunk breaking a tie; none while no
    /// piece is loose.
    largest_piece: Option<LargestPiece>,
}

#[derive(Serialize, Debug, PartialEq)]
struct LargestPiece {
    chunks: usize,
    speed_m_s: f32,
}

/// The reading of `root`, the construction's standing body at its built frame, and `pieces`, its
/// live loose pieces each with its frame and speed (m/s), under the in-place limit `in_place_m`.
pub(super) fn read<'a>(
    panel: &Panel,
    in_place_m: f32,
    (root, built): (&Breakage, Transform),
    pieces: impl Iterator<Item = (&'a Piece, &'a Breakage, Transform, f32)>,
) -> Reading {
    let chunks = &root.live.structure.chunks;
    let mut carried: Vec<Option<Transform>> = (root.live.state.fate.iter())
        .map(|fate| (*fate == Fate::Standing).then_some(built))
        .collect();
    let mut largest: Option<(usize, u16, f32)> = None;
    for (piece, breakage, frame, speed) in pieces {
        let held: Vec<u16> = (piece.chunks.iter().copied())
            .filter(|&c| breakage.live.state.fate[usize::from(c)] == Fate::Standing)
            .collect();
        for &c in &held {
            assert!(
                carried[usize::from(c)].replace(frame).is_none(),
                "one body carries construction {}'s chunk {c}",
                root.construction
            );
        }
        if let Some(&first) = held.first()
            && largest.is_none_or(|(n, lowest, _)| (held.len(), lowest) > (n, first))
        {
            largest = Some((held.len(), first, speed));
        }
    }
    let kind = &root.kind;
    let (at, normal) = (Vec3::from(panel.at), Vec3::from(panel.normal));
    let (mut above, mut in_place_m3, mut below, mut opening) = (0.0, 0.0, 0, 0.0);
    // Carried volume, m³, and the sum of its chunks' volume times displacement, m⁴.
    let (mut carried_m3, mut moved_m4) = (0.0, 0.0);
    let mut displaced = Vec::new();
    for (c, chunk) in chunks.iter().enumerate() {
        let block = &kind.layout.blocks[c];
        let moved = carried[c].map(|frame| {
            (block.points.iter())
                .map(|&p| frame.transform_point(p).distance(built.transform_point(p)))
                .fold(0.0, f32::max)
        });
        let in_place = moved.is_some_and(|moved| moved <= in_place_m);
        if chunk.centroid.y < GRADE_M {
            below += 1;
        } else {
            above += chunk.volume;
            in_place_m3 += if in_place { chunk.volume } else { 0.0 };
            if let Some(moved) = moved {
                carried_m3 += chunk.volume;
                moved_m4 += chunk.volume * moved;
            }
        }
        if !in_place {
            displaced.push(u16::try_from(c).expect("the layout refuses law chunks past 16 bits"));
        }
        let element = usize::from(kind.layout.element_of[c]);
        if !in_place && kind.asset.elements[element].component == panel.component {
            opening += face_area(block, at, normal);
        }
    }
    Reading {
        construction: root.construction,
        displaced,
        above_grade_m3: above,
        in_place_m3,
        in_place_share: in_place_m3 / above,
        carried_m3,
        displacement_m: (carried_m3 > 0.0).then(|| moved_m4 / carried_m3),
        below_grade_chunks: below,
        opening_m2: opening,
        largest_piece: largest.map(|(chunks, _, speed_m_s)| LargestPiece { chunks, speed_m_s }),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::construction::tests::column_construction_from;

    /// Fixture: a column of three unit cubes standing 1 m below grade, and a ray along +X at
    /// 1 m over grade meeting its west face.
    pub(in crate::net::ram) fn column_panel() -> Panel {
        let kind = column_construction_from(-1.0);
        Panel::met(&kind, [Vec3::new(-5.0, 1.0, 0.0)], Vec3::X).expect("the column faces the ray")
    }

    /// HOW MUCH STANDS: a chunk is in place while every corner lies within the one limit of where
    /// it was built; the share counts volume built above grade, the mean displacement weighs what a
    /// body carries there by volume, and the opening is the panel's face over chunks not in place,
    /// clipped at grade. The ram investigation compares passes by them. A mutant reading the
    /// centroid keeps the turned cube in place; one counting the buried cube, keeping a displaced
    /// cube in place or the face below grade reads otherwise.
    #[test]
    fn a_chunk_stands_in_place_within_one_limit_above_grade() {
        let kind = column_construction_from(-1.0);
        let panel = column_panel();
        assert_eq!(panel.face_m2, 2.0, "a 3 m column standing 1 m below grade");
        let mut root = Breakage::new(kind.clone(), 3);
        root.live.state.fate[2] = Fate::Fallen;
        let mut loose = Breakage::new(kind, 3);
        loose.live.state.fate = vec![Fate::Dust, Fate::Dust, Fate::Standing];
        let piece = Piece {
            construction: 3,
            chunks: vec![2],
            born: 0,
            retired: None,
        };
        // Fixture: a half-metre limit, and the top cube lifted within it, past it, and turned a
        // radian about its centroid, which moves a corner 2·(√2/2)·sin(½) ≈ 0.68 m (DERIVED).
        let mut turned = Transform::IDENTITY;
        turned.rotate_around(
            root.live.structure.chunks[2].centroid,
            Quat::from_rotation_x(1.0),
        );
        let turn_m = 2.0 * std::f32::consts::FRAC_1_SQRT_2 * 0.5_f32.sin();
        for (name, frame, moved, displaced, in_place_m3, opening_m2) in [
            (
                "lifted 0.4 m",
                Transform::from_xyz(0.0, 0.4, 0.0),
                0.4,
                vec![],
                2.0,
                0.0,
            ),
            (
                "lifted 1 m",
                Transform::from_xyz(0.0, 1.0, 0.0),
                1.0,
                vec![2],
                1.0,
                1.0,
            ),
            ("turned 1 rad", turned, turn_m, vec![2], 1.0, 1.0),
        ] {
            let pieces = [(&piece, &loose, frame, 3.0)].into_iter();
            let reading = read(&panel, 0.5, (&root, Transform::IDENTITY), pieces);
            // The column's standing cube is where it was built; the top cube moved `moved`.
            let mean = reading
                .displacement_m
                .expect("a body carries the column above grade");
            assert!(
                (mean - moved / 2.0).abs() < 1e-5,
                "{name}: mean displacement {mean} m"
            );
            assert_eq!(
                Reading {
                    displacement_m: None,
                    ..reading
                },
                Reading {
                    construction: 3,
                    displaced,
                    above_grade_m3: 2.0,
                    in_place_m3,
                    in_place_share: in_place_m3 / 2.0,
                    carried_m3: 2.0,
                    displacement_m: None,
                    below_grade_chunks: 1,
                    opening_m2,
                    largest_piece: Some(LargestPiece {
                        chunks: 1,
                        speed_m_s: 3.0
                    }),
                },
                "{name}"
            );
        }
    }
}
