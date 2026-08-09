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
use bevy::math::DVec3;
use bevy::prelude::*;

use super::BallisticSurfaces;
use super::walk::{Contact, FaceHit, ShellKey, WalkError, WalkLaws};

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
    /// WORLD position the corridor hangs off; see [`super::walk::RayCorridor::anchor`].
    pub anchor: Vec3,
    /// Corridor start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    /// Unit travel direction.
    pub axis: Vec3,
    pub length: f32,
    /// Shells this ray was declared to START inside.
    ///
    /// Honors the core's `(open, close]` seed contract from the driving side: a face BEHIND the
    /// origin belongs to the seed and is dropped, and a face a rounding-hair behind it on a ray that
    /// was NOT seeded is the face the ray is sitting on and is clamped to `t = 0`, where the walk
    /// processes it. The two rules are complements, so an entry is counted exactly once however the
    /// f32 falls.
    pub seeded: &'a [ShellKey],
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
    colliders: &Query<(&Position, &Rotation, &Collider, Option<&BallisticSurfaces>)>,
    out: &mut Vec<FaceHit>,
) -> Result<(), WalkError> {
    for &(node, primitive) in candidates {
        let Ok((position, rotation, collider, surfaces)) = colliders.get(primitive) else {
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
        // ORDER IS THE POINT. `anchor - position.0` is a difference of two WORLD positions, so it is
        // near-exact when they are close (and its magnitude is metres, not kilometres); the
        // corridor's own offset is then added at that scale. Forming the corridor's world position
        // first and subtracting afterwards would round it to the world grid — 0.24 mm at the edge of
        // the map — and hand the narrow phase a ray that starts a quarter millimetre off the face it
        // was computed from.
        let local_origin = inverse * ((corridor.anchor - position.0) + corridor.origin);
        let local_axis = inverse * corridor.axis;
        let seeded = |shell: u32| {
            corridor.seeded.contains(&ShellKey {
                volume: node,
                primitive,
                shell,
            })
        };

        let before = out.len();
        match collider.shape_scaled().as_typed_shape() {
            TypedShape::TriMesh(mesh) => {
                // SURFACE IDENTITY IS NOT OPTIONAL FOR ARMOUR. Without it the walk cannot tell two
                // shells closing at one `t` from one shell claimed twice, and it must not guess —
                // so a trimesh whose certificate is missing or the wrong length is an unprobed
                // candidate, which is the same silence as an absent one.
                let surfaces = surfaces
                    .filter(|surfaces| {
                        surfaces.shells.len() == mesh.num_triangles()
                            && surfaces.corners.len() == mesh.num_triangles()
                    })
                    .ok_or(WalkError::CollectorFailed {
                        volume: node,
                        reason: "a trimesh candidate carries no per-triangle manifold certificate",
                    })?;
                // THE CERTIFICATE IS ABOUT ONE SCALE. Everything the bake proved — closure, winding,
                // positive volume, the welded ids the contacts below are named with — it proved
                // about `position * scale`, and avian re-derives the scaled shape from whatever the
                // hierarchy says at the time. A collider that reaches the world at any other scale
                // has an uncertified topology, so it is refused rather than walked. Bit-exact, not
                // near: there is no scale that is almost the one that was proven.
                if collider.scale().to_array().map(f32::to_bits)
                    != surfaces.scale.to_array().map(f32::to_bits)
                {
                    return Err(WalkError::CollectorFailed {
                        volume: node,
                        reason: "a collider reached the world at a scale the bake never certified",
                    });
                }
                collect_trimesh(
                    mesh,
                    local_origin,
                    local_axis,
                    corridor.length,
                    node,
                    primitive,
                    surfaces,
                    &seeded,
                    *rotation,
                    prune_margin(local_origin, corridor.laws),
                    corridor.laws,
                    out,
                )?
            }
            // The sandbox slabs. A convex solid is one closed surface by definition, so its shell
            // id is `0` and there is nothing to look up.
            _ => collect_convex(
                collider,
                position.0,
                *rotation,
                corridor,
                node,
                primitive,
                seeded(0),
                out,
            )?,
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
/// origin is ON that boundary", stated as a box.
///
/// RE-DERIVED after the corridor became anchor-relative, because that changed which rounding is left
/// to absorb. It is scaled on the LOCAL ray origin — the corridor's position in the collider's own
/// frame — since that is now the only quantity the crossing arithmetic rounds at. A ray probing a
/// tank part sits metres from its centre at most, so the floor lands near `topology_abs`: a few
/// micrometres, wherever on the map the shot happens.
///
/// Before anchoring it had to scale on the WORLD position instead, and that is what made this a
/// squeeze rather than a tolerance: `topology_rel · 2500 m` reaches [`super::MARCH_EPS`] exactly, so
/// the floor the geometry demanded and the ceiling safety demanded crossed INSIDE the map. Codex
/// measured a live handoff 0.3848 mm off its own plane at 2.5 km, past the 0.25 mm ceiling. The
/// anchor removed the conflict; the ceiling stays as defence in depth.
///
/// The CEILING is not decoration. The march resumes one `MARCH_EPS` past every face it leaves, so a
/// margin that reached a whole nudge would re-collect the EXIT face of the plate just perforated —
/// an exit for a primitive the walk is not inside, `UnexpectedExit`, and a round stopped dead in
/// mid-air one millimetre past the plate it punched through. A quarter of the nudge is now orders
/// above anything the floor asks for, which is exactly the state a clamp should be in: never
/// reached, and still there.
fn prune_margin(local_origin: Vec3, laws: &WalkLaws) -> f32 {
    (laws.topology_abs + laws.topology_rel * local_origin.abs().max_element())
        .min(super::MARCH_EPS * PRUNE_MARGIN_SHARE)
}

/// Ceiling on [`prune_margin`], as a share of the march's own boundary nudge — see there for why a
/// share of THAT constant is the honest way to state it.
const PRUNE_MARGIN_SHARE: f32 = 0.25;

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
    surfaces: &BallisticSurfaces,
    seeded: &dyn Fn(u32) -> bool,
    rotation: Rotation,
    margin: f32,
    laws: &WalkLaws,
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
    let shear = RayShear::new(origin, axis);
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
        let corners = [triangle.a, triangle.b, triangle.c];
        let Some(claim) = cross_triangle(&shear, axis, triangle.a, triangle.b, triangle.c) else {
            continue;
        };
        let shell = surfaces.shells[index as usize];
        // The welded feature the claim is about, and the `t` that feature has. Both are computed
        // from the feature itself, so every triangle of a fan reports the same contact and the same
        // bits — the fan is canonical before the walk ever sees it.
        let (contact, t) = contact_of(
            &shear,
            axis,
            index,
            &corners,
            &surfaces.corners[index as usize],
            &claim,
        );
        if let Some(hit) = admit(t, length, seeded(shell), laws) {
            out.push(FaceHit {
                volume: node,
                primitive,
                shell,
                contact,
                triangle: index,
                t: hit,
                // Back to world space. The winding normal is the mesh's own orientation — never
                // parry's, which is flipped to oppose the ray and so cannot tell entry from exit.
                true_normal: rotation * claim.normal,
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
///
/// # Inside, undeclared, and unbounded is an ERROR
///
/// A corridor can lie wholly INSIDE a convex solid: both `solid: true` probes answer zero, the
/// backward boundary probe reaches no face, and there is nothing honest to report. Reporting nothing
/// is not honest either — an empty corridor is what open air looks like, so the walk finds no
/// material, [`super::walk::begin`] returns `Miss`, and the round flies through a solid volume at
/// zero cost with no `Impact` raised at all. Free penetration, silent, and indistinguishable from
/// armour that was never modelled.
///
/// So it is a structured error. The seed contract is what carries containment legitimately: a sample
/// that really is inside a volume arrives declared as such, read from a ray this module already
/// walked. Undeclared containment means the caller lost track of where its samples are, and that is
/// exactly the thing the walk refuses to infer its way past.
///
/// The trimesh phase cannot make this check — there is no cheap containment test on a mesh, and "no
/// faces in reach" is the NORMAL reading of a corridor crossing the hollow interior of a hull. Its
/// protection is the seed contract alone. Convex shapes are the sandbox slabs and carry no
/// production armour, which is the reason to make this loud rather than permissive: nothing real
/// depends on it staying quiet.
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
) -> Result<(), WalkError> {
    let Ok(axis) = Dir3::new(corridor.axis) else {
        return Ok(());
    };
    // Parry is asked in world space, so this phase re-forms the world origin. It carries the world
    // grid's rounding with it, and that is acceptable HERE and nowhere else: convex shapes are the
    // sandbox slabs, no production armour takes this path, and its own boundary probe already looks
    // a millimetre behind the origin — four orders more slack than the rounding could ever be.
    let world_origin = corridor.anchor + corridor.origin;
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
        world_origin,
        Vec3::from(axis),
        corridor.length,
        true,
    );
    let far = world_origin + corridor.axis * corridor.length;
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
        if let Some(t) = admit(t, corridor.length, seeded, corridor.laws) {
            out.push(FaceHit {
                volume: node,
                primitive,
                shell: 0,
                contact: Contact::Face(face),
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
            if !seeded {
                match collider.cast_ray(
                    position,
                    rotation,
                    world_origin,
                    -Vec3::from(axis),
                    BOUNDARY_PROBE,
                    false,
                ) {
                    // At `t = 0` deliberately, NOT at `-behind`. This phase has already made the
                    // "sitting on the face" judgement itself, and BOUNDED it: the probe reaches one
                    // `BOUNDARY_PROBE` and no further, so nothing farther behind can arrive here at
                    // all. `admit`'s clamp is the trimesh phase's bound, where no such probe limits
                    // how far back a sloped face may be found.
                    Some((_, normal)) => push(0.0, normal, true, 0),
                    // Inside, undeclared, and the entry face is nowhere near — see the note above.
                    // Silence here is free penetration, so this is where it stops.
                    None => {
                        return Err(WalkError::CollectorFailed {
                            volume: node,
                            reason: "a sample begins inside a convex volume it was not seeded for, \
                                     with no entry face within the boundary probe",
                        });
                    }
                }
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
    Ok(())
}

/// Whether a crossing at `t` belongs to this corridor, and where.
///
/// The seed contract, from the driving side: behind the origin and seeded → the seed already
/// accounts for it; behind the origin and NOT seeded → the ray is SITTING ON the face, so it lands
/// at `t = 0`; at or past the far end → it belongs to the next corridor (the interval is half-open).
///
/// # The clamp is bounded, and the bound is the whole rule
///
/// "Sitting on the face" is what the unseeded clamp exists for, and it is a statement about a
/// rounding hair — the corridor origin IS that face's position, recomputed. Applied to any distance
/// behind the origin it says something else entirely: that a face the ray traversed long ago is
/// still ahead of it.
///
/// That is not hypothetical. A transit corridor restarts at the plate's own exit, and on a SLOPED
/// plate the exit triangles have extent along the ray, so the collector finds them well behind the
/// origin and the clamp dragged them back to `t = 0` — an exit with nothing open, `UnexpectedExit`,
/// fail-closed. The §13.6 fuzzer measured 40 of 40 head-on lines across the UFP stopping at the
/// plate's own exit face, losing the exit, the spall and everything behind it. Square plates hide
/// it: their exit triangles are perpendicular to the ray and have no extent along it, which is why
/// every axis-aligned fixture passed.
///
/// So the clamp reaches exactly as far as "on the face" means — [`super::walk::coincident`], the
/// module's own answer to whether two `t` name one boundary. Anything farther behind is already
/// traversed and is dropped. The seeded-drop / unseeded-clamp dichotomy is untouched; only the
/// unbounded reach dies.
fn admit(t: f32, length: f32, seeded: bool, laws: &WalkLaws) -> Option<f32> {
    if !t.is_finite() || t >= length {
        return None;
    }
    if t < 0.0 {
        // Seeded: the seed owns it. Unseeded and ON the origin: the ray is sitting on this face.
        // Unseeded and genuinely behind: the ray has already crossed it, and this corridor starts
        // after it.
        return (!seeded && super::walk::coincident(0.0, t, laws)).then_some(0.0);
    }
    Some(t)
}

/// The ray reduced to the frame the containment test runs in: origin at zero, travel along `+z`
/// after a permutation and a shear (Woop/Benthin/Wald, *Watertight Ray/Triangle Intersection*, 2013).
///
/// It is a function of the RAY ALONE. Every triangle of a corridor is sheared by the same constants,
/// which is what makes the edge tests below comparable between triangles at all.
struct RayShear {
    origin: Vec3,
    /// The permute-and-shear map, as its two surviving ROWS — the `z` row is discarded, since the
    /// containment test is purely lateral. Held as vectors rather than as axis indices and scalars so
    /// projection is two dot products and no dynamic component indexing.
    row_x: Vec3,
    row_y: Vec3,
    /// The same map in f64, for the band where the f32 projection's own rounding decides the
    /// containment answer. Derived from the same f32 `origin` and `axis`, so it describes the SAME
    /// ray — it removes the arithmetic's rounding, never re-poses the query.
    origin_exact: DVec3,
    row_x_exact: DVec3,
    row_y_exact: DVec3,
}

/// One vertex in the sheared frame, with a bound on how far the f32 arithmetic could have moved it.
#[derive(Clone, Copy)]
struct Projected {
    p: (f32, f32),
    /// Bound on `|computed − exact|` for EITHER coordinate (see [`PROJECTION_SLACK`]).
    slack: f32,
}

/// The f32 projection's error, as a multiple of the vertex's own magnitude — over the certified
/// coordinate domain, and nowhere else.
///
/// # The derivation, in full
///
/// A row holds `1` at `kx` and `s = fl(−axis[kx] / axis[kz])` at `kz`, so with `d = V − o` exactly
/// and `m = max|v_i|` the computed coordinate is `p = fl(v_kx + fl(s · v_kz))` over `v = fl(d)`.
/// `kz` is the ray's DOMINANT axis, so `|S| ≤ 1` and `|s| ≤ 1 + u`, `u = 2⁻²⁴`. Then:
///
/// - subtraction: `|v_i − d_i| ≤ u|d_i| ≤ u·m`, one term (a difference that lands in the subnormal
///   range is exact, so this term needs no absolute companion);
/// - the shear constant: `|s − S| ≤ u|S| ≤ u`, contributing `u|v_kz| ≤ u·m`;
/// - the product's rounding: `≤ u|s·v_kz| + η`, and the shear applied to the subtraction's own error
///   a further `|S|·u·m ≤ u·m`;
/// - the sum's rounding: `≤ u·|v_kx + fl(s·v_kz)| + η ≤ 2u·m(1 + u) + η`.
///
/// Six `u·m`, a tail in `u²`, and TWO absolute terms `η = 2⁻¹⁵⁰` — the half-ulp floor of gradual
/// underflow, which is what the relative model cannot express. `PROJECTION_SLACK` is eight `u`,
/// so it dominates iff `2u·m ≥ 2η`, that is iff
///
/// > **`m ≥ η/u = 2⁻¹²⁶ = f32::MIN_POSITIVE`.**
///
/// That is the domain condition, and it is a condition on the vertex's offset from the CORRIDOR
/// ORIGIN, which no bake gate can bound from below — the origin is not a vertex and may sit
/// arbitrarily close to one. What [`crate::bake::CERTIFIED_RANGE`] bounds instead is the CONSEQUENCE of
/// failing it:
///
/// - `m < 2⁻¹²⁶` means the origin agrees with this vertex to within `MIN_POSITIVE` in every
///   component, so at most one welded vertex position of a triangle can be in that state, and the
///   origin itself is inside the certified box to within `2⁻¹²⁶`;
/// - for that vertex the UNCLAIMED error is `6u·m + 2η < 2⁻¹⁴⁷`, against a claimed `8u·m ≥ 0`;
/// - the other two points are certified, so `|q_i| = |v_kx + s·v_kz| ≤ 2·(2·2¹⁶)` and
///   `|q₀| + |q₁| ≤ 2¹⁹`;
/// - so the deficit propagated into [`edge_area`] is `< 2⁻¹⁴⁷ · 2¹⁹ = 2⁻¹²⁸`, and
///   [`edge_area_slack`]'s `f32::MIN_POSITIVE = 2⁻¹²⁶` term covers it four times over — with room
///   left for the ≤ 5η that band expression's own evaluation can lose to underflow.
///
/// So the band is sufficient EVERYWHERE on the certified domain, and outside it this kernel makes no
/// claim at all: the codex triangle in
/// [`a_subnormal_triangle_is_outside_the_certified_domain`] has a subnormal vertex and a coordinate
/// of `10¹⁰`, is refused at the door by both bounds, and is declined here — correctly as far as this
/// bound is concerned, because this bound never promised anything about it.
///
/// It is a bound on the rounding, NOT a tolerance: the exact answer for the stored vertices is
/// always inside it, so a decision made outside it is the exact one.
///
/// It is not small. `v` is the vertex MINUS the corridor origin — metres — while the lateral offset
/// the projection has to resolve is centimetres, so at a 6 m standoff the bound is about a micron,
/// and a micron at a silhouette is a whole chord ([`a_ray_inside_the_silhouette_keeps_its_chord`]).
const PROJECTION_SLACK: f32 = 4.0 * f32::EPSILON;

impl RayShear {
    fn new(origin: Vec3, axis: Vec3) -> Self {
        // `kz` is the ray's dominant axis, so the shear never divides by a small component.
        let abs = axis.abs();
        let kz = if abs.x > abs.y && abs.x > abs.z {
            0
        } else if abs.y > abs.z {
            1
        } else {
            2
        };
        // Swapped when the ray runs backwards along `kz`, which keeps the sheared frame
        // right-handed and so keeps the edge-area signs meaningful.
        let mut kx = (kz + 1) % 3;
        let mut ky = (kx + 1) % 3;
        if axis[kz] < 0.0 {
            std::mem::swap(&mut kx, &mut ky);
        }
        let mut row_x = Vec3::ZERO;
        let mut row_y = Vec3::ZERO;
        row_x[kx] = 1.0;
        row_x[kz] = -axis[kx] / axis[kz];
        row_y[ky] = 1.0;
        row_y[kz] = -axis[ky] / axis[kz];
        let (mut row_x_exact, mut row_y_exact) = (DVec3::ZERO, DVec3::ZERO);
        row_x_exact[kx] = 1.0;
        row_x_exact[kz] = -(axis[kx] as f64) / axis[kz] as f64;
        row_y_exact[ky] = 1.0;
        row_y_exact[kz] = -(axis[ky] as f64) / axis[kz] as f64;
        Self {
            origin,
            row_x,
            row_y,
            origin_exact: origin.as_dvec3(),
            row_x_exact,
            row_y_exact,
        }
    }

    /// One vertex in the sheared frame, as the 2D point the edge tests take cross products of.
    ///
    /// The translation happens FIRST. Folding it into a precomputed bias would leave the corridor
    /// origin's magnitude in the products, and a corridor is metres from geometry that is centimetres
    /// across.
    #[inline]
    fn project(&self, vertex: Vec3) -> Projected {
        let v = vertex - self.origin;
        Projected {
            p: (self.row_x.dot(v), self.row_y.dot(v)),
            slack: PROJECTION_SLACK * v.abs().max_element(),
        }
    }

    /// The same projection with the rounding taken out.
    ///
    /// PER-VERTEX DETERMINISTIC, like the f32 form: a pure function of the stored vertex and the
    /// shear. Two triangles naming one vertex therefore hand the edge test the same two f64 numbers,
    /// which is the whole basis of the sign relation between them — the wider precision inherits the
    /// watertightness argument rather than replacing it.
    #[inline]
    fn project_exact(&self, vertex: Vec3) -> (f64, f64) {
        let v = vertex.as_dvec3() - self.origin_exact;
        (self.row_x_exact.dot(v), self.row_y_exact.dot(v))
    }
}

/// The signed area of the sheared edge `(p, q)` against the ray.
///
/// WATERTIGHTNESS LIVES HERE. Two triangles sharing an edge present it as `(p, q)` and `(q, p)`, and
/// this expression is EXACTLY antisymmetric under that swap in IEEE arithmetic: multiplication
/// commutes bit-for-bit, and `fl(x − y) == −fl(y − x)`. So the two triangles never agree that the ray
/// is outside — a ray on a shared edge is claimed by exactly one of them, or (at an exact zero) by
/// both. A per-triangle barycentric test computes the same edge from two different anchors and has no
/// such relation: at a micron from the edge both round to "outside", the crossing is dropped, and the
/// walk sees an exit with no entry.
#[inline]
fn edge_area(p: (f32, f32), q: (f32, f32)) -> f32 {
    q.0 * p.1 - q.1 * p.0
}

/// The same area with the products carried in f64 — reached when, and only when, the f32 form
/// cancels to exactly zero.
///
/// EXACT ZERO IS THE WHOLE OF THE UNRECOVERABLE SET *GIVEN THE PROJECTED POINTS*, which is what
/// fixes this trigger's width. Round-to-nearest is monotone, so `q₀·p₁ ≥ q₁·p₀` implies
/// `fl(q₀·p₁) ≥ fl(q₁·p₀)`, and the subtraction that follows preserves sign: a nonzero f32 area
/// carries the EXACT sign for those points however violently the two products cancel, and only a
/// cancellation to zero carries none. A trigger widened to "within a relative-error guard of zero"
/// therefore re-decides cases whose sign was already right, and can recover no crossing at all
/// (`the_sign_of_a_sheared_edge_area_is_the_exact_one_or_zero`).
///
/// The points themselves are the OTHER half, and this arithmetic cannot reach it: it inherits
/// whatever [`RayShear::project`] rounded. That half is [`edge_area_slack`]'s.
///
/// Antisymmetric on the same grounds as the f32 form, so widening one triangle's edge widens its
/// neighbour's identically.
#[inline]
fn edge_area_exact(p: (f32, f32), q: (f32, f32)) -> f64 {
    q.0 as f64 * p.1 as f64 - q.1 as f64 * p.0 as f64
}

/// The same area over points that are already f64 — the exact-projection path.
#[inline]
fn edge_area_f64(p: (f64, f64), q: (f64, f64)) -> f64 {
    q.0 * p.1 - q.1 * p.0
}

/// How far [`edge_area`] can sit from the area the EXACT projection of the same stored vertices
/// would give.
///
/// With `|Δp| ≤ eₚ` and `|Δq| ≤ e_q` per coordinate ([`PROJECTION_SLACK`]), the propagated error on
/// `q₀·p₁ − q₁·p₀` is at most `e_q(|p₀| + |p₁| + 2eₚ) + eₚ(|q₀| + |q₁|)`, and the two products and
/// the subtraction add at most `2.1u(|q₀p₁| + |q₁p₀|)` on top — carried here as `4ε = 8u`, which
/// also absorbs the rounding of this expression's own arithmetic. `f32::MIN_POSITIVE` covers
/// gradual underflow, where the relative bounds stop holding.
///
/// It is a SUFFICIENT band, and that is the entire claim: if the f32 areas do not all share a sign
/// while the exact ones do, then some f32 area is on the wrong side of zero and therefore no
/// further from zero than its own error — inside this band. So a rejection outside the band is the
/// exact answer for the stored geometry, and a rejection inside it is re-taken in f64. Nothing else
/// about the band's width is load-bearing: too wide only costs the f64 path.
#[inline]
fn edge_area_slack(p: Projected, q: Projected) -> f32 {
    let (px, py) = (p.p.0.abs(), p.p.1.abs());
    let (qx, qy) = (q.p.0.abs(), q.p.1.abs());
    q.slack * (px + py + 2.0 * p.slack)
        + p.slack * (qx + qy)
        + 4.0 * f32::EPSILON * (qx * py + qy * px)
        + f32::MIN_POSITIVE
}

/// Whether the ray passes on the same side of all three edges — the containment half of the test,
/// over whichever precision produced the areas.
#[inline]
fn same_side<T: PartialOrd + Default>(u: T, v: T, w: T) -> bool {
    let zero = T::default();
    !((u < zero || v < zero || w < zero) && (u > zero || v > zero || w > zero))
}

/// What one triangle claims about a ray: where it crosses, which way the face is wound, and which
/// of its three sheared edges the ray cancelled EXACTLY against.
struct TriangleClaim {
    /// The plane intersection — the interior contact's `t`, and the fallback for a feature whose
    /// own arithmetic cannot resolve it.
    t: f32,
    normal: Vec3,
    /// Bit `k` set means the edge OPPOSITE corner `k` produced an exactly-zero area, so the ray
    /// lies on it. Zero bits is an interior claim; one is an edge; two is the vertex they share.
    zeros: u8,
}

/// Ray-vs-triangle, returning the crossing distance, the triangle's WINDING normal, and the exact
/// feature the ray landed on.
///
/// Ours rather than parry's for two reasons: parry's kernel answers "nearest hit, normal flipped to
/// oppose the ray", which discards exactly the orientation §13.4's pairing runs on; and keeping the
/// arithmetic here keeps it ours to hold stable (`parry ≤ 0.29`'s raycasts were not cross-platform
/// reproducible — the same reason the terrain cast is ours, see `cast_march_segment`).
///
/// Double-sided by construction: the sign of `det` is not tested, so a face is reported whichever
/// way the ray meets it. That is the point — an exit face is a crossing too.
///
/// CONTAINMENT and DISTANCE come from different arithmetic on purpose. Containment is the watertight
/// edge test, whose only contract is the sign relation between neighbouring triangles; `t` stays the
/// plane intersection it has always been, so a crossing this kernel and a barycentric one both admit
/// lands on the same bits.
///
/// # One decision, so a contact means one thing
///
/// The f32 edge areas decide, EXCEPT inside their own rounding band, where the exact projection
/// decides instead and REPLACES them. Outside the band the f32 sign is provably the exact one
/// ([`edge_area_slack`]), so the composite predicate IS the exact-projection predicate — evaluated
/// cheaply where cheap suffices.
///
/// That is what makes a contact well defined. Two arithmetics would tile the projected plane two
/// different ways, and a pair of neighbouring triangles could each claim a ray under its own tiling
/// with nothing cancelling: two claims of one shell naming two different contacts, which the walk
/// can only refuse. One tiling has the antisymmetry property, so neighbours overlap only where an
/// area is exactly zero — and an exact zero is a contact, by name.
#[inline]
fn cross_triangle(
    shear: &RayShear,
    axis: Vec3,
    a: Vec3,
    b: Vec3,
    c: Vec3,
) -> Option<TriangleClaim> {
    let (pa, pb, pc) = (shear.project(a), shear.project(b), shear.project(c));
    let (u, v, w) = (
        edge_area(pb.p, pc.p),
        edge_area(pc.p, pa.p),
        edge_area(pa.p, pb.p),
    );
    // The band contains exact zero (`edge_area_slack` carries `f32::MIN_POSITIVE`), so a cancelled
    // area always escalates and a contact is always classified by the exact projection.
    let banded = u.abs() <= edge_area_slack(pb, pc)
        || v.abs() <= edge_area_slack(pc, pa)
        || w.abs() <= edge_area_slack(pa, pb);
    let zeros = if banded {
        let (pa, pb, pc) = (
            shear.project_exact(a),
            shear.project_exact(b),
            shear.project_exact(c),
        );
        let (u, v, w) = (
            edge_area_f64(pb, pc),
            edge_area_f64(pc, pa),
            edge_area_f64(pa, pb),
        );
        // Three collinear sheared points: the ray lies in the triangle's plane and crosses nothing.
        if !(same_side(u, v, w) && u + v + w != 0.0) {
            return None;
        }
        u8::from(u == 0.0) | (u8::from(v == 0.0) << 1) | (u8::from(w == 0.0) << 2)
    } else {
        if !same_side(u, v, w) {
            return None;
        }
        0
    };

    let e1 = b - a;
    let e2 = c - a;
    let normal = e1.cross(e2);
    // A NEEDLE'S PLANE IS NOT ITS OWN f32 CROSS PRODUCT. Two edges within `PROJECTION_SLACK` of
    // parallel cancel that product down to its last bits, and what survives is noise with a
    // direction: the face is then reported at a distance, and with an orientation, that its own
    // vertices do not support — a crossing tens of millimetres from where it is, and an entry that
    // can read as an exit. `PARALLEL_EPS` does not catch it, because the noise has magnitude.
    //
    // The three stored points still DETERMINE a plane; only the f32 arithmetic cannot find it. So
    // the degenerate case buys the wider products, and nothing else does — a triangle whose f32
    // normal has significant digits keeps every bit of its `t` and its normal.
    let (t, normal) = if normal.length_squared()
        <= PROJECTION_SLACK * PROJECTION_SLACK * e1.length_squared() * e2.length_squared()
    {
        let (a64, b64, c64) = (a.as_dvec3(), b.as_dvec3(), c.as_dvec3());
        let normal = (b64 - a64).cross(c64 - a64);
        let det = -axis.as_dvec3().dot(normal);
        if det.abs() < PARALLEL_EPS as f64 {
            return None;
        }
        (
            ((shear.origin_exact - a64).dot(normal) / det) as f32,
            normal.normalize_or_zero().as_vec3(),
        )
    } else {
        let det = -axis.dot(normal);
        if det.abs() < PARALLEL_EPS {
            return None;
        }
        (
            (shear.origin - a).dot(normal) / det,
            normal.normalize_or_zero(),
        )
    };
    (normal != Vec3::ZERO && t.is_finite()).then_some(TriangleClaim { t, normal, zeros })
}

/// The welded feature a claim is about, and the `t` that feature has along the ray.
///
/// CANONICAL, which is the whole job: every triangle incident on a welded edge or vertex must report
/// the same contact and the same bits, or the walk cannot tell one crossing claimed twice from two
/// crossings. Identity comes from the bake's welded ids; the parameter comes from the feature's own
/// geometry, taken in an order fixed by those ids, so two triangles that spell an edge in opposite
/// directions still compute one number.
fn contact_of(
    shear: &RayShear,
    axis: Vec3,
    triangle: u32,
    positions: &[Vec3; 3],
    welded: &[u32; 3],
    claim: &TriangleClaim,
) -> (Contact, f32) {
    let parameter = |point: Vec3| (point - shear.origin).dot(axis);
    match claim.zeros.count_ones() {
        // The interior: this triangle alone claims it, and its own plane gives the distance.
        0 => (Contact::Face(triangle), claim.t),
        // One cancelled edge: the ray runs through it. Both incident triangles land here.
        1 => {
            // Bit `k` names the edge opposite corner `k`, i.e. the one joining the other two.
            let (i, j) = match claim.zeros {
                0b001 => (1, 2),
                0b010 => (2, 0),
                _ => (0, 1),
            };
            // Ascending welded id, so the two triangles interpolate the same direction.
            let (i, j) = if welded[i] <= welded[j] {
                (i, j)
            } else {
                (j, i)
            };
            let (p, q) = (
                shear.project_exact(positions[i]),
                shear.project_exact(positions[j]),
            );
            // The ray sits at the projected origin, so the parameter along the edge is where the
            // segment crosses it. The wider component divides, so a near-axis-aligned edge does not
            // divide by its own rounding.
            let (from, span) = if (q.0 - p.0).abs() >= (q.1 - p.1).abs() {
                (p.0, q.0 - p.0)
            } else {
                (p.1, q.1 - p.1)
            };
            let point = if span == 0.0 {
                positions[i]
            } else {
                let lambda = (-from / span) as f32;
                positions[i] + (positions[j] - positions[i]) * lambda
            };
            (Contact::Edge(welded[i], welded[j]), parameter(point))
        }
        // Two cancelled edges: the ray runs through the vertex they share, and the whole welded fan
        // around it reports this one contact.
        _ => {
            let corner = match claim.zeros {
                0b011 => 2,
                0b101 => 1,
                _ => 0,
            };
            (
                Contact::Vertex(welded[corner]),
                parameter(positions[corner]),
            )
        }
    }
}

/// The corridor's swept bounding box, for the broad phase.
pub(crate) fn swept_aabb(origin: Vec3, axis: Vec3, length: f32, radius: f32) -> ColliderAabb {
    let far = origin + axis * length;
    let pad = Vec3::splat(radius);
    ColliderAabb::from_min_max(origin.min(far) - pad, origin.max(far) + pad)
}

#[cfg(test)]
mod tests {
    use bevy::math::DVec3;

    use super::*;

    fn tri(o: Vec3, d: Vec3) -> Option<TriangleClaim> {
        // A unit triangle in the z = 1 plane, wound so its normal points +Z.
        cross_triangle(
            &RayShear::new(o, d),
            d,
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        )
    }

    #[test]
    fn a_head_on_crossing_reports_its_distance_and_winding_normal() {
        let claim = tri(Vec3::new(0.1, 0.1, 0.0), Vec3::Z).expect("the ray crosses it");
        let (t, normal) = (claim.t, claim.normal);
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
        let claim = tri(Vec3::new(0.1, 0.1, 2.0), Vec3::NEG_Z).expect("the ray crosses it");
        let (t, normal) = (claim.t, claim.normal);
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
        let t = tri(Vec3::new(0.1, 0.1, 2.0), Vec3::Z)
            .expect("the plane is behind it")
            .t;
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
    /// margin the live collector would compute for it.
    ///
    /// The origin is LOCAL, and after the anchor refactor that is the only frame the margin depends
    /// on: the corridor's world position is subtracted once, on the anchor, before any of this. A
    /// fixture that had to name a world position to state a law about rounding is exactly what
    /// anchoring retired.
    fn trimesh_hits(collider: &Collider, origin: Vec3, axis: Vec3, length: f32) -> Vec<FaceHit> {
        let node = Entity::from_raw_u32(1).expect("a test entity index");
        let laws = WalkLaws::default();
        let mut out = Vec::new();
        let TypedShape::TriMesh(mesh) = collider.shape_scaled().as_typed_shape() else {
            panic!("`Collider::trimesh` builds a trimesh");
        };
        // Every fixture here is one closed box, which is one shell.
        let surfaces = BallisticSurfaces {
            shells: vec![0u32; mesh.num_triangles()].into(),
            corners: (0..mesh.num_triangles())
                .map(|index| mesh.indices()[index])
                .collect::<Vec<_>>()
                .as_slice()
                .into(),
            scale: Vec3::ONE,
        };
        collect_trimesh(
            mesh,
            origin,
            axis,
            length,
            node,
            node,
            &surfaces,
            &|_| false,
            Rotation::default(),
            prune_margin(origin, &laws),
            &laws,
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
        // In ULP, because that is the unit the defect is measured in. The live failure put the
        // origin a dozen ULP of the 25 mm LOCAL coordinate past the face — and after anchoring,
        // local ULP is the only rounding that reaches here at all. The corridor's world position is
        // subtracted once, on the anchor, so a shot at the edge of the map arrives with the same low
        // bits as one beside the world origin.
        let ulp = f32::from_bits(face.to_bits() + 1) - face;
        for past in [0.0, 1.0, 8.0, 64.0] {
            // Off the box's face diagonal, so each face presents ONE triangle and the count below
            // reads as "the face" rather than "however the mesh was triangulated".
            let hits = trimesh_hits(
                &collider,
                Vec3::new(0.1, -0.0003, face - past * ulp),
                Vec3::NEG_Z,
                0.5,
            );
            let entries: Vec<&FaceHit> = hits
                .iter()
                .filter(|hit| hit.true_normal.dot(Vec3::NEG_Z) < 0.0)
                .collect();
            assert_eq!(
                entries.len(),
                1,
                "the entry face is collected though the origin sits {past} ULP past it: {hits:?}",
            );
            assert_eq!(entries[0].t, 0.0, "and it lands at the corridor origin");
        }
    }

    /// THE MARGIN MUST NOT REACH A FACE THE MARCH DELIBERATELY STEPPED PAST.
    ///
    /// The other side of the same squeeze. Every corridor after the first resumes one
    /// [`super::MARCH_EPS`] beyond the face it is leaving, so a margin that grew to a whole nudge
    /// would re-collect the EXIT face of the plate just perforated — [`admit`] would clamp it to
    /// `t = 0`, and the walk would see an exit for a primitive it is not inside: `UnexpectedExit`,
    /// fail-closed, a round stopped dead in mid-air a millimetre past the plate it just punched
    /// through. The unclamped scaling term reaches exactly `MARCH_EPS` at 2.5 km, which is the map,
    /// so the ceiling is what stops one boundary bug from being traded for another.
    #[test]
    fn the_face_the_march_stepped_past_is_not_re_collected() {
        let collider = trimesh_box(Vec3::new(3.0, 3.0, 0.05));
        // The corridor after a perforation: origin one nudge beyond the exit face at local −0.025.
        let resumed = -0.025_f32 - super::super::MARCH_EPS;
        let hits = trimesh_hits(
            &collider,
            Vec3::new(0.1, -0.0003, resumed),
            Vec3::NEG_Z,
            0.5,
        );
        assert!(
            hits.is_empty(),
            "the plate behind the resumed corridor is gone, not re-entered: {hits:?}",
        );
    }

    #[test]
    fn admission_honours_the_seed_contract() {
        let laws = WalkLaws::default();
        // Inside the corridor: kept as-is.
        assert_eq!(admit(0.5, 1.0, false, &laws), Some(0.5));
        // Half-open at the far end: this one is the next corridor's.
        assert_eq!(admit(1.0, 1.0, false, &laws), None);
        // Behind the origin, seeded: the seed already accounts for the entry.
        assert_eq!(admit(-1.0e-7, 1.0, true, &laws), None);
        // Behind the origin, NOT seeded: the ray is sitting on the face.
        assert_eq!(admit(-1.0e-7, 1.0, false, &laws), Some(0.0));
        assert_eq!(admit(f32::NAN, 1.0, false, &laws), None);
        // BEHIND THE ORIGIN AND MEANING IT: already traversed, and not this corridor's to report.
        // A sloped plate's exit triangles have extent along the ray, so a transit corridor that
        // restarts at that exit finds them here — and dragging them to `t = 0` is an exit with
        // nothing open, which the driver fails closed on.
        for behind in [-1.0e-3, -0.05, -1.0] {
            assert_eq!(
                admit(behind, 1.0, false, &laws),
                None,
                "{behind} m behind the origin is traversed, not underfoot",
            );
        }
    }

    // -----------------------------------------------------------------------------------------
    // Edge-crossing watertightness
    // -----------------------------------------------------------------------------------------

    /// The measured conditioning of the live defect: a corridor origin metres away from geometry
    /// tens of centimetres across, on a ray aligned with no axis, meeting a face aligned with no
    /// axis. Round coordinates and axis-aligned faces round symmetrically and hide it.
    const SWEEP_SIZE: Vec3 = Vec3::new(0.4127, 0.4127, 0.1013);
    const SWEEP_DIR: Vec3 = Vec3::new(0.4871, -0.5133, -1.0);
    const SWEEP_STANDOFF: f32 = 6.0837;
    /// Half-width of the sweep, in ULP of the ray origin. The band is a few ULP wide and sits
    /// wherever the rounding puts it, so the grid brackets it rather than naming it.
    const SWEEP_REACH: i32 = 128;

    /// The retired per-triangle containment test, kept as the REFERENCE the sweep measures itself
    /// against.
    ///
    /// It anchors all three edges on the triangle's own first vertex, so two triangles sharing an
    /// edge compute that edge from different quantities: the exact-arithmetic identity between them
    /// survives, the floating-point one does not, and within a few ULP of the edge both round the
    /// ray to "outside". The sweep asserts that this predicate DOES lose crossings on the fixture,
    /// so a green run cannot mean the fixture stopped exercising the band.
    fn barycentric_contains(origin: Vec3, axis: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
        let e1 = b - a;
        let e2 = c - a;
        let det = -axis.dot(e1.cross(e2));
        if det.abs() < PARALLEL_EPS {
            return false;
        }
        let dao = (origin - a).cross(axis);
        let u = e2.dot(dao) / det;
        let v = -e1.dot(dao) / det;
        !(u < 0.0 || v < 0.0 || u + v > 1.0)
    }

    /// The eight corners of the sweep box, rotated off every axis.
    fn sweep_vertices() -> Vec<Vec3> {
        let h = SWEEP_SIZE * 0.5;
        let rotation = Quat::from_euler(EulerRot::YXZ, 0.7137, 0.3119, 0.1171);
        [
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
        .map(|(x, y, z)| rotation * Vec3::new(x * h.x, y * h.y, z * h.z))
        .collect()
    }

    /// The box's faces except the top, which each fixture triangulates for itself.
    fn sweep_sides() -> Vec<[u32; 3]> {
        vec![
            [0, 3, 2],
            [0, 2, 1],
            [0, 1, 5],
            [0, 5, 4],
            [3, 7, 6],
            [3, 6, 2],
            [0, 4, 7],
            [0, 7, 3],
            [1, 2, 6],
            [1, 6, 5],
        ]
    }

    /// Ray origins on a grid of single-ULP steps around the one aimed straight at `target`.
    ///
    /// Stepping the ORIGIN's bits, not the target: the origin is metres from the geometry, so its
    /// own ULP is the finest perturbation the corridor can express, and a smaller step applied to
    /// the target vanishes in the subtraction that forms it.
    fn sweep_origins(target: Vec3, axis: Vec3, reach: i32) -> impl Iterator<Item = Vec3> {
        let base = target - axis * SWEEP_STANDOFF;
        (-reach..=reach).flat_map(move |i| {
            (-reach..=reach).map(move |j| {
                Vec3::new(
                    f32::from_bits((base.x.to_bits() as i32 + i) as u32),
                    f32::from_bits((base.y.to_bits() as i32 + j) as u32),
                    base.z,
                )
            })
        })
    }

    /// A RAY THROUGH A SHARED EDGE MUST NOT FALL BETWEEN THE TWO TRIANGLES.
    ///
    /// The two halves of the top face are anchored on DIFFERENT vertices — the shape the live
    /// failure has (`[51, 58, 59]` beside `[58, 60, 59]`), and the shape a mesh is under no
    /// obligation to avoid. A ray within a few ULP of their shared edge is then rejected by both
    /// independent barycentric tests: the entry vanishes, the walk meets an exit for a primitive it
    /// is not inside, and the driver fails the round closed in mid-armour.
    #[test]
    fn a_ray_on_a_shared_edge_is_claimed_by_one_of_the_two_triangles() {
        let vertices = sweep_vertices();
        let axis = SWEEP_DIR.normalize();
        let target = vertices[4] + (vertices[6] - vertices[4]) * 0.5713;
        let mut lost = 0;
        let mut retired_lost = 0;
        let mut total = 0;
        for origin in sweep_origins(target, axis, SWEEP_REACH) {
            let shear = RayShear::new(origin, axis);
            let near = cross_triangle(&shear, axis, vertices[4], vertices[5], vertices[6]);
            let far = cross_triangle(&shear, axis, vertices[6], vertices[7], vertices[4]);
            if near.is_none() && far.is_none() {
                lost += 1;
            }
            if !barycentric_contains(origin, axis, vertices[4], vertices[5], vertices[6])
                && !barycentric_contains(origin, axis, vertices[6], vertices[7], vertices[4])
            {
                retired_lost += 1;
            }
            total += 1;
        }
        assert_eq!(
            lost, 0,
            "{lost} of {total} rays fell between the two triangles"
        );
        assert!(
            retired_lost > 0,
            "the retired test lost nothing on this fixture, so the sweep is not crossing the edge \
             it was built to cross",
        );
    }

    /// AND THE WALK RESOLVES IT — the consequence, end to end on a closed primitive.
    #[test]
    fn a_shared_edge_crossing_resolves_into_one_interval() {
        let vertices = sweep_vertices();
        let mut indices = sweep_sides();
        indices.extend([[4, 5, 6], [6, 7, 4]]);
        let collider = Collider::trimesh(vertices.clone(), indices);
        let axis = SWEEP_DIR.normalize();
        let target = vertices[4] + (vertices[6] - vertices[4]) * 0.5713;
        for origin in sweep_origins(target, axis, SWEEP_REACH) {
            let spans = sweep_walks(&collider, origin, axis)
                .unwrap_or_else(|error| panic!("origin {origin:?}: {error:?}"));
            assert_eq!(spans, 1, "origin {origin:?} crossed the box once");
        }
    }

    /// Walk one sweep ray and report how many presence intervals the crossing resolved into.
    ///
    /// `walk_ray` rather than a hit count, because that IS the contract: a ray exactly on a shared
    /// edge is legitimately claimed by BOTH incident triangles, and they name that edge, so the walk
    /// reads one crossing. What must never happen is a crossing going missing.
    fn sweep_walks(collider: &Collider, origin: Vec3, axis: Vec3) -> Result<usize, WalkError> {
        let length = SWEEP_STANDOFF * 2.0;
        let node = Entity::from_raw_u32(1).expect("a test entity index");
        let hits = trimesh_hits(collider, origin, axis, length);
        let walk = super::super::walk::walk_ray(
            0,
            &super::super::walk::RayCorridor {
                anchor: Vec3::ZERO,
                origin,
                axis,
                length,
                initial_presence: Vec::new(),
                hits,
            },
            &super::super::walk::VolumeTable::new([(node, 1.0)]).expect("a usable factor"),
            &WalkLaws::default(),
        )?;
        Ok(walk
            .shells
            .first()
            .map_or(0, |presence| presence.spans.len()))
    }

    /// A T-JUNCTION IS A CRACK OF ITS OWN VERTEX'S WIDTH, AND NO KERNEL CAN CLOSE IT.
    ///
    /// The same box, but one half of the top face re-triangulated around a vertex `M` placed on the
    /// diagonal without splitting the triangle on the far side of it. There is no shared edge for
    /// the sign relation to hold across, and `M` lies on the segment only to f32 rounding, so the
    /// surface really is open — over a sliver as wide as that rounding and no wider.
    ///
    /// The bound is the claim. A T-junction that lost more than its own vertex rounding would be a
    /// hole in the asset, which is a modelling fix and not a walk one.
    #[test]
    fn a_t_junction_cracks_only_within_its_own_rounding() {
        let mut vertices = sweep_vertices();
        let (v4, v6) = (vertices[4], vertices[6]);
        // Deliberately not a midpoint or a power-of-two fraction, either of which can land exactly
        // on the segment and hide the crack.
        vertices.push(v4 + (v6 - v4) * 0.3719);
        let mut indices = sweep_sides();
        // The far half keeps the whole diagonal as one edge; the near half is split around `M`.
        indices.extend([[6, 7, 4], [4, 5, 8], [8, 5, 6]]);
        let collider = Collider::trimesh(vertices.clone(), indices);
        let axis = SWEEP_DIR.normalize();
        let target = vertices[8];
        let mut cracked = 0;
        let mut total = 0;
        for origin in sweep_origins(target, axis, SWEEP_REACH) {
            match sweep_walks(&collider, origin, axis) {
                Ok(spans) => assert_eq!(spans, 1, "origin {origin:?} crossed the box once"),
                Err(_) => cracked += 1,
            }
            total += 1;
        }
        // The grid is `2·SWEEP_REACH + 1` ULP across in each direction and centred on the junction
        // vertex itself — the worst place on the whole seam to aim.
        assert!(
            cracked * 100 <= total,
            "{cracked} of {total} rays fell through the T-junction: that is a hole, not a rounding \
             sliver",
        );
    }

    /// `x` moved by `steps` f32 ULP.
    fn ulp_step(x: f32, steps: i32) -> f32 {
        f32::from_bits((x.to_bits() as i32 + steps) as u32)
    }

    /// A NONZERO f32 EDGE AREA IS NEVER ON THE WRONG SIDE, AND AN EXACT ZERO IS NEVER MISSED.
    ///
    /// [`edge_area`] is `fl(fl(q₀·p₁) − fl(q₁·p₀))`. Round-to-nearest is MONOTONE, so
    /// `q₀·p₁ ≥ q₁·p₀` implies `fl(q₀·p₁) ≥ fl(q₁·p₀)`, and the final subtraction is sign-preserving:
    /// the computed area therefore carries the exact sign or it carries none — it can never carry
    /// the opposite one, however violently the two products cancel.
    ///
    /// That is what pins the width of the f64 fallback's trigger. Exact zero is the whole of the
    /// unrecoverable set; a trigger widened to "near zero" re-decides only cases whose f32 sign was
    /// already right, so it cannot recover a crossing that was lost. MEASURED over 1 543 115
    /// genuinely interior grazing rays (f64 Möller–Trumbore reference): the shipped predicate and an
    /// exact-arithmetic evaluation of the same sheared points agree on every one of them, and a
    /// relative-error-widened trigger changes 0 decisions.
    #[test]
    fn the_sign_of_a_sheared_edge_area_is_the_exact_one_or_zero() {
        let mut cancelled = 0u32;
        let mut checked = 0u32;
        for (x, y) in [
            (0.6127f32, 0.7903f32),
            (1.4813, 0.5171),
            (0.25, 2.0),
            (3.7137, -1.1071),
            (0.5, 0.70710677),
        ] {
            let p = (x, y);
            // `q` sweeps a neighbourhood of `−p`, where the two products cancel to the last bits.
            for i in -48..=48i32 {
                for j in -48..=48i32 {
                    let q = (ulp_step(-x, i), ulp_step(-y, j));
                    let f32_area = edge_area(p, q);
                    let exact = edge_area_exact(p, q);
                    checked += 1;
                    if f32_area == 0.0 {
                        cancelled += 1;
                        continue;
                    }
                    assert_eq!(
                        f32_area > 0.0,
                        exact > 0.0,
                        "p {p:?} q {q:?}: f32 {f32_area:e} against {exact:e}",
                    );
                    assert!(
                        exact != 0.0,
                        "p {p:?} q {q:?}: an exact zero read as nonzero"
                    );
                }
            }
        }
        assert!(
            cancelled > 0,
            "{checked} pairs and not one cancellation: the sweep is not crossing the band",
        );
    }

    /// A SLIVER FACE IN A CLOSED SURFACE LOSES NO CROSSING.
    ///
    /// One triangle of the top face is fanned around a point 0.1 µm off its own long diagonal, so
    /// the fan contains a triangle whose sheared projection is a sliver 0.4 m long: every interior
    /// point of it is within a few ULP of two of its own edges, and its edge areas are the
    /// difference of two products that agree to their last bits.
    ///
    /// A sliver's containment answer is not a claim about 3D barycentrics — the shear projects both
    /// halves of every shared edge from the SAME vertices, so the projected triangles tile the plane
    /// exactly, and a ray the sliver declines is claimed by whichever neighbour owns that side of the
    /// edge. The retired per-triangle test had no such relation, which is why it drops rays here.
    #[test]
    fn a_sliver_face_in_a_closed_surface_loses_no_crossing() {
        let mut vertices = sweep_vertices();
        let (v4, v5, v6) = (vertices[4], vertices[5], vertices[6]);
        // Strictly inside the triangle `[4, 5, 6]`, and 0.1 µm off the diagonal `4–6`.
        let foot = v4 + (v6 - v4) * 0.4137;
        let inner = foot + (v5 - foot).normalize() * 1.0e-7;
        vertices.push(inner);
        let mut indices = sweep_sides();
        // The far half keeps the diagonal whole; the near half is fanned around the interior point,
        // so the surface stays closed and `[6, 4, 8]` is the sliver.
        indices.extend([[6, 7, 4], [4, 5, 8], [5, 6, 8], [6, 4, 8]]);
        let collider = Collider::trimesh(vertices.clone(), indices);
        let axis = SWEEP_DIR.normalize();
        let (a, b, c) = (vertices[6], vertices[4], vertices[8]);
        let target = a * 0.170 + b * 0.238 + c * 0.592;
        let mut retired_lost = 0;
        let mut total = 0;
        for origin in sweep_origins(target, axis, SWEEP_REACH) {
            let spans = sweep_walks(&collider, origin, axis)
                .unwrap_or_else(|error| panic!("origin {origin:?}: {error:?}"));
            assert_eq!(spans, 1, "origin {origin:?} crossed the box once");
            let shear = RayShear::new(origin, axis);
            let fan = [[a, b, c], [b, v5, c], [v5, a, c]];
            let shipped = fan
                .iter()
                .filter(|t| cross_triangle(&shear, axis, t[0], t[1], t[2]).is_some())
                .count();
            let retired = fan
                .iter()
                .filter(|t| barycentric_contains(origin, axis, t[0], t[1], t[2]))
                .count();
            if shipped > 0 && retired == 0 {
                retired_lost += 1;
            }
            total += 1;
        }
        assert!(
            retired_lost > 0,
            "the retired test lost nothing over {total} rays, so the sweep is not crossing the              sliver",
        );
    }

    /// One triangle crossing in f64, from the f32 vertices — the reference the shipped kernel is
    /// measured against.
    ///
    /// Möller–Trumbore rather than a second shear, so the reference shares no arithmetic with the
    /// thing it is judging: a bug in the shear cannot hide inside its own oracle. Double-sided, like
    /// the kernel.
    ///
    /// The mesh's f32 vertices are the geometry. The reference is what an infinitely precise ray
    /// through THOSE points would answer, evaluated in f64 — sixteen orders below the micron the
    /// disagreements live at, so it stands in for exact arithmetic here.
    fn moller_trumbore_f64(
        origin: DVec3,
        axis: DVec3,
        a: DVec3,
        b: DVec3,
        c: DVec3,
    ) -> Option<f64> {
        let (e1, e2) = (b - a, c - a);
        let pvec = axis.cross(e2);
        let det = e1.dot(pvec);
        if det.abs() < 1.0e-18 {
            return None;
        }
        let inv = 1.0 / det;
        let tvec = origin - a;
        let u = tvec.dot(pvec) * inv;
        if !(0.0..=1.0).contains(&u) {
            return None;
        }
        let qvec = tvec.cross(e1);
        let v = axis.dot(qvec) * inv;
        if v < 0.0 || u + v > 1.0 {
            return None;
        }
        Some(e2.dot(qvec) * inv)
    }

    /// THE BAND'S DOMAIN, STATED BY THE TRIANGLE THAT LIES OUTSIDE IT.
    ///
    /// [`PROJECTION_SLACK`] is a bound on the projection's rounding only where the relative-error
    /// model it is derived from holds; on a vertex whose corridor-relative offset is subnormal it
    /// underflows to zero and claims an exactness the arithmetic does not have. Codex built the
    /// triangle that turns that into a declined crossing: one vertex at the very bottom of the
    /// subnormal range, two at `10¹⁰`, so the near-zero vertex's unclaimed absolute error is
    /// multiplied up into a wrong-signed edge area of `1.6079e-36` against a band of `1.3468e-38`.
    /// No edge triggers the f64 reconsideration, and `cross_triangle` returns `None` where the
    /// independent f64 reference finds a crossing.
    ///
    /// The answer is not a wider band — it is that this triangle is not geometry. Both ends of it
    /// are outside [`crate::bake::CERTIFIED_RANGE`], so the bake refuses it before a collider is
    /// ever built, and the kernel's bound is written as a claim about that domain. This test is what
    /// makes the exclusion a fact rather than a footnote: the domain predicate REFUSES it, and the
    /// kernel's answer here is recorded, not defended.
    #[test]
    fn a_subnormal_triangle_is_outside_the_certified_domain() {
        let denormal = f32::from_bits(1);
        let a = Vec3::new(-64.0 * denormal, -64.0 * denormal, denormal);
        let b = Vec3::new(1.0e10, 10_017_928_192.0, 0.0);
        let c = Vec3::new(-2.0e10, -1.0e10, 0.0);
        let axis = Vec3::new(0.3, 0.4, 0.866_025_4);
        let origin = Vec3::ZERO;

        // Both ends of the certified range refuse it: the subnormal vertex and the 10¹⁰ one.
        assert!(
            !crate::bake::certified_coordinate(a.z),
            "the subnormal vertex must be refused at the floor",
        );
        assert!(
            !crate::bake::certified_coordinate(b.x),
            "the 10¹⁰ vertex must be refused at the ceiling",
        );
        // And a coordinate a tank actually has is not refused with it.
        assert!(
            crate::bake::certified_coordinate(1.234) && crate::bake::certified_coordinate(0.0),
            "the gate must pass the geometry it exists to protect",
        );

        // What the kernel does out there, recorded. The reference accepts; the kernel declines; the
        // bound never spoke for this triangle either way.
        let reference = moller_trumbore_f64(
            origin.as_dvec3(),
            axis.as_dvec3(),
            a.as_dvec3(),
            b.as_dvec3(),
            c.as_dvec3(),
        );
        assert!(
            reference.is_some(),
            "the counterexample must still be a crossing in exact arithmetic",
        );
        let shear = RayShear::new(origin, axis);
        assert!(
            cross_triangle(&shear, axis, a, b, c).is_none(),
            "the recorded out-of-domain behaviour changed — re-derive the bound before re-pinning",
        );
    }

    /// A CLOSED SURFACE'S SILHOUETTE MUST NOT SWALLOW BOTH CROSSINGS.
    ///
    /// The shear's edge test is watertight BETWEEN triangles: neighbours share an edge's two
    /// endpoints, so the same rounded 2D points serve both and the sign relation is exact. What that
    /// argument never covered is the projection itself. Translation, shear multiply and sum all run
    /// in f32, and at a corridor's scale the subtraction `vertex − origin` is metres wide while the
    /// lateral offset it must resolve is centimetres — so each projected vertex lands up to about a
    /// micron off the point the exact map would put it at, and the whole projected silhouette
    /// breathes by that much.
    ///
    /// A ray a fraction of a micron inside that silhouette then falls OUTSIDE the rounded one, every
    /// incident triangle declines it, and the walk sees no crossing at all: not a fail-closed error,
    /// a silent zero.
    ///
    /// A 513 × 513 ULP grid centred on the box's own corner — the worst place on a closed surface to
    /// aim, where four faces and three silhouette edges meet. 78 871 of those rays have a positive
    /// exact chord, and against the f64 reference the census over 3 158 028 triangle tests reads:
    ///
    /// | | chords erased | chords halved | crossings declined | claims the reference denies |
    /// |---|---|---|---|---|
    /// | f32 projection alone | 271 | 0 | 965 | 463 |
    /// | with the f64 band | 0 | 0 | 0 | 463 |
    ///
    /// The widest erased chord was 0.763528 µm, at origin `(-2.5700798, 2.315606, 5.0677752)`. The
    /// last column does not move because the band never re-decides an acceptance.
    #[test]
    fn a_ray_inside_the_silhouette_keeps_its_chord() {
        let vertices = sweep_vertices();
        let mut indices = sweep_sides();
        indices.extend([[4, 5, 6], [6, 7, 4]]);
        let axis = SWEEP_DIR.normalize();
        let (axis64, tris) = (
            axis.as_dvec3(),
            indices
                .iter()
                .map(|t| {
                    [
                        vertices[t[0] as usize].as_dvec3(),
                        vertices[t[1] as usize].as_dvec3(),
                        vertices[t[2] as usize].as_dvec3(),
                    ]
                })
                .collect::<Vec<_>>(),
        );

        let (mut chorded, mut lost, mut halved) = (0u32, 0u32, 0u32);
        // Per-TRIANGLE disagreements with the reference, in both directions. `over` is deliberate —
        // the band may only add acceptances, so an f32 claim the reference denies is kept and
        // counted rather than flipped. `under` is the defect class itself.
        let (mut over, mut under) = (0u32, 0u32);
        let mut widest = 0.0f64;
        for origin in sweep_origins(vertices[4], axis, 256) {
            let shear = RayShear::new(origin, axis);
            let mut exact: Vec<f64> = Vec::new();
            let mut shipped = 0u32;
            for (t, exact_tri) in indices.iter().zip(&tris) {
                let reference = moller_trumbore_f64(
                    origin.as_dvec3(),
                    axis64,
                    exact_tri[0],
                    exact_tri[1],
                    exact_tri[2],
                );
                let claimed = cross_triangle(
                    &shear,
                    axis,
                    vertices[t[0] as usize],
                    vertices[t[1] as usize],
                    vertices[t[2] as usize],
                )
                .is_some();
                match (claimed, reference) {
                    (true, Some(distance)) => {
                        shipped += 1;
                        exact.push(distance);
                    }
                    (true, None) => {
                        shipped += 1;
                        over += 1;
                    }
                    (false, Some(distance)) => {
                        under += 1;
                        exact.push(distance);
                    }
                    (false, None) => {}
                }
            }
            exact.sort_by(f64::total_cmp);
            let chord = match (exact.first(), exact.last()) {
                (Some(first), Some(last)) => last - first,
                _ => 0.0,
            };

            if chord > 0.0 {
                chorded += 1;
                match shipped {
                    0 => {
                        lost += 1;
                        widest = widest.max(chord);
                    }
                    1 => halved += 1,
                    _ => {}
                }
            }
        }

        assert!(
            chorded > 70_000,
            "{chorded} rays with a chord: the grid has stopped straddling the corner",
        );
        assert_eq!(
            lost, 0,
            "{lost} of {chorded} chords erased entirely, the widest {widest:e} m",
        );
        assert_eq!(halved, 0, "{halved} of {chorded} chords lost one crossing");
        assert_eq!(
            under, 0,
            "{under} triangle crossings the reference finds and the kernel declines",
        );
        // The other direction is the rounded tiling's own over-claim, MEASURED at 463 of 3 158 028
        // triangle tests and identical before and after the band — the f32 decision is never
        // flipped, only supplemented. A ray charged to a face a hair outside it is the direction
        // §13 allows; the bound is here so the count cannot grow unnoticed.
        assert!(
            over * 100 <= chorded,
            "{over} kept f32 claims the reference denies, against {chorded} chords",
        );
    }

    /// A RAY THROUGH A SHARED VERTEX IS CLAIMED BY THE FAN AROUND IT.
    ///
    /// The shared-EDGE relation is exact antisymmetry between two triangles. A vertex has no such
    /// pairing: it is the meeting point of a whole fan, and its watertightness rests on the shear
    /// projecting that one vertex to one 2D point for every triangle that names it. This aims at a
    /// box corner where four faces meet, rotated off every axis, and steps the origin one ULP at a
    /// time across it.
    ///
    /// The claim is the walk's, not a hit count's: a ray on the corner is claimed by the whole fan,
    /// and every face of it names that welded vertex, so the walk reads one contact. What must never
    /// happen is a crossing going missing, which the corridor reports as a `WalkError`.
    #[test]
    fn a_ray_through_a_shared_vertex_is_claimed_by_the_fan() {
        let vertices = sweep_vertices();
        let mut indices = sweep_sides();
        indices.extend([[4, 5, 6], [6, 7, 4]]);
        let collider = Collider::trimesh(vertices.clone(), indices);
        let axis = SWEEP_DIR.normalize();
        // Vertex 4 is named by four of the box's faces.
        let fan: Vec<[u32; 3]> = {
            let mut all = sweep_sides();
            all.extend([[4, 5, 6], [6, 7, 4]]);
            all.into_iter().filter(|t| t.contains(&4)).collect()
        };
        assert_eq!(fan.len(), 4, "the corner is a fan of four");
        let (mut crossed, mut missed, mut retired_lost, mut unrepresentable, mut total) =
            (0, 0, 0, 0, 0);
        for origin in sweep_origins(vertices[4], axis, 32) {
            total += 1;
            match sweep_walks(&collider, origin, axis) {
                Ok(0) => missed += 1,
                Ok(1) => crossed += 1,
                Ok(other) => panic!("origin {origin:?} crossed the box {other} times"),
                // A ray that clips the corner has a chord that shrinks continuously to zero, so
                // some of these rays bound less material than one ULP of `t` can express. That is
                // refused BY NAME rather than charged as nothing: the corridor cannot represent the
                // chord, and a silent zero is the one answer §13.1 forbids.
                Err(WalkError::UnrepresentableChord { .. }) => unrepresentable += 1,
                Err(error) => panic!("origin {origin:?}: {error:?}"),
            }
            let shear = RayShear::new(origin, axis);
            let claim = |t: &[u32; 3], retired: bool| {
                let (a, b, c) = (
                    vertices[t[0] as usize],
                    vertices[t[1] as usize],
                    vertices[t[2] as usize],
                );
                if retired {
                    barycentric_contains(origin, axis, a, b, c)
                } else {
                    cross_triangle(&shear, axis, a, b, c).is_some()
                }
            };
            if fan.iter().any(|t| claim(t, false)) && !fan.iter().any(|t| claim(t, true)) {
                retired_lost += 1;
            }
        }
        assert!(
            crossed > 0 && missed > 0,
            "the grid must bracket the corner: {crossed} crossed, {missed} missed",
        );
        assert!(
            retired_lost > 0,
            "the retired test lost nothing at the corner, so the sweep is not crossing the fan",
        );
        // The refusal band is the corner's own sub-ULP sliver, not a hole in the surface: it is a
        // narrow fringe of a grid aimed at the very worst point on the box.
        assert!(
            unrepresentable * 20 <= total,
            "{unrepresentable} of {total} corner rays were refused as unrepresentable — that is a \
             band, not a fringe",
        );
    }
}
