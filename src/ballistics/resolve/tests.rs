//! The driver's two-stage crossing — the hand-off from the entrance corridor to the transit
//! corridor counts every entry exactly once — and the one mechanism: the runtime march and the
//! §13.6 gate's driver decide the same crossings.

use avian3d::prelude::{CollisionLayers, LayerMask, Position, RigidBody, Rotation};
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::ballistics::march_tests::{box_trimesh, world_with_trimesh_plates};
use crate::ballistics::walk::FaceHit;
use crate::ballistics::{BallisticVolume, RoundBody, bound, capability};

/// Fixture round and plate: an 88-class disc against 50 mm of steel. Fixture data; nothing below
/// depends on whether it perforates.
const ROUND: RoundBody = RoundBody {
    mass: 10.2,
    caliber: 0.088,
    nose_factor: 7.0 / 24.0,
};
const SPEED: f32 = 800.0;
const PLATE: Vec3 = Vec3::new(3.0, 3.0, 0.05);
/// Steel: reference-mm per metre.
const STEEL: f32 = 1000.0;
/// Where the march segment starts, short of first contact — the anchor's distance from the
/// hand-off, which sizes the seed's "at the restart" band (DERIVED: 5.2e-6 m at 10.6 m).
const STANDOFF: f32 = 10.585;
/// Plate orientations per map position, and shots per plate.
const PLATES: usize = 12;
const SHOTS: usize = 12;

/// The first boundary each sample's transit corridor closes is owned exactly once: by the seed
/// (declared presence) or by one entry crossing in the corridor before that exit.
fn assert_single_ownership(transit: &DiscCorridor, pose: &str) {
    for (index, sample) in transit.samples.iter().enumerate() {
        let mut hits: Vec<&FaceHit> = sample.hits.iter().collect();
        hits.sort_by(|a, b| a.t.total_cmp(&b.t));
        let Some(at) = hits
            .iter()
            .position(|hit| hit.true_normal.dot(transit.axis) > 0.0)
        else {
            continue;
        };
        let exit = hits[at];
        let key = ShellKey {
            volume: exit.volume,
            primitive: exit.primitive,
            shell: exit.shell,
        };
        let entries = hits[..at]
            .iter()
            .filter(|hit| {
                hit.volume == key.volume
                    && hit.primitive == key.primitive
                    && hit.shell == key.shell
                    && hit.true_normal.dot(transit.axis) < 0.0
            })
            .count();
        let seeded = usize::from(sample.initial_presence.contains(&key));
        assert_eq!(
            entries + seeded,
            1,
            "{pose}, sample {index}: the entry of {key:?} is owned by the seed {seeded} time(s) \
             and by the corridor {entries} time(s)",
        );
    }
}

/// THE HAND-OFF PARTITION, END TO END, ON GENERIC GEOMETRY.
///
/// Each sample hands off on its own entry face, so the transit corridor re-measures that face within
/// rounding of its origin, and whether the seed or the corridor owns it is decided inside the
/// topology band — the place a dropped entry (`UnexpectedExit`) or a doubled one
/// (`UnexpectedEntry`) lives. A plate in general orientation, struck at up to 60° anywhere on its
/// face, near the world origin and far from it, from the march's own stand-off before contact.
/// Every pose the round bites into must hand off with every sample's entry owned once and the
/// transit corridor pairing; the terminal outcome is not asserted.
#[test]
fn every_entry_is_owned_once_across_the_hand_off() {
    let mut transits = 0;
    // A fixed pseudo-random sequence: general orientations, never axis-aligned, so the plate's
    // faces are rotated in every collider-local coordinate the crossing arithmetic rounds in.
    let mut state = 0x9e37_79b9_u32;
    let mut unit = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    for far in [
        Vec3::ZERO,
        Vec3::new(37.3, 0.0, -21.9),
        Vec3::new(1234.5, 0.0, 987.6),
    ] {
        for _ in 0..PLATES {
            let mut app = world_with_trimesh_plates(&[]);
            let centre = far + Vec3::new(unit() - 0.5, 2.0 + unit() - 0.5, unit() - 0.5);
            let rotation = Quat::from_euler(
                EulerRot::YXZ,
                std::f32::consts::TAU * unit(),
                std::f32::consts::PI * (unit() - 0.5),
                std::f32::consts::TAU * unit(),
            );
            app.world_mut().spawn((
                Transform::from_translation(centre).with_rotation(rotation),
                RigidBody::Static,
                box_trimesh(PLATE),
                CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "RHA".to_string(),
                },
            ));
            for _ in 0..8 {
                app.update();
            }
            // The plate's +Z face, struck at up to 60° from its normal.
            let normal = rotation * Vec3::Z;
            let (side, up) = normal.any_orthonormal_pair();
            for _ in 0..SHOTS {
                let tilt = Quat::from_axis_angle(
                    (side * (unit() - 0.5) + up * (unit() - 0.5)).normalize_or(side),
                    60.0_f32.to_radians() * unit(),
                );
                let direction = tilt * -normal;
                let point = centre
                    + normal * PLATE.z * 0.5
                    + (side * (unit() - 0.5) + up * (unit() - 0.5)) * 1.5;
                let origin = point - direction * STANDOFF;
                let pose =
                    format!("plate {rotation} at {centre}, shot from {origin} along {direction}");
                let result = handed_off(&mut app, origin, direction);
                match result {
                    Ok(Some(corridor)) => {
                        assert_single_ownership(&corridor, &pose);
                        transits += 1;
                    }
                    Ok(None) => {}
                    Err(error) => panic!("{pose}: the hand-off failed to pair: {error:?}"),
                }
            }
        }
    }
    assert!(
        transits >= 3 * PLATES * SHOTS / 2,
        "the sweep exercised the hand-off ({transits} transits)"
    );
}

/// The runtime's two-stage crossing from `origin` along `direction`, at the march's stand-off: the
/// entrance corridor, the transit starts, the transit corridor (returned for the ownership check)
/// and the transit's pairing. `None` when the entrance does not bite.
fn handed_off(
    app: &mut App,
    origin: Vec3,
    direction: Vec3,
) -> Result<Option<DiscCorridor>, WalkError> {
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                // The game's damage model: exercise the production driver and inspect ownership.
                // Membership belongs to each reached start, not to the incoming corridor origin.
                let mut flight = Flight {
                    position: origin,
                    direction: Dir3::new(direction).expect("a real direction"),
                    speed: SPEED,
                    frame: DiscFrame::anchored(direction).expect("a real direction"),
                    continuation: None,
                    membership: Membership::Required,
                };
                flight.settle(&context, &|_| true, disc_radius(ROUND.caliber))?;
                decide(
                    &context,
                    &|_| true,
                    &bound(ROUND),
                    ROUND.mass,
                    crate::rounds::ContactPreset::fixture(),
                    &flight,
                    STANDOFF,
                )?;
                let frame = DiscFrame::anchored(direction).expect("a real direction");
                let radius = ROUND.caliber * 0.5;
                let entrance = closing_walk(
                    &context,
                    &|_| true,
                    origin,
                    Vec3::ZERO,
                    direction,
                    frame,
                    radius,
                    &[],
                    STANDOFF + ENTRANCE_SPAN,
                )?;
                let shot = Shot {
                    caliber: ROUND.caliber,
                    speed: SPEED,
                    contact: crate::rounds::ContactPreset::fixture(),
                };
                let Begin::Transit(request) = walk::begin(&entrance, &shot, &context.laws)? else {
                    return Ok(None);
                };
                let starts = transit_starts(&context, &|_| true, &request)?;
                let corridor = build_corridor(
                    &context,
                    &|_| true,
                    request.anchor,
                    request.origin,
                    *request.axis,
                    ENTRANCE_SPAN,
                    request.frame,
                    request.radius,
                    &starts.seeds,
                )?;
                let transit = closing_walk(
                    &context,
                    &|_| true,
                    request.anchor,
                    request.origin,
                    *request.axis,
                    request.frame,
                    request.radius,
                    &starts.seeds,
                    ENTRANCE_SPAN,
                )?;
                let round = bound(ROUND);
                walk::finish(
                    &transit,
                    &request,
                    &round,
                    round.at(SPEED),
                    &LIMITS,
                    round.body().mass,
                )?;
                Ok::<_, WalkError>(Some(corridor))
            },
        )
        .expect("the probe system runs")
}

// ---------------------------------------------------------------------------------------------
// One mechanism: the runtime march and the §13.6 gate's driver decide the same crossings
// ---------------------------------------------------------------------------------------------

use crate::ballistics::fuzz::{self, Drive};
use crate::ballistics::march_tests::{FIXTURE_88, FIXTURE_MG, spawn_round};
use crate::ballistics::{
    ComponentHealth, RetainSpentShells, SpallMarks, Verdict, fixture_drag_k, freeflight_step,
};

/// The fixture tick the plate worlds step by (`march_tests::plate_world`'s manual 16 ms).
const DT: f32 = 0.016;
/// Where every shot of this section starts: straight above the geometry, falling, so gravity and
/// drag change the speed but never the direction, and the gate's straight flight is the runtime's.
const DROP_FROM: Vec3 = Vec3::new(0.1, 3.0, -0.2);

/// The top of a wide target 6.5 m below a plate at `y = 1`, where a partly covered
/// round lands after its first crossing. Fixture data.
const FAR_BELOW: f32 = -5.5;

/// Every impact the runtime fired, as `(verdict, position)`.
#[derive(Resource, Default)]
struct Read(Vec<(Verdict, Vec3)>);

fn read_impact(impact: On<Impact>, mut read: ResMut<Read>) {
    read.0.push((impact.outcome.verdict(), impact.position));
}

/// A static trimesh volume of `factor` reference-mm per metre, optionally carrying HP.
fn volume(app: &mut App, size: Vec3, at: Transform, factor: f32, hp: Option<u32>) -> Entity {
    volume_of(app, size, at, walk::VolumeMaterial::fixture(factor), hp)
}

/// A static trimesh volume of `material`, optionally carrying HP.
fn volume_of(
    app: &mut App,
    size: Vec3,
    at: Transform,
    material: walk::VolumeMaterial,
    hp: Option<u32>,
) -> Entity {
    let mut entity = app.world_mut().spawn((
        at,
        RigidBody::Static,
        box_trimesh(size),
        CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
        BallisticVolume {
            material,
            substance: "fixture".to_string(),
        },
    ));
    if let Some(hp) = hp {
        entity.insert(ComponentHealth::full(hp));
    }
    entity.id()
}

/// The speed and travel budget the runtime march flies its first tick with, from the shared advance.
fn first_tick(body: RoundBody, speed: f32) -> (f32, f32) {
    let stepped = freeflight_step(
        Vec3::NEG_Y * speed,
        fixture_drag_k(body.caliber, body.mass),
        DT,
    );
    let speed = stepped.length();
    (speed, speed * DT)
}

/// The gate's side: the same shot, through [`fuzz::drive`], for the runtime's first-tick budget.
fn driven(app: &mut App, body: RoundBody, speed: f32) -> Drive {
    try_driven(app, body, speed)
        .unwrap_or_else(|failure| panic!("the driven flight failed to walk: {failure:?}"))
}

/// [`driven`], with the driver's failure returned rather than raised.
fn try_driven(app: &mut App, body: RoundBody, speed: f32) -> Result<Drive, fuzz::DriveFailure> {
    try_driven_with_contact(app, body, speed, crate::rounds::ContactPreset::fixture())
}

fn try_driven_with_contact(
    app: &mut App,
    body: RoundBody,
    speed: f32,
    preset: crate::rounds::ContactPreset,
) -> Result<Drive, fuzz::DriveFailure> {
    let (speed, budget) = first_tick(body, speed);
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                fuzz::drive(
                    &context,
                    body,
                    preset,
                    speed,
                    DROP_FROM,
                    Vec3::NEG_Y,
                    budget,
                )
            },
        )
        .expect("the driver system runs")
}

/// The runtime's side: one tick of the real march, read back as impacts, and whether any direction
/// of a spall cone the round threw dosed a box.
fn marched(app: &mut App, body: RoundBody, speed: f32) -> (Vec<(Verdict, Vec3)>, bool, Entity) {
    let shell = spawn_round(app, body, DROP_FROM, Vec3::NEG_Y * speed);
    app.update();
    let field = app.world().get::<SpallMarks>(shell).is_some_and(|marks| {
        marks
            .cones
            .iter()
            .flat_map(|cone| &cone.rays)
            .any(|ray| !ray.doses.is_empty())
    });
    (app.world().resource::<Read>().0.clone(), field, shell)
}

/// What the runtime turns each decision into: one impact per bounce or bite, none for a clear.
fn impacts_of(drive: &Drive) -> Vec<(Verdict, Vec3)> {
    drive
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Clear { .. } => None,
            Decision::Bounce(bounce) => Some((Verdict::Ricochet, bounce.position)),
            Decision::Bite(bite) => Some(match bite.end {
                BiteEnd::Embedded { at, .. } => (Verdict::Stopped, at),
                BiteEnd::Perforated { .. } => (Verdict::Through, bite.entrance),
            }),
        })
        .collect()
}

/// One arrangement, run both ways in two identical worlds.
struct Agreement {
    /// The impacts both sides read, in order.
    impacts: Vec<(Verdict, Vec3)>,
    /// GEOMETRIC REACH: some crossing the gate decided put a positive TRANSIT chord into the target.
    reached: bool,
    /// OBSERVABLE ADMISSION: the transit doses the gate decided for the target, summed in order and
    /// converted by the runtime's own arithmetic, remove HP. A positive chord whose dose rounds to
    /// no HP point is reach without admission.
    admitted: bool,
    /// The runtime removed HP from the target, by any route.
    damaged: bool,
    /// A spall cone the runtime threw dosed a box — a route the admission does not observe.
    field: bool,
    /// The round bounced off an HP-bearing volume and jarred it — a route the admission does not
    /// observe either.
    shock: bool,
}

impl Agreement {
    /// The runtime's only HP route was transit, the one the admission observes.
    fn transit_only(&self) -> bool {
        !self.field && !self.shock
    }
}

/// Build the arrangement twice, fly `body` through the runtime in one and through the gate's driver
/// in the other, and require that they agree on every impact, and — where transit was the runtime's
/// only HP route ([`Agreement::transit_only`]) — on whether the target was admitted.
fn agree(name: &str, body: RoundBody, speed: f32, build: impl Fn(&mut App) -> Entity) -> Agreement {
    let mut runtime = world_with_trimesh_plates(&[]);
    runtime
        .insert_resource(RetainSpentShells(true))
        .init_resource::<Read>()
        .add_observer(read_impact);
    let target = build(&mut runtime);
    for _ in 0..8 {
        runtime.update();
    }
    runtime.world_mut().resource_mut::<Read>().0.clear();
    let (read, field, shell) = marched(&mut runtime, body, speed);
    let runtime_transit: f32 = runtime
        .world()
        .get::<crate::ballistics::PenetrationMarks>(shell)
        .unwrap()
        .log
        .iter()
        .filter_map(|mark| match mark {
            ArmourMark::Crossing { volumes, .. } => Some(volumes),
            _ => None,
        })
        .flatten()
        .filter(|volume| volume.volume == target)
        .map(|volume| volume.energy)
        .sum();
    let hp = runtime
        .world()
        .get::<ComponentHealth>(target)
        .expect("the target carries HP");
    let damaged = hp.held() < hp.capacity();

    let mut gate = world_with_trimesh_plates(&[]);
    let gate_target = build(&mut gate);
    for _ in 0..8 {
        gate.update();
    }
    let flown = driven(&mut gate, body, speed);
    let decided = impacts_of(&flown);
    let reached = flown.reaches(gate_target);
    let pool = gate
        .world()
        .get::<ComponentHealth>(gate_target)
        .expect("the target carries HP")
        .held();
    let transit = flown
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Bite(bite) => Some(&bite.deposits),
            _ => None,
        })
        .flatten()
        .map(|line| &line.dose)
        .filter(|dose| dose.address == gate_target)
        .fold(0.0, |total, dose| total + dose.amount);
    let admitted =
        pool > 0 && crate::ballistics::pool_units_of(crate::ballistics::energy_units(transit)) > 0;
    assert_eq!(
        crate::ballistics::pool_units_of(crate::ballistics::energy_units(runtime_transit)) > 0,
        admitted,
        "{name}: the runtime and gate must agree on transit independently of field dose",
    );
    let shock = flown.decisions.iter().any(|decision| {
        matches!(decision, Decision::Bounce(bounce)
            if gate.world().get::<ComponentHealth>(bounce.struck).is_some())
    });

    assert_eq!(
        read.len(),
        decided.len(),
        "{name}: the runtime read {read:?}, the gate decided {decided:?}",
    );
    for ((verdict, at), (decided_verdict, decided_at)) in read.iter().zip(&decided) {
        assert!(
            verdict == decided_verdict && at.distance(*decided_at) < 1.0e-4,
            "{name}: the runtime read {read:?}, the gate decided {decided:?}",
        );
    }
    let agreement = Agreement {
        impacts: read,
        reached,
        admitted,
        damaged,
        field,
        shock,
    };
    assert!(
        !admitted || damaged,
        "{name}: the gate admitted a transit chord into the target and the runtime removed no HP",
    );
    if agreement.transit_only() {
        assert_eq!(
            damaged, admitted,
            "{name}: the runtime's transit damaged the target: {damaged}; the gate admitted it: \
             {admitted}",
        );
    }
    agreement
}

