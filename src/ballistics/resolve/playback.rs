//! The game's damage model: an immutable crossing and a cursor through its resolved clock.
//! Geometry, witness lines and cone directions are read once. Playback changes no outcome and
//! performs no spatial query; it delivers each addressed effect when its path moment is reached.

use super::*;
use crate::ballistics::{DoseContext, ShotDose, SpallCone, SpallMarks};

use super::super::path::{FlightOutcome, PathImpact, Span as Stretch};

struct Crossing {
    decision: Decision,
    flight: Flight,
    round: SolidShot,
    path: Vec<Stretch>,
    effects: Vec<(f64, Effect)>,
    seconds: f64,
}

/// The game's damage model converts each immutable dose once, before any tick partition is known.
/// A line lands at its last exposed path point; a field lands at its source's exit. These are
/// point effects, so playback never re-rounds a fraction of a dose on a tick boundary.
enum Effect {
    Mark,
    /// The round reaches a tank's first volume of the crossing ([`PenetrationMarks::reached`]).
    Reach(Entity),
    FuzeEntry {
        force: f64,
        owner: Option<Entity>,
        speed: f32,
        approach: f32,
    },
    Line {
        crossing: VolumeCrossing,
        units: u64,
        context: DoseContext,
        momentum: Vec3,
    },
    Witness {
        crossing: VolumeCrossing,
        incoming: Dir3,
        speed: f32,
    },
    Cone {
        cone: Option<SpallCone>,
        gated: usize,
        doses: Vec<(Entity, u64, DoseContext)>,
    },
    Finish,
}

/// The game's damage model: only this cursor mutates across ticks; the crossing it reads does not.
pub(crate) struct Playback {
    crossing: Crossing,
    /// The context's deposit flag: whether this end's pools take the crossing's doses.
    deposit: bool,
    elapsed: f64,
    next_effect: usize,
    mark: Option<usize>,
    carrier: Vec3,
    frames: Vec<(Entity, crate::damage_cam::record::HullFrame)>,
}

impl Crossing {
    /// The game's damage model: the one clock of a decided crossing, its resolved path
    /// stretch by stretch. Playback spends it and the fuzzer reads it ([`Decision::seconds`]).
    fn of(decision: Decision, flight: &Flight, round: SolidShot) -> Self {
        let mut crossing = Crossing {
            decision,
            flight: flight.clone(),
            round,
            path: Vec::new(),
            effects: Vec::new(),
            seconds: 0.0,
        };
        match crossing.decision.clone() {
            Decision::Clear { travel, leg, .. } => crossing.add_stretch(
                leg.from,
                leg.axis,
                f64::from(travel),
                f64::from(flight.speed),
                f64::from(flight.speed),
                f64::from(travel / flight.speed),
            ),
            Decision::Bounce(bounce) => {
                let approach = f64::from(bounce.travel);
                crossing.add_stretch(
                    bounce.approach.from,
                    bounce.incoming,
                    approach,
                    f64::from(bounce.speed_in),
                    f64::from(bounce.speed_in),
                    approach / f64::from(bounce.speed_in),
                );
            }
            Decision::Bite(bite) => {
                crossing.add_stretch(
                    bite.legs[0].from,
                    bite.incoming,
                    f64::from(bite.approach),
                    f64::from(bite.speed_in),
                    f64::from(bite.speed_in),
                    f64::from(bite.approach / bite.speed_in),
                );
                let transit = &bite.legs[1];
                let (mut at, mut speed) = (0.0, f64::from(bite.speed_in));
                for motion in &bite.motion {
                    let gap = (motion.start - at).max(0.0);
                    crossing.add_stretch(
                        transit.from + transit.axis * at as f32,
                        transit.axis,
                        gap,
                        speed,
                        speed,
                        gap / speed,
                    );
                    crossing.add_stretch(
                        transit.from + transit.axis * motion.start as f32,
                        transit.axis,
                        motion.end - motion.start,
                        motion.speed_in,
                        motion.speed_out,
                        motion.seconds,
                    );
                    at = motion.end;
                    speed = motion.speed_out;
                }
                if matches!(bite.end, BiteEnd::Perforated { .. }) {
                    let suffix = (f64::from(bite.travel - bite.approach) - at).max(0.0);
                    crossing.add_stretch(
                        transit.from + transit.axis * at as f32,
                        transit.axis,
                        suffix,
                        speed,
                        speed,
                        suffix / speed,
                    );
                }
            }
        }
        crossing
    }

