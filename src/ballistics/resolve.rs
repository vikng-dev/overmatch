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
//! gate observes the decisions and applies nothing.
//!
//! # Fail closed, never free
//!
//! On any structured [`WalkError`] the round STOPS at the contact it was resolving: no perforation,
//! no spall, no transit damage. A wrong answer here is free penetration, and free penetration is
//! indistinguishable from armour that was never modelled — the defect class §13 exists to kill.
//! Crashing a multiplayer authority is too strong a response; a stopped shell and a rate-limited
//! warning is not.

use avian3d::prelude::{Collider, Forces, Position, Rotation, SpatialQueryFilter};
use bevy::prelude::*;

use super::collect::{self, Corridor};
use super::walk::{
    self, Begin, DiscCorridor, DiscFrame, DiscWalk, Outcome, SampleCorridor, SampleSeed, ShellKey,
    Shot, VolumeTable, WalkError, WalkLaws,
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

/// The resume point of a perforation and the seeds read there, in ONE anchor frame.
///
/// INVARIANT: the next corridor starts at exactly the point the seeds were read at. Both are
/// anchor-relative — `origin` is `transit.origin + axis · (t + MARCH_EPS)` in the transit's own
/// frame, `seeds` are [`DiscWalk::resume_at`] that same progress — so they agree to corridor-scale
/// rounding wherever on the map the shot is. A resume point rebuilt in world f32 is quantized at
/// the world's spacing (2.44e-4 m past 2048 m, DERIVED), far wider than the topology band the seeds
/// were judged in, and an entry face that close to the plane is then owned by the seed AND
/// re-collected ahead of the origin, or by nobody. The flight carries this unchanged into the next
/// crossing and converts to world only for its cast and for presentation.
#[derive(Clone, Debug)]
pub(crate) struct Interior {
    /// WORLD anchor of the corridor the seeds were read on.
    pub anchor: Vec3,
    /// The resume point, RELATIVE to [`anchor`](Self::anchor).
    pub origin: Vec3,
    pub seeds: Vec<SampleSeed>,
}

impl Interior {
    /// Some sample resumes inside material. The next crossing must then be resolved from these seeds
    /// before the flight may stop marching: the material's exit lies ahead with its entry behind,
    /// and only the seeds say the sample is inside.
    pub(crate) fn holds_material(&self) -> bool {
        self.seeds.iter().any(|seed| !seed.inside.is_empty())
    }
}

/// The state successive crossings read and advance: where the round is, where it is going, and the
/// sampling frame and continuation it carries out of the last crossing.
///
/// The runtime march and the §13.6 gate drive the SAME state through the same [`decide`] and
/// [`Flight::follow`], so a crossing sequence is one state machine wherever it is run.
#[derive(Clone, Debug)]
pub(crate) struct Flight {
    /// WORLD position the next cast starts from (one [`MARCH_EPS`] along `direction` from here).
    pub position: Vec3,
    pub direction: Dir3,
    /// m/s.
    pub speed: f32,
    /// The sampling basis for `direction`, transported through every bend so far. Resolution reads
    /// THIS, never a basis stored for an earlier heading.
    pub frame: DiscFrame,
    /// The last perforation's resume point and seeds, anchor-relative; `None` in open air.
    pub continuation: Option<Interior>,
}

/// What [`Flight::follow`] moved the flight by.
pub(crate) struct Followed {
    /// Line-of-sight metres charged against the travel budget, including the boundary nudge.
    pub travel: f32,
    /// The round came to rest.
    pub stopped: bool,
}

impl Flight {
    /// WORLD origin of the next cast and of a corridor that has no continuation to resume from: the
    /// position, nudged off whatever face the flight is leaving.
    pub(crate) fn cast_origin(&self) -> Vec3 {
        self.position + self.direction * MARCH_EPS
    }

    /// A continuation holds a sample inside material ([`Interior::holds_material`]).
    pub(crate) fn holds_material(&self) -> bool {
        self.continuation
            .as_ref()
            .is_some_and(Interior::holds_material)
    }

    /// Advance the flight along a decision: the state the next crossing starts from. The sampling
    /// frame is transported through every direction change, including the null one.
    pub(crate) fn follow(&mut self, decision: Decision) -> Followed {
        let nudge = MARCH_EPS;
        match decision {
            Decision::Clear { resume, travel } => {
                self.frame = self
                    .frame
                    .transport(Vec3::from(self.direction), Vec3::from(self.direction));
                self.position = resume;
                self.continuation = None;
                Followed {
                    travel: nudge + travel,
                    stopped: false,
                }
            }
            Decision::Bounce(bounce) => {
                self.frame = self
                    .frame
                    .transport(Vec3::from(self.direction), Vec3::from(bounce.direction));
                self.direction = bounce.direction;
                self.speed = bounce.speed;
                self.position = bounce.resume;
                self.continuation = None;
                Followed {
                    travel: nudge + bounce.travel,
                    stopped: false,
                }
            }
            Decision::Bite(bite) => match bite.end {
                BiteEnd::Embedded { at, .. } => {
                    self.position = at;
                    self.continuation = None;
                    Followed {
                        travel: nudge + bite.travel,
                        stopped: true,
                    }
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
                    Followed {
                        travel: nudge + bite.travel,
                        stopped: false,
                    }
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
        /// The far end of the examined corridor, where the flight resumes.
        resume: Vec3,
        /// The examined corridor's length.
        travel: f32,
    },
    /// Too oblique and not overmatched: deflected off the face — no entry, no spall.
    Bounce(Bounce),
    /// It bit in: stopped inside the material or came out the far side.
    Bite(Bite),
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
    /// Line-of-sight metres flown to the face, along the way in. The lift to `resume` is not travel:
    /// it repositions the body clear of the face it bounced off.
    pub travel: f32,
    /// Where the flight resumes: lifted clear of the face along `n̄` by the disc's own radius. At
    /// oblique incidence a disc's lateral offsets lie mostly ALONG the struck surface's normal, so
    /// resuming at the contact point leaves half the ring behind the face and first contact fires
    /// again on the very surface just left. A point needs only the nudge; a body needs its radius.
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
    /// Line-of-sight metres flown, summed over the segments actually flown: the approach to the
    /// AXIS hand-off along the way in, then the transit along the bend.
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
        /// The axis at the exit, where the flight resumes.
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
/// `contact` is the distance from [`Flight::cast_origin`] to the nearest sample's first armour
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
    // The cast origin becomes the corridor ANCHOR, and every position from here to the end of the
    // crossing is measured from it. The transit corridor inherits the same anchor, which is what
    // makes its handoff a small offset from the face positions the entrance measured rather than an
    // independently rounded world position (see `walk::RayCorridor::anchor`). Resuming from a
    // perforation keeps that perforation's anchor, for the same reason (see [`Interior`]).
    let (anchor, start, seeds) = match &flight.continuation {
        Some(interior) => (interior.anchor, interior.origin, interior.seeds.as_slice()),
        None => (flight.cast_origin(), Vec3::ZERO, &[][..]),
    };
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
                travel: (read.position - start).dot(Vec3::from(dir)).max(0.0),
                resume: position + read.normal * radius,
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
                start,
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
    // Where the entrance corridor started, RELATIVE to the transit's anchor.
    start: Vec3,
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
    // The APPROACH: corridor origin to the axis handoff, along the way in. The transit's own `t`
    // then measures the rest, along the bend.
    let approach = (transit.origin - start).dot(Vec3::from(incoming)).max(0.0);
    let entrance = world(plan.entrance.position);
    let (end, t) = match plan.outcome {
        Outcome::Embedded { at, t } => (
            BiteEnd::Embedded {
                at: world(at),
                impulse: body.mass * v_in,
            },
            t,
        ),
        Outcome::Perforated { exit, t, .. } => {
            let residual = speed_for(body, (capability_in - plan.cost_spent).max(0.0));
            // Read at the point the NEXT corridor will actually start from — the flight nudges every
            // cast origin off the face it is leaving, and a seed taken one nudge earlier describes a
            // plate the round has already cleared.
            let resume = t + MARCH_EPS;
            (
                BiteEnd::Perforated {
                    exit: world(exit),
                    resume: world(transit.origin + transit.axis * t),
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
                t,
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
        travel: approach + t,
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
    let candidates = candidates(context, not_own, anchor + origin, axis, length, radius);
    // Sample 0 is the axis, by the core's contract. When the march is resuming inside material the
    // offsets come from the SEEDS, which carry each sample's own resume point; otherwise the disc is
    // freshly laid out on the transported frame.
    let offsets: Vec<Vec3> = if seeds.is_empty() {
        if radius > 0.0 {
            walk::disc_offsets(&frame, radius, walk::DEFAULT_RING)
        } else {
            vec![Vec3::ZERO]
        }
    } else {
        seeds.iter().map(|seed| seed.offset).collect()
    };

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
/// Nothing here decides anything: every quantity comes out of the [`Decision`], so the §13.6 gate,
/// which observes the same decisions and applies none of this, reads the outcome the runtime acts on.
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
