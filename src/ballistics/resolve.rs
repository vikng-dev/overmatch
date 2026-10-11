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
//! gate observes the decisions and applies nothing. The seam is not yet complete: spall fragments
//! are resolved in [`apply`], not decided (see there).
//!
//! # Fail closed, never free
//!
//! On any structured [`WalkError`] the round STOPS at the contact it was resolving: no perforation,
//! no spall, no transit damage. A wrong answer here is free penetration, and free penetration is
//! indistinguishable from armour that was never modelled — the defect class §13 exists to kill.
//! Crashing a multiplayer authority is too strong a response; a stopped shell and a rate-limited
//! warning is not.

use avian3d::parry::shape::TypedShape;
use avian3d::prelude::{Collider, Forces, Position, Rotation, SimpleCollider, SpatialQueryFilter};
use bevy::prelude::*;

use super::collect::{self, Corridor};
use super::walk::{
    self, Begin, DiscCorridor, DiscFrame, DiscWalk, Outcome, RayCorridor, SampleCorridor,
    SampleSeed, ShellKey, Shot, VolumeTable, WalkError, WalkLaws,
};
use super::{
    BallisticSurfaces, ComponentHealth, Impact, ImpactOutcome, ImpactSurface, MARCH_EPS,
    MarchingShell, PenetrationEvent, ProjectileMarchWorld, RoundBody, ShellRicochet, ShellTerminal,
    TerminalVerdict, apply_hit_impulse, capability, hit_ancestor, speed_for, throw_spall_burst,
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

/// One step's travel budget, spent by its successive crossings, and the limits nothing may pass.
///
/// Shared by the runtime march and the §13.6 gate's driver, so both stop a runaway step at the same
/// crossing with the same structured error.
pub(crate) struct Budget {
    /// Metres of travel left; negative once a drain runs past the budget.
    remaining: f32,
    crossings: usize,
}

impl Budget {
    pub(crate) fn new(travel: f32) -> Self {
        Self {
            remaining: travel,
            crossings: 0,
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

    /// Charge one more crossing of `travel` metres, BEFORE its consequences are applied — or the
    /// structured error that stops the round where it stands. Exhaustion fails closed: a round that
    /// hits a limit stops; it is never waved through.
    pub(crate) fn charge(&mut self, travel: f32) -> Result<(), WalkError> {
        let crossings = self.crossings + 1;
        let remaining = self.remaining - travel;
        let overrun = (-remaining).max(0.0);
        if crossings > MAX_STEP_CROSSINGS || overrun > MAX_OVERRUN {
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

/// Main-penetrator transit damage = the cost it paid crossing that volume × this (design §6).
const TRANSIT_K: f32 = 1.0;

/// Shock a glancing bounce jars into an exposed component: impact energy × squareness. Armour has no
/// HP and shrugs it off.
const SHOCK_K: f32 = 0.045;

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
const PROBE_MARGIN: f32 = MARCH_EPS;

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
    /// staggered at the entrance plane, then bent — and that footprint can be WIDER than the
    /// calibre, and stays so into later ticks (DERIVED: full coverage at 60° incidence bends 12°
    /// under the default normalization, a width factor of `cos 12° + tan 60° · sin 12° ≈ 1.338`).
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
                BiteEnd::Embedded { at, .. } => {
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
}

/// A deflection off the entrance face (§4). The deflection ANGLE and the bleed both already carry η
/// from the core (§13.5) — a graze is a partial ricochet in direction as well as in speed.
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
    /// HP the bounce jars into the struck component before that component's own floor: impact
    /// energy × squareness ([`SHOCK_K`]). Armour has no HP and shrugs it off.
    pub shock: f32,
    pub direction: Dir3,
    /// Speed retained (m/s).
    pub speed: f32,
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
}

/// An entry into the material (§§3–6).
#[derive(Clone, Debug)]
pub(crate) struct Bite {
    /// Where the round punched in: the struck face.
    pub entrance: Vec3,
    pub normal: Vec3,
    /// Entrance coverage η.
    pub coverage: f32,
    pub overmatched: bool,
    /// The rigid body owning the entrance face, which takes the momentum exchange (§13.5).
    pub owner: Option<Entity>,
    /// Per-entity transit charges, already clipped at the embed progress: a round that stops in the
    /// first plate deposits nothing behind it.
    pub deposits: Vec<walk::EntityDeposit>,
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
    pub end: BiteEnd,
}

/// How a bite ended.
#[derive(Clone, Debug)]
pub(crate) enum BiteEnd {
    /// Defeated: buried at `at`, the whole remaining momentum to the entrance body.
    Embedded { at: Vec3, impulse: Vec3 },
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
        /// One spall source per downward field step the round reached, including the exit (§13.2's
        /// field law): `(position, η-weighted budget)`.
        spall: Vec<(Vec3, f32)>,
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
    pub armor: &'a SpatialQueryFilter,
    /// Authority = not a replica: only then does a crossing mutate health.
    pub deposit: bool,
    pub laws: WalkLaws,
}

/// Decide one crossing of the union field from first contact, with no consequence applied:
/// corridor closure, the entrance read ([`walk::begin`]), the transit collection along the bend,
/// [`walk::finish`], the residual speed, the travel and the continuation.
///
/// `contact` is the distance from [`Flight::sweep_origin`] to the nearest sample's first armour
/// contact; it sizes the first corridor and nothing else.
pub(crate) fn decide(
    context: &ResolveContext<'_, '_, '_>,
    not_own: &dyn Fn(Entity) -> bool,
    body: RoundBody,
    flight: &Flight,
    contact: f32,
) -> Result<Decision, WalkError> {
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
    let capability_in = capability(body, speed);
    let shot = Shot {
        caliber: body.caliber,
        capability: capability_in,
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

    match walk::begin(&entrance, &shot, &context.laws) {
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
            })
        }
        Begin::Ricochet {
            entrance: read,
            direction,
            speed_scale,
        } => {
            let struck = entrance.events[0].entrance_volume;
            let position = anchor + read.position;
            let out = Dir3::new(direction).unwrap_or(dir);
            let bled = speed * speed_scale;
            Ok(Decision::Bounce(Bounce {
                position,
                normal: read.normal,
                coverage: read.coverage,
                struck,
                owner: owner(struck),
                shock: SHOCK_K * capability_in * read.incidence.cos(),
                direction: out,
                speed: bled,
                impulse: body.mass * (v_in - Vec3::from(out) * bled),
                travel: (read.position - from).dot(Vec3::from(dir)).max(0.0) + MARCH_EPS,
                resume: position + read.normal * radius + out * MARCH_EPS,
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
            let plan = walk::finish(&transit, &request, &shot, &context.laws)?;
            Ok(Decision::Bite(bite(
                &transit,
                plan,
                body,
                capability_in,
                from,
                dir,
                bent,
                v_in,
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
    plan: walk::ResolutionPlan,
    body: RoundBody,
    capability_in: f32,
    // Where the flight's position is, RELATIVE to the transit's anchor.
    from: Vec3,
    incoming: Dir3,
    bent: Dir3,
    v_in: Vec3,
    owner: impl Fn(Entity) -> Option<Entity>,
    laws: &WalkLaws,
) -> Bite {
    // THE BOUNDARY. Everything the walk reports is anchor-relative. What leaves for presentation and
    // physics — the entrance, the exit, the embed point, spall sources, the resume point — is
    // converted to WORLD here, in one place. The continuation ([`Interior`]) deliberately stays
    // anchor-relative: it is the next corridor's input, and rebuilding it in world would round it at
    // the world's spacing (see there).
    let world = |local: Vec3| transit.anchor + local;
    let struck = transit.events[0].entrance_volume;
    // The APPROACH: the flight's position to the axis handoff, along the way in. The transit's own
    // `t` then measures the rest, along the bend.
    let approach = (transit.origin - from).dot(Vec3::from(incoming)).max(0.0);
    let entrance = world(plan.entrance.position);
    let (end, flown) = match plan.outcome {
        Outcome::Embedded { at, t } => (
            BiteEnd::Embedded {
                at: world(at),
                impulse: body.mass * v_in,
            },
            t,
        ),
        Outcome::Perforated { exit, t, .. } => {
            let residual = speed_for(body, (capability_in - plan.cost_spent).max(0.0));
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
                    impulse: body.mass * (v_in - Vec3::from(bent) * residual),
                    spall: plan
                        .spall
                        .iter()
                        .map(|mark| (world(mark.position), mark.budget))
                        .collect(),
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
        normal: plan.entrance.normal,
        coverage: plan.entrance.coverage,
        overmatched: plan.entrance.overmatched,
        owner: owner(struck),
        deposits: plan.deposits,
        travel: approach + flown,
        end,
    }
}

/// Collect and walk a corridor, extending it until the first crossing CLOSES.
///
/// "Closes" is two conditions, not one: the walk must pair (no primitive left open at the end), and
/// the first event must end far enough inside the corridor that no ε-weld could reach past it. A
/// corridor that stopped one millimetre after an exit could not tell a finished run from the first
/// half of a welded sandwich.
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
                let settled = walked.events.first().is_none_or(|event| {
                    event.end + context.laws.weld_max_lookahead as f64 <= length as f64
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
            // which is the same rule the march has always used to tell armour from terrain.
            if let Some((node, _)) =
                hit_ancestor(entity, &context.world.volumes, &context.world.parents)
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
    let mut entries: Vec<(Entity, f32)> = Vec::new();
    for sample in &corridor.samples {
        for hit in &sample.hits {
            if !entries.iter().any(|(entity, _)| *entity == hit.volume) {
                let factor = context
                    .world
                    .volumes
                    .get(hit.volume)
                    .map(|volume| volume.material_factor)
                    .map_err(|_| WalkError::UnknownVolume { volume: hit.volume })?;
                entries.push((hit.volume, factor));
            }
        }
        for key in &sample.initial_presence {
            if !entries.iter().any(|(entity, _)| *entity == key.volume) {
                let factor = context
                    .world
                    .volumes
                    .get(key.volume)
                    .map(|volume| volume.material_factor)
                    .map_err(|_| WalkError::UnknownVolume { volume: key.volume })?;
                entries.push((key.volume, factor));
            }
        }
    }
    VolumeTable::new(entries)
}

/// Apply a decision's consequences on the runtime: HP, impulses, the shell's marks and path, spall,
/// and the impact and authority events. Returns the HP actually removed.
///
/// Transit deposits, a bounce's shock, the impulses and the events all come out of the
/// [`Decision`]. Spall does NOT: the decision carries each spall source and its budget, but the
/// fragment count, their directions and what each fragment hits and deposits are resolved here,
/// against the live world ([`throw_spall_burst`]). So the §13.6 gate, which observes the decisions
/// and applies none of this, reads every transit deposit and every bounce the runtime acts on, and
/// no fragment's HP. Resolving fragments into the decision is step C's work.
pub(crate) fn apply(
    decision: &Decision,
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) -> f32 {
    match decision {
        Decision::Clear { .. } => 0.0,
        Decision::Bounce(bounce) => apply_bounce(
            bounce,
            shell,
            context,
            terminal_emitted,
            health,
            bodies,
            commands,
        ),
        Decision::Bite(bite) => apply_bite(
            bite,
            shell,
            context,
            terminal_emitted,
            health,
            bodies,
            commands,
        ),
    }
}

fn apply_bounce(
    bounce: &Bounce,
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) -> f32 {
    let mut damage = 0.0;
    if context.deposit
        && let Ok(mut hp) = health.get_mut(bounce.struck)
    {
        let before = hp.current;
        hp.current = (before - bounce.shock).max(0.0);
        damage += before - hp.current;
    }
    if let Some(owner) = bounce.owner {
        apply_hit_impulse(bodies, owner, bounce.impulse, bounce.position);
    }
    commands.trigger(Impact {
        position: bounce.position,
        normal: bounce.normal,
        caliber: shell.projectile.caliber,
        surface: ImpactSurface::Armor,
        outcome: ImpactOutcome::Ricochet {
            direction: Vec3::from(bounce.direction),
        },
    });
    shell.marks.ricochets.push(bounce.position);
    shell.path.points.push(bounce.position);
    if let Some(shot) = shell.shot
        && !*terminal_emitted
    {
        commands.trigger(ShellRicochet {
            shot: shot.0,
            origin: bounce.position,
            direction: Vec3::from(bounce.direction),
            speed: bounce.speed,
            sequence: (shell.marks.ricochets.len() - 1) as u32,
        });
    }
    damage
}

fn apply_bite(
    bite: &Bite,
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<Forces>,
    commands: &mut Commands,
) -> f32 {
    let mut damage = 0.0;
    // Transit damage: every HP-bearing volume is charged for the material OF ITS OWN the round chewed
    // (§13.2's damage law — no ownership, no priority, no argmax).
    if context.deposit {
        for deposit in &bite.deposits {
            if let Ok(mut hp) = health.get_mut(deposit.entity) {
                let before = hp.current;
                hp.current = (before - deposit.cost * TRANSIT_K).max(0.0);
                damage += before - hp.current;
            }
        }
    }

    let (verdict, position, terminal_at) = match &bite.end {
        BiteEnd::Embedded { at, impulse } => {
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
        BiteEnd::Perforated {
            exit,
            direction,
            speed,
            impulse,
            spall,
            ..
        } => {
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
            // Each source carries its own already-η-weighted budget.
            for (source, budget) in spall {
                damage += throw_spall_burst(
                    shell.spall,
                    *source,
                    *direction,
                    *budget,
                    shell.projectile.caliber,
                    *speed,
                    context.world,
                    health,
                    context.armor,
                    context.deposit,
                );
            }
            (TerminalVerdict::Through, bite.entrance, bite.entrance)
        }
    };

    commands.trigger(Impact {
        position,
        normal: bite.normal,
        caliber: shell.projectile.caliber,
        surface: ImpactSurface::Armor,
        outcome: verdict.into(),
    });
    if let Some(shot) = shell.shot
        && !*terminal_emitted
    {
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
    damage
}
