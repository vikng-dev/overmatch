//! Checked map bounds in world XZ metres. Only `PlayArea::checked` constructs the admitted type;
//! consumers borrow its polygon and read its bounds without parsing or validating again.

use std::path::Path;

use crate::{provenance::Sourced, terrain_grid::TerrainExtent};

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
        area
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

    fn checked(text: &str) -> PlayArea {
        PlayArea::checked(text, EXTENT, Path::new("fixture/boundary.ron"))
    }

    /// Server and wall consumers require simple, oriented, bounded polygons; each mutant admits
    /// a different invalid class. Fixture coordinates carry no shipped map constraint.
    #[test]
    fn polygon_refusals_name_the_file_field_and_fix() {
        let valid = boundary_text(EXTENT);
        let start = valid.find('[').unwrap();
        let end = valid.find(']').unwrap() + 1;
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
            let text = format!("{}{}{}", &valid[..start], polygon, &valid[end..]);
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
        let valid = boundary_text(EXTENT);
        let start = valid.find('[').unwrap();
        let end = valid.find(']').unwrap() + 1;
        let text = format!(
            "{}{}{}",
            &valid[..start],
            "[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0), (2.0, 2.0), (1.0, 1.0), (0.0, 2.0)]",
            &valid[end..]
        );
        let area = checked(&text);
        assert_eq!(area.polygon().len(), 6);
        checked(&valid); // The terrain edge itself is included.
    }
}
