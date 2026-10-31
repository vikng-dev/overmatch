//! The §13 union walk, driven against the live world.
//!
//! Nothing here is entity-restricted and nothing defaults to zero: a corridor is collected, the
//! union field is integrated over everything in it, and every failure is loud. A collection
//! restricted to the struck entity would enter an overlapping or abutting neighbour from inside,
//! find no exit for it, and cross it at zero cost (§13.1's table).
//!
//! # The atomic resolution corridor
//!
//! §13.4's walk is defined over a corridor that CLOSES — one that contains the whole crossing plus
//! enough clear space past it to prove no ε-weld reaches further. So the driver does not stop at the
//! tick's travel budget: it collects from first contact, and if the walk reports
//! [`WalkError::IncompleteCorridor`] it EXTENDS and re-collects, up to [`MAX_CORRIDOR`]. It never
//! synthesizes the missing exit. That choice is what
//! keeps continuation state out of the resolver — the only thing that survives a crossing is which
//! primitives the disc is still inside, and that is a lookup on rays already walked
//! ([`DiscWalk::resume_at`]), never an inference.
//!
//! # One mechanism, decided before it is applied
//!
//! [`decide`] resolves a crossing from a [`Flight`] — the round's position, direction, speed,
//! CURRENT sampling frame and continuation — into a typed [`Decision`] with nothing mutated.
//! [`apply`] carries the runtime's consequences out of it (HP, impulses, marks, spall, events), and
//! [`Flight::follow`] advances the flight to where the next crossing starts. The runtime march and
//! the §13.6 gate (`fuzz`) both drive successive crossings through exactly this state machine; the
//! gate observes the decisions and applies nothing. The seam is not yet complete: a spall cone's
//! directions are marched in [`apply`], not decided (see there).
//!
//! # Fields
//!
//! A spall cone's directions are walked by the same collector and pairing walk as the line's
//! corridors ([`field_presence`]), and stopped and dosed over the ordered presence it returns
//! ([`field_direction`]). The field law they serve — strength, profile, shadowing, dose — is the
//! design doc's §5 (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! # Fail closed, never free
//!
//! On any structured [`WalkError`] the round STOPS at the contact it was resolving: no perforation,
//! no spall, no transit damage. A wrong answer here is free penetration, and free penetration is
//! indistinguishable from armour that was never modelled — the defect class §13 exists to kill.
//! Crashing a multiplayer authority is too strong a response; a stopped shell and a rate-limited
//! warning is not.

use std::cell::OnceCell;

use avian3d::parry::shape::TypedShape;
use avian3d::prelude::{Collider, ColliderAabb, Forces, Position, Rotation, SimpleCollider};
use bevy::prelude::*;

use super::collect::{self, Corridor};
use super::mechanism::{Dose, DoseSource, Limits, Mechanism, SolidShot};
use super::walk::{
    self, Begin, DiscCorridor, DiscFrame, DiscWalk, Outcome, RayCorridor, SampleCorridor,
    SampleSeed, ShellKey, Shot, SpallSource, VolumeTable, WalkError, WalkLaws,
};
use super::{
    ArmourMark, BallisticSurfaces, ComponentHealth, CrossingEnd, Impact, ImpactOutcome,
    ImpactSurface, MARCH_EPS, MarchingShell, PenetrationEvent, ProjectileMarchWorld, ShellRicochet,
    ShellStrike, ShellTerminal, TerminalVerdict, Verdict, VolumeCrossing, apply_hit_impulse,
    throw_spall_cone,
};

#[cfg(test)]
mod tests;

/// How far past first contact the first corridor reaches. Most crossings close well inside it; the
/// driver extends when they do not, so this is a starting guess, not a limit.
const ENTRANCE_SPAN: f32 = 0.5;

/// Ceiling on corridor extension. A crossing that has not closed in 50 m of material is not a
/// crossing.
pub(crate) const MAX_CORRIDOR: f32 = 50.0;

/// Ceiling on how far past its travel budget one step may carry the flight — a chosen travel
/// limit, not a certified bound.
///
/// Past the budget a step runs only to close material a sample is held inside
/// ([`Interior::holds_material`]). Each such crossing closes within one [`MAX_CORRIDOR`], but a
/// succession of them does not: held crossings chained end to end can carry the flight arbitrarily
/// far while each one stays certifiable, so exceeding this does not prove the geometry malformed. It
/// bounds how far downrange one step may apply consequences; the value is set at the corridor's
/// scale as policy, and a step that reaches it stops the round (fail closed).
pub(crate) const MAX_OVERRUN: f32 = MAX_CORRIDOR;

/// Ceiling on the crossings one step may decide — a chosen resource limit, not a certified bound.
///
/// Nothing maps a step's crossings onto a bounded set of faces: successive corridors can meet
/// disjoint faces, a bounce can return to a face already struck, and an empty span consumes distance
/// rather than a face. [`collect::MAX_FACES`] bounds one corridor's collection, not a step. So the
/// value reuses that scale as policy, bounding the work one step may do; a step that reaches it
/// stops the round (fail closed), whether or not the geometry was well formed.
pub(crate) const MAX_STEP_CROSSINGS: usize = collect::MAX_FACES;

/// The crossing ceiling a world's steps run under in place of [`MAX_STEP_CROSSINGS`], when it names
/// one: a fixture reaches the limit in a few crossings instead of thousands.
#[derive(Resource, Clone, Copy)]
pub(crate) struct StepCrossings(pub(crate) usize);

/// One step's travel budget, spent by its successive crossings, and the limits nothing may pass.
///
/// Shared by the runtime march and the §13.6 gate's driver, so both stop a runaway step at the same
/// crossing with the same structured error.
pub(crate) struct Budget {
    /// Metres of travel left; negative once a drain runs past the budget.
    remaining: f32,
    crossings: usize,
    /// The crossings the step may decide.
    max_crossings: usize,
}

impl Budget {
    /// `travel` metres, spent by at most `max_crossings` crossings.
    pub(crate) fn new(travel: f32, max_crossings: usize) -> Self {
        Self {
            remaining: travel,
            crossings: 0,
            max_crossings,
        }
    }

    /// Metres of travel left in the step (negative while draining past it).
    pub(crate) fn remaining(&self) -> f32 {
        self.remaining
    }

    /// The step has more to resolve: travel left, or material the flight is held inside, whose
    /// crossing resolves now, past the budget if need be (see [`MAX_OVERRUN`]).
    pub(crate) fn continues(&self, flight: &Flight) -> bool {
        self.remaining > MARCH_EPS || flight.holds_material()
    }

    /// Spend `travel` metres of open flight, counting no crossing; a negative `travel` hands
    /// metres back.
    pub(crate) fn fly(&mut self, travel: f32) {
        self.remaining -= travel;
    }

    /// Charge one more crossing of `travel` metres, BEFORE its consequences are applied — or the
    /// structured error that stops the round where it stands. Exhaustion fails closed: a round that
    /// hits a limit stops; it is never waved through.
    pub(crate) fn charge(&mut self, travel: f32) -> Result<(), WalkError> {
        let crossings = self.crossings + 1;
        let remaining = self.remaining - travel;
        let overrun = (-remaining).max(0.0);
        if crossings > self.max_crossings || overrun > MAX_OVERRUN {
            return Err(WalkError::DrainExhausted { crossings, overrun });
        }
        self.crossings = crossings;
        self.remaining = remaining;
        Ok(())
    }
}

/// Below this calibre the disc degenerates to its axis (`k = 1`).
///
/// The ring sits at `calibre / 2`, so for an MG round it is 4 mm from the axis — inside the geometry
/// tolerance of everything it can meet, and §13.5 already says a fragment IS a shell with `r → 0`.
/// Paying twelve extra casts to sample a 4 mm disc buys nothing, and the machine-gun is the round we
/// fire by the beltful.
///
/// The 20 mm VALUE is arbitrary — it sits above every round currently fired beltwise and below
/// every cannon shell, and nothing was tuned finer than that. Revisit when higher-caliber automatic
/// weapons or autocannons with a high fire rate arrive: they cross or crowd the threshold, and a
/// sub-threshold round resolves as a zero-width line that can thread gaps narrower than the real
/// projectile. Cost basis for the revisit, MEASURED on the M4 (branch `bench/disc-min-caliber`):
/// 40 / 177 / 400 µs per impact at k = 1 / 5 / 13; free flight is
/// k-independent. Precondition: the degenerate-bake volumes on the fuzzer residue list — a ring
/// samples 13× more surface, so fail-closed impacts scale ~4% → 14% with k on today's asset.
pub(crate) const DISC_MIN_CALIBER: f32 = 0.020;

/// The limits every crossing's mechanism advances under: a round is at rest once its readout would
/// fall below zero reference-mm — exhausted, and nothing more.
const LIMITS: Limits = Limits { rest: 0.0 };

/// The disc radius a round of `caliber` samples at — zero below [`DISC_MIN_CALIBER`].
pub(crate) fn disc_radius(caliber: f32) -> f32 {
    if caliber >= DISC_MIN_CALIBER {
        caliber * 0.5
    } else {
        0.0
    }
}

/// Where each sample's next sweep starts and what it is inside there, in ONE anchor frame.
///
/// INVARIANT: every sample's start (`anchor + origin + seeds[i].offset`) is a point its own ray has
/// already reached — the end of an open-air sweep, of a corridor that met nothing, or a
/// perforation's resume point one nudge past the exit along that sample's ray — and
/// `seeds[i].inside` is what that ray's walk says the sample is inside there. A fresh disc — at the
/// muzzle, after a bounce's lift, a replica's re-seed, a catch-up or a held round's release — is no
/// exception: its starts are walked too, by the containment probe ([`certified`]) that reaches each
/// one from outside every candidate's bounds along its own ray. Offsets are whatever the rays left
/// behind — staggered along an oblique plate after a perforation, tilted against the travel after a
/// bend — and are never re-squared, rotated or flattened: any of those moves a start off the ray
/// that certified it.
///
/// The origin and the seeds share one anchor so they agree to corridor-scale rounding wherever on
/// the map the shot is. A resume point rebuilt in world f32 is quantized at the world's spacing
/// (DERIVED: 2.44e-4 m past 2048 m), far wider than the topology band the seeds were judged in, and an
/// entry face that close to the plane is then owned by the seed AND re-collected ahead of the origin,
/// or by nobody. The flight converts to world only for its cast and for presentation.
///
/// Persisted on the shell across ticks ([`super::Projectile`]), so a tick boundary is not a start.
#[derive(Clone, Debug)]
pub(crate) struct Interior {
    /// WORLD anchor of the corridor the seeds were read on.
    pub anchor: Vec3,
    /// The axis sample's start, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    /// One per sample, in canonical sample order; `seeds[0]` is the axis (zero offset).
    pub seeds: Vec<SampleSeed>,
}