    fn add_stretch(
        &mut self,
        from: Vec3,
        axis: Dir3,
        length: f64,
        before: f64,
        after: f64,
        seconds: f64,
    ) {
        if length > 0.0 {
            assert!(
                seconds.is_finite() && seconds > 0.0,
                "a nonempty resolved path has a finite positive clock"
            );
            self.seconds += seconds;
            self.path.push(Stretch {
                from,
                axis,
                length,
                before,
                after,
                seconds,
            });
        }
    }

    fn seconds_to(&self, distance: f64) -> f64 {
        let (mut remaining, mut seconds) = (distance.max(0.0), 0.0);
        for stretch in &self.path {
            if remaining < stretch.length {
                return seconds + stretch.seconds_at(&self.round, remaining / stretch.length);
            }
            seconds += stretch.seconds;
            remaining -= stretch.length;
        }
        self.seconds
    }

    fn point_distance(&self, point: Vec3) -> f64 {
        let (mut prefix, mut nearest) = (0.0, (f32::INFINITY, 0.0));
        for stretch in &self.path {
            let along = (point - stretch.from)
                .dot(*stretch.axis)
                .clamp(0.0, stretch.length as f32);
            let separation = point.distance_squared(stretch.from + stretch.axis * along);
            if separation <= nearest.0 {
                nearest = (separation, prefix + f64::from(along));
            }
            prefix += stretch.length;
        }
        nearest.1
    }

    fn at(&self, seconds: f64) -> Flight {
        let mut flight = self.flight.clone();
        if seconds >= self.seconds {
            flight.follow(self.decision.clone());
            return flight;
        }
        let mut remaining = seconds;
        for stretch in &self.path {
            if remaining < stretch.seconds {
                let (position, speed) = stretch.at(&self.round, remaining);
                flight.frame = flight.frame.transport(*flight.direction, *stretch.axis);
                flight.direction = stretch.axis;
                flight.position = position;
                flight.speed = speed;
                // A point inside a decided crossing is playback, never a fresh discovery start.
                flight.continuation = None;
                return flight;
            }
            remaining -= stretch.seconds;
        }
        panic!("the crossing's path spans its elapsed clock");
    }
}

impl Decision {
    /// The game's damage model: elapsed seconds along the resolved path, from `flight` (the
    /// flight the decision was made on) to its end: the playback clock itself ([`Crossing::of`]),
    /// so the §13.6 gate spends exactly what the runtime does.
    pub(crate) fn seconds(&self, flight: &Flight, round: SolidShot) -> f64 {
        Crossing::of(self.clone(), flight, round).seconds
    }
}