/// A soft HP-bearing box `width` across, centred under the drop line with its top at `top`: the
/// target every arrangement aims at. Fixture data.
///
/// Ownership is attribution only; plate-exit fields may also reach this target. Transit parity is
/// checked from the recorded line doses, independently of those fields.
fn target_below(app: &mut App, top: f32, width: f32) -> Entity {
    let target = volume(
        app,
        Vec3::new(width, 0.4, width),
        Transform::from_translation(Vec3::new(DROP_FROM.x, top - 0.2, DROP_FROM.z)),
        STEEL * 0.1,
        Some(1000),
    );
    let tank = app.world_mut().spawn_empty().id();
    app.world_mut()
        .entity_mut(target)
        .insert(crate::damage::VolumeOf(tank));
    target
}

/// A plate of `thickness` whose normal is tilted `tilt` from `+Y` about `+Z`, its top face through
/// the drop line at height `at`.
fn tilted_plate(app: &mut App, thickness: f32, tilt: f32, at: f32, factor: f32) -> Entity {
    let rotation = Quat::from_rotation_z(tilt);
    let normal = rotation * Vec3::Y;
    let on_line = Vec3::new(DROP_FROM.x, at, DROP_FROM.z);
    volume(
        app,
        Vec3::new(3.0, thickness, 3.0),
        Transform::from_translation(on_line - normal * thickness * 0.5).with_rotation(rotation),
        factor,
        None,
    )
}

/// STOP, BOUNCE, PARTIAL COVERAGE, CONTINUATION AND TARGET REACH, ONE MECHANISM.
///
/// The gate admits a route through exactly the crossings the runtime would make: the same entrance
/// rules, stops, bounces and continuations, decided by the same state machine. Each
/// arrangement is generic geometry with a soft HP-bearing target below it; the round falls straight
/// down so the runtime's gravity and drag leave its direction alone, and the gate flies the
/// runtime's own first-tick speed and budget.
///
/// THE PARITY IS TRANSIT PARITY. The admission observes transit deposits; the runtime also removes
/// HP by a bounce's shock and by spall cones. Compare line doses independently of those fields;
/// ownership supplies no isolation. The last two arrangements keep the round's own path clear of
/// the HP-bearing box:
/// a round that bounces off the box itself jars it ([`Bounce::shock`]), and a perforation's spall
/// cone reaches a box beside the round's path. Both sides still agree on every impact; the box is
/// damaged and not admitted. Fixture data.
#[test]
fn the_runtime_and_the_gate_decide_the_same_crossings() {
    // Stop: a plate thicker than the round can defeat.
    let stopped = agree("stop", FIXTURE_88, 800.0, |app| {
        tilted_plate(app, 0.6, 0.0, 1.0, STEEL);
        target_below(app, 0.0, 2.0)
    });
    assert!(stopped.transit_only());
    assert_eq!(
        stopped.impacts.iter().map(|i| i.0).collect::<Vec<_>>(),
        [Verdict::Stopped],
    );

    // Fixture: a pointed round's normal potency would defeat this chord, but contact pressure
    // makes it skid. The slide passes beside the narrow target below.
    let pointed = RoundBody {
        nose_factor: crate::spec::NoseShape::Ogive { crh: 6.0 }.factor(),
        ..FIXTURE_MG
    };
    let (speed, _) = first_tick(pointed, 755.0);
    assert!(
        0.003 / 75.0_f32.to_radians().cos() * STEEL < capability(pointed, speed),
        "the straight chord alone would admit the target"
    );
    let bounced = agree("bounce", pointed, 755.0, |app| {
        tilted_plate(app, 0.003, 75.0_f32.to_radians(), 1.0, STEEL);
        target_below(app, 0.0, 0.3)
    });
    assert_eq!(
        bounced.impacts.iter().map(|i| i.0).collect::<Vec<_>>(),
        [Verdict::Ricochet],
    );
    assert!(
        !bounced.admitted,
        "a bounced round never reaches the target"
    );
    assert!(bounced.transit_only(), "armour has no HP to jar");

    // Partial coverage: a plate whose edge only part of an 88's ring overlaps; the axis misses it.
    let partial = agree("partial coverage", FIXTURE_88, 800.0, |app| {
        volume(
            app,
            Vec3::new(1.0, 0.02, 1.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x + 0.5 + 0.02, 1.0, DROP_FROM.z)),
            STEEL,
            None,
        );
        target_below(app, FAR_BELOW, 6.0)
    });
    assert!(
        partial.impacts.first().is_some_and(|i| i.0.entered()),
        "the ring bit into the plate edge: {:?}",
        partial.impacts,
    );

    // Continuation: a thin plate, then a volume tilted so its entry does not weld to the plate's
    // exit, entered half a nudge before the resumption plane — the perforation's seed owns it.
    let continued = agree("continuation", FIXTURE_MG, 755.0, |app| {
        let tilt = 65.0_f32.to_radians();
        let exit = 1.0 - 0.0025;
        let resume = exit - MARCH_EPS;
        let entry = resume + 5.0e-4;
        let centre = resume - 0.12;
        let half = (entry - centre) * tilt.cos();
        volume(
            app,
            Vec3::new(1.0, 0.005, 1.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z)),
            STEEL,
            None,
        );
        volume(
            app,
            Vec3::new(1.0, half * 2.0, 1.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, centre, DROP_FROM.z))
                .with_rotation(Quat::from_rotation_z(tilt)),
            STEEL * 0.01,
            None,
        );
        target_below(app, 0.0, 2.0)
    });
    assert!(
        continued
            .impacts
            .first()
            .is_some_and(|i| i.0 == Verdict::Through),
        "the plate is perforated: {:?}",
        continued.impacts,
    );

    // Target reach: a skin too thin to stop the round, the target behind it.
    let reached = agree("target reach", FIXTURE_88, 800.0, |app| {
        tilted_plate(app, 0.002, 0.0, 1.0, STEEL);
        target_below(app, 0.0, 2.0)
    });
    assert!(
        reached.admitted,
        "the round reaches the target behind the skin"
    );

    // Shock: a machine-gun round grazes an HP-bearing soft slab at 75°, too thick for it to
    // overmatch.
    let shocked = agree("shock", FIXTURE_MG, 755.0, |app| {
        let tilt = 75.0_f32.to_radians();
        let rotation = Quat::from_rotation_z(tilt);
        let normal = rotation * Vec3::Y;
        volume(
            app,
            Vec3::new(3.0, 0.4, 3.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z) - normal * 0.2)
                .with_rotation(rotation),
            STEEL * 0.1,
            Some(1000),
        )
    });
    assert_eq!(
        shocked.impacts.iter().map(|i| i.0).collect::<Vec<_>>(),
        [Verdict::Ricochet],
    );
    assert!(
        shocked.shock && shocked.damaged && !shocked.admitted,
        "the bounce jars the box it bounced off, and that is not a transit route",
    );

    // Field: the fixture round perforates the plate; an HP-bearing box sits 0.3 m below the plate, beside the disc's path but inside the spall cone.
    let thrown = agree("field", FIXTURE_88, 800.0, |app| {
        volume(
            app,
            Vec3::new(2.0, 0.06, 2.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 0.97, DROP_FROM.z)),
            STEEL,
            None,
        );
        volume(
            app,
            Vec3::new(0.5, 0.2, 0.6),
            Transform::from_translation(Vec3::new(DROP_FROM.x + 0.06 + 0.25, 0.54, DROP_FROM.z)),
            STEEL * 0.1,
            Some(1000),
        )
    });
    assert_eq!(
        thrown.impacts.first().map(|i| i.0),
        Some(Verdict::Through),
        "the plate is perforated: {:?}",
        thrown.impacts,
    );
    assert!(
        thrown.field && thrown.damaged && !thrown.admitted,
        "the cone reaches the box beside the path, and that is not a transit route: field \
         {}, damaged {}, admitted {}",
        thrown.field,
        thrown.damaged,
        thrown.admitted,
    );
}

/// A ROUND WHOSE BUDGET FALLS SHORT OF A CROSSING REACHES NOTHING BEHIND.
///
/// The crossing asks one readout step more than the round carries: the round comes to rest inside
/// it, short of the far face, with no chord into the target. The gate must agree. Fixture: a
/// one-sample round, a metre of material whose factor is one f32 step above the round's
/// readout per metre, faces at exactly representable heights. A crossing costing exactly the
/// readout is [`an_exact_budget_crossing_terminates_before_the_target`]; one asking exactly the
/// round's whole potential is the mechanism's
/// (`mechanism::tests::spending_exactly_the_potential_leaves_the_round_at_rest_at_the_far_face`):
/// the round's state is its speed, and no f32 factor states that potential to the bit.
#[test]
fn a_budget_short_of_the_crossing_terminates_before_the_target() {
    let stepped = freeflight_step(
        Vec3::NEG_Y * 755.0,
        fixture_drag_k(FIXTURE_MG.caliber, FIXTURE_MG.mass),
        DT,
    );
    assert_eq!(
        Dir3::new(stepped).map(Vec3::from),
        Ok(Vec3::NEG_Y),
        "the fall is exactly vertical, so the chord is exactly the metre"
    );
    let (speed, _) = first_tick(FIXTURE_MG, 755.0);
    // One f32 step above the round's readout: the metre asks for more than the round carries,
    // however the f64 state rounds.
    let factor = capability(FIXTURE_MG, speed).next_up();
    let short = agree("one step short", FIXTURE_MG, 755.0, |app| {
        volume(
            app,
            Vec3::new(1.0, 1.0, 1.0),
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
            factor,
            None,
        );
        target_below(app, 0.0, 2.0)
    });
    assert_eq!(
        short.impacts.iter().map(|i| i.0).collect::<Vec<_>>(),
        [Verdict::Stopped],
        "stopped inside the metre, short of its far face",
    );
    assert!(!short.admitted, "no chord into the target");
    assert!(short.transit_only());
}

/// The game's damage model: a sub-resolution residual cannot reach a distant target before
/// its clock allows it. The readout rounds down, so this fixture leaves the material with a
/// positive residual; a distance budget would spend initial-speed metres at that tiny speed.
/// The runtime and the probe driver must both stop this tick short of the target. Fixture data:
/// a one-sample round, one metre of material, a target behind it and representable face heights.
#[test]
fn an_exact_budget_crossing_terminates_before_the_target() {
    let stepped = freeflight_step(
        Vec3::NEG_Y * 755.0,
        fixture_drag_k(FIXTURE_MG.caliber, FIXTURE_MG.mass),
        DT,
    );
    assert_eq!(
        Dir3::new(stepped).map(Vec3::from),
        Ok(Vec3::NEG_Y),
        "the fall is exactly vertical, so the chord is exactly the metre"
    );
    let (speed, _) = first_tick(FIXTURE_MG, 755.0);
    let factor = capability(FIXTURE_MG, speed);
    let exact = agree("exact budget", FIXTURE_MG, 755.0, |app| {
        volume(
            app,
            Vec3::new(1.0, 1.0, 1.0),
            Transform::from_translation(Vec3::new(0.0, 1.0, 0.0)),
            factor,
            None,
        );
        target_below(app, 0.0, 2.0)
    });
    assert_eq!(
        exact.impacts.iter().map(|i| i.0).collect::<Vec<_>>(),
        [Verdict::Through],
        "the residual has not had time to reach the target",
    );
    assert!(
        !exact.reached,
        "a positive residual does not grant instantaneous travel to the target"
    );
    assert!(
        !exact.admitted,
        "the target takes no transit dose; a field may still reach it: admitted {}, damaged {}",
        exact.admitted, exact.damaged,
    );
}

// ---------------------------------------------------------------------------------------------
// The step's limits: a drain never runs unbounded past the budget
// ---------------------------------------------------------------------------------------------

/// A box whose bottom face is sheared to `slope` (rise per metre of local `+X`), as a certified
/// trimesh: one closed shell, outward-wound, the box's own corner ids welded. Fixture geometry for an
/// exit face that is not opposed to the next entry, so the two never ε-weld.
fn wedge_trimesh(half: Vec3, slope: f32) -> (Collider, BallisticSurfaces) {
    mapped_box_trimesh(|corner| {
        let sheared = if corner.y < 0.0 {
            slope * corner.x * half.x
        } else {
            0.0
        };
        Vec3::new(
            corner.x * half.x,
            corner.y * half.y + sheared,
            corner.z * half.z,
        )
    })
}

/// A closed outward-wound trimesh: `box_trimesh`'s cube with corners at `±1`, each mapped by
/// `place`, and the cube's own certificate. `place` must be affine with a positive determinant, so
/// the cube's winding, closure and welded corner ids carry over unchanged.
fn mapped_box_trimesh(place: impl Fn(Vec3) -> Vec3) -> (Collider, BallisticSurfaces) {
    let (_, surfaces) = box_trimesh(Vec3::ONE);
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
    .map(|(x, y, z): (f32, f32, f32)| place(Vec3::new(x, y, z)))
    .collect();
    // `box_trimesh`'s own winding, so `surfaces` (built for it) certifies this mesh too.
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
    (Collider::trimesh(vertices, indices), surfaces)
}

/// A CHAIN OF HELD CONTINUATIONS STOPS AT THE STEP'S LIMITS.
///
/// Each crossing is bounded; their succession is not, unless the driver bounds it. A chain of soft
/// volumes, each entered less than a nudge after the previous one's exit, leaves every
/// perforation's resume point inside the next volume, so the flight holds material after every
/// crossing and the step drains on past its budget, applying consequences ever further downrange.
/// The game's damage model must panic at the crossing a limit refuses in the runtime march
/// and in the gate's driver alike. Here the count trips first: a held link must be longer than a
/// nudge, and every corridor must close in a gap shorter than one, which pins the period near a
/// nudge (DERIVED below), so the worlds' crossing ceiling ([`StepCrossings`]) is reached
/// millimetres downrange. The immutable batch is refused before delivering any consequence.
///
/// Generic geometry, a single-sample round falling straight down the
/// chain: square entry faces, exit faces sheared 65° (not opposed to the
/// next entry, so no ε-weld), and the chain starting three periods short of the budget's end.
/// Fixture data throughout, the ceiling of 10 crossings included.
#[test]
fn a_chain_of_held_continuations_stops_at_the_step_s_limits() {
    const CEILING: usize = 10;
    let body = FIXTURE_MG;
    assert!(body.caliber < DISC_MIN_CALIBER, "a one-sample round");
    let span = ENTRANCE_SPAN as f64;
    let nudge = MARCH_EPS as f64;
    let gap = 0.5 * nudge;
    // A held crossing resumes one nudge past the previous exit and reads its contact at its own
    // volume's exit. Its entrance corridor then ends `span` past that exit, and its transit corridor
    // `span` past the resume point; both must end in a gap, so for some `m`
    // `m·period ∈ (span − gap, span)` and `(m + 1)·period ∈ (span + nudge − gap, span + nudge)`.
    let m = 400.0;
    let period = 0.5 * ((span - 0.5 * gap) / m + (span + nudge - 0.5 * gap) / (m + 1.0));
    assert!(
        m * period > span - gap
            && m * period < span
            && (m + 1.0) * period > span + nudge - gap
            && (m + 1.0) * period < span + nudge
            && period > nudge,
        "the period closes both corridors in a gap and keeps each link held: {period}",
    );
    // The first volume is entered fresh; both its corridors end `span` past its top, which must
    // land in a gap: `first + (m − 1)·period = span − gap/2`.
    let first = span - 0.5 * gap - (m - 1.0) * period;
    let slope = 65.0_f64.to_radians().tan();
    let (_, budget) = first_tick(body, 755.0);
    let top = DROP_FROM.y as f64 - budget as f64 + 3.0 * period;
    let links = CEILING + 10;
    // The ray crosses each wedge at local `x = lean`; its half-width keeps the sheared bottom below
    // the top everywhere.
    let lean = 1.0e-3;
    let build = |app: &mut App| {
        for index in 0..links {
            let (at, along) = if index == 0 {
                (top, first)
            } else {
                (
                    top - first - gap - (index - 1) as f64 * period,
                    period - gap,
                )
            };
            let half_y = 0.5 * (along + slope * lean);
            let half_x = 0.5 * (lean + 2.0 * half_y / slope);
            app.world_mut().spawn((
                Transform::from_translation(Vec3::new(
                    DROP_FROM.x - lean as f32,
                    (at - half_y) as f32,
                    DROP_FROM.z,
                )),
                RigidBody::Static,
                wedge_trimesh(
                    Vec3::new(half_x as f32, half_y as f32, 2.0e-3),
                    slope as f32,
                ),
                CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL * 1.0e-5),
                    substance: "fixture".to_string(),
                },
            ));
        }
    };

    let mut runtime = world_with_trimesh_plates(&[]);
    runtime
        .insert_resource(StepCrossings(CEILING))
        .init_resource::<Read>()
        .add_observer(read_impact);
    build(&mut runtime);
    for _ in 0..8 {
        runtime.update();
    }
    runtime.world_mut().resource_mut::<Read>().0.clear();
    spawn_round(&mut runtime, body, DROP_FROM, Vec3::NEG_Y * 755.0);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| runtime.update()))
        .expect_err("the game's damage model panics at the crossing limit");
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("a diagnostic panic");
    assert!(
        message.contains("a finite step fits its crossing limit"),
        "{message}"
    );
    let read = runtime.world().resource::<Read>().0.clone();
    assert_eq!(
        read.iter()
            .filter(|(verdict, _)| *verdict == Verdict::Through)
            .count(),
        0,
        "the refused immutable batch applies no consequences before the invariant panic"
    );

    let mut gate = world_with_trimesh_plates(&[]);
    gate.insert_resource(StepCrossings(CEILING));
    build(&mut gate);
    for _ in 0..8 {
        gate.update();
    }
    let failure = try_driven(&mut gate, body, 755.0)
        .expect_err("the gate's driver stops the chain at the same limit");
    assert!(
        matches!(
            failure.error,
            WalkError::DrainExhausted { crossings, .. } if crossings == CEILING + 1
        ),
        "the driver names the exhausted count: {failure:?}",
    );
}