impl Interior {
    /// A layout at `position` with every sample outside everything — true only where no armour is
    /// near the samples, which [`certified`] establishes before it lays one.
    pub(crate) fn laid(position: Vec3, offsets: Vec<Vec3>) -> Self {
        Self {
            anchor: position,
            origin: Vec3::ZERO,
            seeds: offsets
                .into_iter()
                .enumerate()
                .map(|(sample, offset)| SampleSeed {
                    sample,
                    offset,
                    t: 0.0,
                    inside: Vec::new(),
                })
                .collect(),
        }
    }

    /// Some sample resumes inside material. The next crossing must then be resolved from these seeds
    /// before the flight may stop marching: the material's exit lies ahead with its entry behind,
    /// and only the seeds say the sample is inside.
    pub(crate) fn holds_material(&self) -> bool {
        self.seeds.iter().any(|seed| !seed.inside.is_empty())
    }

    /// WORLD start of the axis sample.
    pub(crate) fn world_origin(&self) -> Vec3 {
        self.anchor + self.origin
    }

    /// The same starts, re-anchored at the axis start so a new step's corridors sit within one
    /// step's reach of their anchor (the scale [`collect`]'s sitting reach is bounded for). The
    /// residual `origin` keeps what world rounding of the new anchor lost, so the starts move by
    /// corridor-scale rounding only.
    pub(crate) fn rebased(self) -> Self {
        let anchor = self.world_origin();
        Self {
            origin: self.origin - (anchor - self.anchor),
            anchor,
            seeds: self.seeds,
        }
    }
}

/// The offsets of a fresh disc of `radius` laid on `frame`: the axis, then the ring.
pub(crate) fn layout(frame: &DiscFrame, radius: f32) -> Vec<Vec3> {
    if radius > 0.0 {
        walk::disc_offsets(frame, radius, walk::DEFAULT_RING)
    } else {
        vec![Vec3::ZERO]
    }
}

/// How far outside the candidates' bounds a containment probe starts and ends, and how far a
/// candidate's bounds are grown when asked whether they contain a start.
///
/// It must exceed the collector's sitting reach (at most a quarter of [`MARCH_EPS`],
/// `collect::sitting_reach`), so no face of a candidate lies within reach of either end and the
/// probe's own origin is never read as sitting on one, and no face within reach of a start belongs
/// to a candidate the probe set aside.
pub(super) const PROBE_MARGIN: f32 = MARCH_EPS;

/// Ceiling on the volumes one containment probe reads — a chosen resource limit, not a derived
/// bound.
///
/// A probe runs one narrow phase per candidate per sample, so its work grows with the candidates
/// whose bounds contain a start; nothing in the geometry bounds how many of those there are. The
/// value is policy, set well above what probes read today (MEASURED: at most 4 candidates and 78
/// faces per probe over the 25 402 probes the library's test suite runs, the §13.6 gate's CI-scale
/// run included), and a start that reaches it fails closed ([`WalkError::UncertifiedStart`]),
/// whether or not the geometry was well formed.
pub(crate) const MAX_PROBE_CANDIDATES: usize = 64;

/// Ceiling on the faces one containment probe collects, summed over its samples — a chosen
/// resource limit, not a derived bound. It reuses one corridor's [`collect::MAX_FACES`] as policy;
/// a probe that reaches it fails closed ([`WalkError::UncertifiedStart`]).
///
/// With [`MAX_PROBE_CANDIDATES`] it bounds what a probe COLLECTS and walks. It does not bound the
/// triangles a candidate's narrow phase tests to find them: that is each candidate's own mesh, finite
/// asset data with no geometry-independent cap.
pub(crate) const MAX_PROBE_FACES: usize = collect::MAX_FACES;

/// A fresh disc laid on `frame` at the world point `at`, travelling `axis`, with every sample's
/// membership ESTABLISHED rather than declared.
///
/// Only armour whose bounds contain a sample can contain it, so the broad phase over the disc's
/// own box names every candidate ([`candidates`], with the march's own self-exclusion) — as
/// current as Avian's collider AABBs, which every corridor's broad phase relies on equally — and the
/// probe reads only those whose bounds, grown by [`PROBE_MARGIN`], contain some sample's start.
/// With none, every sample is outside everything. Otherwise each sample's ray is walked from a point
/// outside all those candidates' bounds — a world AABB contains its collider — through its start to
/// the far side of them, with nothing declared: the same collector and the same pairing walk as any
/// corridor, so a sample's membership is the parity of oriented crossings behind it, and the
/// corridor must close or the walk errors. The seeds are read at the start
/// ([`walk::RayWalk::inside_at`]), under the band that and the collector's hand-off clamp already
/// share: a face within rounding of a start is left to the corridor that sweeps from it. Each
/// sample's ray is walked alone ([`walk::walk_ray`]): membership needs no association across the
/// disc.
///
/// The probe is formed AT each start and reaches back from it ([`Corridor::behind`]): its line is
/// built from `anchor = at` and `origin = offset`, the very values the returned starts hold and the
/// corridor resumed from them casts with, so certification and resumption decide every crossing on
/// one line in one arithmetic. A probe origin re-formed metres behind the start would round the
/// start's low bits away, and a face the ray runs along would then be read on the wrong side of a
/// start it lies within rounding of.
///
/// Certified for the two shapes the collector reads honestly: a trimesh carrying its manifold
/// certificate, and a convex solid. Any other shape is [`WalkError::UncertifiedStart`], as is a
/// probe longer than [`MAX_CORRIDOR`] or past its work ceilings ([`MAX_PROBE_CANDIDATES`],
/// [`MAX_PROBE_FACES`]); every structured error fails the round closed.
pub(crate) fn certified(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    at: Vec3,
    axis: Dir3,
    frame: DiscFrame,
    radius: f32,
) -> Result<Interior, WalkError> {
    let offsets = layout(&frame, radius);
    let axis = Vec3::from(axis);
    let reach = offsets
        .iter()
        .map(|offset| offset.length())
        .fold(0.0, f32::max);

    // The candidates whose bounds contain a start, and the union of those bounds, relative to `at`.
    let mut near = Vec::new();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for (node, primitive) in candidates(context, not_own, at, axis, 0.0, reach) {
        let Ok((position, rotation, collider, _)) = context.colliders.get(primitive) else {
            return Err(WalkError::CollectorFailed {
                volume: node,
                reason: "a candidate collider has no resolved pose",
            });
        };
        let bounds = collider.aabb(position.0, *rotation);
        let (min, max) = (bounds.min - at, bounds.max - at);
        let margin = Vec3::splat(PROBE_MARGIN);
        if !offsets
            .iter()
            .any(|&offset| offset.cmpge(min - margin).all() && offset.cmple(max + margin).all())
        {
            continue;
        }
        let shape = collider.shape_scaled();
        if !matches!(shape.as_typed_shape(), TypedShape::TriMesh(_)) && !shape.is_convex() {
            return Err(WalkError::UncertifiedStart {
                volume: node,
                reason: "a candidate is neither a certified trimesh nor convex",
            });
        }
        lo = lo.min(min);
        hi = hi.max(max);
        near.push((node, primitive));
    }
    if near.is_empty() {
        return Ok(Interior::laid(at, offsets));
    }
    if near.len() > MAX_PROBE_CANDIDATES {
        return Err(WalkError::UncertifiedStart {
            volume: near[0].0,
            reason: "the containment probe reads more candidates than its ceiling",
        });
    }
    // How far along `direction` the point leaves the box: past the first slab it leaves, it is
    // outside the box.
    let leave = |point: Vec3, direction: Vec3| {
        (0..3)
            .filter_map(|i| match direction[i] {
                d if d > 0.0 => Some((hi[i] - point[i]) / d),
                d if d < 0.0 => Some((lo[i] - point[i]) / d),
                _ => None,
            })
            .fold(f32::INFINITY, f32::min)
            .max(0.0)
    };
    let behind = offsets
        .iter()
        .map(|&offset| leave(offset, -axis))
        .fold(0.0, f32::max)
        + PROBE_MARGIN;
    let ahead = offsets
        .iter()
        .map(|&offset| leave(offset, axis))
        .fold(0.0, f32::max)
        + PROBE_MARGIN;
    let length = behind + ahead;
    if !length.is_finite() || length > MAX_CORRIDOR {
        return Err(WalkError::UncertifiedStart {
            volume: near[0].0,
            reason: "the containment probe through the candidates' bounds exceeds the corridor \
                     ceiling",
        });
    }

    let mut samples = Vec::with_capacity(offsets.len());
    let mut faces = 0;
    for offset in offsets {
        let mut hits = Vec::new();
        collect::collect(
            &Corridor {
                anchor: at,
                origin: offset,
                axis,
                length,
                behind,
                seeded: &[],
                handoff: None,
                laws: &context.laws,
            },
            &near,
            context.colliders,
            &mut hits,
        )?;
        faces += hits.len();
        if faces > MAX_PROBE_FACES {
            return Err(WalkError::UncertifiedStart {
                volume: near[0].0,
                reason: "the containment probe collects more faces than its ceiling",
            });
        }
        samples.push(SampleCorridor {
            offset,
            initial_presence: Vec::new(),
            hits,
        });
    }
    let probe = DiscCorridor {
        anchor: at,
        origin: -axis * behind,
        axis,
        length,
        radius,
        frame,
        samples,
    };
    let volumes = volume_table(context, &probe)?;
    let handoff = behind as f64;
    let seeds = probe
        .samples
        .into_iter()
        .enumerate()
        .map(|(sample, corridor)| {
            let walked = walk::walk_ray(
                sample,
                &RayCorridor {
                    anchor: at,
                    origin: probe.origin + corridor.offset,
                    axis,
                    length,
                    initial_presence: Vec::new(),
                    hits: corridor.hits,
                },
                &volumes,
                &context.laws,
            )?;
            Ok(SampleSeed {
                sample,
                offset: corridor.offset,
                t: handoff,
                inside: walked.inside_at(handoff, &context.laws),
            })
        })
        .collect::<Result<Vec<_>, WalkError>>()?;
    Ok(Interior {
        anchor: at,
        origin: Vec3::ZERO,
        seeds,
    })
}

