//! The game's damage model: public, timed paths, sampled without scene queries.
//! The authority exports its immutable stretches; replicas use their clock at the shared cursor.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::mechanism::{Mechanism, SolidShot, SolidShotState};
use super::{Impact, ImpactOutcome, ImpactSurface, Projectile, ShellPath, Shot};
use crate::ShotId;

/// The game's damage model: a constant-resistance stretch and its resolved clock (metres, m/s, s).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Span {
    pub(crate) from: Vec3,
    pub(crate) axis: Dir3,
    pub(crate) length: f64,
    pub(crate) before: f64,
    pub(crate) after: f64,
    pub(crate) seconds: f64,
}

impl Span {
    pub(crate) fn speed_at(&self, round: &SolidShot, fraction: f64) -> f64 {
        round.speed_between(self.before, self.after, fraction)
    }

    pub(crate) fn seconds_at(&self, round: &SolidShot, fraction: f64) -> f64 {
        if fraction <= 0.0 {
            return 0.0;
        }
        if fraction >= 1.0 {
            return self.seconds;
        }
        round.flight_time(
            SolidShotState { speed: self.before },
            SolidShotState {
                speed: self.speed_at(round, fraction),
            },
            self.length * fraction,
        )
    }

    pub(crate) fn at(&self, round: &SolidShot, seconds: f64) -> (Vec3, f32) {
        let fraction = if seconds >= self.seconds {
            1.0
        } else if seconds <= 0.0 {
            0.0
        } else {
            // DERIVED: bisection's interval is narrower than length / 2^48.
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..48 {
                let mid = (lo + hi) * 0.5;
                if self.seconds_at(round, mid) < seconds {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            (lo + hi) * 0.5
        };
        (
            self.from + self.axis * (self.length * fraction) as f32,
            self.speed_at(round, fraction) as f32,
        )
    }
}

/// The game's damage model: the public effect at a path's end, never an armour test on a replica.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PathImpact {
    Bounce {
        at: Vec3,
        normal: Vec3,
        direction: Vec3,
        ordinal: u32,
        feedback: crate::substances::SurfaceFeedback,
    },
    Through {
        at: Vec3,
        normal: Vec3,
        ordinal: u32,
        feedback: crate::substances::SurfaceFeedback,
    },
    Stopped {
        at: Vec3,
        normal: Vec3,
        ordinal: u32,
        feedback: crate::substances::SurfaceFeedback,
    },
    Ground {
        at: Vec3,
        normal: Vec3,
    },
}

/// The game's damage model: one immutable authority path, starting within a simulation tick.
#[derive(Event, Clone, Debug, Serialize, Deserialize)]
pub struct FlightOutcome {
    pub(crate) shot: ShotId,
    pub(crate) body: super::RoundBody,
    pub(crate) drag_k: f32,
    pub(crate) sequence: u32,
    pub(crate) tick: u32,
    pub(crate) fraction: f64,
    pub(crate) spans: Vec<Span>,
    pub(crate) carrier: Vec3,
    pub(crate) resume: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) stopped: bool,
    pub(crate) impact: Option<PathImpact>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutcomeError {
    Clock,
    Geometry,
    Speed,
    Capacity,
}

/// The game's damage model: network data checked once before any replica can sample it.
#[derive(Clone)]
pub(crate) struct ValidatedPath(FlightOutcome);

impl TryFrom<FlightOutcome> for ValidatedPath {
    type Error = OutcomeError;
    fn try_from(path: FlightOutcome) -> Result<Self, Self::Error> {
        path.validate()?;
        Ok(Self(path))
    }
}

impl std::ops::Deref for ValidatedPath {
    type Target = FlightOutcome;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl FlightOutcome {
    pub(crate) fn validate(&self) -> Result<(), OutcomeError> {
        if !self.fraction.is_finite()
            || !(0.0..1.0).contains(&self.fraction)
            || super::elapsed_ticks(self.tick, self.shot.fire_tick).is_none()
        {
            return Err(OutcomeError::Clock);
        }
        if self.spans.len() > super::resolve::MAX_STEP_CROSSINGS {
            return Err(OutcomeError::Capacity);
        }
        if !self.carrier.is_finite() || !self.resume.is_finite() || !self.velocity.is_finite() {
            return Err(OutcomeError::Geometry);
        }
        if !self.body.mass.is_finite()
            || self.body.mass <= 0.0
            || !self.body.caliber.is_finite()
            || self.body.caliber <= 0.0
            || !self.body.nose_factor.is_finite()
            || self.body.nose_factor <= 0.0
            || !self.drag_k.is_finite()
            || self.drag_k < 0.0
        {
            return Err(OutcomeError::Speed);
        }
        let mut duration = 0.0;
        for span in &self.spans {
            if !span.from.is_finite()
                || !span.axis.is_finite()
                || (span.axis.length_squared() - 1.0).abs() > 1.0e-3
                || !span.length.is_finite()
                || span.length <= 0.0
                || span.length > f64::from(f32::MAX)
            {
                return Err(OutcomeError::Geometry);
            }
            if !span.before.is_finite()
                || !span.after.is_finite()
                || span.before <= 0.0
                || span.after < 0.0
                || span.after > span.before
                || span.before > f64::from(f32::MAX)
            {
                return Err(OutcomeError::Speed);
            }
            if !span.seconds.is_finite() || span.seconds <= 0.0 {
                return Err(OutcomeError::Clock);
            }
            duration += span.seconds;
        }
        if !duration.is_finite() {
            return Err(OutcomeError::Clock);
        }
        if let Some(impact) = &self.impact {
            let (at, normal) = match impact {
                PathImpact::Bounce {
                    at,
                    normal,
                    direction,
                    ..
                } => {
                    if Dir3::new(*direction).is_err() {
                        return Err(OutcomeError::Geometry);
                    }
                    (at, normal)
                }
                PathImpact::Through { at, normal, .. }
                | PathImpact::Stopped { at, normal, .. }
                | PathImpact::Ground { at, normal } => (at, normal),
            };
            if !at.is_finite() || Dir3::new(*normal).is_err() {
                return Err(OutcomeError::Geometry);
            }
        }
        Ok(())
    }

    pub(crate) fn seconds(&self) -> f64 {
        self.spans.iter().map(|span| span.seconds).sum()
    }

    fn at(&self, round: &SolidShot, seconds: f64) -> (Vec3, Vec3) {
        let mut left = seconds;
        for span in &self.spans {
            if left < span.seconds {
                let (at, speed) = span.at(round, left);
                return (
                    at + self.carrier * seconds as f32,
                    span.axis * speed + self.carrier,
                );
            }
            left -= span.seconds;
        }
        (self.resume, self.velocity)
    }

    fn impact(&self, style: crate::rounds::ImpactStyle, commands: &mut Commands) {
        let Some(impact) = &self.impact else { return }; // A clear span changes no surface effect.
        let (position, normal, surface, outcome, ordinal) = match *impact {
            PathImpact::Bounce {
                at,
                normal,
                direction,
                ordinal,
                feedback,
            } => (
                at,
                normal,
                ImpactSurface::Material(feedback),
                ImpactOutcome::Ricochet { direction },
                Some(ordinal),
            ),
            PathImpact::Through {
                at,
                normal,
                ordinal,
                feedback,
            } => (
                at,
                normal,
                ImpactSurface::Material(feedback),
                ImpactOutcome::Through,
                Some(ordinal),
            ),
            PathImpact::Stopped {
                at,
                normal,
                ordinal,
                feedback,
            } => (
                at,
                normal,
                ImpactSurface::Material(feedback),
                ImpactOutcome::Stopped,
                Some(ordinal),
            ),
            PathImpact::Ground { at, normal } => (
                at,
                normal,
                ImpactSurface::Terrain,
                ImpactOutcome::Struck,
                None,
            ),
        };
        commands.trigger(Impact {
            position,
            normal,
            style,
            surface,
            outcome,
        });
        if let Some(ordinal) = ordinal {
            commands.trigger(super::AuthorityOutcomeDrawn {
                shot: self.shot,
                ordinal,
            });
        }
    }
}

/// Replica sampling precedes transform propagation and every trail consumer.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PathPresent;

/// The shared presentation cursor, published by the network adapter once per render frame.
#[derive(Resource, Default)]
pub(crate) struct PathClock {
    pub(crate) tick: u32,
    pub(crate) fraction: f64,
}

/// Replica state is constructed from fire data at birth; no loaded asset supplies flight state.
pub(crate) struct Replica {
    fire_tick: u32,
    origin: Vec3,
    velocity: Vec3,
    outcomes: Vec<ValidatedPath>,
    drawn: std::collections::BTreeSet<u32>,
    cache: Option<FreeCache>,
    current: Option<u32>,
    last_age: Option<f64>,
    played: std::collections::BTreeSet<u32>,
}

struct FreeCache {
    source: Option<u32>,
    at: f64,
    axis: super::resolve::AxisStart,
    velocity: Vec3,
    stepped: bool,
}

impl Replica {
    pub(crate) fn born(fire_tick: u32, origin: Vec3, velocity: Vec3) -> Self {
        Self {
            fire_tick,
            origin,
            velocity,
            outcomes: Vec::new(),
            drawn: default(),
            cache: None,
            current: None,
            last_age: None,
            played: default(),
        }
    }

