//! World-contact patches for the game's running gear (game state only).
//! `patches` resolves coverage; `sample` supplies geometry, material and point motion to the
//! mechanical law. Collider identity addresses the tick-start snapshot's owner and reaction.
//! Grid columns sample one facet plane; solids retain exact clipped coverage. Rounded analytic
//! surfaces refine their sampled profiles where the normal turns.

use super::forces::{ForceParams, GripParams, profile_resultant, rigid_rolling_resistance};
use super::oracle::{ContactOracle, DepthOracle, TerrainContact, TerrainOracle};
use super::rubble::{RubbleBodies, Slab, SlabOverlap};
use bevy::prelude::*;

mod admission;
pub(super) mod convex;
mod terrain;
pub(crate) use admission::{plugin, solid};

/// A column holds one sampled grid plane through its integration. Analytic terrain remains
/// directional and solids retain their upper envelope. No integration sample re-queries the
/// grid; creases within a column are the quadrature's approximation.
pub(crate) struct ColumnOracle<'a, O> {
    field: &'a O,
    plane: Option<terrain::Plane>,
}

impl<'a, O: TerrainOracle> ColumnOracle<'a, O> {
    pub(crate) fn new(
        field: &'a O,
        point: Option<Vec3>,
        stages: Option<&crate::cost::stages::Stages>,
    ) -> Self {
        let plane = point.and_then(|point| {
            field.height_grid().and_then(|grid| {
                if let Some(stages) = stages {
                    stages.add(crate::cost::stages::Work::BeltTerrainSamples, 1);
                }
                terrain::Plane::sample(grid, point)
            })
        });
        Self { field, plane }
    }
}

impl<O: TerrainOracle> DepthOracle for ColumnOracle<'_, O> {
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        self.profile_depth_along(station, out, reach)
    }

    fn profile_depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32 {
        let field = self.field.profile_depth_along(station, out, reach);
        self.plane.as_ref().map_or(field, |plane| {
            field.max(plane.contact(station, out, reach).depth(reach))
        })
    }
}

impl<O: TerrainOracle> ContactOracle for ColumnOracle<'_, O> {
    fn ground_at(&self, point: Vec3) -> Option<crate::ground_field::ContactGround> {
        self.field.ground_at(point)
    }

    fn refine_profile_normals(&self) -> bool {
        self.field.refine_profile_normals()
    }

    fn contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
        self.profile_contact_along(station, out, reach)
    }

    fn profile_contact_along(&self, station: Vec3, out: Vec3, reach: f32) -> TerrainContact {
        let field = self.field.profile_contact_along(station, out, reach);
        self.plane.as_ref().map_or(field, |plane| {
            field.union(plane.contact(station, out, reach))
        })
    }
}

/// One shoe column's world-contact envelope and sampled free-travel profile, in metres.
pub(crate) struct PatchQuery<'a> {
    pub(crate) slab: &'a Slab,
    pub(crate) overlaps: &'a [SlabOverlap],
    pub(crate) strip: (f32, f32),
    pub(crate) field_offset: f32,
    pub(crate) weight: f32,
    pub(crate) pen: [f32; 3],
    pub(crate) travel: [f32; 3],
}

pub(crate) fn patches<O: TerrainOracle>(
    query: PatchQuery<'_>,
    params: &ForceParams,
    oracle: &O,
    pieces: &RubbleBodies,
) -> Vec<SupportPatch> {
    let PatchQuery {
        slab,
        overlaps,
        strip,
        field_offset,
        weight,
        pen,
        travel,
    } = query;
    let faces = overlaps
        .iter()
        .flat_map(|hit| hit.faces.iter().map(move |face| (hit.piece, face)));
    let affected = convex::raised_faces(faces, strip, slab.len, pen, travel);
    let field_patch = SupportPatch {
        origin: slab.origin + slab.lat * field_offset,
        len: slab.len,
        weight,
        pen,
        travel,
        face: None,
        integral: None,
    };
    let split;
    let patches = if affected.is_empty() {
        std::slice::from_ref(&field_patch)
    } else {
        pieces.count(
            crate::cost::stages::Work::BeltPartitionFaces,
            affected.len(),
        );
        split = convex::patches(slab, strip, field_offset, pen, travel, &affected, pieces);
        &split
    };
    terrain_surface_patches(patches, slab.axis, slab.out, params, oracle)
}