/// The colliders one field throw can reach, read from the broad phase once for all of its
/// directions ([`field_direction`]): every collider the throw admits whose broad-phase bounds
/// ([`ColliderAabb`]) meet a region holding every box a direction's queries sweep, kept in the
/// order a query sorts its candidates in.
///
/// A direction's candidates are the kept colliders whose bounds meet its own swept box: the test
/// avian's AABB query applies to each collider of a tree leaf it reaches. The query of the region
/// reaches every collider a query of a box inside it reaches, and a query of that box reaches
/// every collider whose bounds meet it, because a collider tree's node bounds hold its leaves'
/// (enlarged) bounds and a leaf's hold its colliders' [`ColliderAabb`]s (avian's
/// `EnlargedAabb::update` keeps the enlarged bounds around the tight ones). So each direction reads
/// exactly the colliders, in the order, that a query of its own box returns.
pub(crate) struct FieldReach {
    /// Armour colliders (a [`ProjectileMarchWorld::armour`] ancestor) the throw admits.
    hard: Vec<Reached>,
    /// Witness colliders the throw admits, with their witness node.
    witnesses: Vec<(Entity, Entity, ColliderAabb)>,
    /// The union of the pose bounds of the hard colliders within [`MAX_CORRIDOR`] of the throw
    /// point ([`horizon`]), read on first use: it does not depend on the direction.
    union: OnceCell<(Vec3, Vec3)>,
}

/// One armour collider of a [`FieldReach`].
struct Reached {
    node: Entity,
    collider: Entity,
    broad: ColliderAabb,
    /// Its bounds at its pose, `None` when it has no resolved pose; read on first use.
    bounds: OnceCell<Option<ColliderAabb>>,
}

impl Reached {
    fn bounds(&self, context: &ResolveContext<'_, '_, '_>) -> Option<ColliderAabb> {
        *self.bounds.get_or_init(|| {
            context
                .colliders
                .get(self.collider)
                .ok()
                .map(|(position, rotation, collider, _)| collider.aabb(position.0, *rotation))
        })
    }
}

impl FieldReach {
    /// The reach of a field thrown from `at`. Every box a direction queries spans `at` to `at +
    /// axis × L` with `L ≤` [`MAX_CORRIDOR`] (the presence corridor's, or the witness pass's to a
    /// stop on that walk, whose crossings lie short of its length), or is the cube of half-side
    /// [`MAX_CORRIDOR`] about `at` ([`horizon`]); the cube a metre wider holds every one. DERIVED
    /// from IEEE 754 round-to-nearest, at any magnitude of `at`: each face is an f32 `at + a`
    /// rounded and the wide cube's `at ± 51 m` rounded, and rounding is monotone, so a face with
    /// `|a| ≤ 51 m` stays inside. ASSUMED: `|axis_i × L| ≤ 51 m`, which a unit axis meets with a
    /// metre of room (bevy's `Dir3` panics in debug builds when its length is about 1 % off).
    pub(crate) fn of_throw(
        context: &ResolveContext<'_, '_, '_>,
        admit: &dyn Fn(Entity) -> bool,
        at: Vec3,
    ) -> Self {
        let half = Vec3::splat(MAX_CORRIDOR + 1.0);
        Self::within(
            context,
            admit,
            ColliderAabb::from_min_max(at - half, at + half),
        )
    }

    /// The colliders `admit` keeps whose broad-phase bounds meet `region`.
    pub(crate) fn within(
        context: &ResolveContext<'_, '_, '_>,
        admit: &dyn Fn(Entity) -> bool,
        region: ColliderAabb,
    ) -> Self {
        let world = context.world;
        let (mut hard, mut witnesses) = (Vec::new(), Vec::new());
        world
            .spatial
            .aabb_intersections_with_aabb_callback(region, |entity| {
                let Ok(&broad) = world.aabbs.get(entity) else {
                    return true;
                };
                if let Some((node, _)) = world.armour(entity) {
                    if admit(entity) {
                        hard.push(Reached {
                            node,
                            collider: entity,
                            broad,
                            bounds: OnceCell::new(),
                        });
                    }
                } else if world.witness_surfaces.contains(entity)
                    && admit(entity)
                    && let Some((node, _)) =
                        super::hit_ancestor(entity, &world.witnesses, &world.parents)
                {
                    witnesses.push((node, entity, broad));
                }
                true
            });
        hard.sort_unstable_by_key(|reached| (reached.node, reached.collider));
        witnesses.sort_unstable_by_key(|&(node, collider, _)| (node, collider));
        Self {
            hard,
            witnesses,
            union: OnceCell::new(),
        }
    }

    /// The armour colliders whose broad-phase bounds meet `swept`.
    fn hard_meeting(&self, swept: ColliderAabb) -> impl Iterator<Item = &Reached> {
        self.hard
            .iter()
            .filter(move |reached| reached.broad.intersects(&swept))
    }

    /// The witness colliders whose broad-phase bounds meet `swept`, as `(witness, collider)`.
    pub(super) fn witnesses_meeting(&self, swept: ColliderAabb) -> Vec<(Entity, Entity)> {
        self.witnesses
            .iter()
            .filter(|(_, _, broad)| broad.intersects(&swept))
            .map(|&(node, collider, _)| (node, collider))
            .collect()
    }
}

/// One direction of a field, walked ([`field_direction`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct FieldRay {
    /// Where the direction ended: where it entered the hard volume that stopped it, or, entering
    /// none, at the struck tank's horizon ([`horizon`]). Witness-independent.
    pub end: Vec3,
    /// The doses it addressed, in entry order.
    pub doses: Vec<Dose>,
    /// The witness pass's failure along this direction: `end` and the hard doses stand, and no
    /// witness is dosed by it.
    pub witness_error: Option<WalkError>,
}

/// The ordered presence along one direction of a field thrown from the world point `at`: the
/// volumes the throw admits (the struck tank's, its [`FieldReach`]), walked from outside all of
/// their bounds through `at` to the far side of them, with nothing declared. Returns the walk and
/// the progress of `at` along it.
///
/// The same collector and pairing walk as any corridor ([`walk::walk_ray`]), so its layers carry
/// every volume present along the ray in order, nested and overlapping ones included, and what the
/// direction starts inside is the parity of oriented crossings behind `at`, never assumed. It reaches
/// back from `at` the way [`certified`] does ([`Corridor::behind`]): the line is formed at `at`
/// itself. Candidates are the admitted volumes whose bounds, grown by [`PROBE_MARGIN`], the
/// forward ray meets within [`MAX_CORRIDOR`]; the walk starts and ends outside the union of those
/// grown bounds, so no candidate's face lies within the collector's sitting reach of either end.
/// A walk longer than [`MAX_CORRIDOR`] is [`WalkError::UncertifiedStart`].
pub(crate) fn field_presence(
    context: &ResolveContext<'_, '_, '_>,
    field: &FieldReach,
    at: Vec3,
    dir: Dir3,
) -> Result<Option<(f64, walk::RayWalk)>, WalkError> {
    let axis = Vec3::from(dir);
    let margin = Vec3::splat(PROBE_MARGIN);
    // Where the line `at + t·axis` lies inside the box `[min, max]` (relative to `at`): the
    // parameter interval, empty when it misses.
    let meets = |min: Vec3, max: Vec3| {
        let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
        for i in 0..3 {
            if axis[i] == 0.0 {
                if min[i] > 0.0 || max[i] < 0.0 {
                    return None;
                }
            } else {
                let (a, b) = (min[i] / axis[i], max[i] / axis[i]);
                enter = enter.max(a.min(b));
                leave = leave.min(a.max(b));
            }
        }
        (enter <= leave).then_some((enter, leave))
    };
    let mut near = Vec::new();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for reached in field.hard_meeting(collect::swept_aabb(at, axis, MAX_CORRIDOR, 0.0)) {
        let Some(bounds) = reached.bounds(context) else {
            return Err(WalkError::CollectorFailed {
                volume: reached.node,
                reason: "a candidate collider has no resolved pose",
            });
        };
        let (min, max) = (bounds.min - at - margin, bounds.max - at + margin);
        if meets(min, max).is_none_or(|(_, leave)| leave < 0.0) {
            continue;
        }
        lo = lo.min(min);
        hi = hi.max(max);
        near.push((reached.node, reached.collider));
    }
    if near.is_empty() {
        return Ok(None);
    }
    let Some((enter, leave)) = meets(lo, hi) else {
        return Err(WalkError::UncertifiedStart {
            volume: near[0].0,
            reason: "a field direction's line misses the bounds its candidates were admitted by",
        });
    };
    // `at` is inside the union or behind it, so the walk starts behind both.
    let behind = (-enter).max(0.0);
    let length = behind + leave.max(0.0);
    if !length.is_finite() || length > MAX_CORRIDOR {
        return Err(WalkError::UncertifiedStart {
            volume: near[0].0,
            reason: "a field direction's walk through the struck tank's bounds exceeds the \
                     corridor ceiling",
        });
    }
    let mut hits = Vec::new();
    collect::collect(
        &Corridor {
            anchor: at,
            origin: Vec3::ZERO,
            axis,
            length,
            behind,
            seeded: &[],
            handoff: None,
            laws: &context.laws,
        },
        &near,
        context.colliders,
        &mut hits,
    )?;
    let mut entries: Vec<(Entity, walk::VolumeMaterial)> = Vec::new();
    for hit in &hits {
        if !entries.iter().any(|(volume, _)| *volume == hit.volume) {
            let material = context
                .world
                .volumes
                .get(hit.volume)
                .map(|found| found.material)
                .map_err(|_| WalkError::UnknownVolume { volume: hit.volume })?;
            entries.push((hit.volume, material));
        }
    }
    let walked = walk::walk_ray(
        0,
        &RayCorridor {
            anchor: at,
            origin: -axis * behind,
            axis,
            length,
            initial_presence: Vec::new(),
            hits,
        },
        &VolumeTable::new(entries)?,
        &context.laws,
    )?;
    Ok(Some((behind as f64, walked)))
}

