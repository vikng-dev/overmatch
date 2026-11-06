//! World-contact patches for the game's running gear (game state only).
//! `patches` resolves coverage; `sample` supplies geometry, material and point motion to the
//! mechanical law. Collider identity addresses the tick-start snapshot's owner and reaction.
//! The field and collider adapters retain their current geometric approximations here.

use super::forces::{ForceParams, GripParams, profile_resultant, rigid_rolling_resistance};
use super::oracle::{TerrainContact, TerrainOracle};
use super::rubble::{RubbleBodies, Slab, SlabOverlap};
use bevy::prelude::*;

mod admission;
pub(crate) use admission::{plugin, solid};

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
    let affected = raised_footprints(overlaps, strip, slab.len, pen, travel);
    let field_patch = SupportPatch {
        origin: slab.origin + slab.lat * field_offset,
        len: slab.len,
        weight,
        pen,
        travel,
        collider: None,
    };
    let split;
    let patches = if affected.is_empty() {
        std::slice::from_ref(&field_patch)
    } else {
        split = footprint_patches(slab, strip, field_offset, pen, travel, &affected, pieces);
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
    patch: SupportPatch,
    axis: Vec3,
    out: Vec3,
    centroid: f32,
    params: &ForceParams,
    oracle: &O,
    pieces: &RubbleBodies,
) -> Option<SurfaceSample> {
    let (point, normal) = patch_contact(patch, axis, out, centroid, params, oracle)?;
    let surface = surface_under(params.grip, patch.collider, point, oracle, pieces);
    Some(SurfaceSample {
        point,
        normal,
        velocity: patch
            .collider
            .map_or(Vec3::ZERO, |collider| pieces.velocity_at(collider, point)),
        collider: patch.collider,
        mu: surface.mu,
        rolling: surface.rolling,
    })
}

/// Sample the three-point piecewise-linear profile (`v` at `0`, `len/2`, `len`) at `x` — the
/// same two-half interpolation the pressure profile itself is built from, so the penetration
/// and the travel datum at the centroid are read off ONE shape.
fn sample_halves(x: f32, len: f32, v: [f32; 3]) -> f32 {
    let half = len / 2.0;
    if x <= half {
        v[0] + (v[1] - v[0]) * (x / half)
    } else {
        v[1] + (v[2] - v[1]) * ((x - half) / half)
    }
}