/// A force-bearing sample: world metres, unit normal, surface m/s and material coefficients.
/// `collider` is the snapshot key for mechanical response; the field has infinite inertia.
pub(crate) struct SurfaceSample {
    pub(crate) point: Vec3,
    pub(crate) normal: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) collider: Option<Entity>,
    pub(crate) mu: f32,
    pub(crate) rolling: f32,
}

pub(crate) fn sample<O: TerrainOracle>(
    patch: &SupportPatch,
    axis: Vec3,
    out: Vec3,
    centroid: f32,
    params: &ForceParams,
    oracle: &O,
    pieces: &RubbleBodies,
) -> Option<SurfaceSample> {
    let (point, normal) = patch_contact(patch, axis, out, centroid, params, oracle)?;
    let collider = patch.face.as_ref().map(|face| face.collider);
    let surface = surface_under(params.grip, collider, point, oracle, pieces);
    Some(SurfaceSample {
        point,
        normal,
        velocity: collider.map_or(Vec3::ZERO, |collider| pieces.velocity_at(collider, point)),
        collider,
        mu: surface.mu,
        rolling: surface.rolling,
    })
}

/// Interpolate `v` at `0`, `len/2`, `len`, keeping penetration and travel on the same shape.
fn sample_halves(x: f32, len: f32, v: [f32; 3]) -> f32 {
    let half = len / 2.0;
    if x <= half {
        v[0] + (v[1] - v[0]) * (x / half)
    } else {
        v[1] + (v[2] - v[1]) * ((x - half) / half)
    }
}

/// A sampled field profile or an exact clipped face. Resultants integrate positive penetration
/// in m² and contacting length in m, normalized by shoe width; `weight` scales field columns.
#[derive(Clone)]
pub(crate) struct SupportPatch {
    origin: Vec3,
    len: f32,
    pub(crate) weight: f32,
    pen: [f32; 3],
    travel: [f32; 3],
    face: Option<convex::ContactFace>,
    integral: Option<convex::Integral>,
}

impl SupportPatch {
    pub(crate) fn max_depth(&self) -> f32 {
        self.integral.as_ref().map_or_else(
            || self.pen.into_iter().fold(f32::NEG_INFINITY, f32::max),
            |i| i.max_depth,
        )
    }

    pub(crate) fn resultant(&self) -> Option<(f32, f32, f32)> {
        match &self.integral {
            Some(i) => Some((i.area, i.length, i.centroid.x)),
            None => profile_resultant(self.len, self.pen),
        }
    }
}

/// Resolution: surface quadrature stops below half the field's rounding radius. It confines
/// a curved lip's normal to its pressure share instead of rotating a whole link's load.
const SURFACE_PATCH_LENGTH_M: f32 = super::oracle::FIELD_ROUNDING / 2.0;
/// Resolution: squared unit-normal difference; below this the support direction error is
/// under about 0.6 degrees (DERIVED sqrt(1e-4) radians), avoiding subdivision of planar ground.
const SURFACE_NORMAL_ERROR_SQ: f32 = 1e-4;

