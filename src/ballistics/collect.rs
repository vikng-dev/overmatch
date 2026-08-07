//! The all-crossings corridor collector — the spatial half of the §13 walk.
//!
//! [`walk`](super::walk) is pure and takes a hit list as given. This is where that list comes from:
//! a corridor of world-space rays turned into EVERY face crossing along them, with each face's TRUE
//! orientation.
//!
//! # Why not `SpatialQuery::ray_hits`
//!
//! Avian's "all hits" query cannot express §13.4's all-hits collection, for three independent
//! reasons (verified against `avian3d-0.7.0/src/spatial_query/system_param.rs:277,354`):
//!
//! - It calls `collider.cast_ray` ONCE per collider, which for a trimesh returns the nearest
//!   triangle of that collider — the entry face, never the exit. Half of every crossing is missing.
//! - It silently truncates at `max_hits`, and the traversal order is not guaranteed, so "take the
//!   first N" is not even deterministic.
//! - It reports parry's normal, which is flipped to oppose the ray. A backface read from inside is
//!   then indistinguishable from a head-on entry — the exact confusion §13.1 blames for the serial
//!   resolver's free crossings.
//!
//! So the collector runs its own two-phase query: a broad phase over the corridor's swept AABB, then
//! a per-collider narrow phase that collects every crossing.
//!
//! # Two narrow phases, deliberately
//!
//! - **Trimesh** — production armour, and the only shape the bind ever builds for a ballistic volume
//!   (`tank::spawn::insert_ballistic_volumes`). Traverses parry's BVH collecting every triangle the
//!   ray crosses, and takes the normal from the triangle's WINDING, which is the mesh's real
//!   orientation rather than a query artefact.
//! - **Convex** — the sandbox/test slabs (`Collider::cuboid`). A convex solid presents exactly one
//!   interval to a ray, so two casts settle it: forward for the entry, backward from the corridor's
//!   far end for the exit. No production armour takes this path.
//!
//! # Never a partial list
//!
//! Every failure is a structured [`WalkError`], because a corridor that quietly dropped a face is
//! indistinguishable from armour that is not there — and armour that is not there is what §13 exists
//! to stop happening silently.

use avian3d::parry::shape::TypedShape;
use avian3d::prelude::{Collider, ColliderAabb, Position, Rotation};
use bevy::prelude::*;

use super::walk::{FaceHit, PrimitiveKey, WalkError, WalkLaws};

/// Faces one corridor may collect before the collector gives up.
///
/// A ceiling, not a budget: reaching it is [`WalkError::CorridorOverflow`], never a truncated list.
/// The Tiger's densest ballistic primitive is a few thousand triangles TOTAL, and a ray crosses a
/// vanishing fraction of them, so this is orders above anything real — it exists to turn a runaway
/// (a degenerate mesh, a corridor extended past all reason) into a loud stop.
const MAX_FACES: usize = 4096;

/// Below this, the ray is parallel to the triangle's plane and there is no crossing to report. In
/// the TOPOLOGY tolerance family (see [`super::walk::WalkLaws`]), deliberately far below the weld
/// knob: it decides whether a face exists at all, not whether two faces are the same face.
const PARALLEL_EPS: f32 = 1.0e-12;

/// How far behind a corridor origin the convex narrow phase looks for the face the ray is sitting
/// on. The march's own boundary nudge, so the two agree about what "just behind" means.
const BOUNDARY_PROBE: f32 = 1.0e-3;

/// What the collector needs from the world. Borrowed rather than owned so the march can hand it the
/// queries it already holds.
pub(crate) struct Corridor<'a> {
    pub origin: Vec3,
    /// Unit travel direction.
    pub axis: Vec3,
    pub length: f32,
    /// Primitives this ray was declared to START inside.
    ///
    /// Honors the core's `(open, close]` seed contract from the driving side: a face BEHIND the
    /// origin belongs to the seed and is dropped, and a face a rounding-hair behind it on a ray that
    /// was NOT seeded is the face the ray is sitting on and is clamped to `t = 0`, where the walk
    /// processes it. The two rules are complements, so an entry is counted exactly once however the
    /// f32 falls.
    pub seeded: &'a [PrimitiveKey],
    /// Shared with the walk so the collector and the core agree about when two `t` name the same
    /// face — a second opinion about that is how an entry gets counted twice or not at all. The
    /// trimesh prune reads it through [`prune_margin`], to decide when a corridor origin is ON a
    /// face rather than past it.
    pub laws: &'a WalkLaws,
}