/// THE STEP REFUSES THE CROSSING PAST ITS COUNT, before anything is charged.
#[test]
fn a_step_refuses_the_crossing_past_its_count() {
    let mut budget = Budget::new(1.0, MAX_STEP_CROSSINGS);
    for _ in 0..MAX_STEP_CROSSINGS {
        budget.charge().expect("within the count");
    }
    assert_eq!(
        budget.charge(),
        Err(WalkError::DrainExhausted {
            crossings: MAX_STEP_CROSSINGS + 1,
        }),
    );
    assert_eq!(
        budget.remaining(),
        1.0,
        "a refused crossing charges nothing"
    );
}

// ---------------------------------------------------------------------------------------------
// A fresh start's membership is established, never declared
// ---------------------------------------------------------------------------------------------

/// A static certified trimesh volume of steel, posed by `at`.
fn mesh_volume(app: &mut App, mesh: (Collider, BallisticSurfaces), at: Transform) -> Entity {
    app.world_mut()
        .spawn((
            at,
            RigidBody::Static,
            mesh,
            CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "fixture".to_string(),
            },
        ))
        .id()
}

/// A world holding only what `build` spawns, settled so its poses and bounds are live.
fn settled(build: impl FnOnce(&mut App) -> Vec<Entity>) -> (App, Vec<Entity>) {
    let mut app = world_with_trimesh_plates(&[]);
    let built = build(&mut app);
    for _ in 0..8 {
        app.update();
    }
    (app, built)
}

/// A fresh disc of `radius` laid at `at` and certified along `swept` — the certification's own
/// result — and, when it certifies, the walk of the first corridor swept from its starts along
/// `launch`.
fn certified_then_resumed(
    app: &mut App,
    at: Vec3,
    swept: Vec3,
    launch: Vec3,
    radius: f32,
) -> Result<(Interior, Result<DiscWalk, WalkError>), WalkError> {
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                let frame = DiscFrame::anchored(swept).expect("a real direction");
                let mut flight = Flight::launched(
                    Dir3::new(swept).expect("a direction"),
                    Dir3::new(launch).expect("a direction"),
                    SPEED,
                    frame,
                    Interior::laid(at, layout(&frame, radius)),
                );
                flight.continuation = Some(certified(
                    &context,
                    &|_| true,
                    at,
                    Dir3::new(swept).expect("a direction"),
                    frame,
                    radius,
                )?);
                let starts = flight
                    .continuation
                    .clone()
                    .expect("a launched flight carries its starts");
                let walked = closing_walk(
                    &context,
                    &|_| true,
                    starts.anchor,
                    starts.origin,
                    Vec3::from(flight.direction),
                    flight.frame,
                    radius,
                    &starts.seeds,
                    ENTRANCE_SPAN,
                );
                Ok((starts, walked))
            },
        )
        .expect("the probe system runs")
}

/// [`certified_then_resumed`] where both the certificate and the resumed corridor must succeed.
fn resumed(app: &mut App, at: Vec3, swept: Vec3, launch: Vec3, radius: f32) -> Interior {
    let (starts, walked) = certified_then_resumed(app, at, swept, launch, radius)
        .unwrap_or_else(|error| panic!("the start failed to certify: {error:?}"));
    walked.unwrap_or_else(|error| panic!("the corridor resumed along {launch} failed: {error:?}"));
    starts
}

/// The volumes a seed declares its sample inside, sorted.
fn inside_volumes(seed: &SampleSeed) -> Vec<Entity> {
    let mut volumes: Vec<Entity> = seed.inside.iter().map(|key| key.volume).collect();
    volumes.sort();
    volumes
}

/// Fixture data, from a DERIVED counterexample: a closed prism extruding the parallelogram
/// `(−3,−4), (3,4), (3,5), (−3,−3)` through `z ∈ [−0.5, 0.5]` m at the identity pose. Its lower
/// face is `y = 4x/3`; the start `(0, 2⁻²⁶, 0)` lies just inside it.
fn grazed_prism(app: &mut App) -> Vec<Entity> {
    let prism = mapped_box_trimesh(|corner| {
        Vec3::new(
            3.0 * corner.x,
            4.0 * corner.x + 0.5 * corner.y + 0.5,
            0.5 * corner.z,
        )
    });
    vec![mesh_volume(app, prism, Transform::IDENTITY)]
}

/// The grazed prism's start and its certification direction, which runs along the lower face.
const GRAZED_START: Vec3 = Vec3::new(0.0, 1.0 / 67_108_864.0, 0.0);
const GRAZED_SWEPT: Vec3 = Vec3::new(0.6, 0.8, 0.0);

/// A START IS CERTIFIED ON THE RAY ITS RESUMED CORRIDOR CASTS, OR NOT AT ALL.
///
/// The certification direction runs along the prism's lower face, so membership is decided by which
/// side of that face the start's own line lies, at the scale of the start's last bits. A probe line
/// formed metres behind the start rounds those bits away and reads a parallel line outside the
/// prism; the corridor resumed along `(0.6, 0.799999, 0)` then meets the lower face's exit (DERIVED:
/// ~15 mm ahead) with no entry owned, `UnexpectedExit`. The certificate must either read the start
/// inside, with the resumed corridor pairing, or refuse with a structured error. One axis sample.
///
/// Certified one f32 step off the lower face's own direction, `(0.6000001, 0.8, 0)`, the start's
/// line still runs within a micron of the face across the whole prism, now crossing it
/// transversely (DERIVED: ~0.15 m ahead of the start), and its orientation survives f32. A probe
/// line formed metres behind the start loses the start's `2⁻²⁶` and crosses the face behind it
/// instead, reading the start outside; the corridor resumed along the same direction then meets the
/// exit with no entry owned. Read on the start's own line, the start is inside and the resumed
/// corridor pairs.
#[test]
fn a_start_is_certified_on_the_ray_its_corridor_resumes_along() {
    let (mut app, built) = settled(grazed_prism);
    let swept = Vec3::new(0.600_000_1, 0.8, 0.0);
    let starts = resumed(&mut app, GRAZED_START, swept, swept, 0.0);
    assert_eq!(
        inside_volumes(&starts.seeds[0]),
        built,
        "the start a hair inside the face lies inside the prism",
    );
    let launch = Vec3::new(0.6, 0.799_999, 0.0).normalize();
    match certified_then_resumed(&mut app, GRAZED_START, GRAZED_SWEPT, launch, 0.0) {
        Ok((starts, walked)) => {
            walked.unwrap_or_else(|error| panic!("the resumed corridor failed: {error:?}"));
            assert_eq!(
                inside_volumes(&starts.seeds[0]),
                built,
                "a certified start lies inside the prism",
            );
        }
        Err(error) => assert!(
            !matches!(
                error,
                WalkError::UnexpectedExit { .. } | WalkError::UnexpectedEntry { .. }
            ),
            "a refused certificate is a closure or ceiling refusal, never a pairing violation: \
             {error:?}",
        ),
    }
}

/// A CROSSING THE COLLECTOR ADMITS IS NEVER READ AS A TANGENT.
///
/// The collector admits a triangle whenever the exact crossing determinant is non-zero, and signs it
/// by that same determinant (`walk::FaceHit::entering`); the walk reads that sign and never the f32
/// dot of the axis with the face's normalized normal. Along `(0.6, 0.8, 0)` in f32 the prism's
/// lower-face normal `(0.8, −0.6, 0)` dots to exactly zero, while the exact determinant does not
/// vanish (DERIVED: `0.6f·8 − 0.8f·6 ≈ 1.2e-7`), so the start's own line exits through that face;
/// read from the dot, that exit is a tangent and the probe ends with the prism open,
/// `IncompleteCorridor`.
#[test]
fn a_crossing_the_collector_admits_is_never_read_as_a_tangent() {
    let (mut app, built) = settled(grazed_prism);
    let starts = resumed(&mut app, GRAZED_START, GRAZED_SWEPT, GRAZED_SWEPT, 0.0);
    assert_eq!(inside_volumes(&starts.seeds[0]), built);
}

/// A START ON A FACE ITS RAY RUNS ALONG RESUMES CONSISTENTLY, WHICHEVER WAY THE FLIGHT BENDS.
///
/// Fixture data: a unit cube at the origin and a start on its top face `z = 0.5`, certified along
/// `+X`, so every top-face triangle is coplanar with the probe and claims nothing; the probe meets
/// the end faces at their top edges. The boundary convention reads the start inside, and the
/// corridor resumed from it must pair straight on, bent up out of the face, and bent down into the
/// cube.
#[test]
fn a_start_on_a_face_along_its_ray_resumes_consistently() {
    let (mut app, built) = settled(|app| {
        vec![mesh_volume(
            app,
            box_trimesh(Vec3::ONE),
            Transform::IDENTITY,
        )]
    });
    let at = Vec3::new(0.0, 0.1, 0.5);
    for launch in [
        Vec3::X,
        Vec3::new(1.0, 0.0, 0.1).normalize(),
        Vec3::new(1.0, 0.0, -0.1).normalize(),
    ] {
        let starts = resumed(&mut app, at, Vec3::X, launch, 0.0);
        assert_eq!(
            inside_volumes(&starts.seeds[0]),
            built,
            "the start on the face reads inside the cube",
        );
    }
}

/// A START INSIDE NESTED VOLUMES IS INSIDE EACH OF THEM, PER KEY.
///
/// Fixture data: a 0.5 m trimesh cube inside a 1 m convex cube inside a 2 m trimesh cube, all at the
/// origin, and a full disc laid at the origin along a general direction. Every sample is inside all
/// three, and the corridor resumed from the starts pairs every exit.
#[test]
fn a_start_inside_nested_volumes_is_inside_each() {
    let (mut app, mut built) = settled(|app| {
        let convex = app
            .world_mut()
            .spawn((
                Transform::IDENTITY,
                RigidBody::Static,
                Collider::cuboid(1.0, 1.0, 1.0),
                CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
                BallisticVolume {
                    material: walk::VolumeMaterial::fixture(STEEL),
                    substance: "fixture".to_string(),
                },
            ))
            .id();
        vec![
            mesh_volume(app, box_trimesh(Vec3::splat(2.0)), Transform::IDENTITY),
            convex,
            mesh_volume(app, box_trimesh(Vec3::splat(0.5)), Transform::IDENTITY),
        ]
    });
    built.sort();
    let axis = Vec3::new(0.3, -0.2, 0.9).normalize();
    let starts = resumed(&mut app, Vec3::ZERO, axis, axis, disc_radius(ROUND.caliber));
    assert_eq!(starts.seeds.len(), 1 + walk::DEFAULT_RING);
    for seed in &starts.seeds {
        assert_eq!(
            inside_volumes(seed),
            built,
            "sample {} is inside both nested volumes",
            seed.sample,
        );
    }
}

// ---------------------------------------------------------------------------------------------
// A containment probe reads only what could contain a start, within its work ceilings
// ---------------------------------------------------------------------------------------------

/// [`certified`] for a disc of `radius` laid at `at` along `axis`, on the frame anchored there.
fn certify(app: &mut App, at: Vec3, axis: Vec3, radius: f32) -> Result<Interior, WalkError> {
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                certified(
                    &context,
                    &|_| true,
                    at,
                    Dir3::new(axis).expect("a direction"),
                    DiscFrame::anchored(axis).expect("a real direction"),
                    radius,
                )
            },
        )
        .expect("the probe system runs")
}

/// A CANDIDATE WHOSE BOUNDS CONTAIN NO START IS NOT PROBED.
///
/// Fixture data, from a DERIVED counterexample: a disc of radius 0.044 m laid at the origin along
/// `+Z`, and a box over `x, y ∈ [0.030, 0.040]`, `z ∈ [−30, 30]` m. Its bounds meet the disc's
/// enclosing cube, but neither the axis nor any ring sample lies in its `xy` rectangle, so it cannot
/// contain a start. Reading only candidates that could contain a start avoids an irrelevant
/// DERIVED 60 m probe; every sample is outside everything.
#[test]
fn a_candidate_whose_bounds_contain_no_start_is_not_probed() {
    let (mut app, _) = settled(|app| {
        vec![mesh_volume(
            app,
            box_trimesh(Vec3::new(0.010, 0.010, 60.0)),
            Transform::from_translation(Vec3::new(0.035, 0.035, 0.0)),
        )]
    });
    let radius = 0.044;
    let frame = DiscFrame::anchored(Vec3::Z).expect("a real direction");
    assert!(
        layout(&frame, radius).iter().all(|offset| {
            !(0.030..=0.040).contains(&offset.x) || !(0.030..=0.040).contains(&offset.y)
        }),
        "the fixture keeps every sample out of the box's rectangle",
    );
    let starts = certify(&mut app, Vec3::ZERO, Vec3::Z, radius)
        .unwrap_or_else(|error| panic!("the start failed to certify: {error:?}"));
    assert_eq!(starts.seeds.len(), 1 + walk::DEFAULT_RING);
    assert!(
        starts.seeds.iter().all(|seed| seed.inside.is_empty()),
        "every sample is outside everything: {:?}",
        starts.seeds,
    );
}

/// The game's damage model reads every finite containment candidate.
///
/// Fixture data: 65 nested cubes around the start, every one of which
/// could contain it.
#[test]
fn a_probe_reads_every_finite_candidate() {
    let (mut app, _) = settled(|app| {
        (0..=64)
            .map(|index| {
                let size = 0.5 + 0.01 * index as f32;
                mesh_volume(app, box_trimesh(Vec3::splat(size)), Transform::IDENTITY)
            })
            .collect()
    });
    let starts =
        certify(&mut app, Vec3::ZERO, Vec3::Z, 0.0).expect("finite certified geometry is readable");
    assert_eq!(
        starts.seeds[0].inside.len(),
        65,
        "every nested fixture shell contributes membership"
    );
}

/// The game's damage model reads every sample's finite containment faces.
///
/// Fixture data: one trimesh of `count` separate closed boxes stacked along the probe at a 0.1 m
/// pitch, so each sample crosses DERIVED 4400 faces. The collector must grow to hold every face;
/// dropping one would break containment membership for the resumed walk.
#[test]
fn a_probe_reads_every_samples_finite_faces() {
    let count = 2200; // Fixture data: DERIVED 4400 faces per sample.
    let (cube, cube_surfaces) = box_trimesh(Vec3::ONE);
    let (cube_vertices, cube_indices) = {
        let mesh = cube.shape().as_trimesh().expect("a trimesh");
        (mesh.vertices().to_vec(), mesh.indices().to_vec())
    };
    assert_eq!(cube_surfaces.corners.as_ref(), cube_indices.as_slice());
    let mut vertices = Vec::new();
    let mut indices: Vec<[u32; 3]> = Vec::new();
    let mut shells = Vec::new();
    for index in 0..count {
        let base = vertices.len() as u32;
        let centre = 0.1 * index as f32 - 0.05 * count as f32;
        vertices.extend(
            cube_vertices
                .iter()
                .map(|corner| Vec3::new(corner.x * 0.2, corner.y * 0.2, corner.z * 0.05 + centre)),
        );
        indices.extend(
            cube_indices
                .iter()
                .map(|tri| tri.map(|corner| corner + base)),
        );
        shells.extend(std::iter::repeat_n(index as u32, cube_indices.len()));
    }
    let surfaces = BallisticSurfaces {
        shells: shells.into(),
        corners: indices.as_slice().into(),
    };
    let stack = (Collider::trimesh(vertices, indices), surfaces);
    let (mut app, _) = settled(|app| vec![mesh_volume(app, stack, Transform::IDENTITY)]);
    let face_count = app
        .world_mut()
        .run_system_once(|world: ProjectileMarchWorld, colliders: Colliders| {
            let context = ResolveContext {
                world: &world,
                colliders: &colliders,
                deposit: false,
                laws: WalkLaws::default(),
            };
            build_corridor(
                &context,
                &|_| true,
                Vec3::new(0.031, 0.017, -111.0),
                Vec3::ZERO,
                Vec3::Z,
                222.0,
                DiscFrame::anchored(Vec3::Z).unwrap(),
                0.0,
                &[],
            )
            .map(|corridor| corridor.samples[0].hits.len())
        })
        .unwrap()
        .expect("all finite fixture faces are collected");
    assert_eq!(
        face_count,
        2 * count,
        "each closed fixture box contributes both faces"
    );
    let starts = certify(&mut app, Vec3::new(0.0, 0.0, 0.05), Vec3::Z, 0.044)
        .expect("the complete disc's finite faces are readable");
    assert_eq!(starts.seeds.len(), 1 + walk::DEFAULT_RING);
}

// ---------------------------------------------------------------------------------------------
// The spall cone's field law (design doc §5): class invariants on fixture worlds
// ---------------------------------------------------------------------------------------------

use crate::ballistics::ProjectileMarchWorld;
use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor};

/// The colliders a fixture's field walk reads, as the runtime's systems hold them.
type Colliders<'w, 's> = Query<
    'w,
    's,
    (
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static BallisticSurfaces>,
    ),
>;

/// Walk every `(origin, direction, strength)` of `rays` with weight `weight` through every volume
/// of the world (volumes of no tank: one tank), each throw leaving `exited`, each walk failing the
/// test.
fn field_rays(
    app: &mut App,
    rays: Vec<(Vec3, Dir3, f32)>,
    weight: f32,
    exited: Vec<Entity>,
) -> Vec<FieldRay> {
    app.world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Colliders,
                  health: Query<&mut ComponentHealth>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                let within = |collider: Entity| world.volume_of(collider).is_some();
                rays.iter()
                    .map(|&(origin, dir, strength)| {
                        field_direction(
                            &context,
                            &FieldReach::of_throw(&context, &within, origin),
                            &|volume| health.contains(volume),
                            &exited,
                            origin,
                            dir,
                            strength,
                            weight,
                        )
                        .expect("a fixture direction walks")
                    })
                    .collect::<Vec<FieldRay>>()
            },
        )
        .expect("the field system runs")
}