/// March one direction of a field thrown from the world point `at` through the volumes its throw
/// admits (the struck tank's, `field`), from its ordered presence ([`field_presence`]): where the
/// hard world stops it ([`first_solid`]), then the witnesses along it
/// ([`super::wound::entries_in`]).
///
/// `strength` (J) is constant along the direction. It stops where it enters the first
/// volume: one it starts inside it enters at `at`, except one of `exited`, the volumes the throw
/// leaves, which it has not entered while it is still leaving them. That volume, when it `is_box`
/// (an HP-bearing one) and is not one of `exited`, takes `strength × weight`, never scaled by its
/// own factor; armour takes nothing; nothing beyond is reached. A volume of `exited` is the body
/// that threw the field, whose share of the energy the line has already dosed: met again further
/// along (the far wall of a hollow body), it stops the direction as armour does and takes nothing.
/// A direction that enters no volume (`exited` ones it is leaving aside) ends at the struck tank's
/// [`horizon`], so its reach does not depend on what witnesses lie along it. A direction of no
/// positive strength doses nothing.
///
/// Every volume above is hard: a witness (ADR-0040) is never a layer of the presence, so it neither
/// stops nor shadows. Each witness the throw admits that the direction's line enters short of where
/// it stopped takes `strength × weight`, once, the arithmetic of a hard box's dose (a witness
/// holding `at` is entered there). `end` and the hard dose are what they would be with no witness
/// present. The doses are in entry order, a hard box before a witness entered at the same point. A
/// witness pass that fails doses no witness and leaves `end` and the hard dose as they are
/// ([`FieldRay::witness_error`]).
///
/// MONOTONICITY. Every dose is `strength × weight` or nothing, and adding a volume can only move
/// the stop to an earlier entry or leave it where it is, so it raises no other volume's dose,
/// provided the walk's start is unchanged: added bounds that reach behind `at` move the start, and
/// every boundary's f64 position with it.
#[expect(
    clippy::too_many_arguments,
    reason = "the field kernel: the world, the struck tank, the throw and the direction's own state"
)]
pub(crate) fn field_direction(
    context: &ResolveContext<'_, '_, '_>,
    field: &FieldReach,
    is_box: &dyn Fn(Entity) -> bool,
    exited: &[Entity],
    at: Vec3,
    dir: Dir3,
    strength: f32,
    weight: f32,
) -> Result<FieldRay, WalkError> {
    let (start, stop) = match field_presence(context, field, at, dir)? {
        None => (0.0, None),
        Some((start, walked)) => (start, first_solid(&walked.layers, start, exited, is_box)),
    };
    let dose = |address: Entity| Dose {
        address,
        amount: strength * weight,
        source: DoseSource::Field,
    };
    // `reach`: the stop's progress along the walk, infinite for a direction that entered nothing.
    let (end, reach, mut doses) = match stop {
        Some((t, entered)) => (
            at + Vec3::from(dir) * (t - start) as f32,
            t,
            entered
                .filter(|_| strength > 0.0)
                .map(|volume| (t, dose(volume)))
                .into_iter()
                .collect::<Vec<(f64, Dose)>>(),
        ),
        None => (horizon(context, field, at, dir), f64::INFINITY, Vec::new()),
    };
    let ahead = if reach.is_finite() {
        (reach - start) as f32
    } else {
        MAX_CORRIDOR
    };
    let witness_error = match super::wound::entries_in(context, field, at, dir, ahead) {
        Ok(entered) => {
            for (witness, t) in entered {
                let t = start + t;
                if t < reach && strength > 0.0 && is_box(witness) {
                    doses.push((t, dose(witness)));
                }
            }
            None
        }
        Err(error) => Some(error),
    };
    doses.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(FieldRay {
        end,
        doses: doses.into_iter().map(|(_, dose)| dose).collect(),
        witness_error,
    })
}

/// The struck tank's horizon along a field direction thrown from `at`: where the line leaves the
/// union of the bounds of every hard collider the throw admits within [`MAX_CORRIDOR`] of `at`
/// (its [`FieldReach`], built for `at`); `at` itself where there is none or the union lies behind
/// it. Witnesses take no part. The union does not depend on the direction, so a throw reads it
/// once.
fn horizon(context: &ResolveContext<'_, '_, '_>, field: &FieldReach, at: Vec3, dir: Dir3) -> Vec3 {
    let (lo, hi) = *field.union.get_or_init(|| {
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        let cube = collect::swept_aabb(at, Vec3::from(dir), 0.0, MAX_CORRIDOR);
        for bounds in field
            .hard_meeting(cube)
            .filter_map(|reached| reached.bounds(context))
        {
            lo = lo.min(bounds.min);
            hi = hi.max(bounds.max);
        }
        (lo, hi)
    });
    let (mut enter, mut leave) = (f32::NEG_INFINITY, f32::INFINITY);
    for i in 0..3 {
        if dir[i] == 0.0 {
            if at[i] < lo[i] || at[i] > hi[i] {
                return at;
            }
        } else {
            let (a, b) = ((lo[i] - at[i]) / dir[i], (hi[i] - at[i]) / dir[i]);
            enter = enter.max(a.min(b));
            leave = leave.min(a.max(b));
        }
    }
    if enter <= leave && leave.is_finite() && leave > 0.0 {
        at + Vec3::from(dir) * leave
    } else {
        at
    }
}

/// Where a direction walked over `layers`, from the progress `start` of its throw point, stops
/// ([`field_direction`]): the progress at which it enters the first volume that is not one of
/// `exited` it is still leaving, and the first of the volumes it enters there that `is_box` and is
/// not one of `exited`, the body that threw the field, which is no box to its own field. `None`
/// when it enters none. Layers that end at or before `start` lie behind the throw point and take
/// no part.
fn first_solid(
    layers: &[walk::Layer],
    start: f64,
    exited: &[Entity],
    is_box: &dyn Fn(Entity) -> bool,
) -> Option<(f64, Option<Entity>)> {
    let mut leaving: Vec<Entity> = exited.to_vec();
    for layer in layers.iter().filter(|layer| layer.end > start) {
        leaving.retain(|volume| layer.present.iter().any(|p| p.volume == *volume));
        let entered: Vec<Entity> = layer
            .present
            .iter()
            .map(|presence| presence.volume)
            .filter(|volume| !leaving.contains(volume))
            .collect();
        if !entered.is_empty() {
            let at = layer.start.max(start);
            let dosed = entered
                .into_iter()
                .find(|volume| is_box(*volume) && !exited.contains(volume));
            return Some((at, dosed));
        }
    }
    None
}

/// The state successive crossings read and advance: where the round is, where it is going, and the
/// sampling frame and per-sample starts it carries out of the last crossing.
///
/// The runtime march and the §13.6 gate drive the SAME state through the same [`decide`] and
/// [`Flight::follow`], so a crossing sequence is one state machine wherever it is run.
///
/// # Where a sweep starts
///
/// A sample may start a sweep only where its membership is KNOWN: at its own start in the
/// continuation ([`Interior`]). A flight without one — just bounced, or a replica just re-seeded —
/// has a fresh disc to lay, square to `direction` at `position`, and [`settle`](Self::settle)
/// establishes its membership before anything sweeps from it. Nothing nudges past a start: a face
/// within a nudge ahead would be stepped over with its entry owned by nobody, and the sample would
/// begin inside the plate and report an exit it never entered.
#[derive(Clone, Debug)]
pub(crate) struct Flight {
    /// WORLD position of the round's axis, for presentation and for a fresh layout. With a
    /// continuation it is that continuation's axis start in world ([`Interior::world_origin`]).
    pub position: Vec3,
    pub direction: Dir3,
    /// m/s.
    pub speed: f32,
    /// The sampling basis for `direction`, transported through every bend so far: the frame a fresh
    /// disc is laid on. Resolution reads THIS, never a basis stored for an earlier heading.
    pub frame: DiscFrame,
    /// Each sample's start and membership; `None` only between a fresh start and its
    /// [`settle`](Self::settle).
    pub continuation: Option<Interior>,
}