/// Collect every face crossing along one corridor, appending to `out`.
///
/// Ordering is NOT this function's job: [`super::walk::walk_ray`] imposes its own total order over
/// `(t, volume, primitive, triangle)`, so the broad phase's traversal order cannot reach the result.
/// What IS this function's job is completeness — every crossing, or an error.
pub(crate) fn collect(
    corridor: &Corridor<'_>,
    // Candidate colliders, already filtered to armour the shell may hit (layer + self-exclusion).
    candidates: &[(Entity, Entity)],
    colliders: &Query<(&Position, &Rotation, &Collider)>,
    out: &mut Vec<FaceHit>,
) -> Result<(), WalkError> {
    for &(node, primitive) in candidates {
        let Ok((position, rotation, collider)) = colliders.get(primitive) else {
            // A candidate the broad phase named but whose pose is not available cannot be probed,
            // and guessing that it is empty is exactly the silent-zero this module refuses.
            return Err(WalkError::CollectorFailed {
                volume: node,
                reason: "a candidate collider has no resolved pose",
            });
        };
        // Into the collider's local frame: one inverse rotation, rather than transforming every
        // triangle out to world space.
        let inverse = rotation.inverse();
        let local_origin = inverse * (corridor.origin - position.0);
        let local_axis = inverse * corridor.axis;
        let seeded = corridor.seeded.contains(&PrimitiveKey {
            volume: node,
            primitive,
        });

        let before = out.len();
        match collider.shape_scaled().as_typed_shape() {
            TypedShape::TriMesh(mesh) => collect_trimesh(
                mesh,
                local_origin,
                local_axis,
                corridor.length,
                node,
                primitive,
                seeded,
                *rotation,
                prune_margin(corridor),
                out,
            )?,
            _ => collect_convex(
                collider, position.0, *rotation, corridor, node, primitive, seeded, out,
            ),
        }
        if out.len() > MAX_FACES {
            return Err(WalkError::CorridorOverflow {
                volume: node,
                collected: out.len(),
                limit: MAX_FACES,
            });
        }
        debug_assert!(
            out[before..].iter().all(|hit| hit.t.is_finite()),
            "the collector emitted a non-finite crossing"
        );
    }
    Ok(())
}

/// How far outside the corridor's own extent the trimesh prune reaches, so that the prune cannot be
/// the thing that decides whether a face EXISTS — [`admit`] owns that decision.
///
/// A transit corridor's origin IS an entry face's position, recomputed through a walk, a handoff and
/// a world-to-local subtraction, so it lands a few ULP on one side of that plane or the other. When
/// it lands PAST, the face's own AABB — degenerate in the axis it is perpendicular to — falls
/// outside the corridor's box by that same hair and the leaf is never tested. `admit` is written for
/// exactly this ("behind the origin and unseeded → the ray is sitting on the face") and never gets
/// the chance.
///
/// Same tolerance family, and the same scale-awareness, as [`super::walk::coincident`]: this is "the
/// origin is ON that boundary", stated as a box. Micrometres — three orders below the march's own
/// boundary nudge, so a face the march deliberately stepped past stays outside.
fn prune_margin(corridor: &Corridor<'_>) -> f32 {
    corridor.laws.topology_abs + corridor.laws.topology_rel * corridor.origin.abs().max_element()
}

/// Every triangle the ray crosses, with the winding normal.
#[expect(
    clippy::too_many_arguments,
    reason = "private kernel of `collect`; the alternative is a struct used exactly once"
)]
fn collect_trimesh(
    mesh: &avian3d::parry::shape::TriMesh,
    origin: Vec3,
    axis: Vec3,
    length: f32,
    node: Entity,
    primitive: Entity,
    seeded: bool,
    rotation: Rotation,
    margin: f32,
    out: &mut Vec<FaceHit>,
) -> Result<(), WalkError> {
    // Prune by BOX OVERLAP against the corridor's own extent, not by a ray cast against each node.
    //
    // A ray cast is the obvious prune and it is subtly wrong here: an axis-aligned face's AABB is
    // DEGENERATE (zero thickness in one axis), and a corridor origin sitting exactly on such a face
    // — which the transit corridor's does, every time, because it IS that face's position — can miss
    // it entirely. The entry then vanishes and the walk reports an exit it never entered. Overlap
    // prunes nearly as well, and every surviving leaf still gets an exact intersection test, so the
    // prune cannot change WHICH crossings exist either way — PROVIDED it is grown by
    // [`prune_margin`], because a box tested with a bare inequality has its own exact boundary, and
    // an origin one ULP past the face lands on the wrong side of it.
    let far = origin + axis * length;
    let lo = origin.min(far) - Vec3::splat(margin);
    let hi = origin.max(far) + Vec3::splat(margin);
    for index in mesh.bvh().leaves(|node| {
        let aabb = node.aabb();
        aabb.maxs.x >= lo.x
            && aabb.mins.x <= hi.x
            && aabb.maxs.y >= lo.y
            && aabb.mins.y <= hi.y
            && aabb.maxs.z >= lo.z
            && aabb.mins.z <= hi.z
    }) {
        let triangle = mesh.triangle(index);
        let Some((t, normal)) = cross_triangle(origin, axis, triangle.a, triangle.b, triangle.c)
        else {
            continue;
        };
        if let Some(hit) = admit(t, length, seeded) {
            out.push(FaceHit {
                volume: node,
                primitive,
                triangle: index,
                t: hit,
                // Back to world space. The winding normal is the mesh's own orientation — never
                // parry's, which is flipped to oppose the ray and so cannot tell entry from exit.
                true_normal: rotation * normal,
            });
        }
        if out.len() > MAX_FACES {
            return Err(WalkError::CorridorOverflow {
                volume: node,
                collected: out.len(),
                limit: MAX_FACES,
            });
        }
    }
    Ok(())
}