impl Playback {
    pub(crate) fn new(
        decision: Decision,
        flight: &Flight,
        round: SolidShot,
        context: &ResolveContext<'_, '_, '_>,
        admit: &dyn Fn(Entity) -> bool,
        wound: f32,
        is_box: &dyn Fn(Entity) -> bool,
    ) -> Self {
        let mut crossing = Crossing::of(decision, flight, round);
        if let Decision::Bite(bite) = crossing.decision.clone() {
            let entrance = f64::from(bite.approach / bite.speed_in);
            crossing.effects.push((entrance, Effect::Mark));
            // The game's damage model: a crossing shared by overlapping tanks reaches each at
            // its first volume's entry, so a tank's encounter opens on the state it was in then.
            // Never before the mark: the stable sort keeps the mark first at equal moments.
            let mut reaches: Vec<(Entity, f64)> = Vec::new();
            for volume in &bite.volumes {
                let Ok(owner) = context.world.owners.get(volume.volume) else {
                    continue; // Unowned matter belongs to no tank's encounter.
                };
                let at = crossing
                    .seconds_to(crossing.point_distance(volume.entry))
                    .max(entrance);
                match reaches.iter_mut().find(|(tank, _)| *tank == owner.tank()) {
                    Some((_, first)) => *first = first.min(at),
                    None => reaches.push((owner.tank(), at)),
                }
            }
            for (tank, at) in reaches {
                crossing.effects.push((at, Effect::Reach(tank)));
            }
            for dose in &bite.deposits {
                let volume = bite
                    .volumes
                    .iter()
                    .find(|volume| volume.volume == dose.address)
                    .expect("each line dose has a resolved exposed volume span");
                let moment =
                    crossing.seconds_to(f64::from(bite.approach) + bite.volume_ends[&dose.address]);
                crossing.effects.push((
                    moment,
                    Effect::Line {
                        crossing: *volume,
                        units: super::super::energy_units(dose.amount),
                        context: DoseContext::Line {
                            entry: volume.entry,
                            along: *bite.incoming,
                        },
                        momentum: bite
                            .exchanges
                            .iter()
                            .find(|exchange| exchange.address == dose.address)
                            .expect("each line dose reads its resolved physical exchange")
                            .momentum,
                    },
                ));
            }
            for source in &bite.spall {
                let (mut spall, mut dose) = (SpallMarks::default(), ShotDose::default());
                super::super::throw_spall_cone(&mut spall, &mut dose, source, context, is_box);
                let doses = dose
                    .tallies
                    .into_iter()
                    .flat_map(|tally| {
                        tally
                            .deposits
                            .into_iter()
                            .map(move |landed| (tally.address, landed.units, landed.context))
                    })
                    .collect();
                let moment = crossing.seconds_to(f64::from(bite.approach) + source.t);
                crossing.effects.push((
                    moment,
                    Effect::Cone {
                        cone: spall.cones.pop(),
                        gated: spall.gated,
                        doses,
                    },
                ));
            }
        }
        let (witnesses, incoming) =
            super::super::wound::line(context, admit, crossing.decision.legs(), wound)
                .unwrap_or_else(|error| error.panic());
        if let Some(incoming) = incoming {
            for witness in witnesses {
                let moment = crossing.seconds_to(crossing.point_distance(witness.end));
                crossing.effects.push((
                    moment,
                    Effect::Witness {
                        crossing: witness,
                        incoming,
                        speed: flight.speed,
                    },
                ));
            }
        }
        let finish = match &crossing.decision {
            Decision::Bite(bite) => crossing.seconds_to(
                f64::from(bite.approach)
                    + bite
                        .motion
                        .last()
                        .expect("a bite advances a mechanism encounter")
                        .end,
            ),
            Decision::Bounce(bounce) => crossing.seconds_to(f64::from(bounce.travel)),
            Decision::Clear { .. } => crossing.seconds,
        };
        crossing.effects.push((finish, Effect::Finish));
        crossing.effects.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self {
            crossing,
            deposit: context.deposit,
            elapsed: 0.0,
            next_effect: 0,
            mark: None,
            carrier: Vec3::ZERO,
            frames: context.world.hull_frames(),
        }
    }

    pub(crate) fn on_entry(
        &mut self,
        force: f64,
        owner: Option<Entity>,
        speed: f32,
        approach: f32,
    ) {
        self.crossing.effects.insert(
            0,
            (
                0.0,
                Effect::FuzeEntry {
                    force,
                    owner,
                    speed,
                    approach,
                },
            ),
        );
    }

    /// The game's damage model exports the very stretches the authority plays, with no topology.
    pub(crate) fn public_path(
        &self,
        shot: crate::ShotId,
        sequence: u32,
        tick: u32,
        fraction: f64,
        ordinal: u32,
        drag_k: f32,
    ) -> FlightOutcome {
        let world = self.flight_at_end();
        let (stopped, impact) = match self.world_decision() {
            Decision::Clear { .. } => (false, None),
            Decision::Bounce(bounce) => (
                false,
                Some(PathImpact::Bounce {
                    at: bounce.position,
                    normal: bounce.normal,
                    feedback: bounce.feedback,
                    direction: *bounce.direction,
                    ordinal,
                }),
            ),
            Decision::Bite(bite) => match bite.end {
                BiteEnd::Embedded { at, .. } => (
                    true,
                    Some(PathImpact::Stopped {
                        at,
                        normal: bite.normal,
                        feedback: bite.feedback,
                        ordinal,
                    }),
                ),
                BiteEnd::Perforated { .. } => (
                    false,
                    Some(PathImpact::Through {
                        at: bite.entrance,
                        normal: bite.normal,
                        feedback: bite.feedback,
                        ordinal,
                    }),
                ),
            },
        };
        FlightOutcome {
            body: self.crossing.round.body(),
            drag_k,
            shot,
            sequence,
            tick,
            fraction,
            spans: self.crossing.path.clone(),
            carrier: self.carrier,
            resume: world.0,
            velocity: world.1,
            stopped,
            impact,
        }
    }