impl Flight {
    /// A step's flight from the shell's persisted starts, or — with none — from a fresh disc laid
    /// on `frame` at `position`, square to `swept`, its membership established ([`certified`]).
    ///
    /// The last sweep ran along `swept`; the flight now travels along `direction`. The starts are
    /// kept exactly where the last sweep left them and each sample's ray simply bends: moving a start
    /// to square the disc to the new direction would put it on ground no ray has swept. No bend is
    /// too large for this, a reversal included. `frame` is transported through the bend by minimal
    /// rotation for the next fresh layout.
    ///
    /// The cost is the footprint, and this representation of it is provisional, for the bounded
    /// bends of the rounds fired today. An untouched laid disc stays square to the direction it was
    /// laid on (the muzzle's, a bounce's), so its sampled cross-section projects by `|cos θ|` of
    /// the bend since, in the plane of the bend (DERIVED: 0.12 % narrower for a 2.8° drop, 13.4 %
    /// at 30°, 50 % at 60°, a line at 90°). A perforation's starts are no square disc at all —
    /// staggered along the entrance where each sample handed off, then bent — and that footprint can
    /// be WIDER than the calibre, and stays so into later ticks (DERIVED: full coverage at 60°
    /// incidence into the reference substance bends 12° under the default normalization, a width
    /// factor of `cos 12° + tan 60° · sin 12° ≈ 1.338`; no substance is declared above the
    /// reference).
    /// Slow rounds and steep lobs need the starts transported along per-sample paths instead.
    #[expect(
        clippy::too_many_arguments,
        reason = "the flight's whole state plus the world its fresh starts are certified against"
    )]
    pub(crate) fn launched(
        context: &ResolveContext<'_, '_, '_>,
        not_own: &dyn Fn(Entity) -> bool,
        position: Vec3,
        swept: Dir3,
        direction: Dir3,
        speed: f32,
        frame: DiscFrame,
        radius: f32,
        starts: Option<Interior>,
    ) -> Result<Self, WalkError> {
        let starts = match starts {
            Some(starts) => starts.rebased(),
            None => certified(context, not_own, position, swept, frame, radius)?,
        };
        Ok(Self {
            position: starts.world_origin(),
            direction,
            speed,
            frame: frame.transport(Vec3::from(swept), Vec3::from(direction)),
            continuation: Some(starts),
        })
    }

    /// Lay the fresh disc a bounce or a re-seed left — on `frame` at `position`, square to
    /// `direction` — with its membership established ([`certified`]). A flight that already has its
    /// starts is left alone.
    pub(crate) fn settle(
        &mut self,
        context: &ResolveContext<'_, '_, '_>,
        not_own: &dyn Fn(Entity) -> bool,
        radius: f32,
    ) -> Result<(), WalkError> {
        if self.continuation.is_none() {
            self.continuation = Some(certified(
                context,
                not_own,
                self.position,
                self.direction,
                self.frame,
                radius,
            )?);
        }
        Ok(())
    }

    /// WORLD start of the axis sample: the cast's origin and, for a fresh layout, the disc's centre.
    pub(crate) fn sweep_origin(&self) -> Vec3 {
        self.continuation
            .as_ref()
            .map_or(self.position, Interior::world_origin)
    }

    /// Each sample's offset from [`sweep_origin`](Self::sweep_origin), in canonical sample order.
    pub(crate) fn sample_offsets(&self, radius: f32) -> Vec<Vec3> {
        match &self.continuation {
            Some(interior) => interior.seeds.iter().map(|seed| seed.offset).collect(),
            None => layout(&self.frame, radius),
        }
    }

    /// A continuation holds a sample inside material ([`Interior::holds_material`]).
    pub(crate) fn holds_material(&self) -> bool {
        self.continuation
            .as_ref()
            .is_some_and(Interior::holds_material)
    }

    /// Fly `displacement` of open air: every sample's ray was swept that far clear, so every start
    /// advances by it.
    ///
    /// The continuation's axis start is the one position: [`position`](Self::position) is derived
    /// from it, never advanced alongside it. A world position advanced on its own re-rounds at world
    /// spacing every step (DERIVED: 2.44e-4 m past 2048 m) and walks away from the starts the casts
    /// use; the anchor-relative origin keeps what that rounding loses.
    pub(crate) fn fly(&mut self, displacement: Vec3) {
        match &mut self.continuation {
            Some(interior) => {
                interior.origin += displacement;
                self.position = interior.world_origin();
            }
            None => self.position += displacement,
        }
    }

    /// Advance the flight along a decision: the state the next crossing starts from. The sampling
    /// frame is transported through every direction change, including the null one. Returns whether
    /// the round came to rest; the travel it charges is [`Decision::charged`].
    pub(crate) fn follow(&mut self, decision: Decision) -> bool {
        match decision {
            Decision::Clear {
                resume,
                continuation,
                ..
            } => {
                self.frame = self
                    .frame
                    .transport(Vec3::from(self.direction), Vec3::from(self.direction));
                self.position = resume;
                self.continuation = Some(continuation);
                false
            }
            Decision::Bounce(bounce) => {
                self.frame = self
                    .frame
                    .transport(Vec3::from(self.direction), Vec3::from(bounce.direction));
                self.direction = bounce.direction;
                self.speed = bounce.speed;
                self.position = bounce.resume;
                self.continuation = None;
                false
            }
            Decision::Bite(bite) => match bite.end {
                BiteEnd::Embedded { at, direction, .. } => {
                    self.frame = self
                        .frame
                        .transport(Vec3::from(self.direction), Vec3::from(direction));
                    self.direction = direction;
                    self.position = at;
                    self.continuation = None;
                    true
                }
                BiteEnd::Perforated {
                    resume,
                    direction,
                    speed,
                    continuation,
                    ..
                } => {
                    self.frame = self
                        .frame
                        .transport(Vec3::from(self.direction), Vec3::from(direction));
                    self.direction = direction;
                    self.speed = speed;
                    self.position = resume;
                    self.continuation = Some(continuation);
                    false
                }
            },
        }
    }
}

/// What one crossing decided, before any consequence is applied: the runtime applies it
/// ([`apply`]); the §13.6 gate only observes it.
///
/// Positions are WORLD; the continuation alone stays anchor-relative ([`Interior`]).
#[derive(Clone, Debug)]
pub(crate) enum Decision {
    /// First contact said there was armour; the disc, which is the resolution, met none. No
    /// fabricated event (§13.6) — and no re-probing: the round flies past the WHOLE corridor just
    /// examined, because that corridor is exactly the span now known to be empty. Advancing only to
    /// the contact would re-detect it and creep forward one boundary nudge at a time.
    Clear {
        /// The far end of the examined corridor along the axis, where the flight resumes.
        resume: Vec3,
        /// Metres from the axis start to `resume`: the examined corridor.
        travel: f32,
        /// Each sample at the far end of its own examined ray, with what its walk says it is inside
        /// there.
        continuation: Interior,
        /// The examined corridor as the round flew it.
        leg: Leg,
    },
    /// Too oblique and not overmatched: deflected off the face — no entry, no spall.
    Bounce(Bounce),
    /// It bit in: stopped inside the material or came out the far side.
    Bite(Bite),
}

impl Decision {
    /// Line-of-sight metres following this decision charges against the step's budget, measured
    /// from the flight's position — known before anything is applied, so [`Budget::charge`] can
    /// refuse a crossing before its consequences land.
    pub(crate) fn charged(&self) -> f32 {
        match self {
            Decision::Clear { travel, .. } => *travel,
            Decision::Bounce(bounce) => bounce.travel,
            Decision::Bite(bite) => bite.travel,
        }
    }

    /// The straight stretches the round's axis flies following this decision, in order, ending
    /// where the next one starts or where it came to rest — what the witness pass intersects
    /// ([`super::wound`]). A bounce's lift to its resume point repositions the body and is no leg.
    pub(crate) fn legs(&self) -> &[Leg] {
        match self {
            Decision::Clear { leg, .. } => std::slice::from_ref(leg),
            Decision::Bounce(bounce) => std::slice::from_ref(&bounce.approach),
            Decision::Bite(bite) => &bite.legs,
        }
    }
}

/// One straight stretch of the round's flight (WORLD): the axis start, the direction, and each disc
/// sample's own stretch along it in canonical sample order — `(offset, length)`, the sample flying
/// `length` m from `from + offset`. An offset keeps its along-axis part: at an oblique handoff each
/// sample starts and stops where the walk's own ray for it does, not on a plane square to the axis.
#[derive(Clone, Debug)]
pub(crate) struct Leg {
    pub from: Vec3,
    pub axis: Dir3,
    pub samples: Vec<(Vec3, f32)>,
}

impl Leg {
    /// Every sample flying `length` m from its own start, `from + offsets[i]`.
    pub(crate) fn new(from: Vec3, axis: Dir3, length: f32, offsets: &[Vec3]) -> Self {
        Leg {
            from,
            axis,
            samples: offsets
                .iter()
                .map(|offset| (*offset, length.max(0.0)))
                .collect(),
        }
    }

    /// Each sample flying from its own start, `from + offsets[i]`, to `ends[i]` (index-aligned): the
    /// metres along `axis` between them, none for an end behind its start.
    pub(crate) fn ending_at(from: Vec3, axis: Dir3, offsets: &[Vec3], ends: &[Vec3]) -> Self {
        Leg {
            from,
            axis,
            samples: offsets
                .iter()
                .zip(ends)
                .map(|(offset, end)| (*offset, (*end - from - *offset).dot(*axis).max(0.0)))
                .collect(),
        }
    }
}

/// A deflection off the entrance face (§4). The deflection ANGLE and the speed lost both already
/// carry η from the core (§13.5) — a graze is a partial ricochet in direction as well as in speed.
#[derive(Clone, Debug)]
pub(crate) struct Bounce {
    pub position: Vec3,
    pub normal: Vec3,
    /// Entrance coverage η.
    pub coverage: f32,
    /// The volume owning the struck face.
    pub struck: Entity,
    /// The rigid body owning `struck`, which takes the momentum exchange.
    pub owner: Option<Entity>,
    /// The dose the bounce jars into the struck component, joules: the kinetic energy the round
    /// lost at the bounce, `½ m (v_in² − v_out²)`, `m` the mass that meets the plate, as a line
    /// dose is the energy lost crossing a box. At full coverage the round keeps its speed along the
    /// face ([`Self::speed`]), so this is the energy of the motion pointed into the face,
    /// `½ m (v_in cos θ)²`, `θ` the incidence from `normal`. It converts at the energy rate. Armour
    /// has no HP and shrugs it off.
    pub shock: f32,
    pub direction: Dir3,
    /// Speed retained (m/s): at full coverage the striking speed's part along the face,
    /// `v_in sin θ`, restitution 0 ([`walk::Begin::Ricochet`]).
    pub speed: f32,
    /// Direction of travel and speed (m/s) at contact.
    pub incoming: Dir3,
    pub speed_in: f32,
    /// Momentum handed to `owner` at `position`.
    pub impulse: Vec3,
    /// Line-of-sight metres flown from the flight's position to the face, along the way in, and the
    /// nudge off it. The lift to `resume` is not travel: it repositions the body clear of the face
    /// it bounced off.
    pub travel: f32,
    /// Where the flight resumes: lifted off the face along `n̄` by the disc's own radius, then one
    /// [`MARCH_EPS`] along the outgoing direction. At oblique incidence a disc's lateral offsets
    /// lie mostly ALONG the struck surface's normal, so resuming at the contact point leaves half
    /// the ring behind the face and first contact fires again on the very surface just left. A
    /// point needs only the nudge, which steps off the face this bounce resolved, never past one
    /// nobody owns; a body needs its radius too. The lift clears the struck face, not the world:
    /// the fresh disc laid here can start inside neighbouring armour, and its membership is
    /// established before it sweeps ([`Flight::settle`]).
    pub resume: Vec3,
    /// The way in: the flight's position to the face, along `incoming`.
    pub approach: Leg,
}