/// The one interval a convex solid presents.
///
/// Two casts, because a convex shape has exactly one entry and one exit and parry will happily give
/// both: forward from the origin, and backward from the corridor's far end. Comparing them is also
/// how "the ray started inside" is DETECTED rather than assumed — from inside, `solid: false` makes
/// the forward cast return the same exit face the backward cast found, and the two `t` coincide.
///
/// Normals are re-signed from the geometry (`entry opposes the ray, exit agrees`), which for a
/// convex solid is exact and owes parry's normal convention nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "private kernel of `collect`; the alternative is a struct used exactly once"
)]
fn collect_convex(
    collider: &Collider,
    position: Vec3,
    rotation: Rotation,
    corridor: &Corridor<'_>,
    node: Entity,
    primitive: Entity,
    seeded: bool,
    out: &mut Vec<FaceHit>,
) {
    let Ok(axis) = Dir3::new(corridor.axis) else {
        return;
    };
    // Both ends are probed with `solid: true`, and that is the whole trick.
    //
    // `solid: false` reports the EXIT when a ray begins inside a shape — so it cannot distinguish
    // "began inside" from "grazed an edge" (entry and exit coincide out at distance), and it answers
    // for the wrong face whenever a cast origin lands exactly ON a surface, which the corridor's own
    // endpoints do constantly because they ARE face positions a previous walk computed. `solid: true`
    // answers one unambiguous question instead: zero if the origin is within, otherwise the distance
    // to the nearest surface.
    let entry = collider.cast_ray(
        position,
        rotation,
        corridor.origin,
        Vec3::from(axis),
        corridor.length,
        true,
    );
    let far = corridor.origin + corridor.axis * corridor.length;
    let exit = collider.cast_ray(
        position,
        rotation,
        far,
        -Vec3::from(axis),
        corridor.length,
        true,
    );

    let mut push = |t: f32, normal: Vec3, entry: bool, face: u32| {
        // Re-sign from the role, not from what the query returned.
        let oriented = if (normal.dot(corridor.axis) < 0.0) == entry {
            normal
        } else {
            -normal
        };
        if let Some(t) = admit(t, corridor.length, seeded) {
            out.push(FaceHit {
                volume: node,
                primitive,
                triangle: face,
                t,
                true_normal: oriented,
            });
        }
    };

    match entry {
        // The corridor BEGINS inside. The entry is behind the origin, so look for it there — the
        // origin lands on a face constantly, and that face is the one the ray is sitting on. Anything
        // deeper than this probe is not: that sample should have been seeded, and if it was not, the
        // walk says so rather than this papering over it.
        //
        // The trimesh path needs none of this — its own kernel reports faces at negative `t`, and
        // `admit` places them.
        Some((distance, _)) if distance <= 0.0 => {
            if !seeded
                && let Some((behind, normal)) = collider.cast_ray(
                    position,
                    rotation,
                    corridor.origin,
                    -Vec3::from(axis),
                    BOUNDARY_PROBE,
                    false,
                )
            {
                push(-behind, normal, true, 0);
            }
        }
        Some((distance, normal)) => push(distance, normal, true, 0),
        None => {}
    }

    match exit {
        // The corridor ENDS inside. There is no exit to report, and inventing one is the whole
        // defect class: the walk reports `IncompleteCorridor` and the driver extends.
        Some((distance, _)) if distance <= 0.0 => {}
        Some((distance, normal)) => push(corridor.length - distance, normal, false, 1),
        None => {}
    }
}

