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
//! [`WalkError::IncompleteCorridor`] it extends and re-collects through the finite loaded geometry.
//! It never synthesizes the missing exit or imposes a policy length on a material crossing. This
//! keeps continuation state out of the resolver — the only thing that survives a crossing is which
//! primitives the disc is still inside, and that is a lookup on rays already walked
//! ([`DiscWalk::resume_at`]), never an inference.
//!
//! # One mechanism, decided before it is applied
//!
//! [`decide`] resolves a crossing from a [`Flight`] — the round's position, direction, speed,
//! CURRENT sampling frame and continuation — into a typed [`Decision`] with nothing mutated.
//! Timed playback carries the runtime consequences out of it (HP, impulses, marks, fields, events), and
//! [`Flight::follow`] advances the flight to where the next crossing starts. The runtime march and
//! the §13.6 gate (`fuzz`) both drive successive crossings through exactly this state machine; the
//! gate observes the decisions and applies nothing. Playback freezes field directions and
//! witness lines before delivering any crossing consequence.
//!
//! # Fields
//!
//! A spall cone's directions are walked by the same collector and pairing walk as the line's
//! corridors ([`field_presence`]), and stopped and dosed over the ordered presence it returns
//! ([`field_direction`]). The field law they serve — strength, profile, shadowing, dose — is the
//! design doc's §5 (`.agents/docs/design/armor-penetration-and-damage.md`).
//!
//! # The game's damage model fails at its earliest gate
//!
//! The asset door refuses invalid geometry and materials. Spawn binds certified shapes to their
//! volumes. Every remaining walk contradiction is a bug: gameplay panics in every build, with
//! the violated invariant and structured diagnostic. Only an incomplete intermediate corridor
//! is retried, by extending it; no error becomes a stopped round or a skipped dose.

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
};

mod playback;
pub(crate) use playback::Playback;

#[cfg(test)]
mod tests;

/// How far past first contact the first corridor reaches. Most crossings close well inside it; the
/// driver extends when they do not, so this is a starting guess, not a limit.
const ENTRANCE_SPAN: f32 = 0.5;

/// The game's damage model: spatial reach of a field throw (ASSUMED metres), also used as an
/// initial discovery guess for held material. Material corridors extend independently of it.
pub(crate) const FIELD_REACH: f32 = 50.0;

/// The game's damage model: chosen ceiling on discovery decisions per step (ASSUMED work bound).
/// A decision may collect arbitrarily many faces; this bounds successive decisions, not geometry.
pub(crate) const MAX_STEP_CROSSINGS: usize = 4096;

/// The crossing ceiling a world's steps run under in place of [`MAX_STEP_CROSSINGS`], when it names
/// one: a fixture reaches the limit in a few crossings instead of thousands.
#[derive(Resource, Clone, Copy)]
pub(crate) struct StepCrossings(pub(crate) usize);

/// The game's damage model: one tick's seconds and a ceiling on discovery work. Playback
/// spends only available seconds; deciding an immutable crossing charges work, not travel.
pub(crate) struct Budget {
    remaining: f64,
    crossings: usize,
    max_crossings: usize,
}

impl Budget {
    pub(crate) fn new(seconds: f64, max_crossings: usize) -> Self {
        assert!(
            seconds.is_finite() && seconds >= 0.0,
            "a step has finite nonnegative seconds"
        );
        Self {
            remaining: seconds,
            crossings: 0,
            max_crossings,
        }
    }

    pub(crate) fn remaining(&self) -> f64 {
        self.remaining
    }

    pub(crate) fn continues(&self, flight: &Flight) -> bool {
        self.remaining > 0.0 || flight.holds_material()
    }

    /// Spend `seconds` of flight, saturating the remaining clock at zero. The gate charges a
    /// complete decision; runtime playback truncates at the tick boundary. Open-air metres
    /// converted through f32 can also round past the remainder they were cut from.
    pub(crate) fn fly(&mut self, seconds: f64) {
        assert!(
            seconds.is_finite() && seconds >= 0.0,
            "flight spends finite nonnegative seconds"
        );
        self.remaining = (self.remaining - seconds).max(0.0);
    }