/// An entry into the material (§§3–6).
#[derive(Clone, Debug)]
pub(crate) struct Bite {
    /// Where the round punched in: the struck face.
    pub entrance: Vec3,
    pub normal: Vec3,
    /// The volume owning the entrance face.
    pub struck: Entity,
    /// Entrance coverage η.
    pub coverage: f32,
    pub overmatched: bool,
    /// The rigid body owning the entrance face, which takes the momentum exchange (§13.5).
    pub owner: Option<Entity>,
    /// Per-entity transit charges, already clipped at the embed progress: a round that stops in the
    /// first plate deposits nothing behind it.
    pub deposits: Vec<Dose>,
    /// One spall source per body exit the round reached, embedded or through, positions in WORLD
    /// ([`walk::SpallSource`]).
    pub spall: Vec<SpallSource>,
    /// Direction of travel and speed (m/s) at the entrance.
    pub incoming: Dir3,
    pub speed_in: f32,
    /// Every volume the line put a chord into, with how far it got through it, in volume order.
    pub volumes: Vec<VolumeCrossing>,
    /// Line-of-sight metres flown from the flight's position, summed over the segments actually
    /// flown: the approach to the AXIS hand-off along the way in, then the transit along the bend,
    /// and for a perforation the nudge on to the resume point.
    ///
    /// Not reconstructible from the cast. The cast knows only where its nearest sample ray first
    /// touched armour, and at oblique incidence that ray leads the axis by `r·tan(incidence)` —
    /// DERIVED: 135 mm for an 88 at 72° (`0.044 m × tan 72°`). Charging to it skips the approach and
    /// hands the round that much free travel inside the same tick, which moves subsequent impacts
    /// across tick accounting boundaries.
    pub travel: f32,
    /// Line-of-sight metres of `travel` flown from the flight's position to the axis hand-off,
    /// along the way in; the rest is the transit along the bend.
    pub approach: f32,
    pub end: BiteEnd,
    /// The approach to the axis hand-off along `incoming`, then the transit along the bend to the
    /// rest point or the resume point.
    pub legs: [Leg; 2],
}

impl Bite {
    /// Metres of path from the flight's position to `point` (WORLD) on the crossing: the approach
    /// to the axis hand-off, then the transit along the bend to `point`'s place on its axis. At the
    /// rest or resume point it is [`Self::travel`], to rounding.
    pub(crate) fn path_to(&self, point: Vec3) -> f32 {
        let transit = &self.legs[1];
        self.approach + (point - transit.from).dot(*transit.axis)
    }
}

/// How a bite ended.
#[derive(Clone, Debug)]
pub(crate) enum BiteEnd {
    /// Defeated: buried at `at`, travelling along `direction` through the body when it stopped
    /// (the bend at the entrance, not the way in), the whole remaining momentum to the entrance
    /// body.
    Embedded {
        at: Vec3,
        direction: Dir3,
        impulse: Vec3,
    },
    /// Punched through.
    Perforated {
        /// The aggregate exit face — what the crossing's marks record.
        exit: Vec3,
        /// Where the flight resumes: the axis one [`MARCH_EPS`] past the exit, the point the
        /// continuation's seeds were read at — WORLD, for the cast; the corridor resumes from the
        /// continuation's own anchor-relative origin.
        resume: Vec3,
        direction: Dir3,
        /// Residual speed (m/s).
        speed: f32,
        /// The momentum the shell lost crossing, to the entrance body at the entrance.
        impulse: Vec3,
        /// Which primitives each sample is still inside where the flight resumes. A shell that
        /// punches an outer plate while one ring sample is already inside the crewman behind it
        /// must resume knowing that, or the next corridor reports an exit it never entered.
        continuation: Interior,
    },
}

/// Everything the driver borrows from the world. Grouped so the phase takes one parameter rather
/// than nine, exactly as [`ProjectileMarchWorld`] groups the queries.
pub(crate) struct ResolveContext<'a, 'w, 's> {
    pub world: &'a ProjectileMarchWorld<'w, 's>,
    pub colliders: &'a Query<
        'w,
        's,
        (
            &'static Position,
            &'static Rotation,
            &'static Collider,
            Option<&'static BallisticSurfaces>,
        ),
    >,
    /// Authority = not a replica: only then does a crossing mutate health.
    pub deposit: bool,
    pub laws: WalkLaws,
}

/// The game's damage model: the resisting force (N, the force law's dE/dx in J/m) the struck body
/// puts on `round` at first contact, and the tank that owns it; what an impact fuze reads
/// ([`super::burst::Fuze::entered`]). The force is the disc mean of the entrance walk's first
/// occupied encounter, each sample's factor read by the union law the advance reads
/// ([`super::mechanism::overlap_factor`]), so a rim sample off the body pulls the mean down. `None`
/// when the flight has no established disc or the walk meets nothing.
pub(crate) fn entry_force(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    round: &SolidShot,
    flight: &Flight,
    contact: f32,
) -> Result<Option<(f64, Option<Entity>)>, WalkError> {
    let Some(interior) = &flight.continuation else {
        return Ok(None);
    };
    let entrance = closing_walk(
        context,
        not_own,
        interior.anchor,
        interior.origin,
        Vec3::from(flight.direction),
        flight.frame,
        disc_radius(round.body().caliber),
        &interior.seeds,
        contact + ENTRANCE_SPAN,
    )?;
    let Some(event) = entrance.events.first() else {
        return Ok(None);
    };
    let Some(encounter) = event.encounters.first() else {
        return Ok(None);
    };
    let factor = encounter
        .samples
        .iter()
        .map(|sample| super::mechanism::overlap_factor(&sample.present) as f64)
        .sum::<f64>()
        / encounter.disc as f64;
    let owner = context
        .world
        .owners
        .get(event.entrance_volume)
        .ok()
        .map(|owner| owner.tank());
    Ok(Some((round.resisting_force(flight.speed, factor), owner)))
}

/// Decide one crossing of the union field from first contact, with no consequence applied:
/// corridor closure, the entrance read ([`walk::begin`]), the transit collection along the bend,
/// [`walk::finish`] advancing the round's bound mechanism, the residual speed, the travel and the
/// continuation.
///
/// `round` is what meets the plate, the penetrator's mechanism; `mass` the whole round's (kg),
/// since the momentum a bounce or a bite delivers is the whole round's, its body stopping at the
/// plate with its penetrator. `contact` is the distance from [`Flight::sweep_origin`] to the
/// nearest sample's first armour contact; it sizes the first corridor and nothing else.
pub(crate) fn decide(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    round: &SolidShot,
    mass: f32,
    flight: &Flight,
    contact: f32,
) -> Result<Decision, WalkError> {
    let body = round.body();
    let radius = disc_radius(body.caliber);
    let dir = flight.direction;
    let speed = flight.speed;

    // --- ENTRANCE: the disc as it meets the surface, along the incoming axis.
    //
    // The flight's position becomes the corridor ANCHOR, and every position from here to the end of
    // the crossing is measured from it. The transit corridor inherits the same anchor, which is what
    // makes its handoff a small offset from the face positions the entrance measured rather than an
    // independently rounded world position (see `walk::RayCorridor::anchor`). Resuming from a
    // perforation keeps that perforation's anchor, for the same reason (see [`Interior`]).
    //
    // `start` is the axis sample's start, anchor-relative, and travel is charged from it.
    let Some(interior) = &flight.continuation else {
        return Err(WalkError::BadCorridor {
            sample: 0,
            reason: "a fresh disc reached a crossing before its membership was established",
        });
    };
    let (anchor, start, seeds) = (interior.anchor, interior.origin, interior.seeds.as_slice());
    let from = start;
    let entrance = closing_walk(
        context,
        not_own,
        anchor,
        start,
        Vec3::from(dir),
        flight.frame,
        radius,
        seeds,
        contact + ENTRANCE_SPAN,
    )?;
    let state_in = round.at(speed);
    let shot = Shot {
        caliber: body.caliber,
    };
    let owner = |struck: Entity| {
        context
            .world
            .owners
            .get(struck)
            .ok()
            .map(|owner| owner.tank())
    };
    let v_in = Vec3::from(dir) * speed;

    match walk::begin(&entrance, &shot, &context.laws)? {
        Begin::Miss => {
            let examined = contact + ENTRANCE_SPAN;
            Ok(Decision::Clear {
                resume: anchor + (start + dir * examined),
                travel: examined,
                continuation: Interior {
                    anchor,
                    origin: start + dir * examined,
                    seeds: entrance.resume_at(examined as f64, &context.laws),
                },
                leg: Leg::new(anchor + start, dir, examined, &entrance.offsets),
            })
        }
        Begin::Ricochet {
            entrance: read,
            direction,
            speed_scale,
            hand_offs,
        } => {
            let struck = read.volume;
            let position = anchor + read.position;
            let out = Dir3::new(direction).unwrap_or(dir);
            let kept = speed * speed_scale;
            let approach = (read.position - from).dot(Vec3::from(dir)).max(0.0);
            let way_in = Leg {
                from: anchor + from,
                axis: dir,
                samples: entrance
                    .offsets
                    .iter()
                    .zip(hand_offs)
                    .map(|(offset, t)| (*offset, t as f32))
                    .collect(),
            };
            Ok(Decision::Bounce(Bounce {
                position,
                normal: read.normal,
                coverage: read.coverage,
                struck,
                owner: owner(struck),
                shock: (round.energy(state_in) - round.energy(round.at(kept))).max(0.0) as f32,
                direction: out,
                speed: kept,
                incoming: dir,
                speed_in: speed,
                impulse: mass * (v_in - Vec3::from(out) * kept),
                travel: approach + MARCH_EPS,
                resume: position + read.normal * radius + out * MARCH_EPS,
                approach: way_in,
            }))
        }
        Begin::Transit(request) => {
            // --- TRANSIT: re-collected along the BENT axis, seeded from the handoff. The entrance
            // hits describe the incoming rays and cannot describe this.
            let bent = Dir3::new(request.axis).unwrap_or(dir);
            let transit = closing_walk(
                context,
                not_own,
                request.anchor,
                request.origin,
                request.axis,
                request.frame,
                request.radius,
                &request.seeds,
                ENTRANCE_SPAN,
            )?;
            let plan = walk::finish(&transit, &request, round, state_in, &LIMITS, &context.laws)?;
            Ok(Decision::Bite(bite(
                &transit,
                plan,
                mass,
                from,
                dir,
                &entrance.offsets,
                bent,
                speed,
                owner,
                &context.laws,
            )))
        }
    }
}