/// Whether a crossing at `t` belongs to this corridor, and where.
///
/// The seed contract, from the driving side: behind the origin and seeded → the seed already
/// accounts for it; behind the origin and NOT seeded → the ray is sitting on the face, so it lands
/// at `t = 0`; at or past the far end → it belongs to the next corridor (the interval is half-open).
fn admit(t: f32, length: f32, seeded: bool) -> Option<f32> {
    if !t.is_finite() || t >= length {
        return None;
    }
    if t < 0.0 {
        return (!seeded).then_some(0.0);
    }
    Some(t)
}

/// Ray-vs-triangle, returning the crossing distance and the triangle's WINDING normal.
///
/// Ours rather than parry's for two reasons: parry's kernel answers "nearest hit, normal flipped to
/// oppose the ray", which discards exactly the orientation §13.4's pairing runs on; and keeping the
/// arithmetic here keeps it ours to hold stable (`parry ≤ 0.29`'s raycasts were not cross-platform
/// reproducible — the same reason the terrain cast is ours, see `cast_march_segment`).
///
/// Double-sided by construction: the sign of `det` is not tested, so a face is reported whichever
/// way the ray meets it. That is the point — an exit face is a crossing too.
fn cross_triangle(origin: Vec3, axis: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<(f32, Vec3)> {
    let e1 = b - a;
    let e2 = c - a;
    let normal = e1.cross(e2);
    let det = -axis.dot(normal);
    if det.abs() < PARALLEL_EPS {
        return None;
    }
    let ao = origin - a;
    let dao = ao.cross(axis);
    let u = e2.dot(dao) / det;
    let v = -e1.dot(dao) / det;
    if u < 0.0 || v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = ao.dot(normal) / det;
    let normal = normal.normalize_or_zero();
    (normal != Vec3::ZERO).then_some((t, normal))
}

/// The corridor's swept bounding box, for the broad phase.
pub(crate) fn swept_aabb(origin: Vec3, axis: Vec3, length: f32, radius: f32) -> ColliderAabb {
    let far = origin + axis * length;
    let pad = Vec3::splat(radius);
    ColliderAabb::from_min_max(origin.min(far) - pad, origin.max(far) + pad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tri(o: Vec3, d: Vec3) -> Option<(f32, Vec3)> {
        // A unit triangle in the z = 1 plane, wound so its normal points +Z.
        cross_triangle(
            o,
            d,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        )
    }

    #[test]
    fn a_head_on_crossing_reports_its_distance_and_winding_normal() {
        let (t, normal) = tri(Vec3::new(0.1, 0.1, 0.0), Vec3::Z).expect("the ray crosses it");
        assert!((t - 1.0).abs() < 1.0e-6, "{t}");
        // The WINDING normal, not one flipped to oppose the ray: this face is wound away from the
        // shooter, so it reads as an EXIT — which is exactly the fact parry's convention destroys.
        assert!((normal - Vec3::Z).length() < 1.0e-6, "{normal}");
        assert!(normal.dot(Vec3::Z) > 0.0);
    }

    /// Double-sided by construction. A mesh's exit face is a crossing, and a collector that culled
    /// backfaces would lose every exit — half of every interval.
    #[test]
    fn a_crossing_from_behind_reports_the_same_winding_normal() {
        let (t, normal) = tri(Vec3::new(0.1, 0.1, 2.0), Vec3::NEG_Z).expect("the ray crosses it");
        assert!((t - 1.0).abs() < 1.0e-6, "{t}");
        assert!((normal - Vec3::Z).length() < 1.0e-6, "{normal}");
    }

    #[test]
    fn a_ray_missing_the_triangle_reports_nothing() {
        assert!(tri(Vec3::new(0.9, 0.9, 0.0), Vec3::Z).is_none(), "outside");
        assert!(tri(Vec3::new(-0.1, 0.1, 0.0), Vec3::Z).is_none(), "u < 0");
    }

    /// A ray in the triangle's plane crosses nothing — there is no distance to report, and the
    /// division that would compute one is the one that blows up.
    #[test]
    fn a_ray_parallel_to_the_plane_reports_nothing() {
        assert!(tri(Vec3::new(0.1, 0.1, 1.0), Vec3::X).is_none());
    }

    /// Behind the origin is still a crossing — the collector, not the kernel, decides what a
    /// corridor admits.
    #[test]
    fn a_crossing_behind_the_origin_keeps_its_negative_distance() {
        let (t, _) = tri(Vec3::new(0.1, 0.1, 2.0), Vec3::Z).expect("the plane is behind it");
        assert!(t < 0.0, "{t}");
    }

    /// An axis-aligned trimesh box, wound outwards, centred on the origin.
    fn trimesh_box(size: Vec3) -> Collider {
        let h = size * 0.5;
        let vertices: Vec<Vec3> = [
            (-1.0, -1.0, -1.0),
            (1.0, -1.0, -1.0),
            (1.0, 1.0, -1.0),
            (-1.0, 1.0, -1.0),
            (-1.0, -1.0, 1.0),
            (1.0, -1.0, 1.0),
            (1.0, 1.0, 1.0),
            (-1.0, 1.0, 1.0),
        ]
        .into_iter()
        .map(|(x, y, z)| Vec3::new(x * h.x, y * h.y, z * h.z))
        .collect();
        let indices = vec![
            [0, 3, 2],
            [0, 2, 1],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ];
        Collider::trimesh(vertices, indices)
    }

    /// Run the trimesh narrow phase against `collider` for a ray in the collider's own frame, at the
    /// margin the live collector would compute for that corridor.
    fn trimesh_hits(collider: &Collider, origin: Vec3, axis: Vec3, length: f32) -> Vec<FaceHit> {
        let node = Entity::from_raw_u32(1).expect("a test entity index");
        let laws = WalkLaws::default();
        let corridor = Corridor {
            origin,
            axis,
            length,
            seeded: &[],
            laws: &laws,
        };
        let mut out = Vec::new();
        let TypedShape::TriMesh(mesh) = collider.shape_scaled().as_typed_shape() else {
            panic!("`Collider::trimesh` builds a trimesh");
        };
        collect_trimesh(
            mesh,
            origin,
            axis,
            length,
            node,
            node,
            false,
            Rotation::default(),
            prune_margin(&corridor),
            &mut out,
        )
        .expect("the fixture collects cleanly");
        out
    }

    /// THE PRUNE MUST NOT DECIDE WHETHER A FACE EXISTS.
    ///
    /// A transit corridor's origin IS the entry face's own position, recomputed through a walk and a
    /// handoff — so one f32 ulp of rounding puts it a hair PAST that face about half the time. The
    /// face's own AABB is DEGENERATE in the axis it is perpendicular to, so it then lies entirely
    /// outside the corridor's box and the prune drops it. [`admit`] is written for exactly this case
    /// ("behind the origin and unseeded → the ray is sitting on the face, clamp to `t = 0`") and
    /// never got the chance: the entry vanished, and the walk reported an exit it never entered
    /// ([`WalkError::UnexpectedExit`]), which the driver fails closed on — a round that stopped dead
    /// on a plate it should have punched straight through.
    #[test]
    fn a_face_the_origin_sits_a_hair_past_is_still_collected() {
        let collider = trimesh_box(Vec3::new(3.0, 3.0, 0.05));
        let face = 0.025_f32;
        // In ULP, because that is the unit the defect is measured in: the live failure put the
        // corridor origin a dozen ULP past a 25 mm face, purely from the `world_origin −
        // collider_position` that takes the corridor into the collider's frame.
        for past in [0, 1, 8, 64] {
            let origin = f32::from_bits(face.to_bits() - past);
            // Off the box's face diagonal, so each face presents ONE triangle and the count below
            // reads as "the face" rather than "however the mesh was triangulated".
            let hits = trimesh_hits(&collider, Vec3::new(0.1, -0.0003, origin), Vec3::NEG_Z, 0.5);
            let entries: Vec<&FaceHit> = hits
                .iter()
                .filter(|hit| hit.true_normal.dot(Vec3::NEG_Z) < 0.0)
                .collect();
            assert_eq!(
                entries.len(),
                1,
                "the entry face is collected though the origin sits {past} past it: {hits:?}",
            );
            assert_eq!(entries[0].t, 0.0, "and it lands at the corridor origin");
        }
    }

    #[test]
    fn admission_honours_the_seed_contract() {
        // Inside the corridor: kept as-is.
        assert_eq!(admit(0.5, 1.0, false), Some(0.5));
        // Half-open at the far end: this one is the next corridor's.
        assert_eq!(admit(1.0, 1.0, false), None);
        // Behind the origin, seeded: the seed already accounts for the entry.
        assert_eq!(admit(-1.0e-7, 1.0, true), None);
        // Behind the origin, NOT seeded: the ray is sitting on the face.
        assert_eq!(admit(-1.0e-7, 1.0, false), Some(0.0));
        assert_eq!(admit(f32::NAN, 1.0, false), None);
    }
}
