//! The §13 union walk, driven against the live world — what replaced the serial resolver.
//!
//! The old `resolve_armor_crossing` resolved ONE volume at a time and probed ahead restricted to the
//! struck entity, so an overlapping or exactly-abutting neighbour was entered from inside, its exit
//! probe found nothing, and it was crossed at zero cost (§13.1's table). Nothing here is
//! entity-restricted and nothing defaults to zero: a corridor is collected, the union field is
//! integrated over everything in it, and every failure is loud.
//!
//! # The atomic resolution corridor
//!
//! §13.4's walk is defined over a corridor that CLOSES — one that contains the whole crossing plus
//! enough clear space past it to prove no ε-weld reaches further. So the driver does not stop at the
//! tick's travel budget: it collects from first contact, and if the walk reports
//! [`WalkError::IncompleteCorridor`] it EXTENDS and re-collects, up to the 50 m ceiling the serial
//! resolver's own `PROBE` already set. It never synthesizes the missing exit. That choice is what
//! keeps continuation state out of the resolver — the only thing that survives a crossing is which
//! primitives the disc is still inside, and that is a lookup on rays already walked
//! ([`DiscWalk::resume_at`]), never an inference.
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
    self, Begin, DiscCorridor, DiscFrame, DiscWalk, Outcome, PrimitiveKey, SampleCorridor,
    SampleSeed, Shot, VolumeTable, WalkError, WalkLaws,
};
use super::{
    ArmorCrossing, ComponentHealth, HullShockLedger, Impact, ImpactSurface, MarchingShell,
    PenetrationEvent, ProjectileMarchWorld, ShellRicochet, ShellTerminal, ShockCause,
    apply_hit_impulse, capability, hit_ancestor, speed_for, throw_spall_burst,
};

/// How far past first contact the first corridor reaches. Most crossings close well inside it; the
/// driver extends when they do not, so this is a starting guess, not a limit.
const ENTRANCE_SPAN: f32 = 0.5;

/// Ceiling on corridor extension — carried verbatim from the serial resolver's `PROBE`, which probed
/// this far ahead for a single plate's far face. A crossing that has not closed in 50 m of material
/// is not a crossing.
const MAX_CORRIDOR: f32 = 50.0;

/// Below this calibre the disc degenerates to its axis (`k = 1`).
///
/// The ring sits at `calibre / 2`, so for an MG round it is 4 mm from the axis — inside the geometry
/// tolerance of everything it can meet, and §13.5 already says a fragment IS a shell with `r → 0`.
/// Paying twelve extra casts to sample a 4 mm disc buys nothing, and the machine-gun is the round we
/// fire by the beltful.
pub(crate) const DISC_MIN_CALIBER: f32 = 0.020;

/// Main-penetrator transit damage = the cost it paid crossing that volume × this (design §6).
const TRANSIT_K: f32 = 1.0;

/// Shock a glancing bounce jars into an exposed component: impact energy × squareness. Armour has no
/// HP and shrugs it off.
const SHOCK_K: f32 = 0.045;

/// What one resolved crossing did, and what the march must carry out of it.
pub(crate) struct Crossing {
    pub outcome: ArmorCrossing,
    pub damage: f32,
    /// Line-of-sight metres flown from the corridor origin to where the march resumes, SUMMED over
    /// the segments actually flown.
    ///
    /// The driver cannot reconstruct this and must not try. A crossing is two segments, not one: the
    /// round flies from the corridor origin to the AXIS handoff along the incoming direction, and
    /// from there to the exit along the BENT one. The driver knows only where its cast first touched
    /// armour — the nearest of k sample rays — and at oblique incidence that ray leads the axis by
    /// `r·tan(incidence)`, which is 135 mm for an 88 at 72°. Charging to it skipped the approach and
    /// handed the round that much free travel inside the same tick, which moves subsequent impacts
    /// across tick and REV-25 accounting boundaries.
    ///
    /// The disc lift a ricochet resumes on is deliberately NOT counted: it is a repositioning of the
    /// body clear of the face it bounced off, not distance the round flew.
    pub travel: f32,
    /// Which primitives each disc sample is still inside where the march resumes.
    ///
    /// Empty unless the round perforated. This is the ONLY state a crossing leaves behind, and it
    /// exists because the alternative is guessing: a shell that punches an outer plate while one ring
    /// sample is already inside the crewman behind it must resume knowing that, or the next corridor
    /// reports an exit it never entered.
    pub seeds: Vec<SampleSeed>,
    /// Where the march must resume, when the contact point itself is not far enough clear.
    ///
    /// A bounce is the case that needs it. The point model could resume one boundary nudge along the
    /// outgoing ray and be done; a DISC cannot, because at oblique incidence its lateral offsets lie
    /// mostly ALONG the struck surface's normal — after a 75° deflection, half the ring is still
    /// tens of millimetres behind the face. First contact would fire again immediately, on the very
    /// surface the round just left. So a ricochet lifts the shell clear along `n̄` by the disc's own
    /// radius: the same statement the 1 mm nudge makes for a point, sized for a body.
    pub resume: Option<Vec3>,
}