/// The bite's plan in world terms, with its residual speed, travel and continuation.
#[expect(
    clippy::too_many_arguments,
    reason = "private kernel of `decide`; every argument is crossing state already in hand"
)]
fn bite(
    transit: &DiscWalk,
    plan: walk::ResolutionPlan<super::mechanism::SolidShotState>,
    // kg.
    mass: f32,
    // Where the flight's position is, RELATIVE to the transit's anchor.
    from: Vec3,
    incoming: Dir3,
    // The entrance disc's sample offsets, for the approach leg.
    entrance_offsets: &[Vec3],
    bent: Dir3,
    // m/s at the entrance.
    speed_in: f32,
    owner: impl Fn(Entity) -> Option<Entity>,
    laws: &WalkLaws,
) -> Bite {
    let v_in = Vec3::from(incoming) * speed_in;
    // THE BOUNDARY. Everything the walk reports is anchor-relative. What leaves for presentation and
    // physics — the entrance, the exit, the embed point, spall sources, the resume point — is
    // converted to WORLD here, in one place. The continuation ([`Interior`]) deliberately stays
    // anchor-relative: it is the next corridor's input, and rebuilding it in world would round it at
    // the world's spacing (see there).
    let world = |local: Vec3| transit.anchor + local;
    let struck = plan.entrance.volume;
    // The APPROACH: the flight's position to the axis handoff, along the way in. The transit's own
    // `t` then measures the rest, along the bend.
    let approach = (transit.origin - from).dot(Vec3::from(incoming)).max(0.0);
    // Each sample's way in ends at its own handoff, where its transit ray starts.
    let handoffs: Vec<Vec3> = transit
        .offsets
        .iter()
        .map(|offset| transit.origin + *offset)
        .collect();
    let mut way_in = Leg::ending_at(from, incoming, entrance_offsets, &handoffs);
    way_in.from = world(from);
    let entrance = world(plan.entrance.position);
    let rest = match plan.outcome {
        Outcome::Embedded { t, .. } => Some(t as f64),
        Outcome::Perforated { .. } => None,
    };
    let volumes = volume_crossings(transit, &plan.crossing, &plan.deposits, rest, &world);
    let (end, flown) = match plan.outcome {
        Outcome::Embedded { at, t } => (
            BiteEnd::Embedded {
                at: world(at),
                direction: bent,
                impulse: mass * v_in,
            },
            t,
        ),
        Outcome::Perforated { exit, t, .. } => {
            let residual = plan.state.speed as f32;
            // Read at the point the NEXT corridor will actually start from — the flight resumes one
            // nudge past the exit it is leaving, and a seed taken one nudge earlier describes a plate
            // the round has already cleared.
            let resume = t + MARCH_EPS;
            (
                BiteEnd::Perforated {
                    exit: world(exit),
                    resume: world(transit.origin + transit.axis * resume),
                    direction: bent,
                    speed: residual,
                    impulse: mass * (v_in - Vec3::from(bent) * residual),
                    continuation: Interior {
                        anchor: transit.anchor,
                        origin: transit.origin + transit.axis * resume,
                        seeds: transit.resume_at(resume as f64, laws),
                    },
                },
                resume,
            )
        }
    };
    Bite {
        entrance,
        incoming,
        speed_in,
        volumes,
        normal: plan.entrance.normal,
        struck,
        coverage: plan.entrance.coverage,
        overmatched: plan.entrance.overmatched,
        owner: owner(struck),
        deposits: plan.deposits,
        spall: plan
            .spall
            .into_iter()
            .map(|source| SpallSource {
                position: world(source.position),
                ..source
            })
            .collect(),
        travel: approach + flown,
        approach,
        end,
        legs: [
            way_in,
            Leg::new(world(transit.origin), bent, flown, &transit.offsets),
        ],
    }
}

/// How far the line got through each volume it put a chord into: the axis points where the disc's
/// first sample met the volume and where its last left it (or the rest point, inside), from the
/// crossing's encounter contacts. A volume is perforated when every sample's contact with it ends
/// at or before `rest` — the whole disc came out — and always on a perforation (`rest` `None`).
fn volume_crossings(
    transit: &DiscWalk,
    event: &walk::DiscEvent,
    deposits: &[Dose],
    rest: Option<f64>,
    world: &impl Fn(Vec3) -> Vec3,
) -> Vec<VolumeCrossing> {
    let mut spans: std::collections::BTreeMap<Entity, (f64, f64)> = Default::default();
    for contact in event
        .encounters
        .iter()
        .flat_map(|encounter| &encounter.samples)
        .flat_map(|sample| &sample.present)
    {
        let span = spans
            .entry(contact.volume)
            .or_insert((contact.start, contact.end));
        span.0 = span.0.min(contact.start);
        span.1 = span.1.max(contact.end);
    }
    let at = |t: f64| world(transit.origin + transit.axis * t as f32);
    deposits
        .iter()
        .filter_map(|dose| {
            let DoseSource::Line { chord, cost } = dose.source else {
                return None;
            };
            let &(start, end) = spans.get(&dose.address)?;
            let perforated = rest.is_none_or(|rest| end <= rest);
            Some(VolumeCrossing {
                volume: dose.address,
                entry: at(start),
                end: at(if perforated { end } else { rest.unwrap_or(end) }),
                chord,
                cost,
                energy: dose.amount,
                perforated,
            })
        })
        .collect()
}

/// Collect and walk a corridor, extending it until the first crossing CLOSES.
///
/// "Closes" is two conditions, not one: the walk must pair (no primitive left open at the end), and
/// the first crossing ([`DiscWalk::first_crossing`]) must end far enough inside the corridor that no
/// ε-weld could reach past it. A corridor that stopped one millimetre after an
/// exit could not tell a finished run from the first half of a welded sandwich.
#[expect(
    clippy::too_many_arguments,
    reason = "private kernel of `decide`; every argument is corridor geometry"
)]
fn closing_walk(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    anchor: Vec3,
    origin: Vec3,
    axis: Vec3,
    frame: DiscFrame,
    radius: f32,
    seeds: &[SampleSeed],
    initial: f32,
) -> Result<DiscWalk, WalkError> {
    let mut length = initial.clamp(ENTRANCE_SPAN, MAX_CORRIDOR);
    loop {
        let corridor = build_corridor(
            context, not_own, anchor, origin, axis, length, frame, radius, seeds,
        )?;
        let volumes = volume_table(context, &corridor)?;
        match walk::walk_disc(&corridor, &volumes, &context.laws) {
            Ok(walked) => {
                let end = walked
                    .first_crossing()
                    .iter()
                    .map(|&piece| walked.events[piece].end)
                    .reduce(f64::max);
                let settled = end.is_none_or(|end| {
                    end + context.laws.weld_max_lookahead as f64 <= length as f64
                });
                if settled || length >= MAX_CORRIDOR {
                    return Ok(walked);
                }
            }
            // The corridor ran out with material still open. Extend it — the one thing never done
            // is to invent the exit.
            Err(WalkError::IncompleteCorridor { .. }) if length < MAX_CORRIDOR => {}
            Err(error) => return Err(error),
        }
        length = (length * 2.0).min(MAX_CORRIDOR);
    }
}

/// The candidate colliders whose AABB the corridor's swept box touches, as `(volume node, collider)`.
///
/// Broad phase by AABB rather than by ray so a volume that CONTAINS the whole corridor is still a
/// candidate — a ray query would report nothing for it, and "nothing" is the answer that must never
/// come back from armour the shell is inside.
pub(super) fn candidates(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    origin: Vec3,
    axis: Vec3,
    length: f32,
    radius: f32,
) -> Vec<(Entity, Entity)> {
    let mut out = Vec::new();
    context.world.spatial.aabb_intersections_with_aabb_callback(
        collect::swept_aabb(origin, axis, length, radius),
        |entity| {
            // Ancestry IS the armour test: only a collider under a `BallisticVolume` node resolves,
            // which is the same rule the march has always used to tell armour from terrain; a
            // witness's collider never does (ADR-0040).
            if let Some((node, _)) = context.world.armour(entity)
                && not_own(entity)
            {
                out.push((node, entity));
            }
            true
        },
    );
    out.sort();
    out
}