/// Fixture field of the cones below, the ball at a 24.6° rim (DERIVED): fixture data, not a
/// substance's value.
const FIELD: FieldDescriptor = FieldDescriptor {
    law: crate::ballistics::spall_cone::FieldLaw::Ball,
    ratio: 2.4,
};
/// Fixture direction count where the resolution is not the point.
const FIELD_N: usize = 96;
/// HP of every fixture box: above any dose below, so no dose is clamped away, and within
/// [`crate::ballistics::MAX_POOL_POINTS`].
const FIELD_HP: u32 = 1_000_000;

/// A fixture world built by `build`, stepped so its colliders are queryable; returns the boxes
/// `build` names, in order.
fn field_world(build: impl FnOnce(&mut App) -> Vec<Entity>) -> (App, Vec<Entity>) {
    let mut app = world_with_trimesh_plates(&[]);
    let boxes = build(&mut app);
    for _ in 0..8 {
        app.update();
    }
    (app, boxes)
}

/// A box `size` centred `depth` metres down the cone's axis (`-Y` from the origin) and `lateral`
/// metres along `+X`; `hp` makes it a box, `None` armour.
fn below(app: &mut App, size: Vec3, depth: f32, lateral: f32, factor: f32, hp: bool) -> Entity {
    volume(
        app,
        size,
        Transform::from_translation(Vec3::new(lateral, -depth, 0.0)),
        factor,
        hp.then_some(FIELD_HP),
    )
}

/// Each of `boxes`' dose from one cone of `directions` strata and the field [`FIELD`], of strength
/// `strength`, thrown from the origin down `-Y` through every volume of the world (volumes of no
/// tank: one tank).
fn field_doses(app: &mut App, boxes: &[Entity], strength: f32, directions: usize) -> Vec<f64> {
    field_doses_of(app, boxes, FIELD, strength, directions)
}

/// [`field_doses`] under the field `field`.
fn field_doses_of(
    app: &mut App,
    boxes: &[Entity],
    field: FieldDescriptor,
    strength: f32,
    directions: usize,
) -> Vec<f64> {
    let spread = ConeShape::new(AxisRule::ExitDirection, directions)
        .unwrap()
        .spread(field)
        .unwrap();
    let rays = spread
        .directions(Dir3::NEG_Y)
        .map(|(dir, weight)| (Vec3::ZERO, dir, strength * weight))
        .collect();
    let mut dose = vec![0.0f64; boxes.len()];
    for ray in field_rays(app, rays, 1.0 / directions as f32, Vec::new()) {
        for applied in ray.doses {
            if let Some(index) = boxes.iter().position(|b| *b == applied.address) {
                dose[index] += applied.amount as f64;
            }
        }
    }
    dose
}

/// CLASS INVARIANT 2, RESOLUTION STABILITY: a box's dose converges as the direction count grows —
/// in the convergence regime the change from N to 4N shrinks with every step — and at the shipped
/// count it lies within a stated tolerance of the converged value. Fixture: a soft box off the
/// axis, a metre down. A direction's dose not scaled by its `1/N` share, or a stratum weight that
/// is not its stratum's share, fails it.
///
/// The steps are taken from 384 directions up. Below that the strata the box's edges cut are noise
/// of a stratum's weight, not a trend, and the axis strata the box covers carry the field's
/// heaviest weights: MEASURED on this fixture, 96 → 384 → 1536 changed the dose by 0.016 then
/// 0.055 (of ~1.83), then 0.027 and 0.004.
#[test]
fn a_box_dose_is_stable_under_resolution() {
    let (mut app, boxes) =
        field_world(|app| vec![below(app, Vec3::new(0.4, 0.05, 0.4), 1.0, 0.15, 0.0, true)]);
    let counts = [384, 1536, 6144, 24576];
    let doses: Vec<f64> = counts
        .iter()
        .map(|&n| field_doses(&mut app, &boxes, 10.0, n)[0])
        .collect();
    let changes: Vec<f64> = doses.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    assert!(
        changes.windows(2).all(|w| w[1] <= w[0]),
        "the change shrinks as N quadruples: doses {doses:?}, changes {changes:?}"
    );
    let converged = doses[doses.len() - 1];
    let shipped = ConeShape::shipped().directions_count();
    let at_shipped = field_doses(&mut app, &boxes, 10.0, shipped)[0];
    // Fixture tolerance: a tenth of the converged dose at the shipped resolution.
    assert!(
        (at_shipped - converged).abs() <= 0.1 * converged,
        "N = {shipped}: {at_shipped} against {converged}"
    );
}

/// CLASS INVARIANT 3, HOMOGENEITY: scaling the strength `S` scales every dose by the same factor.
/// A dose with a strength-dependent offset or cut-off fails it.
#[test]
fn doses_are_homogeneous_in_strength() {
    let (mut app, boxes) = field_world(|app| {
        vec![
            below(app, Vec3::new(0.3, 0.05, 0.3), 1.0, 0.1, 0.0, true),
            below(app, Vec3::new(0.2, 0.05, 0.2), 1.5, -0.2, 0.0, true),
        ]
    });
    let one = field_doses(&mut app, &boxes, 10.0, FIELD_N);
    let scaled = field_doses(&mut app, &boxes, 37.0, FIELD_N);
    for (a, b) in one.iter().zip(&scaled) {
        assert!(*a > 0.0, "the fixture boxes lie in the cone");
        assert!((b / a - 3.7).abs() < 1.0e-5, "{b} against 3.7 × {a}");
    }
}

/// CLASS INVARIANT 4, INVERSE SQUARE: strength is not spent in air, so a thin unresisting box on
/// the axis takes a quarter of the dose at twice the distance — the share of the cone it covers. A
/// strength that falls with distance travelled fails it. Fixture: a 0.1 m square; the ball's
/// density across it falls by at most 2.1 % at 1 m and a quarter of that at 2 m (DERIVED:
/// `1 − p_Ω(θ)/p_Ω(0) ≈ 3λ²(3 + λ²)θ²/(2(1 + 3λ²))`, at the corner's θ = 0.071 and 0.035), inside
/// the tolerance.
#[test]
fn an_unshadowed_dose_falls_as_the_inverse_square() {
    let dose_at = |depth: f32| {
        let (mut app, boxes) = field_world(|app| {
            vec![below(
                app,
                Vec3::new(0.1, 0.002, 0.1),
                depth,
                0.0,
                0.0,
                true,
            )]
        });
        field_doses(&mut app, &boxes, 10.0, 16384)[0]
    };
    let ratio = dose_at(2.0) / dose_at(1.0);
    assert!((ratio - 0.25).abs() < 0.01, "D(2r)/D(r) = {ratio}");
}

/// The pen cam's potency colour reads this resolver's dose, on every support: the intensity its law draws at distance `r` along a direction (`s / (Ω r²)`,
/// `pen_cam::potency`, `Ω` the support's solid angle) times a thin unresisting face's area there
/// is the dose the face takes, in the strength's units. The colour's promise — a box's face wholly at or above a
/// threshold takes at least that threshold's pool — relies on it; a pen cam law that drifts from
/// the dose law (a lost `Ω`, a ring's band taken as the cap it lies in, a square dropped, the
/// profile ignored) fails here.
/// Fixture: a ball's cap ([`FIELD`]) with a 0.1 m square face-on on its axis, and a ring's band
/// (`λ = 0.45`, half-width 10°) with a 0.2 m square face-on at its middle, each at 1.0 and 1.5 m,
/// 16384 directions. At 1.5 m the faces cover ~124 and ~146 strata, one stratum under 0.8 % of
/// the dose (DERIVED: `(A / r²) / Ω · N`), and the tolerance is three strata. MEASURED: the ball's
/// 0.5 % and 2.4 % off, the ring's 2.2 % and 1.1 %, ~1.2 % of the ring's at 1.0 m its larger
/// face's (DERIVED: `A / r²` over the square's exact solid angle).
#[test]
fn the_pen_cam_intensity_times_a_face_is_its_dose() {
    use crate::damage_cam::pen_cam::potency::{Potency, intensity};
    let ring = FieldDescriptor::ring(0.45, 10.0_f32.to_radians());
    let middle = libm::atan2(1.0, 0.45) as f32;
    let fields = [
        (FIELD, Vec3::NEG_Y, 0.1),
        (ring, Vec3::new(middle.sin(), -middle.cos(), 0.0), 0.2),
    ];
    for (field, face, side) in fields {
        // The fixture square's face sets only the thresholds, unread here.
        let potency = Potency::of_crew_face(side * side);
        let spread = ConeShape::new(AxisRule::ExitDirection, 16384)
            .unwrap()
            .spread(field)
            .unwrap();
        let (_, weight) = spread
            .directions(Dir3::NEG_Y)
            .max_by(|a, b| a.0.dot(face).total_cmp(&b.0.dot(face)))
            .unwrap();
        for depth in [1.0, 1.5] {
            let (mut app, boxes) = field_world(|app| {
                let at = Transform::from_translation(face * depth)
                    .with_rotation(Quat::from_rotation_arc(Vec3::NEG_Y, face));
                vec![volume(
                    app,
                    Vec3::new(side, 0.002, side),
                    at,
                    0.0,
                    Some(FIELD_HP),
                )]
            });
            let dose = field_doses_of(&mut app, &boxes, field, 10.0, 16384)[0];
            let e = potency.emission(10.0 * weight, field.inner_angle(), field.half_angle());
            let drawn = intensity(e, depth - 0.001) as f64 * (side * side) as f64;
            let ratio = drawn / dose;
            assert!(
                (ratio - 1.0).abs() < 0.025,
                "{field:?}, r = {depth}: I·A / D = {ratio}"
            );
        }
    }
}

/// CLASS INVARIANT 6, FACTOR INDEPENDENCE: a box's dose is the strength arriving at it, never
/// scaled by its own factor. A dose scaled by the box's factor, or charged for its chord through
/// the box, fails it.
#[test]
fn a_box_dose_does_not_depend_on_its_own_factor() {
    let dose_with = |factor: f32| {
        let (mut app, boxes) =
            field_world(|app| vec![below(app, Vec3::new(0.3, 0.2, 0.3), 1.0, 0.1, factor, true)]);
        field_doses(&mut app, &boxes, 10.0, FIELD_N)[0]
    };
    let soft = dose_with(10.0);
    assert!(soft > 0.0);
    assert_eq!(soft.to_bits(), dose_with(900.0).to_bits());
}

/// CLASS INVARIANT 7, SHADOWING IS MONOTONE: adding any volume between the origin and a box —
/// armour or another box, thin or thick — never raises the box's dose. Each fixture volume lies
/// across some of the box's directions, and a hard body stops a direction it meets, so each lowers
/// it: a hard body a direction passes through, armour or a box dosed on the way, fails it.
#[test]
fn adding_a_volume_never_raises_a_dose() {
    let target = |app: &mut App| below(app, Vec3::new(0.4, 0.05, 0.4), 1.2, 0.1, 10.0, true);
    let (mut app, boxes) = field_world(|app| vec![target(app)]);
    let bare = field_doses(&mut app, &boxes, 10.0, FIELD_N)[0];
    assert!(bare > 0.0);
    let between: [(Vec3, f32, f32, bool); 4] = [
        (Vec3::new(0.2, 0.01, 0.2), 0.0, STEEL, false),
        (Vec3::new(2.0, 0.004, 2.0), 0.0, STEEL, false),
        (Vec3::new(0.3, 0.2, 0.3), 0.15, 10.0, true),
        (Vec3::new(2.0, 0.05, 2.0), 0.0, STEEL, false),
    ];
    for (size, lateral, factor, hp) in between {
        let (mut app, boxes) = field_world(|app| {
            below(app, size, 0.6, lateral, factor, hp);
            vec![target(app)]
        });
        let shadowed = field_doses(&mut app, &boxes, 10.0, FIELD_N)[0];
        assert!(
            shadowed < bare,
            "{size:?} at factor {factor}: {shadowed} against {bare}"
        );
    }
}

/// The game's damage model: a field reaches neighboring tanks and unowned environment matter.
/// Ownership never admits a dose or removes an occluder. Both owned and unowned source plates
/// must reach all three boxes; adding a foreign-owned blocker must shadow the box behind it.
/// FIXTURE DATA: a round perforates a thin plate above three off-axis boxes.
#[test]
fn a_cone_reaches_matter_independently_of_tank_ownership() {
    for owned_source in [false, true] {
        for blocked in [false, true] {
            let mut app = world_with_trimesh_plates(&[]);
            let tank = app.world_mut().spawn_empty().id();
            let other = app.world_mut().spawn_empty().id();
            let plate = volume(
                &mut app,
                Vec3::new(2.0, 0.06, 2.0),
                Transform::from_translation(Vec3::new(DROP_FROM.x, 0.97, DROP_FROM.z)),
                STEEL,
                None,
            );
            if owned_source {
                app.world_mut()
                    .entity_mut(plate)
                    .insert(crate::damage::VolumeOf(tank));
            }
            let mut boxes = Vec::new();
            for (dx, dz, owner) in [
                (0.7, 0.0, Some(tank)),
                (-0.7, 0.0, Some(other)),
                (0.0, 0.7, None),
            ] {
                let entity = volume(
                    &mut app,
                    Vec3::new(0.5, 0.2, 0.5),
                    Transform::from_translation(Vec3::new(
                        DROP_FROM.x + dx,
                        -0.5,
                        DROP_FROM.z + dz,
                    )),
                    STEEL * 0.1,
                    Some(1000),
                );
                if let Some(owner) = owner {
                    app.world_mut()
                        .entity_mut(entity)
                        .insert(crate::damage::VolumeOf(owner));
                }
                boxes.push(entity);
            }
            if blocked {
                let blocker = volume(
                    &mut app,
                    Vec3::new(0.8, 0.02, 0.8),
                    Transform::from_translation(Vec3::new(DROP_FROM.x - 0.5, 0.3, DROP_FROM.z)),
                    STEEL,
                    None,
                );
                app.world_mut()
                    .entity_mut(blocker)
                    .insert(crate::damage::VolumeOf(other));
            }
            app.insert_resource(RetainSpentShells(true));
            for _ in 0..8 {
                app.update();
            }
            let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
            app.update();
            for (index, entity) in boxes.iter().enumerate() {
                let hp = app.world().get::<ComponentHealth>(*entity).unwrap();
                assert_eq!(
                    hp.held() < hp.capacity(),
                    !(blocked && index == 1),
                    "source owned={owned_source}, blocker={blocked}, box={index}"
                );
            }
            let first = app
                .world()
                .get::<SpallMarks>(shell)
                .unwrap()
                .cones
                .first()
                .unwrap();
            assert!(
                Vec3::from(first.axis).distance(Vec3::NEG_Y) < 1.0e-4,
                "square on, the axis is the rear normal and the direction: {:?}",
                first.axis
            );
        }
    }
}

/// The game's damage model rounds a box's accumulated shot dose, including fields individually
/// below one pool unit. Pool owners rely on two fixture fields of 0.0003 HP removing one unit
/// together, even when playback settles after each field. One direction reaches one hard box.
#[test]
fn two_subquantum_fields_reach_one_pool_unit_together() {
    use crate::ballistics::{ShotDose, SpallMarks};
    let (mut app, boxes) = field_world(|app| vec![slab_below(app, -1.0, 0.2, 10.0, true)]);
    let target = boxes[0];
    app.insert_resource(crate::ballistics::spall_cone::SpallLaw(
        ConeShape::new(AxisRule::ExitDirection, 1).unwrap(),
    ));
    let lost = app
        .world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Colliders,
                  mut health: Query<&mut ComponentHealth>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: true,
                    laws: WalkLaws::default(),
                };
                let source = SpallSource {
                    t: 0.0,
                    position: Vec3::ZERO,
                    face: None,
                    direction: Vec3::NEG_Y,
                    exited: Vec::new(),
                    spend: 0.0,
                    emitted: (0.0003 / crate::ballistics::spall_cone::cone_unit().hp_per_joule())
                        as f32,
                    debris_ratio: 2.4,
                    strength_ratio: 1.0,
                    exit_speed: 0.0,
                };
                let mut shot = ShotDose::default();
                let mut spall = SpallMarks::default();
                let mut lost = Vec::new();
                for mark in 0..2 {
                    assert!(crate::ballistics::throw_spall_cone(
                        &mut spall,
                        &mut shot,
                        &source,
                        &context,
                        &|volume| volume == target
                    ));
                    lost.push(shot.settle(&mut health, true, mark));
                }
                lost
            },
        )
        .unwrap();
    assert_eq!(
        lost,
        [0, 1],
        "the pool rounds the shot total, not each field"
    );
    let health = app.world().get::<ComponentHealth>(target).unwrap();
    assert_eq!(health.capacity() - health.held(), 1);
}