    fn flight_at_end(&self) -> (Vec3, Vec3) {
        let end = self.crossing.at(self.crossing.seconds);
        (
            end.position + self.carrier * self.crossing.seconds as f32,
            end.direction * end.speed + self.carrier,
        )
    }

    pub(crate) fn seconds(&self) -> f64 {
        self.crossing.seconds
    }

    pub(crate) fn in_moving_frame(&mut self, velocity: Vec3) {
        self.carrier = velocity;
    }

    pub(crate) fn flight(&self) -> Flight {
        let mut flight = self.crossing.at(self.elapsed);
        if self.carrier != Vec3::ZERO {
            flight.position += self.carrier * self.elapsed as f32;
            let velocity = flight.direction * flight.speed + self.carrier;
            flight.speed = velocity.length();
            if let Ok(direction) = Dir3::new(velocity) {
                flight.frame = flight.frame.transport(*flight.direction, *direction);
                flight.direction = direction;
                flight.speed = velocity.length();
            }
            if let Some(interior) = &mut flight.continuation {
                interior.anchor += self.carrier * self.elapsed as f32;
            }
        }
        flight
    }

    fn point(&self, point: Vec3) -> Vec3 {
        if self.carrier == Vec3::ZERO {
            return point;
        }
        point
            + self.carrier
                * self
                    .crossing
                    .seconds_to(self.crossing.point_distance(point)) as f32
    }

    fn volume(&self, mut volume: VolumeCrossing) -> VolumeCrossing {
        volume.entry = self.point(volume.entry);
        volume.end = self.point(volume.end);
        volume
    }

    fn dose_context(&self, context: DoseContext, moment: f64) -> DoseContext {
        match context {
            DoseContext::Line { entry, along } => DoseContext::Line {
                entry: self.point(entry),
                along,
            },
            DoseContext::Field { origin } => DoseContext::Field {
                origin: origin + self.carrier * moment as f32,
            },
        }
    }

    fn world_decision(&self) -> Decision {
        let mut decision = self.crossing.decision.clone();
        if self.carrier == Vec3::ZERO {
            return decision;
        }
        let end = self.crossing.at(self.crossing.seconds);
        let velocity = end.direction * end.speed + self.carrier;
        let direction = Dir3::new(velocity).unwrap_or(end.direction); // At world rest the heading is retained.
        match &mut decision {
            Decision::Clear {
                resume,
                continuation,
                ..
            } => {
                *resume = self.point(*resume);
                continuation.anchor += self.carrier * self.crossing.seconds as f32;
            }
            Decision::Bounce(bounce) => {
                bounce.position = self.point(bounce.position);
                bounce.resume = self.point(bounce.resume);
                bounce.continuation.anchor += self.carrier * self.crossing.seconds as f32;
                bounce.direction = direction;
                bounce.speed = velocity.length();
            }
            Decision::Bite(bite) => {
                bite.entrance = self.point(bite.entrance);
                bite.volumes = bite
                    .volumes
                    .iter()
                    .map(|volume| self.volume(*volume))
                    .collect();
                match &mut bite.end {
                    BiteEnd::Embedded { at, .. } => *at = self.point(*at),
                    BiteEnd::Perforated {
                        exit,
                        resume,
                        direction: out,
                        speed,
                        continuation,
                        ..
                    } => {
                        *exit = self.point(*exit);
                        *resume = self.point(*resume);
                        *out = direction;
                        *speed = velocity.length();
                        continuation.anchor += self.carrier * self.crossing.seconds as f32;
                    }
                }
            }
        }
        decision
    }