    fn start(&self, outcome: &FlightOutcome) -> f64 {
        f64::from(outcome.tick.wrapping_sub(self.fire_tick)) + outcome.fraction
    }

    fn sample(&mut self, age: f64, dt: f32, drag: f32) -> Result<(Vec3, Vec3, bool), OutcomeError> {
        let latest = self
            .outcomes
            .iter()
            .filter(|outcome| self.start(outcome) <= age)
            .max_by_key(|outcome| outcome.sequence);
        let (source, start, origin, velocity, stepped, drag) = match latest {
            Some(outcome) => {
                let start = self.start(outcome);
                let duration = outcome.seconds();
                let round = super::bound(outcome.body);
                let elapsed = (age - start) * f64::from(dt);
                if elapsed < duration {
                    let (at, velocity) = outcome.at(&round, elapsed);
                    return Ok((at, velocity, false));
                }
                if outcome.stopped {
                    return Ok((outcome.resume, Vec3::ZERO, true));
                }
                (
                    Some(outcome.sequence),
                    start + duration / f64::from(dt),
                    outcome.resume,
                    outcome.velocity,
                    (start + duration / f64::from(dt)).fract() != 0.0,
                    outcome.drag_k,
                )
            }
            None => (None, 0.0, self.origin, self.velocity, false, drag),
        };
        let cache = self.cache.get_or_insert(FreeCache {
            source,
            at: start,
            axis: super::resolve::AxisStart {
                anchor: origin,
                origin: Vec3::ZERO,
            },
            velocity,
            stepped,
        });
        if cache.source != source || cache.at > age {
            *cache = FreeCache {
                source,
                at: start,
                axis: super::resolve::AxisStart {
                    anchor: origin,
                    origin: Vec3::ZERO,
                },
                velocity,
                stepped,
            };
        }
        if age.floor() - cache.at.floor() > f64::from(super::MAX_COSMETIC_CATCH_UP_TICKS) {
            return Err(OutcomeError::Capacity);
        }
        // Cache only tick boundaries. Render-frame partitioning never changes a fixed flight step.
        while cache.at.floor() + 1.0 <= age {
            if !cache.stepped {
                cache.velocity = super::freeflight_step(cache.velocity, drag, dt);
            }
            let next = cache.at.floor() + 1.0;
            cache.axis = cache.axis.rebased();
            cache.axis.origin += cache.velocity * ((next - cache.at) as f32 * dt);
            cache.at = next;
            cache.stepped = false;
        }
        let velocity = if cache.stepped {
            cache.velocity
        } else {
            super::freeflight_step(cache.velocity, drag, dt)
        };
        let axis = cache.axis.rebased();
        Ok((
            axis.anchor + (axis.origin + velocity * ((age - cache.at) as f32 * dt)),
            velocity,
            false,
        ))
    }
}

/// The game's damage model: replicas sample fire and server outcomes at the hulls' cursor.
/// A late outcome is applied on this pass, with no collision query or waiting state.
pub(crate) fn present(
    mut shells: Query<(
        Entity,
        &mut Projectile,
        &mut Transform,
        &mut ShellPath,
        &mut super::PenetrationMarks,
        Option<&Shot>,
    )>,
    clock: Res<PathClock>,
    fixed: Res<Time<Fixed>>,
    outcomes: Option<Res<super::SanctionedShots>>,
    mut trace: Option<ResMut<crate::shot_trace::ShotTrace>>,
    mut commands: Commands,
) {
    'shells: for (entity, mut projectile, mut transform, mut path, mut marks, shot) in &mut shells {
        let drag = projectile.drag_k;
        let style = projectile.impact_style();
        let Some(replica) = &mut projectile.replica else {
            continue;
        }; // Authority shells use playback.
        let Some(ticks) = super::elapsed_ticks(clock.tick, replica.fire_tick) else {
            continue;
        }; // Fire is ahead of a cursor that stepped back.
        let age = f64::from(ticks) + clock.fraction;
        if let Some(buffer) = outcomes.as_deref()
            && let Some(shot) = shot
        {
            for outcome in buffer.paths(shot.0) {
                if !replica
                    .outcomes
                    .iter()
                    .any(|known| known.sequence == outcome.sequence)
                {
                    if replica.outcomes.len() >= super::resolve::MAX_STEP_CROSSINGS {
                        warn!(shot = ?shot.0, "client: public path history exceeded the cosmetic capacity");
                        commands.entity(entity).despawn();
                        continue 'shells;
                    }
                    replica.outcomes.push(outcome.clone());
                }
            }
        }
        replica.outcomes.sort_by_key(|outcome| outcome.sequence);
        let latest = replica
            .outcomes
            .iter()
            .filter(|outcome| replica.start(outcome) <= age)
            .max_by_key(|outcome| outcome.sequence);
        if latest.map(|outcome| outcome.sequence) != replica.current {
            if let Some(outcome) = latest
                && replica
                    .last_age
                    .is_some_and(|last| replica.start(outcome) < last)
            {
                path.begin_segment();
            }
            replica.current = latest.map(|outcome| outcome.sequence);
            if let Some(outcome) = latest
                && replica.played.insert(outcome.sequence)
            {
                crate::shot_trace::record(
                    &mut trace,
                    "path_play",
                    clock.tick,
                    outcome.shot,
                    || serde_json::json!({ "seq": outcome.sequence }),
                );
            }
        }
        replica.last_age = Some(age);
        let dt = fixed.timestep().as_secs_f32();
        for outcome in &replica.outcomes {
            if replica.start(outcome) + outcome.seconds() / f64::from(dt) <= age
                && replica.drawn.insert(outcome.sequence)
            {
                if let Some(PathImpact::Bounce { at, .. }) = outcome.impact {
                    marks.ricochets.push(at);
                    path.points.push(at);
                }
                crate::shot_trace::record(
                    &mut trace,
                    "path_done",
                    clock.tick,
                    outcome.shot,
                    || serde_json::json!({ "seq": outcome.sequence }),
                );
                outcome.impact(style, &mut commands);
            }
        }
        let (at, velocity, stopped) = match replica.sample(age, dt, drag) {
            Ok(sample) => sample,
            Err(error) => {
                warn!(
                    ?error,
                    "client: public path catch-up exceeded the cosmetic work horizon"
                );
                commands.entity(entity).despawn();
                continue;
            }
        };
        transform.translation = at;
        if let Ok(direction) = Dir3::new(velocity) {
            transform.look_to(direction, Vec3::Y);
        }
        if path.points.last().is_none_or(|last| *last != at) {
            path.points.push(at);
        }
        projectile.velocity = velocity;
        if stopped {
            commands.entity(entity).despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ballistics::{SanctionedInsert, SanctionedShots, march_tests::FIXTURE_88};
    use std::time::Duration;

    const DT: f32 = 1.0 / 64.0; // DERIVED from the shipping fixed clock.

    fn shot() -> ShotId {
        ShotId {
            combatant: crate::CombatantId(7),
            weapon: 0,
            fire_tick: 100,
        }
    }

    fn material_path() -> FlightOutcome {
        let round = super::super::bound(FIXTURE_88);
        // Fixture data: a slow, long crossing whose playback spans several ticks.
        let (before, after, length) = (800.0, 200.0, 20.0);
        let seconds = round.flight_time(
            SolidShotState { speed: before },
            SolidShotState { speed: after },
            length,
        );
        FlightOutcome {
            shot: shot(),
            sequence: 0,
            tick: 101,
            fraction: 0.75,
            spans: vec![Span {
                from: Vec3::Z * 10.0,
                axis: Dir3::Z,
                length,
                before,
                after,
                seconds,
            }],
            carrier: Vec3::X * 3.0,
            resume: Vec3::Z * 30.0 + Vec3::X * (3.0 * seconds as f32),
            velocity: Vec3::Z * after as f32 + Vec3::X * 3.0,
            stopped: false,
            impact: None,
            body: FIXTURE_88,
            drag_k: 0.0,
        }
    }

    /// The game's damage model: client playback must be the authority's resolved clock,
    /// including the carrier's motion. Mutant: interpolating speed linearly or flying at entry speed.
    #[test]
    fn replica_uses_the_resolved_material_clock_and_carrier() {
        let outcome = material_path();
        assert_eq!(outcome.validate(), Ok(()));
        let mut replica = Replica::born(100, Vec3::ZERO, Vec3::Z * 800.0);
        replica
            .outcomes
            .push(ValidatedPath::try_from(outcome.clone()).unwrap());
        let round = super::super::bound(FIXTURE_88);
        for share in [0.1, 0.5, 0.9] {
            let seconds = outcome.seconds() * share;
            let (expected, speed) = outcome.spans[0].at(&round, seconds);
            let age = 1.75 + seconds / f64::from(DT);
            let (at, velocity, stopped) = replica.sample(age, DT, 0.0).unwrap();
            assert!(!stopped);
            assert!(at.distance(expected + outcome.carrier * seconds as f32) < 1.0e-4);
            assert!((velocity - (Vec3::Z * speed + outcome.carrier)).length() < 1.0e-3);
        }
    }

    /// The game's damage model: a render-frame partition never changes the fixed free-flight law.
    /// Replicas must preserve both a stalled cursor and integer-tick agreement with fire catch-up.
    #[test]
    fn cursor_partition_and_stalls_preserve_free_flight() {
        // Fixture positions cover the origin and both map-scale signs.
        for origin in [
            Vec3::ZERO,
            Vec3::new(100.0, 200.0, -30.0),
            Vec3::splat(4096.0),
            Vec3::splat(-4096.0),
        ] {
            let velocity = Vec3::new(10.0, 80.0, -800.0);
            let drag = 0.0001; // Fixture drag, per metre.
            let mut replica = Replica::born(100, origin, velocity);
            for frame in 0..=80 {
                let age = f64::from(frame) / 4.0;
                let first = replica.sample(age, DT, drag).unwrap();
                assert_eq!(
                    replica.sample(age, DT, drag).unwrap(),
                    first,
                    "a stalled cursor moves nothing"
                );
                if frame % 4 == 0 {
                    let (expected, _, _) =
                        super::super::fast_forward_shell(origin, velocity, drag, DT, frame / 4);
                    assert_eq!(
                        first.0, expected,
                        "the fire kernel at tick {age}, origin {origin:?}"
                    );
                }
            }
        }
    }

    #[derive(Resource, Default)]
    struct Drawn(usize);
    fn drawn(_impact: On<Impact>, mut log: ResMut<Drawn>) {
        log.0 += 1;
    }

    fn replica_app() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Duration::from_millis(16),
            ))
            .init_resource::<PathClock>()
            .init_resource::<SanctionedShots>()
            .init_resource::<Drawn>()
            .add_observer(drawn)
            .add_systems(Update, present);
        let mut projectile = super::super::march_tests::fixture_round(FIXTURE_88, Vec3::Z * 800.0);
        projectile.replica = Some(Replica::born(100, Vec3::ZERO, Vec3::Z * 800.0));
        let entity = app
            .world_mut()
            .spawn((
                projectile,
                Shot(shot()),
                Transform::default(),
                ShellPath::default(),
                super::super::PenetrationMarks::default(),
            ))
            .id();
        *app.world_mut().resource_mut::<PathClock>() = PathClock {
            tick: 110,
            fraction: 0.25,
        };
        (app, entity)
    }