/// The game's damage model records every nonzero cone's receiver dose, including below the
/// pool quantum. Environment energy and later shot contributions rely on the same field walk.
/// Fixture: thin soft slabs and a steel slab, with an HP-bearing receiver below every source.
#[test]
fn a_nonzero_cone_always_records_its_hp_receivers() {
    let energy = crate::ballistics::spall_cone::cone_energy();
    let per_joule_lost = crate::ballistics::spall_cone::cone_unit().hp_per_joule()
        * f64::from(energy.debris_share());
    let lost = 0.15 / per_joule_lost;
    let v_in = 800.0f32;
    let v_out = ((v_in as f64).powi(2) - 2.0 * lost / FIXTURE_88.mass as f64).sqrt() as f32;
    // Reference-mm over the factor's reference-mm per metre.
    let slab = (capability(FIXTURE_88, v_in) - capability(FIXTURE_88, v_out)) / 10.0;
    let unit_factor = 10.0 / crate::ballistics::POOL_UNITS_PER_HP as f32;
    for (thickness, factor) in [(slab, unit_factor), (slab, 10.0), (0.02, STEEL)] {
        let mut app = world_with_trimesh_plates(&[]);
        volume(
            &mut app,
            Vec3::new(2.0, thickness, 2.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 1.0 - 0.5 * thickness, DROP_FROM.z)),
            factor,
            None,
        );
        let receiver = volume(
            &mut app,
            Vec3::new(20.0, 0.1, 20.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 0.0, DROP_FROM.z)),
            0.0,
            Some(FIELD_HP),
        );
        app.insert_resource(RetainSpentShells(true));
        for _ in 0..8 {
            app.update();
        }
        let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
        app.update();
        let world = app.world();
        let spall = world.get::<SpallMarks>(shell).unwrap();
        let counted: usize = world
            .get::<crate::ballistics::PenetrationMarks>(shell)
            .unwrap()
            .log
            .iter()
            .map(|mark| match mark {
                ArmourMark::Crossing { cones, .. } => *cones,
                _ => 0,
            })
            .sum();
        assert_eq!(
            (spall.cones.len(), counted),
            (1, 1),
            "factor {factor}: {:?}",
            spall
                .cones
                .iter()
                .map(|cone| cone.strength)
                .collect::<Vec<_>>()
        );
        assert!(
            spall.cones[0]
                .rays
                .iter()
                .flat_map(|ray| &ray.doses)
                .any(|(address, energy)| *address == receiver && *energy > 0.0),
            "factor {factor}: the HP-bearing receiver keeps the field contribution"
        );
        assert!(
            spall.cones[0].strength > 0.0,
            "factor {factor}: the cone carries energy"
        );
    }
}

/// One fixture shot falling onto a wide HP-bearing slab `thickness` metres thick of `factor`
/// reference-mm per metre, its top face through `y = 1` under [`DROP_FROM`], tilted about `Z` so
/// the round strikes it at `incidence` (rad) from the face's normal: the driven flight and the slab.
/// It is 12 m along the tilt, so the line through a 0.4 m slab at up to 85° (4.6 m, DERIVED) ends
/// on its far face: a line out of its side would read a thinner plate.
fn shot_onto_slab(
    body: RoundBody,
    speed: f32,
    incidence: f32,
    thickness: f32,
    factor: f32,
) -> (Drive, Entity) {
    shot_onto_slab_with_contact(
        body,
        speed,
        incidence,
        thickness,
        factor,
        crate::rounds::ContactPreset::fixture(),
    )
}

fn shot_onto_slab_with_contact(
    body: RoundBody,
    speed: f32,
    incidence: f32,
    thickness: f32,
    factor: f32,
    preset: crate::rounds::ContactPreset,
) -> (Drive, Entity) {
    let mut gate = world_with_trimesh_plates(&[]);
    let rotation = Quat::from_rotation_z(incidence);
    let normal = rotation * Vec3::Y;
    let slab = volume(
        &mut gate,
        Vec3::new(12.0, thickness, 3.0),
        Transform::from_translation(
            Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z) - normal * (0.5 * thickness),
        )
        .with_rotation(rotation),
        factor,
        Some(1000),
    );
    for _ in 0..8 {
        gate.update();
    }
    (
        try_driven_with_contact(&mut gate, body, speed, preset).expect("the fixture walk is valid"),
        slab,
    )
}

/// The bounce a flight made at its first contact; a flight that did anything else is a broken
/// fixture.
fn first_bounce(flown: &Drive) -> &Bounce {
    match flown.decisions.first() {
        Some(Decision::Bounce(bounce)) => bounce,
        _ => panic!("the round bounces first: {:?}", impacts_of(flown)),
    }
}

/// The game's damage model must discover a thin neighbour on the slide, rather than carrying the
/// disc over it. Fixture: a 75-degree steel face and a small neighbour on the outgoing axis, 0.5 m
/// along the face, narrower than the ring spacing so no incoming sample meets it. Its chord and energy
/// must be charged. Playback and the next crossing rely on that discovery.
#[test]
fn a_slide_does_not_skip_a_thin_neighbour() {
    let (mut app, slab, face) = world_with_face(75.0_f32.to_radians());
    let neighbour = volume(
        &mut app,
        Vec3::new(0.001, 0.002, 0.001),
        Transform::from_translation(face.at(-0.5, FIXTURE_88.caliber * 0.5 + REST_CLEARANCE))
            .with_rotation(face.rotation),
        STEEL,
        Some(1000),
    );
    for _ in 0..8 {
        app.update();
    }
    let flown = driven(&mut app, FIXTURE_88, 773.0);
    assert_eq!(first_bounce(&flown).struck, slab);
    assert!(
        flown.reaches(neighbour),
        "the sliding axis crosses the thin neighbour: {:?}",
        impacts_of(&flown)
    );
}

/// A steel face under [`DROP_FROM`]'s drop, `incidence` from it, as the fixture geometry of a
/// slide. Fixture data, in metres.
struct Face {
    contact: Vec3,
    rotation: Quat,
}

impl Face {
    fn normal(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// The point `up_slope` m along the face above the contact and `height` m off it.
    fn at(&self, up_slope: f32, height: f32) -> Vec3 {
        self.contact + self.rotation * Vec3::X * up_slope + self.normal() * height
    }
}

fn world_with_face(incidence: f32) -> (App, Entity, Face) {
    let mut app = world_with_trimesh_plates(&[]);
    let face = Face {
        contact: Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z),
        rotation: Quat::from_rotation_z(incidence),
    };
    let slab = volume(
        &mut app,
        Vec3::new(12.0, 0.4, 3.0),
        Transform::from_translation(face.at(0.0, -0.2)).with_rotation(face.rotation),
        STEEL,
        Some(1000),
    );
    (app, slab, face)
}

/// How far `offset` lies off an axis along `axis`.
fn off_axis(offset: Vec3, axis: Vec3) -> f32 {
    (offset - axis * offset.dot(axis)).length()
}

/// The game's damage model: after any skid, on every leg, every sample lies within the round's
/// calibre about the axis. A skid changes the round's velocity, not its calibre; the line dose, the
/// field casts and every later contact read these samples as its cross-section. Fixture: a face
/// stepped under the disc, so the incoming samples reach hand-offs 0.03 m / cos θ apart. Mutant:
/// inherit the previous leg's offsets, which leaves samples a tenth of a metre off the new axis.
#[test]
fn every_leg_after_a_skid_is_the_rounds_disc_about_its_axis() {
    let incidence = 75.0_f32.to_radians();
    let (mut app, _, face) = world_with_face(incidence);
    let raised = volume(
        &mut app,
        Vec3::new(12.0, 0.4, 3.0),
        Transform::from_translation(face.at(6.1, -0.17)).with_rotation(face.rotation),
        STEEL,
        Some(1000),
    );
    for _ in 0..8 {
        app.update();
    }
    let flown = driven(&mut app, FIXTURE_88, 300.0);
    let reach = FIXTURE_88.caliber * 0.5 * (1.0 + 1.0e-3);
    let mut skids = 0;
    let mut after_skid = false;
    for decision in &flown.decisions {
        if after_skid {
            for leg in decision.legs() {
                for sample in &leg.samples {
                    let off = off_axis(sample.offset, Vec3::from(leg.axis));
                    assert!(off <= reach, "{off} m off a {reach} m round's axis");
                }
            }
        }
        if let Decision::Bounce(bounce) = decision {
            skids += 1;
            after_skid = true;
            let starts = bounce.continuation.as_ref().expect("the fixture slides");
            for seed in &starts.seeds {
                let off = off_axis(seed.offset, Vec3::from(bounce.direction));
                assert!(off <= reach, "{off} m off a {reach} m round's axis");
                assert!(
                    seed.offset.dot(*bounce.direction).abs() < 1.0e-6,
                    "the outgoing disc lies square to its axis"
                );
                assert_eq!(seed.t, 0.0, "a new leg has no inherited cursor");
            }
        }
    }
    assert!(
        skids > 0,
        "the fixture skids off its steps: {:?} {raised:?}",
        impacts_of(&flown)
    );
}

/// The game's damage model: a slide on a flat face stays out of that face for its calibre, with
/// the axis on the ray it came in on and no contact until something changes. Mutants: resume at the
/// axis hand-off (half the disc lies in the face, and the round bites as if head-on), or lift it off
/// the ray.
#[test]
fn a_slide_on_a_flat_face_rests_on_it_for_its_calibre() {
    for (body, speed) in [(FIXTURE_88, 300.0), (FIXTURE_MG, 500.0)] {
        for incidence in [40.0_f32, 60.0, 80.0] {
            let (mut app, slab, face) = world_with_face(incidence.to_radians());
            for _ in 0..8 {
                app.update();
            }
            let flown = driven(&mut app, body, speed);
            let bounce = first_bounce(&flown);
            assert_eq!(bounce.struck, slab);
            assert!(
                matches!(flown.decisions.as_slice(), [Decision::Bounce(_)]),
                "{incidence}°: one contact: {:?}",
                impacts_of(&flown)
            );
            let normal = face.normal();
            assert!(bounce.direction.dot(normal).abs() < 1.0e-5, "{incidence}°");
            let radius = body.caliber * 0.5;
            let height = (bounce.resume - face.contact).dot(normal);
            assert!(
                (height - radius - REST_CLEARANCE).abs() < 1.0e-4,
                "{incidence}°: the axis rests {height} m off the face"
            );
            assert!(
                (bounce.resume - DROP_FROM).cross(Vec3::NEG_Y).length() < 1.0e-4,
                "{incidence}°: the axis stays on the ray it flew: {}",
                bounce.resume
            );
            let continuation = bounce.continuation.as_ref().unwrap();
            for seed in &continuation.seeds {
                let height = (continuation.world_origin() + seed.offset - face.contact).dot(normal);
                assert!(
                    height > 0.5 * REST_CLEARANCE && height < 2.0 * radius + 2.0 * REST_CLEARANCE,
                    "{incidence}°: a sample {height} m off the face it rests on"
                );
            }
        }
    }
}

/// The game's damage model: a step in a slide's path is met and decided, by swept travel from
/// where the last leg ended, with no lift over it and no immunity. Mutants: a free advance of the
/// starts along the face, or a turn higher than the calibre's rest over the step's height. The step's
/// height is below the disc's reach and the distances are fixture data.
#[test]
fn a_step_in_a_slides_path_is_met_from_where_the_last_leg_ended() {
    let (mut app, _, face) = world_with_face(75.0_f32.to_radians());
    let step = volume(
        &mut app,
        Vec3::new(0.4, 0.05, 3.0),
        Transform::from_translation(face.at(-0.9, 0.025)).with_rotation(face.rotation),
        STEEL,
        Some(1000),
    );
    for _ in 0..8 {
        app.update();
    }
    let flown = driven(&mut app, FIXTURE_88, 300.0);
    let bounce = first_bounce(&flown);
    let second = flown
        .decisions
        .get(1)
        .unwrap_or_else(|| panic!("the slide meets the step: {:?}", impacts_of(&flown)));
    let struck = match second {
        Decision::Bounce(bounce) => bounce.struck,
        Decision::Bite(bite) => bite.struck,
        Decision::Clear { .. } => panic!("the step's face is in the disc's path"),
    };
    assert_eq!(struck, step);
    let leg = &second.legs()[0];
    assert!(leg.from.distance(bounce.resume) < 1.0e-4, "{}", leg.from);
    assert!(
        leg.axis.dot(*bounce.direction) > 1.0 - 1.0e-5,
        "the next leg flies the slide"
    );
    let continuation = bounce.continuation.as_ref().unwrap();
    for (seed, sample) in continuation.seeds.iter().zip(&leg.samples) {
        assert!(seed.offset.distance(sample.offset) < 1.0e-5);
    }
}

/// A BOUNCE'S SHOCK IS THE ENERGY THE ROUND LOST AT IT: the dose a bounce jars into the face it
/// struck is `½ m (v_in² − v_out²)` joules, `m` the mass that meets the plate, as a line dose is
/// the energy lost crossing a box, so it converts at the one energy rate; a round that loses more
/// at the bounce jars the face harder. Fixture rounds of different mass and speed graze one
/// HP-bearing slab of 0.4 m of steel at 75° (Fixture); each shot asserts a first-contact skid.
/// A shock read from the
/// readout, the incidence or a constant fails here; the record's line dose for a bounce and every
/// dose of energy converting at one rate rely on it.
#[test]
fn a_bounces_shock_is_the_energy_the_round_lost_there() {
    let mut shocks: Vec<(f64, f32)> = [
        (FIXTURE_MG, 755.0),
        (FIXTURE_MG, 500.0),
        (FIXTURE_88, 800.0),
    ]
    .into_iter()
    .map(|(body, speed)| {
        let (flown, _) = shot_onto_slab(body, speed, 75.0_f32.to_radians(), 0.4, STEEL);
        let bounce = first_bounce(&flown);
        let (v_in, v_out) = (bounce.speed_in as f64, bounce.speed as f64);
        let lost = 0.5 * body.mass as f64 * (v_in * v_in - v_out * v_out);
        assert!(
            (bounce.shock as f64 / lost - 1.0).abs() < 1.0e-5,
            "{} kg at {speed} m/s: shock {} J against {lost} J lost",
            body.mass,
            bounce.shock
        );
        (lost, bounce.shock)
    })
    .collect();
    shocks.sort_by(|a, b| a.0.total_cmp(&b.0));
    for pair in shocks.windows(2) {
        assert!(
            pair[0].1 < pair[1].1,
            "more energy lost, a harder jar: {pair:?}"
        );
    }
}

/// A BOUNCE DOSES THE ENERGY POINTED INTO THE FACE: the round keeps its speed along the face it
/// struck and loses the rest, restitution 0, so a full-coverage bounce jars `½ m (v_in cos θ)²`
/// joules into that face, `θ` the incidence from its normal — the dose falls as `cos² θ`, and a
/// shallower graze deals less. The fixture 88 grazes one HP-bearing slab of 0.4 m of steel at
/// 72°, 75°, 80° and 85° (Fixture); each shot asserts a first-contact skid.
/// A bounce that charged a fixed share of the striking
/// energy (a speed bleed), or the energy along the face, fails here; a struck module's bounce dose
/// in play, and its scale against a perforation's, rely on it.
#[test]
fn a_bounce_doses_the_energy_pointed_into_the_face() {
    let mut doses: Vec<(f32, f32)> = Vec::new();
    for degrees in [72.0_f32, 75.0, 80.0, 85.0] {
        let (flown, _) = shot_onto_slab(FIXTURE_88, 773.0, degrees.to_radians(), 0.4, STEEL);
        let bounce = first_bounce(&flown);
        assert_eq!(
            bounce.coverage, 1.0,
            "{degrees}°: the graze covers the disc"
        );
        let into_face =
            f64::from(bounce.speed_in) * f64::from(bounce.incoming.dot(bounce.normal).abs());
        let expected = 0.5 * f64::from(FIXTURE_88.mass) * into_face * into_face;
        assert!(
            (f64::from(bounce.shock) / expected - 1.0).abs() < 1.0e-4,
            "{degrees}°: shock {} J against ½ m (v cos θ)² = {expected} J",
            bounce.shock
        );
        println!(
            "{degrees}°: in {} m/s, out {} m/s, dose {} J = {} HP",
            bounce.speed_in,
            bounce.speed,
            bounce.shock,
            f64::from(bounce.shock) / crate::ballistics::energy_dose::joules_per_hp()
        );
        doses.push((degrees, bounce.shock));
    }
    for pair in doses.windows(2) {
        assert!(
            pair[1].1 < pair[0].1,
            "a shallower graze deals less: {pair:?}"
        );
    }
}

/// A BOUNCE KEEPS THE SPEED ALONG THE FACE: a full-coverage bounce leaves at the striking
/// velocity's part along the face it struck, `|v_in − (v_in · n̄) n̄| = v_in sin θ`, whatever the
/// round's mass, its speed or the incidence. Two fixture rounds graze one slab of 0.4 m of steel
/// at three fixture incidences, each asserting a first-contact skid.
/// A speed retained by a constant share (a bleed), by the round, or kept
/// whole fails here; the flight after a bounce, its fuze and the next surface it meets rely on it.
#[test]
fn a_bounce_keeps_the_speed_along_the_face() {
    for (body, speed) in [(FIXTURE_88, 773.0), (FIXTURE_MG, 755.0)] {
        for degrees in [72.0_f32, 78.0, 85.0] {
            let (flown, _) = shot_onto_slab(body, speed, degrees.to_radians(), 0.4, STEEL);
            let bounce = first_bounce(&flown);
            assert_eq!(
                bounce.coverage, 1.0,
                "{degrees}°: the graze covers the disc"
            );
            let v_in = Vec3::from(bounce.incoming) * bounce.speed_in;
            let along_face = (v_in - bounce.normal * v_in.dot(bounce.normal)).length();
            assert!(
                (bounce.speed - along_face).abs() <= 1.0e-5 * bounce.speed_in,
                "{} kg at {degrees}°: left at {} m/s, the speed along the face is {along_face}",
                body.mass,
                bounce.speed
            );
        }
    }
}

