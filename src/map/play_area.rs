//! Checked map bounds in world XZ metres. Only `PlayArea::checked` constructs the admitted type;
//! consumers borrow its polygon and read its bounds without parsing or validating again.

use std::path::Path;

use crate::{provenance::Sourced, terrain_grid::TerrainExtent};

/// Resolution: a thick solid for discrete collision detection; the reference case of 100 m/s
/// at 64 Hz travels 1.5625 m per tick (DERIVED).
pub(crate) const WALL_THICKNESS_M: f32 = 20.0;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlayArea {
    polygon: Box<[[f32; 2]]>,
    floor_m: f32,
    ceiling_m: f32,
    friction: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct BoundaryFile {
    polygon: Sourced<Vec<[f32; 2]>>,
    floor_m: Sourced<f32>,
    ceiling_m: Sourced<f32>,
    friction: Sourced<f32>,
}

impl PlayArea {
    pub(super) fn checked(text: &str, extent: TerrainExtent, path: &Path) -> Self {
        let file = path.display().to_string();
        let data: BoundaryFile = crate::provenance::shipped(&file, || {
            let mut deserializer = ron::Deserializer::from_str(text)?;
            let value = serde_path_to_error::deserialize(&mut deserializer)
                .map_err(|err| deserializer.span_error(ron::Error::Message(err.to_string())))?;
            deserializer
                .end()
                .map_err(|err| deserializer.span_error(err))?;
            Ok::<_, ron::error::SpannedError>(value)
        })
        .unwrap_or_else(|err| {
            panic!(
                "map: {err} — author every required boundary field with valid values and provenance"
            )
        });
        let area = Self {
            polygon: data.polygon.0.into_boxed_slice(),
            floor_m: data.floor_m.0,
            ceiling_m: data.ceiling_m.0,
            friction: data.friction.0,
        };
        let polygon = area.polygon();
        assert!(
            polygon.len() >= 3,
            "map: {file} field polygon — author at least three vertices, without repeating the closing vertex"
        );
        let half = extent.world_size_m * 0.5;
        for (index, vertex) in polygon.iter().enumerate() {
            assert!(
                vertex.iter().all(|v| v.is_finite() && v.abs() <= half),
                "map: {file} field polygon[{index}] — author finite XZ metres inside the terrain square, including its edge"
            );
        }
        for index in 0..polygon.len() {
            assert!(
                polygon[index] != polygon[(index + 1) % polygon.len()],
                "map: {file} field polygon[{index}] — remove the repeated vertex; every edge must have length"
            );
        }
        for index in 0..polygon.len() {
            let a = polygon[index];
            let b = polygon[(index + 1) % polygon.len()];
            let c = polygon[(index + 2) % polygon.len()];
            // Collinear continuation is legal; doubling back overlaps adjacent edges.
            assert!(
                orient(a, b, c) != 0.0 || dot_edges(a, b, c) >= 0.0,
                "map: {file} field polygon[{index}] — remove the overlapping adjacent edges to make a simple polygon"
            );
        }
        for index in 0..polygon.len() {
            let a = polygon[index];
            let b = polygon[(index + 1) % polygon.len()];
            for other in (index + 1)..polygon.len() {
                if other == index + 1 || (index == 0 && other == polygon.len() - 1) {
                    continue; // Adjacent edges share exactly their common vertex.
                }
                assert!(
                    !intersects(a, b, polygon[other], polygon[(other + 1) % polygon.len()]),
                    "map: {file} field polygon edges {index} and {other} — remove crossings and nonadjacent touches to make a simple polygon"
                );
            }
        }
        // Shoelace sum is twice signed area. Widen before products to avoid f32 overflow.
        let twice_area: f64 = polygon
            .iter()
            .zip(polygon.iter().cycle().skip(1))
            .map(|(a, b)| f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]))
            .sum();
        assert!(
            twice_area > 0.0,
            "map: {file} field polygon — author counter-clockwise XZ vertices with positive signed area"
        );
        assert!(
            area.floor_m().is_finite() && area.floor_m() < extent.height_offset_m,
            "map: {file} field floor_m — author a finite floor below height_decode.offset_m ({})",
            extent.height_offset_m
        );
        let highest = extent.height_offset_m + extent.height_span_m;
        assert!(
            area.ceiling_m().is_finite() && area.ceiling_m() > highest,
            "map: {file} field ceiling_m — author a finite ceiling above height_decode.offset_m + scale_m ({highest})"
        );
        assert!(
            area.friction().is_finite() && area.friction() >= 0.0,
            "map: {file} field friction — author a finite nonnegative contact friction"
        );
        area.check_outward_boxes(path);
        area
    }

    /// Enclose a convex footprint, including its edges, in a simple polygon without holes.
    /// Pairwise segments include its hull edges; midpoint intervals detect concave notches
    /// even when every supplied vertex is inside. Boundary contact is permitted.
    pub(super) fn contains_points(&self, points: &[[f32; 2]]) -> bool {
        let polygon: Vec<_> = self.polygon.iter().copied().map(widen).collect();
        let points: Vec<_> = points.iter().copied().map(widen).collect();
        points
            .iter()
            .all(|point| point.iter().all(|value| value.is_finite()) && inside(&polygon, *point))
            && points.iter().enumerate().all(|(index, a)| {
                points[index + 1..]
                    .iter()
                    .all(|b| segment_inside(&polygon, *a, *b))
            })
    }

    fn check_outward_boxes(&self, path: &Path) {
        let polygon: Vec<_> = self.polygon.iter().copied().map(widen).collect();
        for (index, (a, b)) in edges(&polygon).enumerate() {
            let delta = sub(b, a);
            let length_sq = dot(delta, delta);
            let depth = f64::from(WALL_THICKNESS_M) * length_sq.sqrt();
            // Unnormalized edge coordinates: along = dot(delta, p-a), outward = -cross.
            // This keeps the inner face exactly on its authored edge, without offset rounding.
            let local: Vec<_> = polygon
                .iter()
                .map(|point| {
                    let relative = sub(*point, a);
                    [dot(delta, relative), -cross(delta, relative)]
                })
                .collect();
            let center = [
                a[0] + delta[0] * 0.5
                    + delta[1] * f64::from(WALL_THICKNESS_M) / (2.0 * length_sq.sqrt()),
                a[1] + delta[1] * 0.5
                    - delta[0] * f64::from(WALL_THICKNESS_M) / (2.0 * length_sq.sqrt()),
            ];
            // Positive-area overlap either contains an original boundary segment in the open
            // box, or contains the whole box (whose center then lies inside the polygon).
            let overlap = edges(&local).any(|(p, q)| segment_enters_box(p, q, [length_sq, depth]))
                || inside(&polygon, center);
            assert!(
                !overlap,
                "map: {} field polygon edge {index} — widen or reshape the notch so its outward {WALL_THICKNESS_M} m wall box does not enter the play-area interior",
                path.display(),
            );
        }
    }

    pub(crate) fn polygon(&self) -> &[[f32; 2]] {
        &self.polygon
    }
    pub(crate) fn floor_m(&self) -> f32 {
        self.floor_m
    }
    pub(crate) fn ceiling_m(&self) -> f32 {
        self.ceiling_m
    }
    pub(crate) fn friction(&self) -> f32 {
        self.friction
    }
}