    pub(crate) fn charge(&mut self) -> Result<(), WalkError> {
        let crossings = self.crossings + 1;
        if crossings > self.max_crossings {
            return Err(WalkError::DrainExhausted { crossings });
        }
        self.crossings = crossings;
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
/// samples 13× more surface, so contradictory impacts scale ~4% → 14% with k on today's asset.
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
/// `seeds[i].inside` is what that ray's walk says the sample is inside there. A fresh disc
/// placed at birth, after a catch-up or after a bounce has no swept membership:
/// [`Flight::born`] and [`Flight::settle`] read it with [`certified`]. A disc laid on an
/// axis start a sweep reached uses [`Interior::laid`]; relative discovery panics if that
/// swept start is inside a collider. Offsets are whatever the rays left
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
/// Membership belongs to discovery. An immutable playback result retains its completed path;
/// the next discovery chain starts only after the held material is resolved.
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
    /// near the samples. Relative discovery verifies fresh starts; [`certified`] reads
    /// contact-time membership in the frozen scene.
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

    /// Re-anchored at the axis start so a new layout's corridors sit within one step's reach of
    /// their anchor (the scale [`collect`]'s sitting reach is bounded for). The residual `origin`
    /// keeps what world rounding of the new anchor lost, so the start moves by corridor-scale
    /// rounding only.
    pub(crate) fn rebased(self) -> Self {
        let anchor = self.world_origin();
        Self {
            origin: self.origin - (anchor - self.anchor),
            anchor,
            seeds: self.seeds,
        }
    }

    /// The axis sample's start, without what any sample is inside.
    pub(crate) fn axis(&self) -> AxisStart {
        AxisStart {
            anchor: self.anchor,
            origin: self.origin,
        }
    }
}

/// Where the axis sample stands as a step ends: a WORLD anchor and the start relative to it
/// ([`Interior::anchor`], [`Interior::origin`]), so the next step's layout keeps what world
/// rounding of a re-taken anchor loses. A position only: what a sample is inside never crosses a
/// tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct AxisStart {
    pub anchor: Vec3,
    pub origin: Vec3,
}