/// The game's damage model reads each struck plate's own critical angle: on its biting side
/// the round enters, on its grazing side it reflects, with shock bounded by the energy pointed
/// into that face at the critical incidence. This holds with a floor or a reach bound too;
/// no ordering of different plates' doses is required by pool authors. Fixture data: 773 m/s,
/// a thin slab, a 456 mm slab and a 0.01 rad offset from each critical angle.
#[test]
fn bounce_dose_bounds_read_the_plate_that_was_struck() {
    let laws = WalkLaws::default();
    let (speed, _) = first_tick(FIXTURE_88, 773.0);
    for thickness in [0.04, 0.456] {
        // Fixture metres.
        let shear = laws.reference.flow_strength / 3.0_f64.sqrt();
        let reference = laws.reference.cavity_strength();
        let ratio = thickness as f64 / FIXTURE_88.caliber as f64;
        let saturation = reference / (4.0 * shear);
        let resistance = reference.min(4.0 * shear * ratio * (ratio / saturation).sqrt());
        let density = crate::rounds::ContactPreset::fixture().density as f64;
        let x = resistance / (2.0 * density * (speed as f64).powi(2));
        let critical = ((1.0 + x * x).sqrt() - x).asin() as f32;
        let (biting, _) = shot_onto_slab(FIXTURE_88, 773.0, critical - 0.01, thickness, STEEL);
        assert!(
            matches!(biting.decisions.first(), Some(Decision::Bite(_))),
            "the round bites below this plate's critical incidence"
        );
        let (graze, part) = shot_onto_slab(FIXTURE_88, 773.0, critical + 0.01, thickness, STEEL);
        let bounce = first_bounce(&graze);
        assert_eq!(bounce.struck, part);
        let upper = 0.5
            * f64::from(FIXTURE_88.mass)
            * f64::from(bounce.speed_in).powi(2)
            * f64::from(critical.cos()).powi(2);
        assert!(
            bounce.shock > 0.0 && f64::from(bounce.shock) < upper,
            "this plate's bounce shock {} J is bounded by {upper} J",
            bounce.shock
        );
    }
}

/// A plate exit's cone is spread by its rear layer's substance's field: two steel plates in
/// contact, each of its own ratio, throw one cone whose descriptor is the rear plate's, and whose
/// every direction carries `S·f_k` of that spread to the bit. A throw that read the front layer,
/// the larger or the smaller ratio, or a fixed one fails one of the two orders. Fixture: an 88
/// square on through 20 mm at 0.8 of the steel factor, then 20 mm of steel; ratios 3.0 and 1.5
/// (19.5° and 41.8° cones, DERIVED), fixture data.
#[test]
fn a_thrown_cone_is_its_rear_layers_field() {
    for (front, rear) in [(3.0, 1.5), (1.5, 3.0)] {
        let mut app = world_with_trimesh_plates(&[]);
        for (y, factor, ratio) in [(1.01, STEEL * 0.8, front), (0.99, STEEL, rear)] {
            volume_of(
                &mut app,
                Vec3::new(2.0, 0.02, 2.0),
                Transform::from_translation(Vec3::new(DROP_FROM.x, y, DROP_FROM.z)),
                walk::VolumeMaterial {
                    debris_ratio: ratio,
                    ..walk::VolumeMaterial::fixture(factor)
                },
                None,
            );
        }
        app.insert_resource(RetainSpentShells(true));
        for _ in 0..8 {
            app.update();
        }
        let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
        app.update();
        let spall = app.world().get::<SpallMarks>(shell).unwrap();
        let [cone] = spall.cones.as_slice() else {
            panic!("{front}/{rear}: {} cones, not one", spall.cones.len());
        };
        let field = FieldDescriptor::ball(rear);
        let spread = ConeShape::shipped().spread(field).unwrap();
        assert_eq!(cone.field, field, "front {front}, rear {rear}");
        assert_eq!(cone.rays.len(), spread.directions_count());
        for (ray, [.., weight]) in cone.rays.iter().zip(spread.strata()) {
            assert_eq!(ray.strength.to_bits(), (cone.strength * weight).to_bits());
        }
    }
}

/// An exit the round reaches throws even when the round then stops in the body behind it. Fixture:
/// an 88 through 20 mm of steel into two metres of a 150-factor block in contact with it.
#[test]
fn an_exit_before_a_stop_throws_at_runtime() {
    let mut app = world_with_trimesh_plates(&[]);
    volume(
        &mut app,
        Vec3::new(2.0, 0.02, 2.0),
        Transform::from_translation(Vec3::new(DROP_FROM.x, 0.99, DROP_FROM.z)),
        STEEL,
        None,
    );
    volume(
        &mut app,
        Vec3::new(2.0, 2.0, 2.0),
        Transform::from_translation(Vec3::new(DROP_FROM.x, -0.02, DROP_FROM.z)),
        150.0,
        None,
    );
    app.insert_resource(RetainSpentShells(true))
        .init_resource::<Read>()
        .add_observer(read_impact);
    for _ in 0..8 {
        app.update();
    }
    app.world_mut().resource_mut::<Read>().0.clear();
    let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
    app.update();
    let verdicts: Vec<Verdict> = app
        .world()
        .resource::<Read>()
        .0
        .iter()
        .map(|i| i.0)
        .collect();
    assert_eq!(verdicts, [Verdict::Stopped], "the round stops in the block");
    let cones = &app.world().get::<SpallMarks>(shell).unwrap().cones;
    assert_eq!(
        cones.len(),
        1,
        "the steel's exit throws; the block it stops in does not"
    );
    assert!(
        (cones[0].origin.y - 0.98).abs() < 2.0e-3,
        "the cone comes off the steel's rear face: {:?}",
        cones[0].origin
    );
}

/// A volume only an unreached member of a grouped exit leaves is not left: the cone's directions
/// start inside it and dose it there. Fixture (DERIVED): an annulus A of four plates 0.1 m thick
/// that every ring sample crosses and the axis does not, and a column box B that only the axis
/// crosses, 2 m long; one substance, so A's exits on the ring and B's on the axis group together.
/// The factor is set so the round stops a metre in, inside B: only A's members are reached, their
/// mean exit lies on the axis inside B, and B — whose exit was never reached — takes the cone's
/// initial dose on every direction.
#[test]
fn a_cone_doses_a_box_only_an_unreached_member_leaves() {
    let mut app = world_with_trimesh_plates(&[]);
    let (x, z) = (DROP_FROM.x, DROP_FROM.z);
    // Ring samples lie 44 mm off the axis (the 88's radius): outside the 15 mm hole, never on a
    // seam between plates, and never in the 8 mm column.
    let (hole, outer, column) = (0.015f32, 0.5f32, 0.008f32);
    // Spend to a metre in: all 13 samples through A's 0.1 m, then the axis alone through 0.9 m of B.
    let factor = capability(FIXTURE_88, 800.0) / (0.1 + 0.9 / 13.0);
    let plate = |app: &mut App, centre: (f32, f32), half: (f32, f32)| {
        volume(
            app,
            Vec3::new(2.0 * half.0, 0.1, 2.0 * half.1),
            Transform::from_translation(Vec3::new(x + centre.0, 0.95, z + centre.1)),
            factor,
            None,
        );
    };
    let side = 0.5 * (outer - hole);
    let mid = 0.5 * (outer + hole);
    plate(&mut app, (mid, 0.0), (side, outer));
    plate(&mut app, (-mid, 0.0), (side, outer));
    plate(&mut app, (0.0, mid), (hole, side));
    plate(&mut app, (0.0, -mid), (hole, side));
    let b = volume(
        &mut app,
        Vec3::new(2.0 * column, 2.0, 2.0 * column),
        Transform::from_translation(Vec3::new(x, 0.0, z)),
        factor,
        Some(FIELD_HP),
    );
    app.insert_resource(RetainSpentShells(true))
        .init_resource::<Read>()
        .add_observer(read_impact);
    for _ in 0..8 {
        app.update();
    }
    app.world_mut().resource_mut::<Read>().0.clear();
    let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
    app.update();
    let read = app.world().resource::<Read>().0.clone();
    assert!(
        matches!(read.as_slice(), [(Verdict::Stopped, at)] if at.y < 0.9 && at.y > -1.0),
        "the round stops inside B, past A's exit and short of B's: {read:?}"
    );
    let cones = &app.world().get::<SpallMarks>(shell).unwrap().cones;
    assert_eq!(cones.len(), 1, "A's exit throws; B's is never reached");
    let cone = &cones[0];
    assert!(
        (cone.origin.y - 0.9).abs() < 2.0e-3
            && Vec2::new(cone.origin.x - x, cone.origin.z - z).length() < column,
        "the cone comes off A's rear face, on the axis inside B: {:?}",
        cone.origin
    );
    let undosed = cone.rays.iter().filter(|ray| ray.doses.is_empty()).count();
    assert_eq!(
        undosed,
        0,
        "every direction starts inside B, never left, and doses it ({} directions)",
        cone.rays.len()
    );
    let b = app.world().get::<ComponentHealth>(b).unwrap();
    assert!(b.held() < b.capacity());
}

/// Where the fixture walks of [`slab_layers`] start, behind the throw point (m), as
/// [`field_presence`] starts a direction's walk behind every bound it meets.
const BEHIND: f64 = 0.25;

/// The layers [`walk::walk_ray`] makes of square-on fixture slabs along one direction, each
/// `(enter, exit, factor)` in metres from the throw point, with every layer that ends before the
/// throw point dropped as [`field_direction`] drops it; and the slabs' volumes, in order. The walk
/// starts [`BEHIND`] the throw point, so a slab entered behind it is one the throw starts inside.
fn slab_layers(slabs: &[(f64, f64, f32)]) -> (Vec<walk::Layer>, Vec<Entity>) {
    let axis = Vec3::NEG_Y;
    let volumes: Vec<Entity> = (1..=slabs.len() as u32)
        .map(|index| Entity::from_raw_u32(index).expect("a test entity index"))
        .collect();
    let mut hits = Vec::new();
    for (index, (&(enter, exit, _), &volume)) in slabs.iter().zip(&volumes).enumerate() {
        let primitive = Entity::from_raw_u32(index as u32 + 1_000).expect("a test entity index");
        for (face, t, normal, entering) in [(0, enter, -axis, true), (1, exit, axis, false)] {
            let id = 2 * index as u32 + face;
            hits.push(FaceHit {
                volume,
                primitive,
                shell: 0,
                contact: walk::Contact::Face(id),
                triangle: id,
                t: BEHIND + t,
                true_normal: normal,
                entering,
            });
        }
    }
    let table = VolumeTable::new(
        volumes
            .iter()
            .zip(slabs)
            .map(|(&volume, &(_, _, factor))| (volume, walk::VolumeMaterial::fixture(factor))),
    );
    let corridor = RayCorridor {
        anchor: Vec3::ZERO,
        origin: Vec3::ZERO,
        axis,
        length: 4.0,
        initial_presence: Vec::new(),
        hits,
    };
    let mut layers = walk::walk_ray(0, &corridor, &table, &WalkLaws::default())
        .expect("the fixture slabs walk")
        .layers;
    layers.retain(|layer| layer.end > BEHIND);
    (layers, volumes)
}

/// A direction stops where it enters the first volume: a box there is the one dosed and nothing
/// beyond it is reached; armour there doses nothing and shadows every box behind it; the plate the
/// throw leaves stops nothing. A hard body passed through, a box beyond the first dosed, armour
/// that does not stop, or the left plate stopping the direction at the throw point fails it.
/// Fixture slabs along the direction.
#[test]
fn a_direction_stops_at_the_first_volume_it_enters() {
    let walk = |armour_first: bool| {
        let mut slabs = vec![(-0.15, 0.05, 1.0), (0.9, 1.1, 0.0), (1.9, 2.1, 0.0)];
        if armour_first {
            slabs.push((0.45, 0.55, 1.0));
        }
        let (layers, volumes) = slab_layers(&slabs);
        let boxes = [volumes[1], volumes[2]];
        let (at, dosed) = first_solid(&layers, BEHIND, &[volumes[0]], &|volume| {
            boxes.contains(&volume)
        })
        .expect("the direction enters a slab");
        (at - BEHIND, dosed, boxes)
    };
    let (depth, dosed, boxes) = walk(false);
    assert_eq!(dosed, Some(boxes[0]), "the near box");
    assert!(
        (depth - 0.9).abs() < 1.0e-6,
        "stopped at the near box's face: {depth}"
    );
    let (depth, dosed, _) = walk(true);
    assert_eq!(dosed, None, "armour shadows the boxes");
    assert!(
        (depth - 0.45).abs() < 1.0e-6,
        "stopped at the plate: {depth}"
    );
}

// ---------------------------------------------------------------------------------------------
// One direction of a field, walked: ordered presence along the ray, membership established
// ---------------------------------------------------------------------------------------------

/// One direction of strength `strength` and weight 1 thrown from `origin` along `-Y` through every
/// volume of the world (volumes of no tank: one tank), with `exited` the volumes the throw leaves:
/// where it ends, and each dose it addressed, in entry order.
fn walk_direction(
    app: &mut App,
    origin: Vec3,
    strength: f32,
    exited: &[Entity],
) -> (Vec3, Vec<(Entity, f32)>) {
    let ray = field_rays(
        app,
        vec![(origin, Dir3::NEG_Y, strength)],
        1.0,
        exited.to_vec(),
    )
    .remove(0);
    (
        ray.end,
        ray.doses
            .into_iter()
            .map(|dose| (dose.address, dose.amount))
            .collect(),
    )
}

/// The dose `walk_direction` addressed to `target`, summed.
fn dose_to(doses: &[(Entity, f32)], target: Entity) -> f32 {
    doses
        .iter()
        .filter(|(address, _)| *address == target)
        .map(|(_, amount)| amount)
        .sum()
}

/// A box of `size` spanning `top` down to `top - size.y` on the `-Y` axis.
fn slab_below(app: &mut App, top: f32, thickness: f32, factor: f32, hp: bool) -> Entity {
    volume(
        app,
        Vec3::new(1.0, thickness, 1.0),
        Transform::from_translation(Vec3::new(0.0, top - thickness * 0.5, 0.0)),
        factor,
        hp.then_some(FIELD_HP),
    )
}

/// A plate thinner than any nudge is still material: a direction meeting half a millimetre of
/// steel stops at its face and the box behind takes nothing. A walk that loses a plate whose faces
/// lie within a nudge of each other fails it.
#[test]
fn a_plate_thinner_than_any_nudge_stops_a_direction() {
    let (mut app, boxes) = field_world(|app| {
        slab_below(app, -1.0, 0.0005, 1000.0, false);
        vec![slab_below(app, -2.0, 0.2, 0.0, true)]
    });
    let (end, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[]);
    assert_eq!(dose_to(&doses, boxes[0]), 0.0, "the box behind is shadowed");
    assert!(
        (end.y + 1.0).abs() < 2.0e-6,
        "the direction stops at the plate's face: {end:?}"
    );
}

/// A direction recognises the volume it is thrown from by identity, never by displacement: starting
/// a millimetre inside the box its throw leaves, it neither doses that box nor stops in it, and the
/// box it steps into takes its whole strength. Named as anything but the volume the throw leaves,
/// the same box is entered at the throw point: it takes the whole strength and stops the direction
/// there. A direction that doses or stops in the volume its throw leaves, or that passes through
/// one it starts inside without leaving, fails it. Fixture: a box ending 1 mm along the direction,
/// a box in contact behind it.
#[test]
fn a_direction_does_not_dose_the_volume_its_throw_leaves() {
    let (mut app, boxes) = field_world(|app| {
        vec![
            slab_below(app, 0.5, 0.501, 150.0, true),
            slab_below(app, -0.001, 0.5, 20.0, true),
        ]
    });
    let (end, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[boxes[0]]);
    assert_eq!(dose_to(&doses, boxes[0]), 0.0, "the throw's own box");
    assert_eq!(dose_to(&doses, boxes[1]), 10.0, "the box stepped into");
    assert!(
        (end.y + 0.001).abs() < 1.0e-5,
        "stopped where it steps into the box behind: {end:?}"
    );
    let (end, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[]);
    assert_eq!(
        dose_to(&doses, boxes[0]),
        10.0,
        "entered at the throw point"
    );
    assert_eq!(dose_to(&doses, boxes[1]), 0.0, "stopped in the first box");
    assert_eq!(end, Vec3::ZERO, "stopped at the throw point");
}

// ---------------------------------------------------------------------------------------------
// A certified convex body is crossed or missed, never a walk failure
// ---------------------------------------------------------------------------------------------

/// Where the off-centre discs strike the fixture trunk, and its radius there: DERIVED from the
/// fixture's first segment, 0.3 m tapering to 0.2 m over 2 m (`trees::tests::fixture_asset`).
const TRUNK_SHOT_HEIGHT: f32 = 1.0;
const TRUNK_RADIUS_AT_SHOT: f32 = 0.25;

/// Stand-offs from a march step's anchor to the trunk: up to one tick's travel, DERIVED as
/// 800 m/s × 15.625 ms = 12.5 m. The anchor's distance sets the scale every corridor parameter is
/// rounded at, so the sweep covers the range a step can start from.
const TRUNK_STANDOFFS: [f32; 5] = [0.5, 2.0, 5.3, 9.7, 12.3];