/// Everything the driver borrows from the march. Grouped so the phase takes one parameter rather
/// than nine, exactly as [`ProjectileMarchWorld`] groups the queries.
pub(crate) struct ResolveContext<'a, 'w, 's> {
    pub world: &'a ProjectileMarchWorld<'w, 's>,
    pub colliders: &'a Query<'w, 's, (&'static Position, &'static Rotation, &'static Collider)>,
    pub armor: &'a SpatialQueryFilter,
    /// Authority = not a replica: only then does a crossing mutate health.
    pub deposit: bool,
    pub laws: WalkLaws,
}

/// Resolve one crossing of the union field, from first contact through to the plan's consequences.
#[expect(
    clippy::too_many_arguments,
    reason = "the march's phase seam: shell, world, flight state and sinks are each irreducible"
)]
pub(crate) fn resolve_crossing(
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    // Corridor origin — the shell's position, nudged off whatever face it is leaving.
    origin: Vec3,
    dir: Dir3,
    speed: f32,
    // Distance from `origin` to the nearest sample's first armour contact.
    contact: f32,
    // Which primitives each sample is already inside (empty in open air).
    seeds: &[SampleSeed],
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
    not_own: &dyn Fn(Entity) -> bool,
    commands: &mut Commands,
) -> Result<Crossing, WalkError> {
    let caliber = shell.projectile.caliber;
    let radius = if caliber >= DISC_MIN_CALIBER {
        caliber * 0.5
    } else {
        0.0
    };
    let frame = shell.projectile.disc;

    // --- ENTRANCE: the disc as it meets the surface, along the incoming axis.
    //
    // The march's contact point becomes the corridor ANCHOR, and every position from here to the end
    // of the crossing is measured from it. The transit corridor inherits the same anchor, which is
    // what makes its handoff a small offset from the face positions the entrance measured rather
    // than an independently rounded world position (see `walk::RayCorridor::anchor`).
    let anchor = origin;
    let entrance = closing_walk(
        context,
        not_own,
        anchor,
        Vec3::ZERO,
        Vec3::from(dir),
        frame,
        radius,
        seeds,
        contact + ENTRANCE_SPAN,
    )?;
    let shot = Shot {
        caliber,
        capability: capability(shell.projectile.mass, speed),
    };

    match walk::begin(&entrance, &shot, &context.laws) {
        // First contact said there was armour here; the disc, which is the resolution, says
        // otherwise. No fabricated event (§13.6) — and no re-probing of ground already covered: the
        // round flies past the WHOLE corridor just examined, because that corridor is exactly the
        // span now known to be empty. Advancing only to the contact would re-detect it and creep
        // forward one boundary nudge at a time.
        Begin::Miss => {
            let examined = contact + ENTRANCE_SPAN;
            Ok(Crossing {
                outcome: ArmorCrossing::Perforated {
                    exit: anchor + dir * examined,
                    direction: dir,
                    speed,
                },
                damage: 0.0,
                travel: examined,
                seeds: Vec::new(),
                resume: None,
            })
        }
        Begin::Ricochet {
            entrance: read,
            direction,
            speed_scale,
        } => {
            let event = &entrance.events[0];
            Ok(ricochet(
                shell,
                context,
                anchor,
                read.position,
                read.normal,
                read.incidence,
                event.entrance_volume,
                dir,
                speed,
                direction,
                speed_scale,
                radius,
                terminal_emitted,
                health,
                bodies,
                commands,
            ))
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
            Ok(perforate_or_embed(
                shell,
                context,
                &transit,
                &plan,
                dir,
                bent,
                speed,
                terminal_emitted,
                health,
                bodies,
                commands,
            ))
        }
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
    reason = "private kernel of `resolve_crossing`; every argument is corridor geometry"
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
                let settled = walked
                    .events
                    .first()
                    .is_none_or(|event| event.end + context.laws.weld_max_lookahead <= length);
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
        let seeded: &[PrimitiveKey] = seeds.get(index).map_or(&[], |seed| &seed.inside);
        let mut hits = Vec::new();
        collect::collect(
            &Corridor {
                anchor,
                origin: origin + offset,
                axis,
                length,
                seeded,
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

/// The bounce: deflect off `n̄`, no entry, no spall (§4). The deflection ANGLE and the bleed both
/// already carry η from the core (§13.5, ruled 2026-08-07) — a graze is a partial ricochet in
/// direction as well as in speed.
#[expect(
    clippy::too_many_arguments,
    reason = "one emission site; the alternative is a struct used exactly once"
)]
fn ricochet(
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    // The corridor's world anchor; `position` and the geometry around it are relative to it.
    anchor: Vec3,
    // Struck face, RELATIVE to the anchor.
    local_position: Vec3,
    normal: Vec3,
    incidence: f32,
    struck: Entity,
    incoming: Dir3,
    speed: f32,
    direction: Vec3,
    speed_scale: f32,
    radius: f32,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
    commands: &mut Commands,
) -> Crossing {
    let mut damage = 0.0;
    let position = anchor + local_position;
    let out = Dir3::new(direction).unwrap_or(incoming);
    let bled = speed * speed_scale;
    let v_in = Vec3::from(incoming) * speed;

    // Even a deflected hit jars an exposed component — scaled by impact energy and by how square
    // the graze was. Armour has no HP, so it shrugs the bounce off.
    if context.deposit
        && let Ok(mut hp) = health.get_mut(struck)
    {
        let shock = SHOCK_K * capability(shell.projectile.mass, speed) * incidence.cos();
        let before = hp.current;
        hp.current = (before - shock).max(0.0);
        damage += before - hp.current;
    }

    let body = context
        .world
        .owners
        .get(struck)
        .ok()
        .map(|owner| owner.tank());
    let victim = body.and_then(|body| context.world.combatants.get(body).ok().copied());
    if let Some(body) = body {
        apply_hit_impulse(
            bodies,
            body,
            shell.projectile.mass * (v_in - Vec3::from(out) * bled),
            position,
            ShockCause::Ricochet,
        );
    }

    commands.trigger(Impact {
        position,
        normal,
        caliber: shell.projectile.caliber,
        surface: ImpactSurface::Armor,
        penetrated: false,
        deflection: Some(Vec3::from(out)),
        authority: None,
    });
    shell.marks.ricochets.push(position);
    shell.path.points.push(position);
    if let Some(shot) = shell.shot
        && !*terminal_emitted
    {
        commands.trigger(ShellRicochet {
            shot: shot.0,
            origin: position,
            direction: Vec3::from(out),
            speed: bled,
            sequence: (shell.marks.ricochets.len() - 1) as u32,
            victim,
        });
    }

    Crossing {
        outcome: ArmorCrossing::Ricochet {
            direction: out,
            speed: bled,
        },
        damage,
        // To the face it bounced off, along the way in. The lift the resume point carries is not
        // travel — see [`Crossing::travel`].
        travel: local_position.dot(Vec3::from(incoming)).max(0.0),
        seeds: Vec::new(),
        resume: Some(position + normal * radius),
    }
}

/// The bite: spend the plan's cost, deposit per-presence damage, throw the plan's spall, and either
/// stop or come out the far side.
#[expect(
    clippy::too_many_arguments,
    reason = "one emission site; the alternative is a struct used exactly once"
)]
fn perforate_or_embed(
    shell: &mut MarchingShell,
    context: &ResolveContext<'_, '_, '_>,
    transit: &DiscWalk,
    plan: &walk::ResolutionPlan,
    incoming: Dir3,
    bent: Dir3,
    speed: f32,
    terminal_emitted: &mut bool,
    health: &mut Query<&mut ComponentHealth>,
    bodies: &mut Query<(
        Forces,
        Option<&mut crate::track::sim::TrackGripWake>,
        Option<&mut HullShockLedger>,
    )>,
    commands: &mut Commands,
) -> Crossing {
    let mut damage = 0.0;
    let plan_entrance = plan.entrance;
    // THE BOUNDARY. Everything the walk reports is anchor-relative; everything leaving this function
    // — impacts, marks, impulse application points, the march's own resume — is world. One place to
    // convert, so no other code has to know which frame it is holding.
    let world = |local: Vec3| transit.anchor + local;
    let entrance_position = world(plan_entrance.position);
    let struck = transit.events[0].entrance_volume;
    let body = context
        .world
        .owners
        .get(struck)
        .ok()
        .map(|owner| owner.tank());
    let victim = body.and_then(|body| context.world.combatants.get(body).ok().copied());
    let v_in = Vec3::from(incoming) * speed;

    // Transit damage: every HP-bearing volume is charged for the material OF ITS OWN the round chewed
    // (§13.2's damage law — no ownership, no priority, no argmax). The plan already clipped every
    // chord at the embed progress, so a round that died in the plate deposits nothing behind it.
    if context.deposit {
        for deposit in &plan.deposits {
            if let Ok(mut hp) = health.get_mut(deposit.entity) {
                let before = hp.current;
                hp.current = (before - deposit.cost * TRANSIT_K).max(0.0);
                damage += before - hp.current;
            }
        }
    }

    // The APPROACH: corridor origin to the axis handoff, along the way in. The transit's own `t`
    // then measures the rest, along the bend. Two segments, and the driver is told their sum rather
    // than left to guess it from a contact distance that belongs to a different ray.
    let approach = transit.origin.dot(Vec3::from(incoming)).max(0.0);

    let (outcome, terminal_at, seeds, flown) = match plan.outcome {
        Outcome::Embedded { at, t } => {
            let at = world(at);
            shell.marks.events.push(PenetrationEvent {
                entry: entrance_position,
                exit: at,
                overmatched: plan_entrance.overmatched,
            });
            shell.path.points.push(at);
            // Stopped: the entrance surface's body absorbs the whole remaining momentum.
            if let Some(body) = body {
                apply_hit_impulse(
                    bodies,
                    body,
                    shell.projectile.mass * v_in,
                    at,
                    ShockCause::Embed,
                );
            }
            (ArmorCrossing::Embedded { at }, at, Vec::new(), approach + t)
        }
        Outcome::Perforated { exit, t, .. } => {
            let exit = world(exit);
            let residual = speed_for(shell.projectile.mass, {
                let capability = capability(shell.projectile.mass, speed);
                (capability - plan.cost_spent).max(0.0)
            });
            // The body keeps the momentum the shell lost crossing it; the shell carries the rest on.
            if let Some(body) = body {
                apply_hit_impulse(
                    bodies,
                    body,
                    shell.projectile.mass * (v_in - Vec3::from(bent) * residual),
                    entrance_position,
                    ShockCause::Perforation,
                );
            }
            shell.marks.events.push(PenetrationEvent {
                entry: entrance_position,
                exit,
                overmatched: plan_entrance.overmatched,
            });
            shell.path.points.push(exit);

            // Spall at every downward field step the round reached, including the exit (§13.2's
            // field law; the "one spall per welded run" reading was superseded 2026-08-07). Each
            // mark carries its own already-η-weighted budget.
            for mark in &plan.spall {
                damage += throw_spall_burst(
                    shell.spall,
                    world(mark.position),
                    bent,
                    mark.budget,
                    shell.projectile.caliber,
                    residual,
                    context.world,
                    health,
                    context.armor,
                    context.deposit,
                );
            }

            // Resume where the WHOLE disc is past the crossing, and carry what any sample is still
            // inside. Resuming at the mean exit would leave the trailing samples in material with
            // nobody to tell the next corridor about it.
            // Read at the point the NEXT corridor will actually start from — the march nudges every
            // cast origin off the face it is leaving, and a seed taken one nudge earlier describes a
            // plate the round has already cleared.
            let seeds = transit.resume_at(t + super::MARCH_EPS, &context.laws);
            (
                ArmorCrossing::Perforated {
                    exit: world(transit.origin + transit.axis * t),
                    direction: bent,
                    speed: residual,
                },
                entrance_position,
                seeds,
                approach + t,
            )
        }
    };

    // The struck FACE reads at the entrance, where the round punched in — the one place "penetrated"
    // is unambiguously true, and (for an embed) the visible surface, since the embed point is inside
    // the steel.
    commands.trigger(Impact {
        position: match plan.outcome {
            Outcome::Embedded { at, .. } => world(at),
            Outcome::Perforated { .. } => entrance_position,
        },
        normal: plan_entrance.normal,
        caliber: shell.projectile.caliber,
        surface: ImpactSurface::Armor,
        penetrated: true,
        deflection: None,
        authority: None,
    });
    if let Some(shot) = shell.shot
        && !*terminal_emitted
    {
        *terminal_emitted = true;
        shell.terminal_report.0 = true;
        commands.trigger(ShellTerminal {
            shot: shot.0,
            position: terminal_at,
            normal: plan_entrance.normal,
            penetrated: true,
            after_bounces: shell.marks.ricochets.len() as u32,
            victim,
        });
    }

    Crossing {
        outcome,
        damage,
        travel: flown,
        seeds,
        resume: None,
    }
}