    /// The game's damage model: a late public path replaces the cosmetic flight on arrival.
    /// No physics plugin or collider query exists in this app; replicas neither hold nor deposit HP.
    /// A perforation continues and a later stop retires, with effects drawn once under duplicates.
    #[test]
    fn late_perforation_continues_and_a_later_stop_retires_without_geometry() {
        let (mut app, entity) = replica_app();
        app.update();
        let free = app.world().get::<Transform>(entity).unwrap().translation;
        let mut through = super::super::path_fixture(shot(), 102);
        through.resume = Vec3::X * 20.0;
        through.impact = Some(PathImpact::Through {
            at: through.resume,
            normal: Vec3::NEG_X,
            ordinal: 0,
            feedback: crate::substances::SurfaceFeedback::fixture(),
        });
        let mut stop = super::super::path_fixture(shot(), 112);
        stop.sequence = 1;
        stop.resume = Vec3::X * 30.0;
        stop.stopped = true;
        stop.impact = Some(PathImpact::Stopped {
            at: stop.resume,
            normal: Vec3::NEG_X,
            ordinal: 0,
            feedback: crate::substances::SurfaceFeedback::fixture(),
        });
        {
            let mut paths = app.world_mut().resource_mut::<SanctionedShots>();
            assert_eq!(paths.insert(stop), SanctionedInsert::Inserted); // Out-of-order arrival.
            assert_eq!(paths.insert(through.clone()), SanctionedInsert::Inserted);
            assert_eq!(paths.insert(through), SanctionedInsert::Duplicate);
        }
        app.update();
        let after = app
            .world()
            .get::<Transform>(entity)
            .expect("perforation keeps flying")
            .translation;
        assert!(
            after.x > 20.0 && after != free,
            "late truth shows on the arrival pass"
        );
        assert_eq!(app.world().resource::<Drawn>().0, 1);
        app.update();
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation,
            after,
            "stalled cursor"
        );
        assert_eq!(app.world().resource::<Drawn>().0, 1, "no duplicate effect");
        app.world_mut().resource_mut::<PathClock>().tick = 112;
        app.update();
        assert!(app.world().get_entity(entity).is_err());
        assert_eq!(app.world().resource::<Drawn>().0, 2);
    }

    /// The game's damage model: even a cosmetic path below the world waits for a server end.
    /// A late outcome must still have a consumer; the replica cannot retire on its free flight.
    #[test]
    fn a_late_server_end_keeps_its_consumer_below_the_world() {
        let (mut app, entity) = replica_app();
        app.world_mut()
            .get_mut::<Projectile>(entity)
            .unwrap()
            .replica = Some(Replica::born(
            100,
            Vec3::Y * (super::super::KILL_FLOOR - 1.0),
            Vec3::NEG_Y,
        ));
        app.update();
        assert!(app.world().get_entity(entity).is_ok());
        let mut end = super::super::path_fixture(shot(), 105);
        end.stopped = true;
        app.world_mut()
            .resource_mut::<SanctionedShots>()
            .insert(end);
        app.update();
        assert!(app.world().get_entity(entity).is_err());
    }

    /// The game's damage model rejects outside path data before its clock is sampled.
    #[test]
    fn non_finite_and_accelerating_spans_are_refused_at_ingress() {
        let mut paths = SanctionedShots::default();
        for bad in [f64::NAN, f64::INFINITY, -1.0, 900.0] {
            let mut outcome = material_path();
            outcome.spans[0].after = bad;
            assert_eq!(
                paths.insert(outcome),
                SanctionedInsert::Refused(OutcomeError::Speed)
            );
        }
        assert!(!paths.has_shot(shot()));
    }
}