/// A CERTIFIED CONVEX BODY IS CROSSED WITH A CHORD AT ANY OFFSET AND INCIDENCE (the game's damage
/// model: a trunk segment is one HP pool, and the line's chord through it carries its dose).
///
/// The trees' own 12-gon frustum collider (`trees::spawn`) struck off-centre by the fixture 88's
/// disc: one disc wholly outboard of the 12-gon's 30° vertex, which meets only the facet whose
/// normal stands 75° off the line of fire, and discs at 0.70–1.10 of the trunk's radius toward its
/// 0° vertex. Every disc meets the trunk, and the fixture round bites its low-resistance material, so each must
/// bite in and lay a positive chord into the trunk. The game's damage model treats a structured
/// walk contradiction as a bug and panics, so the generic convex collector must pair honestly.
///
/// What it holds the convex narrow phase to: `collect::collect_convex` reads the hull through
/// `collect::clip_convex`, one entry strictly before one exit or none, each at the exact plane
/// parameter of a stored face. So the entrance and the transit corridors place a face within the
/// hand-off band of each other (`walk::topology_band` with `collect::sitting_reach`, DERIVED:
/// about 1e-5 m at 10 m), and the partition between the seed (`walk::RayWalk::inside_at`) and the
/// corridor (`collect::admit`) counts every boundary once. An approximate reading that stops short
/// of a face by more than that band puts a seed's hand-off point between its reading and the
/// exact one, and the walk refuses the second entry (`UnexpectedEntry`); one that reads an exit
/// before an entry at a vertical edge is `UnexpectedExit`; one that reads an entry with no exit is
/// `IncompleteCorridor`. Each is a bug in the game's damage model and panics in gameplay.
#[test]
fn a_convex_trunk_is_crossed_with_a_chord_at_any_offset_and_incidence() {
    let mut app = world_with_trimesh_plates(&[]);
    let segment = {
        let world = app.world_mut();
        let mut queue = bevy::ecs::world::CommandQueue::default();
        let mut commands = Commands::new(&mut queue, world);
        let tree = crate::trees::spawn(
            &mut commands,
            &crate::trees::tests::fixture_kind_of("Wood", 100),
            crate::scatter::MapInstance {
                id: "trunk".to_owned(),
                ordinal: 0,
            },
            Transform::IDENTITY,
        );
        queue.apply(world);
        // The segment the shot height lies in: the first, 0 to 2 m up.
        world.get::<Children>(tree).expect("segments")[0]
    };
    for _ in 0..8 {
        app.update();
    }

    let radius = ROUND.caliber * 0.5;
    // Wholly outboard of the 30° vertex by 2 mm, so the disc meets the 75° facet alone.
    let graze = (
        "75° graze".to_owned(),
        30.0_f32.to_radians().cos() * TRUNK_RADIUS_AT_SHOT + radius + 0.002,
    );
    let offsets = [0.70, 0.80, 0.90, 0.95, 1.00, 1.05, 1.10]
        .map(|frac: f32| (format!("{frac:.2} r"), frac * TRUNK_RADIUS_AT_SHOT));
    let direction = Vec3::NEG_Z;
    let mut failures = Vec::new();
    for (pose, x) in std::iter::once(graze).chain(offsets) {
        for standoff in TRUNK_STANDOFFS {
            let origin = Vec3::new(x, TRUNK_SHOT_HEIGHT, standoff);
            let chord = app
                .world_mut()
                .run_system_once(
                    move |world: ProjectileMarchWorld,
                          colliders: Query<(
                        &'static Position,
                        &'static Rotation,
                        &'static Collider,
                        Option<&'static BallisticSurfaces>,
                    )>| {
                        let context = ResolveContext {
                            world: &world,
                            colliders: &colliders,
                            deposit: false,
                            laws: WalkLaws::default(),
                        };
                        let frame = DiscFrame::anchored(direction).expect("a real direction");
                        let entrance = closing_walk(
                            &context,
                            &|_| true,
                            origin,
                            Vec3::ZERO,
                            direction,
                            frame,
                            radius,
                            &[],
                            standoff + ENTRANCE_SPAN,
                        )?;
                        let shot = Shot {
                            caliber: ROUND.caliber,
                            speed: SPEED,
                            contact: crate::rounds::ContactPreset::fixture(),
                        };
                        let Begin::Transit(request) = walk::begin(&entrance, &shot, &context.laws)?
                        else {
                            return Ok(None);
                        };
                        let starts = transit_starts(&context, &|_| true, &request)?;
                        let transit = closing_walk(
                            &context,
                            &|_| true,
                            request.anchor,
                            request.origin,
                            *request.axis,
                            request.frame,
                            request.radius,
                            &starts.seeds,
                            ENTRANCE_SPAN,
                        )?;
                        let round = bound(ROUND);
                        let plan = walk::finish(
                            &transit,
                            &request,
                            &round,
                            round.at(SPEED),
                            &LIMITS,
                            round.body().mass,
                        )?;
                        Ok::<_, WalkError>(plan.deposits.iter().map(|line| &line.dose).find_map(
                            |dose| match dose.source {
                                DoseSource::Line { chord, .. } if dose.address == segment => {
                                    Some(chord)
                                }
                                _ => None,
                            },
                        ))
                    },
                )
                .expect("the probe system runs");
            match chord {
                Ok(Some(chord)) if chord > 0.0 => {}
                other => failures.push(format!("{pose}, stand-off {standoff} m: {other:?}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "every disc that meets the trunk crosses it with a positive chord; {} of {} did not:\n{}",
        failures.len(),
        8 * TRUNK_STANDOFFS.len(),
        failures.join("\n"),
    );
}

// ---------------------------------------------------------------------------------------------
// The hand-off on pieces the disc meets at a graze, side-on or together
// ---------------------------------------------------------------------------------------------

/// A straight tube along local `Z`, `outer` and `bore` radii, `length` long, as a certified trimesh:
/// one closed, outward-wound shell of `segments` facets around, every vertex its own welded id.
fn tube_trimesh(
    outer: f32,
    bore: f32,
    length: f32,
    segments: u32,
) -> (Collider, BallisticSurfaces) {
    let half = length * 0.5;
    // Rings 0 and 1 are the outside at `-half` and `+half`, rings 2 and 3 the bore.
    let rings = [(outer, -half), (outer, half), (bore, -half), (bore, half)];
    let vertices: Vec<Vec3> = rings
        .iter()
        .flat_map(|&(radius, z)| {
            (0..segments).map(move |i| {
                let angle = std::f32::consts::TAU * i as f32 / segments as f32;
                Vec3::new(radius * angle.cos(), radius * angle.sin(), z)
            })
        })
        .collect();
    let at = |ring: u32, i: u32| ring * segments + i % segments;
    let mut indices: Vec<[u32; 3]> = Vec::new();
    for i in 0..segments {
        let j = i + 1;
        // Outside, facing out; bore, facing the axis; the end at `-half` facing `-Z`, the end at
        // `+half` facing `+Z`.
        indices.push([at(0, i), at(0, j), at(1, j)]);
        indices.push([at(0, i), at(1, j), at(1, i)]);
        indices.push([at(2, i), at(3, j), at(2, j)]);
        indices.push([at(2, i), at(3, i), at(3, j)]);
        indices.push([at(0, i), at(2, j), at(0, j)]);
        indices.push([at(0, i), at(2, i), at(2, j)]);
        indices.push([at(1, i), at(1, j), at(3, j)]);
        indices.push([at(1, i), at(3, j), at(3, i)]);
    }
    let surfaces = BallisticSurfaces {
        shells: vec![0u32; indices.len()].into(),
        corners: indices.as_slice().into(),
    };
    (Collider::trimesh(vertices, indices), surfaces)
}

/// A TUBE CROSSED SIDE-ON RESOLVES WALL, BORE AND WALL.
///
/// The disc meets the near wall's curved face at a graze across its rim, and from the bore the far
/// wall's concave face, nearer at the rim than on the axis. A plane shared across the disc lies
/// inside one wall and short of the other on different samples, and behind the starts of the
/// samples already past it; each sample handing off on its own face does neither. Two tubes: one
/// whose bore is the disc's radius, so the rim samples run along the bore's edge and the whole
/// tube is one crossing, and one whose bore clears the disc, so the far wall is a second crossing
/// met from inside the bore. Each square to the round and tilted along it, so the walls are also
/// met obliquely, with lateral offsets across the whole tube. Every drive must walk, and the
/// centred round through the square tube must come out below it.
///
/// FIXTURE DATA: the tubes (radii, 2 m long, 24 facets), the tilts and the offsets.
#[test]
fn a_tube_crossed_side_on_resolves() {
    let tube = Vec3::new(DROP_FROM.x, 1.5, DROP_FROM.z);
    // (outside radius, bore radius, crossings the centred round makes at least)
    for (outer, bore, crossings) in [(0.08, 0.044, 1), (0.09, 0.06, 2)] {
        for (tilt, lateral) in [0.0_f32, 40.0, 65.0].into_iter().flat_map(|tilt| {
            [0.0, 0.01, -0.02, 0.035, -0.044, 0.06, -0.075].map(|lateral| (tilt, lateral))
        }) {
            let (mut app, _) = settled(|app| {
                vec![mesh_volume(
                    app,
                    tube_trimesh(outer, bore, 2.0, 24),
                    Transform::from_translation(tube + Vec3::X * lateral)
                        .with_rotation(Quat::from_rotation_x(tilt.to_radians())),
                )]
            });
            let flown = try_driven(&mut app, FIXTURE_88, 800.0).unwrap_or_else(|failure| {
                panic!("bore {bore}, tilt {tilt}°, lateral {lateral}: {failure:?}")
            });
            if tilt == 0.0 && lateral == 0.0 {
                let bites: Vec<&Bite> = flown
                    .decisions
                    .iter()
                    .filter_map(|decision| match decision {
                        Decision::Bite(bite) => Some(bite.as_ref()),
                        _ => None,
                    })
                    .collect();
                assert!(
                    bites.len() >= crossings,
                    "bore {bore}: the centred round makes {} crossings",
                    bites.len()
                );
                assert!(
                    bites.iter().any(|bite| matches!(
                        bite.end,
                        BiteEnd::Perforated { resume, .. } if resume.y < tube.y - outer
                    )),
                    "bore {bore}: the centred round comes out below the tube"
                );
            }
        }
    }
}

/// The three rounds a 30-bot server stopped at first contact in 240 s (`CorridorMismatch`) when
/// every sample handed off on the first piece's mean plane, as tank-local start and velocity (m,
/// m/s): a grazed piece in front of another, a barrel's far wall, a grazed edge beside a piece at
/// overlapping depth. FIXTURE DATA, MEASURED on that run's log and replayed at the fixture tank's
/// spawn pose.
const FIRST_CONTACT_REPLAYS: [(Vec3, Vec3); 3] = [
    (
        Vec3::new(-0.484_449_24, 2.283_379, 10.736_783),
        Vec3::new(4.001_924_5, -2.115_455_6, -772.9867),
    ),
    (
        Vec3::new(-3.105_393_4, 2.203_470_7, 5.245_123_4),
        Vec3::new(225.038_28, -1.771_982_2, -739.5158),
    ),
    (
        Vec3::new(-0.038_930_178, 2.497_682_6, 0.872_706),
        Vec3::new(740.6852, 54.846_77, -214.257_52),
    ),
];

/// The round the replays fired, a key in the round registry: fixture data.
const REPLAY_ROUND: &str = "88mm_APHE_ring";

/// THE REPLAYED FIRST-CONTACT STOPS RESOLVE.
///
/// The game's damage model: each replay meets armour through the real march and resolves a
/// crossing or bounce. The runtime consumer needs an actual contact; an empty log proves nothing.
#[test]
fn the_replayed_first_contact_stops_resolve() {
    let round = crate::guns::GunRegistry::shipped()
        .round_by_key(REPLAY_ROUND)
        .unwrap_or_else(|err| panic!("the replays' round: {err}"));
    for (index, (local, velocity)) in FIRST_CONTACT_REPLAYS.into_iter().enumerate() {
        let mut app = fuzz::probe_world().expect("the bound tank builds");
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_micros(15_625),
        ))
        .insert_resource(RetainSpentShells(true))
        .add_systems(Update, crate::ballistics::integrate_projectiles)
        .add_observer(crate::ballistics::on_fire_shell);
        let mut fire = crate::ballistics::FireShell::of_round(
            &round,
            fuzz::PROBE_TANK_AT + local,
            fuzz::PROBE_TANK_AT + local,
            Dir3::new(velocity).expect("a replay has a direction"),
            crate::spec::FireMechanism::Single,
            false,
            None,
            None,
        );
        fire.speed = velocity.length();
        app.world_mut().trigger(fire);
        for _ in 0..4 {
            app.update();
        }
        let logs: Vec<Vec<ArmourMark>> = app
            .world_mut()
            .query::<&crate::ballistics::PenetrationMarks>()
            .iter(app.world())
            .map(|marks| marks.log.clone())
            .collect();
        assert_eq!(logs.len(), 1, "replay {index}: one round");
        assert!(
            logs[0].iter().any(|mark| matches!(
                mark,
                ArmourMark::Crossing { .. } | ArmourMark::Bounce { .. }
            )),
            "replay {index}: the round must resolve a crossing or bounce, got {:?}",
            logs[0]
        );
    }
}

/// The game's damage-model fixtures: one axis sample through synthetic, closed hard slabs.
fn exposed_walk(slabs: &[(Entity, f64, f64, f32)]) -> DiscWalk {
    let mut hits = Vec::new();
    for &(volume, start, end, _) in slabs {
        for (triangle, t, entering) in [(0, start, true), (1, end, false)] {
            hits.push(FaceHit {
                volume,
                primitive: volume,
                shell: 0,
                contact: walk::Contact::Face(triangle),
                triangle,
                t,
                true_normal: if entering { Vec3::NEG_Z } else { Vec3::Z },
                entering,
            });
        }
    }
    walk::walk_disc(
        &DiscCorridor {
            anchor: Vec3::ZERO,
            origin: Vec3::ZERO,
            axis: Vec3::Z,
            length: 2.0,
            radius: 0.0,
            frame: DiscFrame::anchored(Vec3::Z).expect("a unit axis"),
            samples: vec![SampleCorridor {
                offset: Vec3::ZERO,
                initial_presence: Vec::new(),
                hits,
            }],
        },
        &VolumeTable::new(
            slabs
                .iter()
                .map(|&(volume, _, _, factor)| (volume, walk::VolumeMaterial::fixture(factor))),
        ),
        &WalkLaws::default(),
    )
    .expect("closed fixture slabs")
}

/// The game's damage model reports the dosed portion for hard volumes, as it does witnesses.
/// Pool-emptying events and the pen cam must not use a buried face as the dose's entry or end;
/// resting later in a hidden portion does not untraverse the exposed portion.
#[test]
fn hard_volume_records_use_only_exposed_contacts() {
    let plate = Entity::from_raw_u32(1).unwrap();
    let module = Entity::from_raw_u32(2).unwrap();
    for (plate_span, module_span, rest, target, exposed, perforated) in [
        ((0.25, 1.0), (0.75, 1.5), None, module, (1.0, 1.5), true),
        (
            (1.0, 1.75),
            (0.5, 1.5),
            Some(1.25),
            module,
            (0.5, 1.0),
            true,
        ),
        (
            (0.25, 1.0),
            (0.75, 1.5),
            Some(1.5),
            module,
            (1.0, 1.5),
            false,
        ),
        (
            (1.0, 1.75),
            (0.5, 1.5),
            Some(1.25),
            plate,
            (1.0, 1.25),
            false,
        ),
    ] {
        let mut walked = exposed_walk(&[
            (plate, plate_span.0, plate_span.1, 1000.0),
            (module, module_span.0, module_span.1, 10.0),
        ]);
        // Fixture: an exposed rim sample, offset from the shared axis readout.
        walked.offsets[0] = Vec3::X * 0.03;
        let doses = [Dose {
            address: target,
            amount: 1.0,
            source: DoseSource::Line {
                chord: 0.5,
                cost: 5.0,
            },
        }];
        let exchanges = [walk::Exchange {
            dose: doses[0],
            debris: Vec::new(),
            momentum: Vec3::Z,
            entrance_momentum: Vec3::ZERO,
            entrance_energy: 0.0,
        }];
        let crossed =
            volume_crossings(&walked, &walked.events[0], &exchanges, rest, &|point| point);
        let contacts = volume_contacts(&walked, &walked.events[0], rest, &|point| point);
        let sampled = &contacts[&target];
        assert_eq!(
            sampled.first().unwrap().0,
            Vec3::X * 0.03 + Vec3::Z * exposed.0
        );
        assert_eq!(
            sampled.last().unwrap().1,
            Vec3::X * 0.03 + Vec3::Z * exposed.1
        );
        assert!(
            (sampled.iter().map(|(a, b)| a.distance(*b)).sum::<f32>() - (exposed.1 - exposed.0))
                .abs()
                < 1e-6
        );
        assert_eq!(crossed[0].energy, exchanges[0].dose.amount);
        assert_eq!(crossed.len(), 1);
        assert_eq!(crossed[0].entry, Vec3::Z * exposed.0);
        assert_eq!(crossed[0].end, Vec3::Z * exposed.1);
        assert_eq!(
            crossed[0].perforated, perforated,
            "the exposed portion must be left before a rest; an exact exit face is still held"
        );
    }
}

/// The game's damage model's resolved leg owns the hard presence used by the witness pass.
/// A later collider read cannot replace it: a mutant that recasts this deliberately unreadable
/// binding errors instead of reporting the already-resolved open chord.
#[test]
fn witness_lines_reuse_the_resolved_hard_presence() {
    use crate::ballistics::{WitnessSurface, WitnessVolume};
    let mut app = world_with_trimesh_plates(&[]);
    let hard = app
        .world_mut()
        .spawn((
            Transform::from_translation(Vec3::Z * 0.75),
            RigidBody::Static,
            Collider::sphere(0.5),
            CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(STEEL),
                substance: "fixture".to_owned(),
            },
        ))
        .id();
    let witness = app
        .world_mut()
        .spawn((
            Transform::from_translation(Vec3::Z),
            RigidBody::Static,
            Collider::cuboid(0.25, 0.25, 1.5),
            CollisionLayers::new([crate::Layer::Witness], LayerMask::ALL),
            WitnessVolume {
                substance: "fixture".to_owned(),
            },
            WitnessSurface,
        ))
        .id();
    for _ in 0..8 {
        app.update();
    }
    let walked = exposed_walk(&[(hard, 0.25, 1.0, STEEL)]);
    let leg = Leg::new(Vec3::ZERO, Dir3::Z, 2.0, &[Vec3::ZERO]).with_presence(&walked);
    let crossed = app
        .world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                super::super::wound::line(&context, &|_| true, std::slice::from_ref(&leg), 1.0)
                    .expect("resolved hard presence is reused")
                    .0
            },
        )
        .expect("the witness system runs");
    assert_eq!(crossed.len(), 1);
    assert_eq!(crossed[0].volume, witness);
    assert_eq!(crossed[0].entry, Vec3::Z);
    assert_eq!(crossed[0].end, Vec3::Z * 1.75);
    assert_eq!(crossed[0].chord, 0.75);
}