fn terrain_surface_patches<O: TerrainOracle>(
    patches: &[SupportPatch],
    axis: Vec3,
    out: Vec3,
    params: &ForceParams,
    oracle: &O,
) -> Vec<SupportPatch> {
    if !oracle.refine_profile_normals() {
        return patches.to_vec();
    }
    fn split<O: TerrainOracle>(
        patch: SupportPatch,
        axis: Vec3,
        out: Vec3,
        params: &ForceParams,
        oracle: &O,
        result: &mut Vec<SupportPatch>,
    ) {
        let Some((_, _, centroid)) = patch.resultant() else {
            return; // A zero-pressure strip carries no surface resultant.
        };
        let hits = [0.0, patch.len / 2.0, patch.len, centroid].map(|x| {
            let travel = sample_halves(x, patch.len, patch.travel);
            oracle.profile_contact_along(
                patch.origin + axis * x + out * (params.face_offset + travel),
                out,
                params.probe_reach + travel,
            )
        });
        let aligned = match hits {
            [
                TerrainContact::Hit { normal: a, .. },
                TerrainContact::Hit { normal: m, .. },
                TerrainContact::Hit { normal: b, .. },
                TerrainContact::Hit { normal: c, depth },
            ] if depth > 0.0 => {
                (*a - *m)
                    .length_squared()
                    .max((*b - *m).length_squared())
                    .max((*c - *m).length_squared())
                    <= SURFACE_NORMAL_ERROR_SQ
            }
            _ => false,
        };
        if aligned || patch.len <= SURFACE_PATCH_LENGTH_M {
            result.push(patch);
            return;
        }
        let half = patch.len / 2.0;
        for start in [0.0, half] {
            let travel = [start, start + half / 2.0, start + half]
                .map(|x| sample_halves(x, patch.len, patch.travel));
            let origin = patch.origin + axis * start;
            let pen = [0.0, half / 2.0, half].map(|x| {
                let t = sample_halves(x, half, travel);
                oracle.profile_depth_along(
                    origin + axis * x + out * (params.face_offset + t),
                    out,
                    params.probe_reach + t,
                )
            });
            let integral = match &patch.integral {
                Some(integral) => match integral.slice(start, half) {
                    Some(child) => Some(child),
                    None => continue, // This half contains no positive covered field area.
                },
                None => None,
            };
            split(
                SupportPatch {
                    origin,
                    len: half,
                    pen,
                    travel,
                    integral,
                    ..patch
                },
                axis,
                out,
                params,
                oracle,
                result,
            );
        }
    }
    let mut result = Vec::new();
    for patch in patches {
        if patch.face.is_some() || patch.max_depth() <= 0.0 {
            result.push(patch.clone());
        } else {
            split(patch.clone(), axis, out, params, oracle, &mut result);
        }
    }
    result
}

/// The pressure centroid's own struck surface. A clearance hit cannot carry the load
/// sampled on neighbouring lips. Convex points are pressure centroids on their actual faces.
fn patch_contact<O: TerrainOracle>(
    patch: &SupportPatch,
    axis: Vec3,
    out: Vec3,
    centroid: f32,
    params: &ForceParams,
    oracle: &O,
) -> Option<(Vec3, Vec3)> {
    let travel = sample_halves(centroid, patch.len, patch.travel);
    let query = patch.origin + axis * centroid + out * (params.face_offset + travel);
    match &patch.face {
        Some(face) => Some((face.point, face.normal)),
        None => match oracle.profile_contact_along(query, out, params.probe_reach + travel) {
            TerrainContact::Hit { depth, normal } if depth > 0.0 => {
                Some((query - out * depth, *normal))
            }
            TerrainContact::Hit { .. } | TerrainContact::Miss | TerrainContact::Buried => None,
        },
    }
}

/// Contact capacity and the Driver read the same coefficients, independent of support elasticity.
pub(crate) struct SurfaceGrip {
    pub(crate) mu: f32,
    pub(crate) rolling: f32,
}

pub(crate) fn surface_under<O: TerrainOracle>(
    grip: Option<GripParams>,
    collider: Option<Entity>,
    p: Vec3,
    oracle: &O,
    pieces: &RubbleBodies,
) -> SurfaceGrip {
    match (grip, collider) {
        (None, _) => SurfaceGrip {
            mu: 0.0,
            rolling: 0.0,
        },
        (Some(grip), Some(collider)) => SurfaceGrip {
            mu: grip.pair(pieces.friction(collider)),
            rolling: rigid_rolling_resistance(),
        },
        (Some(grip), None) => match oracle.ground_at(p) {
            Some(ground) => SurfaceGrip {
                mu: ground.grip,
                rolling: ground.rolling_at(grip.nominal_pressure_kpa),
            },
            None => {
                #[cfg(test)]
                {
                    // Fixture: no-field calibration pads have effective grip 0.9 and no rolling load.
                    SurfaceGrip {
                        mu: 0.9,
                        rolling: 0.0,
                    }
                }
                #[cfg(not(test))]
                {
                    panic!(
                        "a force-bearing terrain contact requires its GroundField; publish ground facts with the terrain before simulation"
                    )
                }
            }
        },
    }
}