type Point = [f64; 2];

fn widen(point: [f32; 2]) -> Point {
    point.map(f64::from)
}

fn sub(a: Point, b: Point) -> Point {
    [a[0] - b[0], a[1] - b[1]]
}

fn dot(a: Point, b: Point) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}

fn cross(a: Point, b: Point) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

fn edges(polygon: &[Point]) -> impl Iterator<Item = (Point, Point)> + '_ {
    polygon
        .iter()
        .copied()
        .zip(polygon.iter().copied().cycle().skip(1))
}

fn inside(polygon: &[Point], point: Point) -> bool {
    let mut enclosed = false;
    for (a, b) in edges(polygon) {
        if cross(sub(b, a), sub(point, a)) == 0.0
            && (0..2).all(|axis| {
                point[axis] >= a[axis].min(b[axis]) && point[axis] <= a[axis].max(b[axis])
            })
        {
            return true;
        }
        if (a[1] > point[1]) != (b[1] > point[1])
            && point[0] < a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1])
        {
            enclosed = !enclosed;
        }
    }
    enclosed
}

fn segment_inside(polygon: &[Point], a: Point, b: Point) -> bool {
    let delta = sub(b, a);
    let length_sq = dot(delta, delta);
    if length_sq == 0.0 {
        return true; // contains_points already checked this repeated projected corner.
    }
    let mut cuts = vec![0.0, 1.0];
    let mut border = Vec::new();
    for (c, d) in edges(polygon) {
        let edge = sub(d, c);
        let relative = sub(c, a);
        let denominator = cross(delta, edge);
        if denominator == 0.0 {
            if cross(relative, delta) == 0.0 {
                let endpoints = [c, d].map(|point| dot(sub(point, a), delta) / length_sq);
                border.push([
                    endpoints[0].min(endpoints[1]),
                    endpoints[0].max(endpoints[1]),
                ]);
                cuts.extend(endpoints.into_iter().filter(|t| (0.0..=1.0).contains(t)));
            }
        } else {
            let t = cross(relative, edge) / denominator;
            let u = cross(relative, delta) / denominator;
            if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
                cuts.push(t);
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    cuts.windows(2).all(|pair| {
        let t = (pair[0] + pair[1]) * 0.5;
        // Collinear overlap is legal without interpolating back off its boundary by rounding.
        border.iter().any(|range| t >= range[0] && t <= range[1])
            || inside(polygon, [a[0] + t * delta[0], a[1] + t * delta[1]])
    })
}

fn segment_enters_box(a: Point, b: Point, size: Point) -> bool {
    let (mut low, mut high) = (0.0_f64, 1.0_f64);
    for axis in 0..2 {
        let delta = b[axis] - a[axis];
        if delta == 0.0 {
            if a[axis] <= 0.0 || a[axis] >= size[axis] {
                return false;
            }
        } else {
            let first = -a[axis] / delta;
            let last = (size[axis] - a[axis]) / delta;
            low = low.max(first.min(last));
            high = high.min(first.max(last));
        }
    }
    low < high
}

fn orient(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    (f64::from(b[0]) - f64::from(a[0])) * (f64::from(c[1]) - f64::from(a[1]))
        - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(a[0]))
}

fn dot_edges(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    (0..2)
        .map(|i| (f64::from(b[i]) - f64::from(a[i])) * (f64::from(c[i]) - f64::from(b[i])))
        .sum()
}

fn intersects(a: [f32; 2], b: [f32; 2], c: [f32; 2], d: [f32; 2]) -> bool {
    let ab = [orient(a, b, c), orient(a, b, d)];
    let cd = [orient(c, d, a), orient(c, d, b)];
    let on = |p: [f32; 2], q: [f32; 2], r: [f32; 2]| {
        (0..2).all(|i| r[i] >= p[i].min(q[i]) && r[i] <= p[i].max(q[i]))
    };
    (ab[0] * ab[1] < 0.0 && cd[0] * cd[1] < 0.0)
        || (ab[0] == 0.0 && on(a, b, c))
        || (ab[1] == 0.0 && on(a, b, d))
        || (cd[0] == 0.0 && on(c, d, a))
        || (cd[1] == 0.0 && on(c, d, b))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    // Fixture: a finite square and decodable height interval, independent of any shipped map.
    const EXTENT: TerrainExtent = TerrainExtent {
        world_size_m: 20.0,
        height_offset_m: -2.0,
        height_span_m: 5.0,
    };

    pub(crate) fn boundary_text(extent: TerrainExtent) -> String {
        let half = extent.world_size_m * 0.5;
        format!(
            r#"(
            polygon: ([(-{half}, -{half}), ({half}, -{half}), ({half}, {half}), (-{half}, {half})], StandIn("fixture square")),
            floor_m: ({}, StandIn("fixture floor")),
            ceiling_m: ({}, StandIn("fixture ceiling")),
            friction: (0.0, Taste("fixture grip")),
        )"#,
            extent.height_offset_m - 1.0,
            extent.height_offset_m + extent.height_span_m + 1.0
        )
    }

    pub(crate) fn polygon_text(extent: TerrainExtent, polygon: &str) -> String {
        let mut text = boundary_text(extent);
        let start = text.find('[').unwrap();
        let end = text.find(']').unwrap() + 1;
        text.replace_range(start..end, polygon);
        text
    }

    fn checked(text: &str) -> PlayArea {
        PlayArea::checked(text, EXTENT, Path::new("fixture/boundary.ron"))
    }

    /// Server and wall consumers require simple, oriented, bounded polygons; each mutant admits
    /// a different invalid class. Fixture coordinates carry no shipped map constraint.
    #[test]
    fn polygon_refusals_name_the_file_field_and_fix() {
        for (polygon, fix) in [
            ("[]", "at least three"),
            ("[(0.0, 0.0), (1.0, 0.0)]", "at least three"),
            ("[(NaN, 0.0), (1.0, 0.0), (0.0, 1.0)]", "finite XZ"),
            ("[(inf, 0.0), (1.0, 0.0), (0.0, 1.0)]", "finite XZ"),
            ("[(0.0, 0.0), (11.0, 0.0), (0.0, 1.0)]", "terrain square"),
            ("[(0.0, 0.0), (0.0, 1.0), (1.0, 0.0)]", "counter-clockwise"),
            (
                "[(0.0, 0.0), (3.0, 0.0), (0.0, 2.0), (2.0, 2.0)]",
                "remove crossings",
            ),
            (
                "[(0.0, 0.0), (1.0, 0.0), (1.0, 0.0), (0.0, 1.0)]",
                "repeated vertex",
            ),
            (
                "[(0.0, 0.0), (2.0, 0.0), (1.0, 0.0), (0.0, 1.0)]",
                "overlapping adjacent",
            ),
            (
                "[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]",
                "overlapping adjacent",
            ),
            (
                "[(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (1.0, 0.0), (0.0, 2.0)]",
                "nonadjacent touches",
            ),
        ] {
            let text = polygon_text(EXTENT, polygon);
            refusal(&text, "polygon", fix);
        }
    }

    fn refusal(text: &str, field: &str, fix: &str) {
        let err =
            std::panic::catch_unwind(|| checked(text)).expect_err("the load gate must refuse");
        let reason = err.downcast_ref::<String>().expect("formatted refusal");
        for part in ["fixture/boundary.ron", field, fix] {
            assert!(reason.contains(part), "{reason}");
        }
    }

    /// Floor retirement and side-wall consumers require strict, finite vertical enclosure.
    /// Friction and provenance are likewise admitted once, before any consumer can read them.
    #[test]
    fn scalar_and_provenance_refusals_identify_the_repair() {
        let valid = boundary_text(EXTENT);
        for (from, to, field, fix) in [
            (
                "floor_m: (-3",
                "floor_m: (-2",
                "floor_m",
                "below height_decode",
            ),
            ("floor_m: (-3", "floor_m: (NaN", "floor_m", "finite floor"),
            (
                "ceiling_m: (4",
                "ceiling_m: (3",
                "ceiling_m",
                "above height_decode",
            ),
            (
                "ceiling_m: (4",
                "ceiling_m: (inf",
                "ceiling_m",
                "finite ceiling",
            ),
            (
                "friction: (0.0",
                "friction: (-1.0",
                "friction",
                "nonnegative",
            ),
            ("friction: (0.0", "friction: (NaN", "friction", "finite"),
            (
                "Taste(\"fixture grip\")",
                "Fixture(\"fixture grip\")",
                "friction",
                "replace this field's label",
            ),
            (
                "Taste(\"fixture grip\")",
                "Taste(\"\")",
                "friction",
                "provenance",
            ),
            (
                "friction: (0.0, Taste(\"fixture grip\"))",
                "friction: 0.0",
                "friction",
                "provenance",
            ),
        ] {
            refusal(&valid.replace(from, to), field, fix);
        }
    }

    /// Irregular simple polygons and collinear continuation are supported by the single format;
    /// wall consumers must not accidentally restrict the type to its provisional square fixture.
    #[test]
    fn concave_and_collinear_polygons_are_admitted() {
        // Fixture: an L-shaped play area with enough room for its outward boxes.
        let extent = TerrainExtent {
            world_size_m: 200.0,
            ..EXTENT
        };
        let text = polygon_text(
            extent,
            "[(-100,-100), (0,-100), (100,-100), (100,100), (0,100), (0,0), (-100,0)]",
        );
        let area = PlayArea::checked(&text, extent, Path::new("fixture/boundary.ron"));
        assert_eq!(area.polygon().len(), 7);
        checked(&boundary_text(EXTENT)); // The terrain edge itself is included.
    }

    /// Wall consumers rely on edge boxes staying outside the admitted region. Mutant: testing
    /// only the polygon's own simplicity admits a notch narrower than its outward extrusion.
    #[test]
    fn wall_box_refusal_names_the_notch_and_its_repair() {
        // Fixture: notch widths below, equal to, and above the actual extrusion depth.
        let extent = TerrainExtent {
            world_size_m: WALL_THICKNESS_M * 10.0,
            ..EXTENT
        };
        let half = extent.half_extent();
        for (ratio, admitted) in [(0.5, false), (1.0, true), (2.0, true)] {
            let x = WALL_THICKNESS_M * ratio * 0.5;
            let polygon = format!(
                "[(-{half},-{half}), ({half},-{half}), ({half},{half}), ({x},{half}), ({x},0), (-{x},0), (-{x},{half}), (-{half},{half})]"
            );
            let text = polygon_text(extent, &polygon);
            let result = std::panic::catch_unwind(|| {
                PlayArea::checked(&text, extent, Path::new("fixture/boundary.ron"))
            });
            if admitted {
                result.expect("a wide notch and boundary-only contact must pass");
            } else {
                let refusal = result.expect_err("a narrow notch must fail before play");
                let reason = refusal.downcast_ref::<String>().unwrap();
                for part in [
                    "fixture/boundary.ron",
                    "field polygon edge",
                    "widen or reshape the notch",
                ] {
                    assert!(reason.contains(part), "{reason}");
                }
            }
        }
    }

    /// The spawn picker and placement gate must enclose whole convex footprints. Checking only
    /// corners admits a footprint bridging a notch; refusing boundary contact also rejects legal data.
    #[test]
    fn convex_footprints_cannot_bridge_concavity() {
        // Fixture: an L-shaped region, with a reflex vertex at the origin.
        let extent = TerrainExtent {
            world_size_m: 200.0,
            ..EXTENT
        };
        let text = polygon_text(
            extent,
            "[(-100,-100), (100,-100), (100,100), (0,100), (0,0), (-100,0)]",
        );
        let area = PlayArea::checked(&text, extent, Path::new("fixture/boundary.ron"));
        let corners = [[-50.0, -10.0], [10.0, -10.0], [10.0, 50.0]];
        assert!(corners.iter().all(|point| area.contains_points(&[*point])));
        assert!(!area.contains_points(&corners));
        assert!(!area.contains_points(&[[-100.0, 0.0], [0.0, 0.0], [0.0, 100.0]]));
        assert!(area.contains_points(&[[-100.0, 0.0], [100.0, 0.0]]));
        assert!(area.contains_points(&[[0.0, 0.0], [0.0, 100.0]]));
    }
}