impl AxisStart {
    /// The same start re-anchored at itself, as [`Interior::rebased`].
    pub(crate) fn rebased(self) -> Self {
        let anchor = self.anchor + self.origin;
        Self {
            origin: self.origin - (anchor - self.anchor),
            anchor,
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

/// Containment work is bounded by the finite candidate geometry loaded into the world. It has no
/// independent candidate or accumulated-disc quota: every admitted shell must be read.
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
/// certificate, and a convex solid. Spawn constructs only these shapes; another is a wiring bug.
/// The probe spans its finite loaded geometry, independently of a resolution corridor's length.
pub(crate) fn certified(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    at: Vec3,
    axis: Dir3,
    frame: DiscFrame,
    radius: f32,
) -> Result<Interior, WalkError> {
    certified_at(
        context,
        not_own,
        AxisStart {
            anchor: at,
            origin: Vec3::ZERO,
        },
        axis,
        frame,
        radius,
    )
}

/// [`certified`] at an axis start held as a WORLD anchor and a start relative to it: every probe
/// line is formed from `start.anchor` and `start.origin + offset`, the values the returned starts
/// hold, so the low bits the anchor's world rounding would drop are kept.
fn certified_at(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    start: AxisStart,
    axis: Dir3,
    frame: DiscFrame,
    radius: f32,
) -> Result<Interior, WalkError> {
    certified_samples(
        context,
        not_own,
        start,
        axis,
        frame,
        radius,
        layout(&frame, radius),
    )
}

/// The game's damage model: contact-time membership for the starts discovery actually swept.
/// The next layout's transported basis must not move or flatten those staggered sample starts.
pub(crate) fn certified_samples(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    start: AxisStart,
    axis: Dir3,
    frame: DiscFrame,
    radius: f32,
    offsets: Vec<Vec3>,
) -> Result<Interior, WalkError> {
    let AxisStart { anchor: at, origin } = start;
    let offsets: Vec<Vec3> = offsets.into_iter().map(|offset| origin + offset).collect();
    let axis = Vec3::from(axis);
    let reach = offsets
        .iter()
        .map(|offset| (*offset - origin).length())
        .fold(0.0, f32::max);

    // The candidates whose bounds contain a start, and the union of those bounds, relative to `at`.
    let mut near = Vec::new();
    let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for (node, primitive) in candidates(context, not_own, at + origin, axis, 0.0, reach) {
        let Ok((position, rotation, collider, _)) = context.colliders.get(primitive) else {
            return Err(WalkError::CollectorFailed {
                volume: node,
                reason: "a candidate collider has no resolved pose",
            });
        };
        let pose = context.world.pose(primitive);
        let bounds = collider.aabb(
            pose.map_or(position.0, |pose| pose.position),
            pose.map_or(*rotation, |pose| Rotation(pose.rotation)),
        );
        let (min, max) = (bounds.min - at, bounds.max - at);
        let margin = Vec3::splat(PROBE_MARGIN);
        if !offsets
            .iter()
            .any(|&offset| offset.cmpge(min - margin).all() && offset.cmple(max + margin).all())
        {
            continue;
        }
        let shape = collider.shape_scaled();
        assert!(
            matches!(shape.as_typed_shape(), TypedShape::TriMesh(_)) || shape.is_convex(),
            "game's damage model: spawn constructs certified trimesh or convex ballistic geometry: {node:?}"
        );
        lo = lo.min(min);
        hi = hi.max(max);
        near.push((node, primitive));
    }
    if near.is_empty() {
        return Ok(Interior {
            anchor: at,
            origin,
            seeds: Interior::laid(at, offsets)
                .seeds
                .into_iter()
                .map(|seed| SampleSeed {
                    offset: seed.offset - origin,
                    ..seed
                })
                .collect(),
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
    assert!(
        length.is_finite(),
        "game's damage model: loaded finite candidate bounds yield a finite containment probe"
    );

    let mut samples = Vec::with_capacity(offsets.len());
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
            &|entity| context.world.pose(entity),
        )?;
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
                offset: corridor.offset - origin,
                t: handoff,
                inside: walked.inside_at(handoff, &context.laws),
            })
        })
        .collect::<Result<Vec<_>, WalkError>>()?;
    Ok(Interior {
        anchor: at,
        origin,
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
    /// The union of the pose bounds of the hard colliders within [`FIELD_REACH`] of the throw
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
                .map(|(position, rotation, collider, _)| {
                    let pose = context.world.pose(self.collider);
                    collider.aabb(
                        pose.map_or(position.0, |pose| pose.position),
                        pose.map_or(*rotation, |pose| Rotation(pose.rotation)),
                    )
                })
        })
    }
}