    /// The game's damage model delivers due effects without reading geometry again. Returns
    /// seconds spent and whether the immutable crossing is completely played back.
    #[expect(
        clippy::too_many_arguments,
        reason = "playback writes health, forces, presentation and the delivered-line sink"
    )]
    pub(crate) fn advance(
        &mut self,
        seconds: f64,
        shell: &mut MarchingShell,
        terminal_emitted: &mut bool,
        health: &mut Query<&mut ComponentHealth>,
        bodies: &mut Query<Forces>,
        commands: &mut Commands,
        lines: &mut Vec<super::super::DeliveredLine>,
    ) -> (f64, bool, u64) {
        let target = (self.elapsed + seconds).min(self.crossing.seconds);
        let spent = target - self.elapsed;
        let mut removed = 0;
        while let Some((moment, effect)) = self.crossing.effects.get(self.next_effect) {
            if *moment > target {
                break;
            }
            let first_mark = shell.marks.log.len();
            match effect {
                Effect::FuzeEntry {
                    force,
                    owner,
                    speed,
                    approach,
                } => shell
                    .projectile
                    .fuze
                    .entered(*force, *speed, *owner, *approach),
                Effect::Mark => {
                    let Decision::Bite(bite) = self.world_decision() else {
                        panic!("only a bite schedules a crossing mark");
                    };
                    self.mark = Some(shell.marks.log.len());
                    shell.marks.log.push(ArmourMark::Crossing {
                        entrance: bite.entrance,
                        normal: bite.normal,
                        incoming: *bite.incoming,
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
                        cones: 0,
                    });
                }
                Effect::Reach(tank) => shell.marks.reached.push((
                    self.mark
                        .expect("a crossing reaches a tank after its entrance mark"),
                    *tank,
                )),
                Effect::Line {
                    crossing,
                    units,
                    context,
                    momentum,
                } => {
                    lines.push(super::super::DeliveredLine {
                        crossing: self.volume(*crossing),
                        momentum: *momentum,
                    });
                    shell.dose.add_units(
                        crossing.volume,
                        *units,
                        self.dose_context(*context, *moment),
                    );
                    removed += shell.dose.settle(
                        health,
                        self.deposit,
                        self.mark.expect("a transit dose follows its entrance mark"),
                    );
                }
                Effect::Witness {
                    crossing,
                    incoming,
                    speed,
                } => {
                    let crossing = self.volume(*crossing);
                    shell
                        .dose
                        .add_wounds(std::slice::from_ref(&crossing), **incoming);
                    let mark = shell.marks.log.len();
                    shell.marks.log.push(ArmourMark::Wound {
                        incoming: **incoming,
                        speed: *speed,
                        volumes: vec![crossing],
                    });
                    removed += shell.dose.settle(health, self.deposit, mark);
                }
                Effect::Cone { cone, gated, doses } => {
                    for &(address, units, context) in doses {
                        shell
                            .dose
                            .add_units(address, units, self.dose_context(context, *moment));
                    }
                    shell.spall.gated += gated;
                    let mark = self
                        .mark
                        .expect("an exit field follows its crossing's entrance mark");
                    if let Some(cone) = cone {
                        let mut cone = cone.clone();
                        let shift = self.carrier * *moment as f32;
                        cone.origin += shift;
                        for ray in &mut cone.rays {
                            ray.end += shift;
                        }
                        shell.spall.cones.push(cone);
                        let ArmourMark::Crossing { cones, .. } = &mut shell.marks.log[mark] else {
                            panic!("a playback mark remains its own crossing");
                        };
                        *cones += 1;
                    }
                    removed += shell.dose.settle(health, self.deposit, mark);
                }
                Effect::Finish => match &self.world_decision() {
                    Decision::Clear { .. } => {}
                    Decision::Bounce(bounce) => {
                        apply_bounce(bounce, shell, terminal_emitted, bodies, commands);
                        removed +=
                            shell
                                .dose
                                .settle(health, self.deposit, shell.marks.log.len() - 1);
                    }
                    Decision::Bite(bite) => finish_bite(
                        bite,
                        self.mark
                            .expect("a finished bite has its own crossing mark"),
                        shell,
                        terminal_emitted,
                        bodies,
                        commands,
                    ),
                },
            }
            for mark in first_mark..shell.marks.log.len() {
                for &(tank, frame) in &self.frames {
                    shell.marks.frames.insert(
                        (mark, tank),
                        frame.translated(self.carrier * *moment as f32),
                    );
                }
            }
            self.next_effect += 1;
        }
        self.elapsed = target;
        (spent, target == self.crossing.seconds, removed)
    }

    pub(crate) fn into_decision(self) -> Decision {
        self.world_decision()
    }
}