/// The game's damage model: material closure is bounded by loaded geometry, never a chosen
/// distance. The crossing resolver needs the real exit even when one solid is larger than a tick.
#[test]
fn a_finite_material_crossing_longer_than_fifty_metres_closes() {
    // Fixture data: a closed sixty-metre prism, entered one metre from the sample's start.
    let (mut app, _) = settled(|app| {
        vec![mesh_volume(
            app,
            box_trimesh(Vec3::new(1.0, 1.0, 60.0)),
            Transform::IDENTITY,
        )]
    });
    let walked = app
        .world_mut()
        .run_system_once(|world: ProjectileMarchWorld, colliders: Colliders| {
            let context = ResolveContext {
                world: &world,
                colliders: &colliders,
                deposit: false,
                laws: WalkLaws::default(),
            };
            closing_walk(
                &context,
                &|_| true,
                Vec3::new(0.01, 0.02, -31.0),
                Vec3::ZERO,
                Vec3::Z,
                DiscFrame::anchored(Vec3::Z).unwrap(),
                0.0,
                &[],
                1.0 + ENTRANCE_SPAN,
            )
        })
        .unwrap()
        .expect("finite certified material has a real exit");
    assert_eq!(walked.events.len(), 1, "the prism is one crossing");
    assert_eq!(walked.events[0].start, 1.0);
    assert_eq!(walked.events[0].end, 61.0);
}

/// The game's damage model's mounted decide path must use pressure and the loaded slope preset.
/// Mutants: ignoring the slope preset or charging by changing travel length.
#[test]
fn mounted_contact_skids_a_thin_graze_bites_head_on_and_charges_only_resistance() {
    let contact = crate::rounds::ContactPreset::fixture();
    let (graze, _) = shot_onto_slab_with_contact(
        FIXTURE_88,
        773.0,
        85.0_f32.to_radians(),
        0.01,
        STEEL,
        contact,
    );
    first_bounce(&graze);
    let (head_on, _) = shot_onto_slab_with_contact(FIXTURE_88, 773.0, 0.0, 0.4, STEEL, contact);
    assert!(matches!(head_on.decisions.first(), Some(Decision::Bite(_))));
    let bite = |preset| {
        let (flight, _) = shot_onto_slab_with_contact(
            FIXTURE_88,
            773.0,
            30.0_f32.to_radians(),
            0.01,
            STEEL,
            preset,
        );
        let Some(Decision::Bite(bite)) = flight.decisions.into_iter().next() else {
            panic!("the fixture slope bites");
        };
        bite
    };
    let charged = bite(contact);
    let uncharged = bite(crate::rounds::ContactPreset {
        slope_tan: 0.0,
        ..contact
    });
    let cost = |bite: &Bite| bite.volumes.iter().map(|v| v.cost).sum::<f32>();
    assert!(cost(&charged) > cost(&uncharged));
    assert!((charged.travel - uncharged.travel).abs() < 1.0e-5);
    assert_eq!(charged.legs[1].axis, charged.incoming);
    assert_eq!(charged.legs[1].axis, uncharged.legs[1].axis);
    for (a, b) in charged.volumes.iter().zip(&uncharged.volumes) {
        assert_eq!((a.entry, a.end), (b.entry, b.end));
    }
    // The mounted clock uses real progress between the charged states. Mutant: charge its length.
    use crate::ballistics::mechanism::Mechanism;
    let mechanism = bound(FIXTURE_88);
    for motion in &charged.motion {
        let expected = mechanism.flight_time(
            mechanism.at(motion.speed_in as f32),
            mechanism.at(motion.speed_out as f32),
            motion.end - motion.start,
        );
        assert!((motion.seconds - expected).abs() < 1.0e-8);
    }

    match (&charged.end, &uncharged.end) {
        (BiteEnd::Perforated { speed: a, .. }, BiteEnd::Perforated { speed: b, .. }) => {
            assert!(a < b)
        }
        _ => panic!("both fixture bites perforate"),
    }
}

/// The game's damage model: a first-contact skid keeps the current flying assembly. Mutant:
/// using penetrator mass for work while impulse reads assembly mass violates one-body exchange.
#[test]
fn a_kept_carrier_skid_reads_one_body_for_work_and_impulse() {
    let mut app = world_with_trimesh_plates(&[]);
    let angle = 75.0_f32.to_radians(); // Fixture geometry and assembly mass.
    let rotation = Quat::from_rotation_z(angle);
    volume(
        &mut app,
        Vec3::new(12.0, 0.4, 3.0),
        Transform::from_translation(
            Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z) - rotation * Vec3::Y * 0.2,
        )
        .with_rotation(rotation),
        STEEL,
        None,
    );
    for _ in 0..8 {
        app.update();
    }
    let mass = ROUND.mass * 4.0;
    let decision = app
        .world_mut()
        .run_system_once(
            move |world: ProjectileMarchWorld,
                  colliders: Query<(
                &'static Position,
                &'static Rotation,
                &'static Collider,
                Option<&'static BallisticSurfaces>,
            )>| {
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                let direction = Dir3::NEG_Y;
                let frame = DiscFrame::anchored(*direction).unwrap();
                let flight = Flight::born(
                    &context,
                    &|_| true,
                    DROP_FROM,
                    direction,
                    direction,
                    SPEED,
                    frame,
                    disc_radius(ROUND.caliber),
                )
                .unwrap();
                decide(
                    &context,
                    &|_| true,
                    &bound(ROUND),
                    mass,
                    crate::rounds::ContactPreset::fixture(),
                    &flight,
                    DROP_FROM.y - 1.0,
                )
                .unwrap()
            },
        )
        .unwrap();
    let Decision::Bounce(bounce) = decision else {
        panic!("the fixture assembly skids");
    };
    let expected_work = (0.5
        * f64::from(mass)
        * (f64::from(bounce.speed_in).powi(2) - f64::from(bounce.speed).powi(2)))
        as f32;
    assert!((bounce.shock - expected_work).abs() < 1.0e-3);
    let expected_impulse =
        mass * (*bounce.incoming * bounce.speed_in - *bounce.direction * bounce.speed);
    assert!((bounce.impulse - expected_impulse).length() < 1.0e-3);
}

/// The game's damage model: impact fuzes read dE/dx along real travel, including slope charge.
/// Mutant: an uncharged entrance force misses a sensitivity between F and qF.
#[test]
fn impact_fuze_reads_the_charged_entry_force() {
    // Fixture geometry: a biting 30-degree face; both reads meet the same leading samples.
    let mut app = world_with_trimesh_plates(&[]);
    let angle = 30.0_f32.to_radians();
    let rotation = Quat::from_rotation_z(angle);
    volume(
        &mut app,
        Vec3::new(12.0, 0.01, 3.0),
        Transform::from_translation(
            Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z) - rotation * Vec3::Y * 0.005,
        )
        .with_rotation(rotation),
        STEEL,
        None,
    );
    for _ in 0..8 {
        app.update();
    }
    let (force, uncharged) = app
        .world_mut()
        .run_system_once(|world: ProjectileMarchWorld, colliders: Colliders| {
            let context = ResolveContext {
                world: &world,
                colliders: &colliders,
                deposit: false,
                laws: WalkLaws::default(),
            };
            let mut flight = Flight {
                position: DROP_FROM,
                direction: Dir3::NEG_Y,
                speed: SPEED,
                frame: DiscFrame::anchored(Vec3::NEG_Y).unwrap(),
                continuation: None,
                membership: Membership::Required,
            };
            flight
                .settle(&context, &|_| true, disc_radius(ROUND.caliber))
                .unwrap();
            let force = |preset| {
                entry_force(
                    &context,
                    &|_| true,
                    &bound(ROUND),
                    preset,
                    &flight,
                    DROP_FROM.y - 1.0,
                )
                .unwrap()
                .expect("the fixture face is occupied")
                .0
            };
            let preset = crate::rounds::ContactPreset::fixture();
            (
                force(preset),
                force(crate::rounds::ContactPreset {
                    slope_tan: 0.0,
                    ..preset
                }),
            )
        })
        .unwrap();
    let preset = crate::rounds::ContactPreset::fixture();
    let charge = 1.0 / (1.0 - f64::from(angle.tan() * preset.slope_tan));
    assert!((force / uncharged - charge).abs() < 1.0e-6);
    let sensitivity = 0.5 * (uncharged + uncharged * charge);
    assert!(uncharged < sensitivity && force > sensitivity);
    let burst = crate::rounds::RoundRegistry::shipped()
        .get("88mm_HE")
        .unwrap()
        .burst
        .unwrap();
    let mut fuze = super::super::burst::Fuze::Unarmed(super::super::burst::Burst {
        trigger: crate::rounds::fuze::Trigger::Impact {
            sensitivity_n: sensitivity as f32,
        },
        ..burst
    });
    fuze.entered(force, SPEED, None, 0.0);
    assert!(matches!(fuze, super::super::burst::Fuze::Armed { .. }));
}

/// The game's damage model: every skid advances the clock or ends the round. A narrow converging
/// wedge cannot exchange forever at one axis point. All dimensions and speeds are fixture data.
#[test]
fn a_converging_wedge_spends_time_or_holds_the_round() {
    for air_angle in [10.0_f32, 2.0] {
        let mut app = world_with_trimesh_plates(&[]);
        let radius = disc_radius(FIXTURE_88.caliber);
        let left = DROP_FROM.x - radius - REST_CLEARANCE;
        volume(
            &mut app,
            Vec3::new(0.4, 16.0, 3.0),
            Transform::from_xyz(left - 0.2, 2.0, DROP_FROM.z),
            STEEL,
            Some(1000),
        );
        let rotation = Quat::from_rotation_z((90.0 - air_angle).to_radians());
        let apex = Vec3::new(left, 0.0, DROP_FROM.z);
        volume(
            &mut app,
            Vec3::new(16.0, 0.4, 3.0),
            Transform::from_translation(apex + rotation * (Vec3::X * 8.0 - Vec3::Y * 0.2))
                .with_rotation(rotation),
            STEEL,
            Some(1000),
        );
        for _ in 0..8 {
            app.update();
        }
        let flown = driven(&mut app, FIXTURE_88, 773.0);
        let mut skids = 0;
        for decision in &flown.decisions {
            if let Decision::Bounce(bounce) = decision {
                skids += 1;
                assert!(
                    bounce.travel > 0.0 || bounce.continuation.is_none(),
                    "{air_angle} degrees: an exchange must spend time or end the round: {bounce:?}"
                );
            }
        }
        assert!(skids > 0, "the wedge fixture reaches contact");
    }
}

/// The game's damage model never rewinds a paid crossing or exchanges against a future plane.
/// Fixture: the axis already stands below the disc's resting height, or exactly at it, while a
/// finite strip's first contact is still ahead. Mutant: negative travel or an immediate exchange.
#[test]
fn a_skid_never_rewinds_and_a_future_finite_contact_is_reached() {
    let read = walk::EntranceRead {
        position: Vec3::ZERO,
        volume: Entity::PLACEHOLDER,
        normal: Vec3::Y,
        coverage: 1.0,
        steel_equivalent: 400.0,
        resistance_charge: 1.0,
    };
    let axis = Dir3::new(Vec3::new(0.0, -0.1, 1.0)).unwrap();
    assert_eq!(
        skid_turn(Vec3::Y * 0.005, axis, &read, 0.044, 0.044, 0.3, 0.0),
        0.0
    );
    assert_eq!(
        skid_turn(
            Vec3::Y * (0.044 + REST_CLEARANCE),
            axis,
            &read,
            0.044,
            0.044,
            2.0,
            1.4
        ),
        1.4
    );
}

/// The game's damage model: a round flown as one line never turns onto the face it skids on, from
/// wherever its tick ended short of that face. On the face its slide would lie in the face's plane,
/// where the containment probe's parity has no answer and the march panics. Fixture: a line round's
/// start under its resting height by a drawn share of it, and one already within the clearance,
/// which has no room to turn and ends. Mutant: turn at the first touch, which stands the axis on the
/// face.
#[test]
fn a_line_round_never_turns_onto_the_face_it_skids_on() {
    let body = FIXTURE_MG;
    let rest = body.caliber * 0.5;
    assert_eq!(disc_radius(body.caliber), 0.0, "the fixture flies one line");
    for incidence in [40.0_f32, 60.0, 80.0] {
        let incidence = incidence.to_radians();
        let at_height = |height: f32| {
            let mut app = world_with_trimesh_plates(&[]);
            let rotation = Quat::from_rotation_z(incidence);
            let face = Face {
                contact: DROP_FROM + Vec3::NEG_Y * (height / incidence.cos()),
                rotation,
            };
            volume(
                &mut app,
                Vec3::new(12.0, 0.4, 3.0),
                Transform::from_translation(face.at(0.0, -0.2)).with_rotation(rotation),
                STEEL,
                Some(1000),
            );
            for _ in 0..8 {
                app.update();
            }
            (driven(&mut app, body, 500.0), face)
        };
        for height in [1.5 * REST_CLEARANCE, 0.5 * rest, rest] {
            let (flown, face) = at_height(height);
            let bounce = first_bounce(&flown);
            assert!(bounce.continuation.is_some(), "{height} m: it slides");
            let stands = (bounce.resume - face.contact).dot(face.normal());
            assert!(
                (stands - REST_CLEARANCE).abs() < 1.0e-5,
                "{height} m: the axis turned {stands} m off the face"
            );
        }
        let (flown, _) = at_height(0.5 * REST_CLEARANCE);
        let bounce = first_bounce(&flown);
        assert!(
            bounce.continuation.is_none() && bounce.travel == 0.0,
            "no room to turn: {bounce:?}"
        );
    }
}

/// The game's damage model: a containment probe ends outside its candidates' grown bounds even from
/// a start a hair past one slab, on a ray nearly parallel to it. World bounds round, so that start
/// can lie inside the face the slab stands for, and the corridor read from a probe that begins there
/// meets an exit with no entry. Fixture: a unit box, a start 0.24 um past its face and a heading
/// 0.08 urad into it. Mutant: leave the ungrown box, which ends the probe at its margin.
#[test]
fn a_probe_leaves_the_grown_bounds_from_a_start_past_a_slab() {
    let (lo, hi) = (Vec3::splat(-1.0), Vec3::splat(1.0));
    let start = Vec3::new(1.0 + 2.4e-7, 0.0, 0.0);
    for axis in [
        Vec3::new(-8.0e-8, 0.85, -0.52).normalize(),
        Vec3::new(8.0e-8, -0.85, 0.52).normalize(),
    ] {
        let (behind, ahead) = probe_reach(&[start], axis, lo, hi);
        let outside = |point: Vec3| {
            let margin = 0.99 * PROBE_MARGIN;
            point.cmplt(lo - margin).any() || point.cmpgt(hi + margin).any()
        };
        assert!(
            outside(start - axis * behind) && outside(start + axis * ahead),
            "{axis}: behind {behind} m, ahead {ahead} m"
        );
    }
}

/// The game's damage model: a blend closes its share of the angle to its target at any angle a unit
/// direction can hold. A low-coverage contact's chain ends only because each exchange brings the
/// heading closer to the face's plane. Mutant: read the angle with `acos`, which leaves a heading
/// under 3.45e-4 rad (DERIVED) unturned.
#[test]
fn a_blend_closes_its_share_of_any_angle() {
    let target = Dir3::new(Vec3::new(0.3, 0.1, -0.9)).unwrap();
    let side = Dir3::new(target.cross(Vec3::Y)).unwrap();
    let between = |a: Dir3, b: Dir3| {
        let (a, b) = (a.as_dvec3(), b.as_dvec3());
        a.cross(b).length().atan2(a.dot(b)) as f32
    };
    for angle in [1.0e-1_f32, 1.0e-2, 1.0e-3, 3.0e-4, 1.0e-4, 1.0e-5, 3.0e-6] {
        let dir = Dir3::new(Quat::from_axis_angle(*side, angle) * *target).unwrap();
        let start = between(dir, target);
        let left = between(super::super::turned_toward(dir, target, 0.5), target);
        assert!(
            (left - 0.5 * start).abs() < 2.0e-7,
            "{start} rad closes half to {left} rad"
        );
    }
}

/// The game's damage model: a chain of low-coverage contacts on one face ends, because each contact
/// brings the heading closer to the face's plane until the rest of the step fits between the rim and
/// the face. Fixture: a heading 1.1e-3 rad into a face, blended at the least coverage a ring reads
/// (1/13), which reaches a unit direction's rounding in about 110 contacts (DERIVED: ln(1.1e-3 /
/// 1e-7) / ln(13 / 12)). Mutant: read the angle with `acos`, which freezes the heading 3.45e-4 rad
/// into the face and walks the rest of the step in contacts of rounding's length, past the step's
/// crossing limit.
#[test]
fn a_chain_of_rim_contacts_converges_on_the_face() {
    let normal = Dir3::new(Vec3::new(0.2, 0.9, -0.3)).unwrap();
    let side = Dir3::new(normal.cross(Vec3::X)).unwrap();
    let along = normal.cross(*side);
    let into = Quat::from_axis_angle(*side, -1.1e-3) * along;
    assert!(into.dot(*normal) < 0.0, "the fixture heads into the face");
    let mut heading = Dir3::new(into).unwrap();
    for _ in 0..130 {
        let slide = Dir3::new(*heading - *normal * heading.dot(*normal)).unwrap();
        heading = super::super::turned_toward(heading, slide, 1.0 / 13.0);
    }
    let left = heading.dot(*normal).abs();
    assert!(left < 1.0e-6, "the heading stands {left} rad off the face");
}