impl FieldReach {
    /// The reach of a field thrown from `at`. Every box a direction queries spans `at` to `at +
    /// axis × L` with `L ≤` [`FIELD_REACH`] (the presence corridor's, or the witness pass's to a
    /// stop on that walk, whose crossings lie short of its length), or is the cube of half-side
    /// [`FIELD_REACH`] about `at` ([`horizon`]); the cube a metre wider holds every one. DERIVED
    /// from IEEE 754 round-to-nearest, at any magnitude of `at`: each face is an f32 `at + a`
    /// rounded and the wide cube's `at ± 51 m` rounded, and rounding is monotone, so a face with
    /// `|a| ≤ 51 m` stays inside. ASSUMED: `|axis_i × L| ≤ 51 m`, which a unit axis meets with a
    /// metre of room (bevy's `Dir3` panics in debug builds when its length is about 1 % off).
    pub(crate) fn of_throw(
        context: &ResolveContext<'_, '_, '_>,
        admit: &dyn Fn(Entity) -> bool,
        at: Vec3,
    ) -> Self {
        let half = Vec3::splat(FIELD_REACH + 1.0);
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
        world.spatial.aabb_intersections_with_aabb_callback(
            ColliderAabb::from_min_max(
                region.min - Vec3::splat(world.motion_padding()),
                region.max + Vec3::splat(world.motion_padding()),
            ),
            |entity| {
                let Ok(&broad) = world.aabbs.get(entity) else {
                    return true;
                };
                let broad = ColliderAabb::from_min_max(
                    broad.min - Vec3::splat(world.motion_padding()),
                    broad.max + Vec3::splat(world.motion_padding()),
                );
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
            },
        );
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
/// forward ray meets within [`FIELD_REACH`]; the walk starts and ends outside the union of those
/// grown bounds, so no candidate's face lies within the collector's sitting reach of either end.
/// Its containment probe spans the finite loaded bounds; it does not truncate their geometry.
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
    for reached in field.hard_meeting(collect::swept_aabb(at, axis, FIELD_REACH, 0.0)) {
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
    let (enter, leave) = meets(lo, hi).expect(
        "the game's damage model admitted each candidate on this line, so their union meets it",
    );
    let behind = (-enter).max(0.0);
    let length = behind + leave.max(0.0);
    assert!(
        length.is_finite(),
        "game's damage model: finite loaded bounds yield a finite field probe"
    );
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
        &|entity| context.world.pose(entity),
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
        &VolumeTable::new(entries),
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
/// contradiction in the witness pass propagates to the caller, which panics in gameplay.
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
        FIELD_REACH
    };
    for (witness, t) in super::wound::entries_in(context, field, at, dir, ahead)? {
        let t = start + t;
        if t < reach && strength > 0.0 && is_box(witness) {
            doses.push((t, dose(witness)));
        }
    }
    doses.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(FieldRay {
        end,
        doses: doses.into_iter().map(|(_, dose)| dose).collect(),
    })
}

/// The struck tank's horizon along a field direction thrown from `at`: where the line leaves the
/// union of the bounds of every hard collider the throw admits within [`FIELD_REACH`] of `at`
/// (its [`FieldReach`], built for `at`); `at` itself where there is none or the union lies behind
/// it. Witnesses take no part. The union does not depend on the direction, so a throw reads it
/// once.
fn horizon(context: &ResolveContext<'_, '_, '_>, field: &FieldReach, at: Vec3, dir: Dir3) -> Vec3 {
    let (lo, hi) = *field.union.get_or_init(|| {
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        let cube = collect::swept_aabb(at, Vec3::from(dir), 0.0, FIELD_REACH);
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
/// continuation ([`Interior`]). An authority flight without one — just bounced —
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
    /// The game's damage model: a new discovery disc on the axis start the last sweep reached,
    /// outside material by the walk law. Its sweep verifies that invariant; no tick-start
    /// containment probe supplies membership. A disc no sweep has reached is placed through
    /// [`Self::born`] instead.
    pub(crate) fn launched(
        swept: Dir3,
        direction: Dir3,
        speed: f32,
        frame: DiscFrame,
        radius: f32,
        axis: AxisStart,
    ) -> Self {
        let start = axis.rebased();
        let mut starts = Interior::laid(start.anchor, layout(&frame, radius));
        starts.origin = start.origin;
        Self {
            position: starts.world_origin(),
            direction,
            speed,
            frame: frame.transport(Vec3::from(swept), Vec3::from(direction)),
            continuation: Some(starts),
        }
    }

    /// The game's damage model: a disc no sweep has reached, placed into the scene
    /// (`ballistics::launch`): the disc [`Self::launched`] lays at `position` with no axis start,
    /// its membership read there by the containment probe ([`certified`]) along `swept`.
    #[expect(
        clippy::too_many_arguments,
        reason = "the flight's whole state plus the world its birth membership is read in"
    )]
    pub(crate) fn born(
        context: &ResolveContext<'_, '_, '_>,
        not_own: &dyn Fn(Entity) -> bool,
        position: Vec3,
        swept: Dir3,
        direction: Dir3,
        speed: f32,
        frame: DiscFrame,
        radius: f32,
    ) -> Result<Self, WalkError> {
        let starts = certified(context, not_own, position, swept, frame, radius)?;
        Ok(Self {
            position: starts.world_origin(),
            direction,
            speed,
            frame: frame.transport(Vec3::from(swept), Vec3::from(direction)),
            continuation: Some(starts),
        })
    }

    /// Lay the fresh disc a bounce left — on `frame` at `position`, square to
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
    /// the round came to rest; its clock is [`Decision::seconds`].
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
/// through timed playback; the §13.6 gate only observes it.
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
    Bite(Box<Bite>),
}

impl Decision {
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
    pub samples: Vec<SampleLeg>,
}

/// One sample of the game's damage-model line: its path and the resolved hard presence on it.
/// Reordering samples carries their clipping masks with their paths.
#[derive(Clone, Debug)]
pub(crate) struct SampleLeg {
    pub offset: Vec3,
    pub length: f32,
    /// `None` names a stretch not collected yet: open flight or standalone line inspection.
    pub hard: Option<Vec<(f64, f64)>>,
}

impl Leg {
    /// Every sample flying `length` m from its own start, `from + offsets[i]`.
    pub(crate) fn new(from: Vec3, axis: Dir3, length: f32, offsets: &[Vec3]) -> Self {
        Leg {
            from,
            axis,
            samples: offsets
                .iter()
                .map(|offset| SampleLeg {
                    offset: *offset,
                    length: length.max(0.0),
                    hard: None,
                })
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
                .map(|(offset, end)| SampleLeg {
                    offset: *offset,
                    length: (*end - from - *offset).dot(*axis).max(0.0),
                    hard: None,
                })
                .collect(),
        }
    }