/// Build the k sample corridors and collect every crossing along each.
///
/// Shared with the §13.6 fuzzer ([`super::fuzz`]) rather than re-derived there: a gate that builds
/// its own corridors proves things about a corridor the march never casts.
#[expect(
    clippy::too_many_arguments,
    reason = "the corridor kernel of `closing_walk` and the fuzzer; every argument is geometry"
)]
pub(super) fn build_corridor(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    anchor: Vec3,
    origin: Vec3,
    axis: Vec3,
    length: f32,
    frame: DiscFrame,
    radius: f32,
    seeds: &[SampleSeed],
) -> Result<DiscCorridor, WalkError> {
    // Sample 0 is the axis, by the core's contract. With seeds the offsets are the seeds', which
    // carry each sample's own start; otherwise the disc is freshly laid out on the transported frame.
    let offsets: Vec<Vec3> = if seeds.is_empty() {
        layout(&frame, radius)
    } else {
        seeds.iter().map(|seed| seed.offset).collect()
    };
    // The broad phase pads by the farthest sample, not the radius: seeded starts are staggered along
    // the axis, up to `r·tan(incidence)` off the disc after an oblique perforation.
    let reach = offsets
        .iter()
        .map(|offset| offset.length())
        .fold(radius, f32::max);
    let candidates = candidates(context, not_own, anchor + origin, axis, length, reach);

    let mut samples = Vec::with_capacity(offsets.len());
    for (index, offset) in offsets.into_iter().enumerate() {
        let seed = seeds.get(index);
        let seeded: &[ShellKey] = seed.map_or(&[], |seed| &seed.inside);
        let mut hits = Vec::new();
        collect::collect(
            &Corridor {
                anchor,
                origin: origin + offset,
                axis,
                length,
                behind: 0.0,
                seeded,
                handoff: seed.map(|seed| seed.t),
                laws: &context.laws,
            },
            &candidates,
            context.colliders,
            &mut hits,
        )?;
        samples.push(SampleCorridor {
            offset,
            initial_presence: seeded.to_vec(),
            hits,
        });
    }
    Ok(DiscCorridor {
        anchor,
        origin,
        axis,
        length,
        radius,
        frame,
        samples,
    })
}

/// The factor of every volume the corridor met. Fail-loud by construction: a volume with no
/// `BallisticVolume` never becomes a hit in the first place, and one with an unusable factor is
/// rejected here rather than integrated.
pub(super) fn volume_table(
    context: &ResolveContext<'_, '_, '_>,
    corridor: &DiscCorridor,
) -> Result<VolumeTable, WalkError> {
    let mut entries: Vec<(Entity, walk::VolumeMaterial)> = Vec::new();
    for sample in &corridor.samples {
        let volumes = sample
            .hits
            .iter()
            .map(|hit| hit.volume)
            .chain(sample.initial_presence.iter().map(|key| key.volume));
        for volume in volumes {
            if !entries.iter().any(|(entity, _)| *entity == volume) {
                let material = context
                    .world
                    .volumes
                    .get(volume)
                    .map(|found| found.material)
                    .map_err(|_| WalkError::UnknownVolume { volume })?;
                entries.push((volume, material));
            }
        }
    }
    VolumeTable::new(entries)
}

/// Apply a decision's consequences on the runtime: HP, impulses, the shell's marks and path, spall,
/// and the impact and authority events. Returns the pool units actually removed.
///
/// Transit doses, a bounce's shock, the impulses and the events all come out of the
/// [`Decision`]. Spall does NOT: the decision carries each spall source, but the cone's directions
/// and what each one enters are resolved here, against the live world ([`throw_spall_cone`]), into
/// addressed doses added to the same [`super::ShotDose`] as the line's. So the §13.6 gate, which
/// observes the decisions and applies none of this, reads every transit dose and every bounce the
/// runtime acts on, and no field dose. HP falls once the decision's doses are all added
/// ([`super::ShotDose::settle`]).
pub(crate) fn apply(
    decision: &Decision,
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) -> u64 {
    match decision {
        Decision::Clear { .. } => return 0,
        Decision::Bounce(bounce) => {
            apply_bounce(bounce, shell, terminal_emitted, bodies, commands);
        }
        Decision::Bite(bite) => apply_bite(
            bite,
            shell,
            context,
            terminal_emitted,
            &|volume| health.contains(volume),
            bodies,
            commands,
        ),
    }
    // Both arms log exactly one mark, so the settle closes the last.
    let mark = shell.marks.log.len().saturating_sub(1);
    shell.dose.settle(health, context.deposit, mark)
}

fn apply_bounce(
    bounce: &Bounce,
    shell: &mut MarchingShell,
    terminal_emitted: &mut bool,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) {
    if bounce.shock > 0.0 {
        let context = super::DoseContext::Line {
            entry: bounce.position,
            along: Vec3::from(bounce.incoming),
        };
        shell.dose.add_energy(bounce.struck, bounce.shock, context);
    }
    if let Some(owner) = bounce.owner {
        apply_hit_impulse(bodies, owner, bounce.impulse, bounce.position);
    }
    commands.trigger(Impact {
        position: bounce.position,
        normal: bounce.normal,
        caliber: shell.projectile.caliber(),
        surface: ImpactSurface::Armor,
        outcome: ImpactOutcome::Ricochet {
            direction: Vec3::from(bounce.direction),
        },
    });
    shell.marks.ricochets.push(bounce.position);
    shell.marks.log.push(ArmourMark::Bounce {
        position: bounce.position,
        normal: bounce.normal,
        incoming: Vec3::from(bounce.incoming),
        outgoing: Vec3::from(bounce.direction),
        speed_in: bounce.speed_in,
        speed_out: bounce.speed,
        struck: bounce.struck,
        shock: bounce.shock,
    });
    shell.path.points.push(bounce.position);
    if let Some(shot) = shell.shot {
        if !*terminal_emitted {
            commands.trigger(ShellRicochet {
                shot: shot.0,
                origin: bounce.position,
                direction: Vec3::from(bounce.direction),
                speed: bounce.speed,
                sequence: (shell.marks.ricochets.len() - 1) as u32,
            });
        }
        commands.trigger(ShellStrike {
            shot: shot.0,
            sequence: (shell.marks.log.len() - 1) as u32,
            struck: bounce.struck,
            at: bounce.position,
            normal: bounce.normal,
            incoming: Vec3::from(bounce.incoming),
            caliber: shell.projectile.caliber(),
            verdict: Verdict::Ricochet,
        });
    }
}

fn apply_bite(
    bite: &Bite,
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    terminal_emitted: &mut bool,
    is_box: &dyn Fn(Entity) -> bool,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) {
    // Transit damage: every HP-bearing volume is charged for the material OF ITS OWN the round chewed
    // (§13.2's damage law — no ownership, no priority, no argmax), each deposit from the line's
    // entry into its own volume. Every deposit has that entry: `walk::finish` builds the bite's
    // deposits as Line doses only, each addressed to a volume present in the first event's
    // encounters, and `volume_crossings` builds one crossing per Line deposit from the same
    // encounters.
    let incoming = Vec3::from(bite.incoming);
    shell.dose.add_all(&bite.deposits, |dose| {
        let entry = bite
            .volumes
            .iter()
            .find(|crossed| crossed.volume == dose.address)
            .expect("a crossing per Line deposit, both from the first event's encounters")
            .entry;
        super::DoseContext::Line {
            entry,
            along: incoming,
        }
    });

    let (verdict, position, terminal_at) = match &bite.end {
        BiteEnd::Embedded { at, impulse, .. } => {
            shell.marks.events.push(PenetrationEvent {
                entry: bite.entrance,
                exit: *at,
                overmatched: bite.overmatched,
            });
            shell.path.points.push(*at);
            if let Some(owner) = bite.owner {
                apply_hit_impulse(bodies, owner, *impulse, *at);
            }
            // The struck FACE reads at the visible surface for an embed, the embed point being
            // inside the steel.
            (TerminalVerdict::Stopped, *at, *at)
        }
        BiteEnd::Perforated { exit, impulse, .. } => {
            // The body keeps the momentum the shell lost crossing it; the shell carries the rest on.
            if let Some(owner) = bite.owner {
                apply_hit_impulse(bodies, owner, *impulse, bite.entrance);
            }
            shell.marks.events.push(PenetrationEvent {
                entry: bite.entrance,
                exit: *exit,
                overmatched: bite.overmatched,
            });
            shell.path.points.push(*exit);
            (TerminalVerdict::Through, bite.entrance, bite.entrance)
        }
    };
    // Every exit the round reached throws, whether it then stopped or went on, unless its cone is
    // below the pool quantum.
    let mut cones = 0;
    for source in &bite.spall {
        if throw_spall_cone(shell.spall, shell.dose, source, bite.owner, context, is_box) {
            cones += 1;
        }
    }
    shell.marks.log.push(ArmourMark::Crossing {
        entrance: bite.entrance,
        normal: bite.normal,
        incoming: Vec3::from(bite.incoming),
        speed_in: bite.speed_in,
        overmatched: bite.overmatched,
        volumes: bite.volumes.clone(),
        end: match &bite.end {
            BiteEnd::Embedded { at, .. } => CrossingEnd::Rest { at: *at },
            BiteEnd::Perforated { exit, speed, .. } => CrossingEnd::Exit {
                at: *exit,
                speed: *speed,
            },
        },
        cones,
    });

    commands.trigger(Impact {
        position,
        normal: bite.normal,
        caliber: shell.projectile.caliber(),
        surface: ImpactSurface::Armor,
        outcome: verdict.into(),
    });
    if let Some(shot) = shell.shot {
        if !*terminal_emitted {
            *terminal_emitted = true;
            shell.terminal_report.0 = true;
            commands.trigger(ShellTerminal {
                shot: shot.0,
                position: terminal_at,
                normal: bite.normal,
                verdict,
                after_bounces: shell.marks.ricochets.len() as u32,
            });
        }
        commands.trigger(ShellStrike {
            shot: shot.0,
            sequence: (shell.marks.log.len() - 1) as u32,
            struck: bite.struck,
            at: bite.entrance,
            normal: bite.normal,
            incoming: Vec3::from(bite.incoming),
            caliber: shell.projectile.caliber(),
            verdict: verdict.into(),
        });
    }
}