/// A covered rectangle: `pen` is signed penetration (m) at pin/mid/pin, `len` is metres,
/// and `weight` is the covered fraction of shoe width. Both adapters feed one support law.
#[derive(Clone, Copy)]
pub(crate) struct SupportPatch {
    origin: Vec3,
    pub(crate) len: f32,
    pub(crate) weight: f32,
    pub(crate) pen: [f32; 3],
    travel: [f32; 3],
    collider: Option<Entity>,
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
    fn split<O: TerrainOracle>(
        patch: SupportPatch,
        axis: Vec3,
        out: Vec3,
        params: &ForceParams,
        oracle: &O,
        result: &mut Vec<SupportPatch>,
    ) {
        let Some((_, _, centroid)) = profile_resultant(patch.len, patch.pen) else {
            return; // A zero-pressure strip carries no surface resultant.
        };
        let hits = [0.0, patch.len / 2.0, patch.len, centroid].map(|x| {
            let travel = sample_halves(x, patch.len, patch.travel);
            oracle.contact_along(
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
                oracle.depth_along(
                    origin + axis * x + out * (params.face_offset + t),
                    out,
                    params.probe_reach + t,
                )
            });
            split(
                SupportPatch {
                    origin,
                    len: half,
                    pen,
                    travel,
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
    for &patch in patches {
        if patch.collider.is_some() || patch.pen.iter().all(|p| *p <= 0.0) {
            result.push(patch);
        } else {
            split(patch, axis, out, params, oracle, &mut result);
        }
    }
    result
}

/// Partition a strip at footprint edges and at field/piece depth crossings. Within each cell
/// only the deepest surface carries the belt, so stacked pieces cannot multiply its support.
/// Column widths are DERIVED from their quadrature shares, retaining the existing field rule
/// while a narrow piece takes exactly its covered fraction of the true shoe width.
fn footprint_patches(
    slab: &Slab,
    strip: (f32, f32),
    field_offset: f32,
    pen: [f32; 3],
    travel: [f32; 3],
    overlaps: &[SlabOverlap],
    pieces: &RubbleBodies,
) -> Vec<SupportPatch> {
    let mut us = vec![0.0, slab.len / 2.0, slab.len];
    let mut ls = vec![strip.0, strip.1];
    for hit in overlaps {
        us.extend([hit.u.0, hit.u.1]);
        ls.extend([
            hit.l.0.clamp(strip.0, strip.1),
            hit.l.1.clamp(strip.0, strip.1),
        ]);
    }
    us.sort_unstable_by(f32::total_cmp);
    us.dedup();
    ls.sort_unstable_by(f32::total_cmp);
    ls.dedup();
    let mut patches = Vec::new();
    for l in ls.windows(2) {
        let lateral = (l[0] + l[1]) / 2.0;
        for u in us.windows(2) {
            let mid = (u[0] + u[1]) / 2.0;
            let deepest = overlaps
                .iter()
                .filter(|hit| {
                    mid > hit.u.0 && mid < hit.u.1 && lateral > hit.l.0 && lateral < hit.l.1
                })
                .fold(None::<&SlabOverlap>, |best, hit| match best {
                    Some(best) if best.depth >= hit.depth => Some(best),
                    _ => Some(hit),
                });
            let field = |x| sample_halves(x, slab.len, pen);
            let local_travel = |x| sample_halves(x, slab.len, travel);
            let mut cuts = vec![u[0], u[1]];
            if let Some(hit) = deepest {
                let a = hit.depth + local_travel(u[0]) - field(u[0]);
                let b = hit.depth + local_travel(u[1]) - field(u[1]);
                if (a < 0.0 && b > 0.0) || (a > 0.0 && b < 0.0) {
                    cuts.insert(1, u[0] + (u[1] - u[0]) * a / (a - b));
                }
            }
            for range in cuts.windows(2) {
                let xs = [range[0], (range[0] + range[1]) / 2.0, range[1]];
                let winner = deepest.filter(|hit| hit.depth + local_travel(xs[1]) > field(xs[1]));
                patches.push(SupportPatch {
                    // A field cell retains its quadrature column's location. Only the covered
                    // piece area moves to its true lateral centroid, so a vanishing footprint
                    // cannot shift the remaining ground's roll lever arm.
                    origin: slab.origin
                        + slab.axis * range[0]
                        + slab.lat
                            * if winner.is_some() {
                                lateral
                            } else {
                                field_offset
                            },
                    len: range[1] - range[0],
                    weight: (l[1] - l[0]) / (slab.lateral.1 - slab.lateral.0),
                    pen: xs.map(|x| match winner {
                        Some(hit) => hit.depth + local_travel(x),
                        None => field(x),
                    }),
                    travel: xs.map(local_travel),
                    collider: winner.map(|hit| pieces.collider(hit.piece)),
                });
            }
        }
    }
    patches
}

/// Pieces that actually replace positive field support in this column strip. A buried or
/// laterally separate piece must not change the remaining field's quadrature or force bits.
fn raised_footprints(
    overlaps: &[SlabOverlap],
    strip: (f32, f32),
    len: f32,
    pen: [f32; 3],
    travel: [f32; 3],
) -> Vec<SlabOverlap> {
    overlaps
        .iter()
        .copied()
        .filter(|hit| {
            hit.l.1 > strip.0
                && hit.l.0 < strip.1
                && [hit.u.0, hit.u.1, (len / 2.0).clamp(hit.u.0, hit.u.1)]
                    .into_iter()
                    .any(|x| {
                        let depth = hit.depth + sample_halves(x, len, travel);
                        depth > 0.0 && depth > sample_halves(x, len, pen)
                    })
        })
        .collect()
}

/// The pressure centroid's own struck surface. A clearance hit cannot carry the load
/// sampled on neighbouring lips; piece overlap orientation remains the documented stand-in.
fn patch_contact<O: TerrainOracle>(
    patch: SupportPatch,
    axis: Vec3,
    out: Vec3,
    centroid: f32,
    params: &ForceParams,
    oracle: &O,
) -> Option<(Vec3, Vec3)> {
    let travel = sample_halves(centroid, patch.len, patch.travel);
    let query = patch.origin + axis * centroid + out * (params.face_offset + travel);
    match patch.collider {
        Some(_) => Some((
            query - out * sample_halves(centroid, patch.len, patch.pen).max(0.0),
            -out,
        )),
        None => match oracle.contact_along(query, out, params.probe_reach + travel) {
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