    /// The game's damage model has already resolved these sample rays. Reuse their presence
    /// rather than recasting at world-rounded origins. A sample with an uncollected suffix still
    /// needs its entire stretch collected before the witness pass can clip against hard matter.
    fn with_presence(mut self, walked: &DiscWalk) -> Self {
        assert_eq!(
            self.samples.len(),
            walked.walks.len(),
            "a resolved leg and its disc walk have the same sample identities"
        );
        for (sample, walk) in self.samples.iter_mut().zip(&walked.walks) {
            if sample.length <= walked.length {
                sample.hard = Some(walk.occupied(0.0, sample.length as f64));
            }
        }
        self
    }
}

/// A deflection off the entrance face (§4). The deflection ANGLE and the speed lost both already
/// carry η from the core (§13.5) — a graze is a partial ricochet in direction as well as in speed.
#[derive(Clone, Debug)]
pub(crate) struct Bounce {
    pub feedback: crate::substances::SurfaceFeedback,
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
    pub feedback: crate::substances::SurfaceFeedback,
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
    /// The game's damage model keeps effect progress in corridor f64, before world rounding.
    pub volume_ends: std::collections::BTreeMap<Entity, f64>,
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
    /// The game's damage model: the transit's resolved force-law clock, in corridor metres.
    pub motion: Vec<walk::Motion>,
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
                leg: Leg::new(anchor + start, dir, examined, &entrance.offsets)
                    .with_presence(&entrance),
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
                    .map(|(offset, t)| SampleLeg {
                        offset: *offset,
                        length: t as f32,
                        hard: None,
                    })
                    .collect(),
            }
            .with_presence(&entrance);
            Ok(Decision::Bounce(Bounce {
                feedback: context
                    .world
                    .volumes
                    .get(struck)
                    .expect("the resolved entrance belongs to a gathered volume")
                    .material
                    .feedback,
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
            let feedback = context
                .world
                .volumes
                .get(plan.entrance.volume)
                .expect("the resolved entrance belongs to a gathered volume")
                .material
                .feedback;
            Ok(Decision::Bite(Box::new(bite(
                &transit,
                plan,
                feedback,
                mass,
                from,
                dir,
                &entrance,
                bent,
                speed,
                owner,
                &context.laws,
            ))))
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
    feedback: crate::substances::SurfaceFeedback,
    // kg.
    mass: f32,
    // Where the flight's position is, RELATIVE to the transit's anchor.
    from: Vec3,
    incoming: Dir3,
    // The resolved entrance rays: the approach's offsets and hard presence.
    entrance_walk: &DiscWalk,
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
    let mut way_in = Leg::ending_at(from, incoming, &entrance_walk.offsets, &handoffs)
        .with_presence(entrance_walk);
    way_in.from = world(from);
    let entrance = world(plan.entrance.position);
    let rest = match plan.outcome {
        Outcome::Embedded { .. } => Some(
            plan.motion
                .last()
                .expect("an embedded crossing advances at least one encounter")
                .end,
        ),
        Outcome::Perforated { .. } if plan.state.speed == 0.0 => Some(
            plan.motion
                .last()
                .expect("a rest at the rear face advanced a mechanism encounter")
                .end,
        ),
        Outcome::Perforated { .. } => None,
    };
    let volumes = volume_crossings(transit, &plan.crossing, &plan.deposits, rest, &world);
    let volume_ends = exposed_spans(&plan.crossing, rest)
        .into_iter()
        .map(|(volume, (_, end))| (volume, rest.map_or(end, |stop| end.min(stop))))
        .collect();
    let (end, flown) = match plan.outcome {
        Outcome::Embedded { at, t } => (
            BiteEnd::Embedded {
                at: world(at),
                direction: bent,
                impulse: mass * v_in,
            },
            t,
        ),
        Outcome::Perforated { exit, t, .. } if plan.state.speed == 0.0 => (
            BiteEnd::Embedded {
                at: world(exit),
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
        feedback,
        entrance,
        incoming,
        speed_in,
        volumes,
        volume_ends,
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
        motion: plan.motion,
        end,
        legs: [
            way_in,
            Leg::new(world(transit.origin), bent, flown, &transit.offsets).with_presence(transit),
        ],
    }
}

/// How far the line got through each volume it put a chord into: the axis points where the disc's
/// first sample entered its exposed portion and where its last left that portion (or rested in
/// it), from dosed encounter contacts only. In the game's damage model, `perforated` means every
/// exposed contact ended before `rest`, even if the round still occupies a hidden portion of the
/// volume; a crossing without a rest traversed all of its exposed portions. An exact exit face
/// has not been cleared at rest, matching [`walk::RayWalk::inside_at`]'s exit membership.
fn volume_crossings(
    transit: &DiscWalk,
    event: &walk::DiscEvent,
    deposits: &[Dose],
    rest: Option<f64>,
    world: &impl Fn(Vec3) -> Vec3,
) -> Vec<VolumeCrossing> {
    let spans = exposed_spans(event, rest);
    let at = |t: f64| world(transit.origin + transit.axis * t as f32);
    deposits
        .iter()
        .filter_map(|dose| {
            let DoseSource::Line { chord, cost } = dose.source else {
                return None;
            };
            let &(start, end) = spans
                .get(&dose.address)
                .expect("a positive Line deposit has an exposed contact in its resolved crossing");
            let (end, perforated) = match rest {
                Some(rest) if end >= rest => (rest, false),
                _ => (end, true),
            };
            Some(VolumeCrossing {
                volume: dose.address,
                entry: at(start),
                end: at(end),
                chord,
                cost,
                energy: dose.amount,
                perforated,
            })
        })
        .collect()
}

/// The game's damage model: exposed volume progress stays in corridor precision until effects
/// are addressed by moment. World positions are readouts, never timing inputs.
fn exposed_spans(
    event: &walk::DiscEvent,
    rest: Option<f64>,
) -> std::collections::BTreeMap<Entity, (f64, f64)> {
    let mut spans: std::collections::BTreeMap<Entity, (f64, f64)> = Default::default();
    for contact in event
        .encounters
        .iter()
        .flat_map(|encounter| &encounter.samples)
        .flat_map(|sample| {
            let strongest = super::mechanism::overlap_factor(&sample.present);
            sample
                .present
                .iter()
                .filter(move |contact| contact.material.factor == strongest)
        })
        .filter(|contact| rest.is_none_or(|rest| contact.start < rest))
    {
        let span = spans
            .entry(contact.volume)
            .or_insert((contact.start, contact.end));
        span.0 = span.0.min(contact.start);
        span.1 = span.1.max(contact.end);
    }
    spans
}

/// The game's damage model: a certified approach and the first contact it contains. A fuze
/// can end its approach at that boundary; continuing discovery must process the contact first.
pub(crate) struct CertifiedSpan {
    pub decision: Decision,
    pub contact: Option<f32>,
}

/// The game's damage model certifies an open discovery span before retaining its end membership.
/// The nearest-hit cast is only discovery; the collector may still own a face within its rounding
/// band. Such a face is resolved by the same crossing driver, never stepped over as clear air.
pub(crate) fn clear_path(
    context: &ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    round: &SolidShot,
    flight: &Flight,
    travel: f32,
) -> Result<CertifiedSpan, WalkError> {
    let interior = flight
        .continuation
        .as_ref()
        .expect("discovery establishes each sample's start");
    let walked = closing_walk(
        context,
        admit,
        interior.anchor,
        interior.origin,
        *flight.direction,
        flight.frame,
        disc_radius(round.body().caliber),
        &interior.seeds,
        travel.max(ENTRANCE_SPAN),
    )?;
    let contact = walked
        .events
        .first()
        .filter(|crossing| crossing.start <= f64::from(travel))
        .map(|crossing| crossing.start.max(0.0) as f32);
    let origin = interior.origin + flight.direction * travel;
    Ok(CertifiedSpan {
        contact,
        decision: Decision::Clear {
            resume: interior.anchor + origin,
            travel,
            continuation: Interior {
                anchor: interior.anchor,
                origin,
                seeds: walked.resume_at(f64::from(travel), &context.laws),
            },
            leg: Leg::new(
                interior.world_origin(),
                flight.direction,
                travel,
                &walked.offsets,
            )
            .with_presence(&walked),
        },
    })
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
    let mut length = initial.max(ENTRANCE_SPAN);
    let horizon = OnceCell::new();
    loop {
        let corridor = build_corridor(
            context, not_own, anchor, origin, axis, length, frame, radius, seeds,
        )?;
        let volumes = volume_table(context, &corridor)?;
        let result = walk::walk_disc(&corridor, &volumes, &context.laws);
        match &result {
            Ok(walked) => {
                let end = walked
                    .first_crossing()
                    .iter()
                    .map(|&piece| walked.events[piece].end)
                    .reduce(f64::max);
                if end
                    .is_none_or(|end| end + context.laws.weld_max_lookahead as f64 <= length as f64)
                {
                    return result;
                }
            }
            Err(WalkError::IncompleteCorridor { .. }) => {}
            Err(_) => return result,
        }
        // The game's damage model: every tree candidate is contained by its world AABB.
        // Beyond their furthest support plane plus weld clearance there can be no new face.
        // Read this finite scene bound only when a corridor actually needs extension.
        let horizon = *horizon.get_or_init(|| {
            let direction = axis.as_dvec3();
            let norm_squared = direction.length_squared();
            let far = context
                .world
                .aabbs
                .iter()
                .map(|bounds| {
                    let a = ((bounds.min.as_dvec3() - anchor.as_dvec3()) - origin.as_dvec3())
                        * direction;
                    let b = ((bounds.max.as_dvec3() - anchor.as_dvec3()) - origin.as_dvec3())
                        * direction;
                    a.max(b).element_sum() / norm_squared
                })
                .fold(f64::from(length), f64::max);
            // Staggered sample starts can lie behind the axis start; include their axial reach.
            let stagger = seeds
                .iter()
                .map(|seed| seed.offset.as_dvec3().dot(direction).abs() / norm_squared)
                .fold(f64::from(radius), f64::max);
            let horizon = ((far
                + stagger
                + f64::from(context.laws.weld_max_lookahead)
                + f64::from(PROBE_MARGIN)
                + f64::from(context.world.motion_padding())) as f32)
                .next_up();
            assert!(
                horizon.is_finite(),
                "game's damage model: finite loaded collider bounds yield a finite closing horizon"
            );
            horizon
        });
        if length >= horizon {
            // An open shell here has no exit in its own bounds: a topology contradiction,
            // rather than valid geometry exceeding a chosen resource ceiling.
            return result;
        }
        length = (length * 2.0).min(horizon);
    }
}

/// The game's damage model: finish the struck physical body's actual material on these
/// sample rays. A finite support bound only sizes the collector; only its real exit faces
/// extend the immutable crossing. No baked box, servo window or future pose supplies a face.
pub(crate) fn body_completion(
    context: &ResolveContext<'_, '_, '_>,
    struck: Entity,
    admit: &dyn Fn(Entity) -> bool,
    flight: &Flight,
    radius: f32,
) -> Result<f32, WalkError> {
    let parts: Vec<_> = context
        .world
        .body_primitives(struck)
        .into_iter()
        .filter(|&(_, primitive)| admit(primitive))
        .collect();
    let offsets = flight.sample_offsets(radius);
    let axis = *flight.direction;
    let (anchor, origin, seeds) = match &flight.continuation {
        Some(interior) => (interior.anchor, interior.origin, interior.seeds.as_slice()),
        None => (flight.position, Vec3::ZERO, &[][..]),
    };
    let far = parts
        .iter()
        .map(|&(_, primitive)| {
            let (position, rotation, collider, _) = context
                .colliders
                .get(primitive)
                .expect("game's damage model: body membership names prepared collider geometry");
            let pose = context.world.pose(primitive);
            let bounds = collider.aabb(
                pose.map_or(position.0, |pose| pose.position),
                pose.map_or(*rotation, |pose| Rotation(pose.rotation)),
            );
            let a =
                ((bounds.min.as_dvec3() - anchor.as_dvec3()) - origin.as_dvec3()) * axis.as_dvec3();
            let b =
                ((bounds.max.as_dvec3() - anchor.as_dvec3()) - origin.as_dvec3()) * axis.as_dvec3();
            a.max(b).element_sum()
        })
        .fold(0.0, f64::max);
    let spread = offsets
        .iter()
        .map(|offset| f64::from(offset.length()))
        .fold(0.0, f64::max);
    let length = ((far + spread + f64::from(PROBE_MARGIN)) as f32).next_up();
    assert!(
        length.is_finite(),
        "game's damage model: finite loaded body geometry has finite support"
    );
    let mut end = 0.0f64;
    for (sample, offset) in offsets.into_iter().enumerate() {
        let seed = seeds.get(sample);
        let mut hits = Vec::new();
        collect::collect(
            &Corridor {
                anchor,
                origin: origin + offset,
                axis,
                length,
                behind: 0.0,
                seeded: seed.map_or(&[], |seed| seed.inside.as_slice()),
                handoff: seed.map(|seed| seed.t),
                laws: &context.laws,
            },
            &parts,
            context.colliders,
            &mut hits,
            &|entity| context.world.pose(entity),
        )?;
        if let Some(exit) = walk::last_exit(&hits) {
            end = end.max(exit);
        }
    }
    Ok(if end > 0.0 {
        (end as f32).next_up()
    } else {
        0.0
    })
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
        collect::swept_aabb(
            origin,
            axis,
            length,
            radius + context.world.motion_padding(),
        ),
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
            &|entity| context.world.pose(entity),
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
    Ok(VolumeTable::new(entries))
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
        surface: ImpactSurface::Material(bounce.feedback),
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
            feedback: bounce.feedback,
            shot: shot.0,
            sequence: (shell.marks.log.len() - 1) as u32,
            struck: bounce.struck,
            at: bounce.position,
            normal: bounce.normal,
            incoming: Vec3::from(bounce.incoming),
            caliber: shell.projectile.caliber(),
            contact_caliber: super::face_contact_caliber(
                Verdict::Ricochet,
                shell.projectile.caliber(),
                shell.projectile.penetrator_caliber(),
            ),
            verdict: Verdict::Ricochet,
        });
    }
}

fn finish_bite(
    bite: &Bite,
    mark: usize,
    shell: &mut MarchingShell,
    terminal_emitted: &mut bool,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) {
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
    commands.trigger(Impact {
        position,
        normal: bite.normal,
        caliber: shell.projectile.caliber(),
        surface: ImpactSurface::Material(bite.feedback),
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
            feedback: bite.feedback,
            shot: shot.0,
            sequence: mark as u32,
            struck: bite.struck,
            at: bite.entrance,
            normal: bite.normal,
            incoming: Vec3::from(bite.incoming),
            caliber: shell.projectile.caliber(),
            contact_caliber: super::face_contact_caliber(
                verdict.into(),
                shell.projectile.caliber(),
                shell.projectile.penetrator_caliber(),
            ),
            verdict: verdict.into(),
        });
    }
}
