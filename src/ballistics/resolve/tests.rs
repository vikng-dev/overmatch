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
/// hand-off, which sizes the seed's "on the hand-off plane" band (DERIVED: 5.2e-6 m at 10.6 m).
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
/// Each ring sample's entry face lies within rounding of the mean entrance plane the transit hands
/// off on, so whether the seed or the transit corridor owns it is decided inside the topology band
/// — the place a dropped entry (`UnexpectedExit`) or a doubled one (`UnexpectedEntry`) lives. A
/// plate in general orientation, struck at up to 60° anywhere on its face, near the world origin
/// and far from it, from the march's own stand-off before contact. Every pose the round bites into must hand off with every sample's
/// entry owned once and the transit corridor pairing; the terminal outcome is not asserted.
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
                let result = app
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
                            };
                            let Begin::Transit(request) =
                                walk::begin(&entrance, &shot, &context.laws)
                            else {
                                return Ok(None);
                            };
                            let corridor = build_corridor(
                                &context,
                                &|_| true,
                                request.anchor,
                                request.origin,
                                request.axis,
                                ENTRANCE_SPAN,
                                request.frame,
                                request.radius,
                                &request.seeds,
                            )?;
                            let transit = closing_walk(
                                &context,
                                &|_| true,
                                request.anchor,
                                request.origin,
                                request.axis,
                                request.frame,
                                request.radius,
                                &request.seeds,
                                ENTRANCE_SPAN,
                            )?;
                            let round = bound(ROUND);
                            walk::finish(
                                &transit,
                                &request,
                                &round,
                                round.at(SPEED),
                                &LIMITS,
                                &context.laws,
                            )?;
                            Ok::<_, WalkError>(Some(corridor))
                        },
                    )
                    .expect("the probe system runs");
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

// ---------------------------------------------------------------------------------------------
// One mechanism: the runtime march and the §13.6 gate's driver decide the same crossings
// ---------------------------------------------------------------------------------------------

use crate::ballistics::fuzz::{self, Drive};
use crate::ballistics::march_tests::{FIXTURE_88, FIXTURE_MG, spawn_round};
use crate::ballistics::{
    ComponentHealth, RetainSpentShells, SpallMarks, Verdict, drag_k, freeflight_step,
};

/// The fixture tick the plate worlds step by (`march_tests::plate_world`'s manual 16 ms).
const DT: f32 = 0.016;
/// Where every shot of this section starts: straight above the geometry, falling, so gravity and
/// drag change the speed but never the direction, and the gate's straight flight is the runtime's.
const DROP_FROM: Vec3 = Vec3::new(0.1, 3.0, -0.2);

/// The top of a wide target 6.5 m below a plate at `y = 1`, where a partly covered or overmatched
/// round lands whatever its bend. Fixture data.
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
        entity.insert(ComponentHealth {
            current: hp,
            max: hp,
        });
    }
    entity.id()
}

/// The speed and travel budget the runtime march flies its first tick with, from the shared advance.
fn first_tick(body: RoundBody, speed: f32) -> (f32, f32) {
    let stepped = freeflight_step(Vec3::NEG_Y * speed, drag_k(body.caliber, body.mass), DT);
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
                fuzz::drive(&context, body, speed, DROP_FROM, Vec3::NEG_Y, budget)
            },
        )
        .expect("the driver system runs")
}

/// The runtime's side: one tick of the real march, read back as impacts, and whether any direction
/// of a spall cone the round threw dosed a box.
fn marched(app: &mut App, body: RoundBody, speed: f32) -> (Vec<(Verdict, Vec3)>, bool) {
    let shell = spawn_round(app, body, DROP_FROM, Vec3::NEG_Y * speed);
    app.update();
    let field = app.world().get::<SpallMarks>(shell).is_some_and(|marks| {
        marks
            .cones
            .iter()
            .flat_map(|cone| &cone.rays)
            .any(|ray| ray.deposited)
    });
    (app.world().resource::<Read>().0.clone(), field)
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
    let (read, field) = marched(&mut runtime, body, speed);
    let hp = runtime
        .world()
        .get::<ComponentHealth>(target)
        .expect("the target carries HP");
    let damaged = hp.current < hp.max;

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
        .current;
    let transit = flown
        .decisions
        .iter()
        .filter_map(|decision| match decision {
            Decision::Bite(bite) => Some(&bite.deposits),
            _ => None,
        })
        .flatten()
        .filter(|dose| dose.address == gate_target)
        .fold(0.0, |total, dose| {
            total + dose.amount * crate::ballistics::TRANSIT_K
        });
    let admitted = pool > 0 && crate::ballistics::hp_points(transit) > 0;
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
/// It belongs to a tank of its own, so no spall cone thrown from the plates above it — volumes of no
/// tank — reaches it: a cone doses only the struck tank's boxes. Each arrangement also asserts that
/// no field dose landed ([`Agreement::transit_only`]).
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
/// rules, bends, stops, bounces and continuations, decided by the same state machine. Each
/// arrangement is generic geometry with a soft HP-bearing target below it; the round falls straight
/// down so the runtime's gravity and drag leave its direction alone, and the gate flies the
/// runtime's own first-tick speed and budget.
///
/// THE PARITY IS TRANSIT PARITY. The admission observes transit deposits; the runtime also removes
/// HP by a bounce's shock and by spall cones. The crossing arrangements are transit-only, asserted,
/// with every target in a tank of its own, outside the plates' cones ([`target_below`]). The last
/// two are the routes outside it, each with the round's own path kept clear of the HP-bearing box:
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

    // Bounce: a thin plate struck at 75° by a round it cannot overmatch, over a target narrow
    // enough that the bounced path (30° off the drop line) passes beside it. The straight line
    // through the plate costs less than the round can spend, so only the bounce keeps it out.
    let (speed, _) = first_tick(FIXTURE_MG, 755.0);
    assert!(
        0.003 / 75.0_f32.to_radians().cos() * STEEL < capability(FIXTURE_MG, speed),
        "the straight chord alone would admit the target"
    );
    let bounced = agree("bounce", FIXTURE_MG, 755.0, |app| {
        tilted_plate(app, 0.003, 75.0_f32.to_radians(), 1.0, STEEL);
        target_below(app, 0.0, 0.6)
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
    assert!(partial.transit_only());
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
    assert!(continued.transit_only());

    // Target reach: a skin too thin to stop the round, the target behind it.
    let reached = agree("target reach", FIXTURE_88, 800.0, |app| {
        tilted_plate(app, 0.002, 0.0, 1.0, STEEL);
        target_below(app, 0.0, 2.0)
    });
    assert!(
        reached.admitted,
        "the round reaches the target behind the skin"
    );
    assert!(reached.transit_only());

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

    // Field: an 88 perforates 60 mm of steel square on; an HP-bearing box of the same (absent) tank
    // sits 0.3 m below the plate, beside the disc's path but inside the spall cone.
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
        drag_k(FIXTURE_MG.caliber, FIXTURE_MG.mass),
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

/// A ROUND WHOSE READOUT EQUALS A CROSSING'S COST LEAVES IT WITH A SUB-RESOLUTION RESIDUAL.
///
/// The crossing's cost equals the round's f32 readout. Speed is the state and the readout its f32
/// rounding (DOWN here: the unrounded readout is above it), so the round crosses the metre with a
/// tiny positive residual speed rather than none. That residual geometrically reaches the target
/// behind — a positive chord — but its dose is below the target's f32 HP resolution: reach without
/// admission, and the runtime removes no HP. Fixture: a one-sample round, a metre of material whose
/// factor is the round's readout per metre, faces at exactly representable heights.
#[test]
fn an_exact_budget_crossing_terminates_before_the_target() {
    let stepped = freeflight_step(
        Vec3::NEG_Y * 755.0,
        drag_k(FIXTURE_MG.caliber, FIXTURE_MG.mass),
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
        [Verdict::Through, Verdict::Stopped],
        "through the metre with a sub-resolution residual, then stopped in the target",
    );
    assert!(
        exact.reached,
        "the residual puts a positive chord into the target"
    );
    assert!(
        !exact.admitted && !exact.damaged,
        "whose dose the target's HP pool cannot resolve: admitted {}, damaged {}",
        exact.admitted,
        exact.damaged,
    );
    assert!(exact.transit_only());
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
/// The driver must stop it — fail closed, at the crossing a limit refuses — in the runtime march
/// and in the gate's driver alike. Here the count trips first: a held link must be longer than a
/// nudge, and every corridor must close in a gap shorter than one, which pins the period near a
/// nudge (DERIVED below), so the worlds' crossing ceiling ([`StepCrossings`]) is reached
/// millimetres downrange, far short of [`MAX_OVERRUN`].
///
/// Generic geometry, a single-sample round falling straight down so no bend moves it off the
/// chain: square entry faces (no normalization bend), exit faces sheared 65° (not opposed to the
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
    runtime.update();
    let read = runtime.world().resource::<Read>().0.clone();
    let through = read
        .iter()
        .filter(|(verdict, _)| *verdict == Verdict::Through)
        .count();
    assert_eq!(
        through, CEILING,
        "the chain is crossed until the count is spent: {through} perforations of {links} links",
    );
    assert!(
        read.iter()
            .filter(|(verdict, _)| *verdict == Verdict::Through)
            .any(|(_, at)| at.y < DROP_FROM.y - budget),
        "and drains past the budget to get there",
    );
    assert_eq!(
        read.last().map(|(verdict, _)| *verdict),
        Some(Verdict::Struck),
        "the step fails closed at its limit rather than draining on",
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

/// THE STEP REFUSES THE CROSSING THAT WOULD CARRY IT PAST ITS OVERRUN, before anything is charged.
#[test]
fn a_step_refuses_the_crossing_past_its_overrun() {
    let mut budget = Budget::new(1.0, MAX_STEP_CROSSINGS);
    budget
        .charge(1.0 + MAX_OVERRUN)
        .expect("exactly the overrun is within it");
    assert_eq!(
        budget.charge(0.5),
        Err(WalkError::DrainExhausted {
            crossings: 2,
            overrun: MAX_OVERRUN + 0.5,
        }),
    );
    assert_eq!(
        budget.remaining(),
        -MAX_OVERRUN,
        "a refused crossing charges nothing"
    );
}

/// THE STEP REFUSES THE CROSSING PAST ITS COUNT, before anything is charged.
#[test]
fn a_step_refuses_the_crossing_past_its_count() {
    let mut budget = Budget::new(1.0, MAX_STEP_CROSSINGS);
    for _ in 0..MAX_STEP_CROSSINGS {
        budget.charge(0.0).expect("within the count");
    }
    assert_eq!(
        budget.charge(0.0),
        Err(WalkError::DrainExhausted {
            crossings: MAX_STEP_CROSSINGS + 1,
            overrun: 0.0,
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

/// Where the graze fixture's axis meets the plate's outer face, the plane `z = BOUNCE.z`.
const BOUNCE: Vec3 = Vec3::new(0.0, 2.0, 0.0);

/// The graze's incoming direction: 75° from the face normal `+Z`.
fn graze_incoming() -> Vec3 {
    let incidence = 75.0_f32.to_radians();
    Vec3::new(incidence.sin(), 0.0, -incidence.cos())
}

/// A thick plate grazed at 75° (no overmatch), and a small box beside the bounce that one outgoing
/// ring sample starts inside once the bounce lifts the disc off the face. Fixture data. DERIVED:
/// that sample sits near `(−0.0104, 0, 0.0868)` m from the bounce point, inside the box, while the
/// incoming footprint reaches at most ~0.0496 m in `z` across the box's `x` interval, so the round
/// never entered the box. Returns the box.
fn graze_beside_a_neighbour(app: &mut App) -> Entity {
    volume(
        app,
        Vec3::new(3.0, 3.0, 0.1),
        Transform::from_translation(BOUNCE - Vec3::Z * 0.05),
        STEEL,
        None,
    );
    volume(
        app,
        Vec3::new(0.007, 0.008, 0.030),
        Transform::from_translation(BOUNCE + Vec3::new(-0.0115, 0.0, 0.085)),
        STEEL,
        None,
    )
}

/// A BOUNCE LIFTED INTO A NEIGHBOUR RESOLVES THE NEIGHBOUR.
///
/// The lift clears the struck face, not the world: the fresh outgoing disc has a sample starting
/// inside untouched neighbouring armour. Declared outside everything, that sample's corridor meets
/// an exit nobody entered and the round fails closed; with its membership established, the walk
/// pairs and the neighbour is resolved like any other material. The terminal outcome is not
/// asserted.
#[test]
fn a_bounce_lifted_into_a_neighbour_resolves_it() {
    let incoming = graze_incoming();
    let origin = BOUNCE - incoming;

    let mut gate = world_with_trimesh_plates(&[]);
    let neighbour = graze_beside_a_neighbour(&mut gate);
    for _ in 0..8 {
        gate.update();
    }
    // The fresh disc after the bounce: its membership established, the neighbour's entry owned by
    // the seed of the sample that starts inside it, and the corridor from those starts pairing.
    let settled = gate
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
                let radius = disc_radius(ROUND.caliber);
                let direction = Dir3::new(incoming).expect("a direction");
                let frame = DiscFrame::anchored(incoming).expect("a real direction");
                let mut flight = Flight {
                    position: origin,
                    direction,
                    speed: SPEED,
                    frame,
                    continuation: None,
                };
                flight.settle(&context, &|_| true, radius)?;
                // Short of the nearest sample's first contact; it only sizes the first corridor.
                let contact = 0.8;
                let decision = decide(
                    &context,
                    &|_| true,
                    &bound(ROUND),
                    ROUND.mass,
                    &flight,
                    contact,
                )?;
                assert!(
                    matches!(decision, Decision::Bounce(_)),
                    "the graze bounces: {decision:?}"
                );
                flight.follow(decision);
                flight.settle(&context, &|_| true, radius)?;
                let starts = flight
                    .continuation
                    .clone()
                    .expect("a settled flight carries its starts");
                let corridor = build_corridor(
                    &context,
                    &|_| true,
                    starts.anchor,
                    starts.origin,
                    Vec3::from(flight.direction),
                    ENTRANCE_SPAN,
                    flight.frame,
                    radius,
                    &starts.seeds,
                )?;
                closing_walk(
                    &context,
                    &|_| true,
                    starts.anchor,
                    starts.origin,
                    Vec3::from(flight.direction),
                    flight.frame,
                    radius,
                    &starts.seeds,
                    ENTRANCE_SPAN,
                )?;
                Ok::<_, WalkError>((starts, corridor))
            },
        )
        .expect("the probe system runs");
    let (starts, corridor) =
        settled.unwrap_or_else(|error| panic!("the bounce's fresh start failed: {error:?}"));
    assert!(
        starts
            .seeds
            .iter()
            .any(|seed| seed.inside.iter().any(|key| key.volume == neighbour)),
        "the fixture puts a fresh sample inside the neighbour: {:?}",
        starts.seeds,
    );
    assert_single_ownership(&corridor, "the bounce's fresh disc");

    let flown = gate
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
                fuzz::drive(&context, ROUND, SPEED, origin, incoming, 1.5)
            },
        )
        .expect("the driver system runs")
        .unwrap_or_else(|failure| panic!("the driver failed closed after the bounce: {failure:?}"));
    assert!(
        matches!(flown.decisions.first(), Some(Decision::Bounce(_))),
        "the graze bounces: {:?}",
        impacts_of(&flown),
    );

    let mut runtime = world_with_trimesh_plates(&[]);
    runtime.init_resource::<Read>().add_observer(read_impact);
    graze_beside_a_neighbour(&mut runtime);
    for _ in 0..8 {
        runtime.update();
    }
    runtime.world_mut().resource_mut::<Read>().0.clear();
    spawn_round(&mut runtime, ROUND, origin, incoming * SPEED);
    for _ in 0..3 {
        runtime.update();
    }
    let read = runtime.world().resource::<Read>().0.clone();
    assert_eq!(
        read.first().map(|(verdict, _)| *verdict),
        Some(Verdict::Ricochet),
        "the graze bounces: {read:?}",
    );
    assert!(
        read.iter().all(|(verdict, _)| *verdict != Verdict::Struck),
        "the march failed closed after the bounce: {read:?}",
    );
}

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
                let flight = Flight::launched(
                    &context,
                    &|_| true,
                    at,
                    Dir3::new(swept).expect("a direction"),
                    Dir3::new(launch).expect("a direction"),
                    SPEED,
                    frame,
                    radius,
                    None,
                )?;
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
/// contain a start. Probing through its bounds would need a 60 m probe, past [`MAX_CORRIDOR`]; read
/// only for what could contain a start, every sample is outside everything.
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

/// A PROBE PAST ITS CANDIDATE CEILING FAILS CLOSED.
///
/// Fixture data: [`MAX_PROBE_CANDIDATES`] + 1 nested cubes around the start, every one of which
/// could contain it.
#[test]
fn a_probe_past_its_candidate_ceiling_fails_closed() {
    let (mut app, _) = settled(|app| {
        (0..=MAX_PROBE_CANDIDATES)
            .map(|index| {
                let size = 0.5 + 0.01 * index as f32;
                mesh_volume(app, box_trimesh(Vec3::splat(size)), Transform::IDENTITY)
            })
            .collect()
    });
    let error = certify(&mut app, Vec3::ZERO, Vec3::Z, 0.0)
        .expect_err("a probe past its candidate ceiling is refused");
    assert!(
        matches!(error, WalkError::UncertifiedStart { reason, .. } if reason.contains("candidates")),
        "the refusal names the candidate ceiling: {error:?}",
    );
}

/// A PROBE PAST ITS FACE CEILING FAILS CLOSED.
///
/// Fixture data: one trimesh of `count` separate closed boxes stacked along the probe at a 0.1 m
/// pitch, so every sample crosses two faces of each and the disc's thirteen samples collect more
/// than [`MAX_PROBE_FACES`] between them.
#[test]
fn a_probe_past_its_face_ceiling_fails_closed() {
    let count = MAX_PROBE_FACES / (2 * (1 + walk::DEFAULT_RING)) + 1;
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
    let error = certify(&mut app, Vec3::new(0.0, 0.0, 0.05), Vec3::Z, 0.044)
        .expect_err("a probe past its face ceiling is refused");
    assert!(
        matches!(error, WalkError::UncertifiedStart { reason, .. } if reason.contains("faces")),
        "the refusal names the face ceiling: {error:?}",
    );
}

// ---------------------------------------------------------------------------------------------
// The spall cone's field law (design doc §5): class invariants on fixture worlds
// ---------------------------------------------------------------------------------------------

use crate::ballistics::ProjectileMarchWorld;
use crate::ballistics::spall_cone::{AxisRule, ConeShape, FieldDescriptor, Passage};

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
/// of the world (volumes of no tank: one tank), none of them exited, each walk failing the test.
fn field_rays(
    app: &mut App,
    rays: Vec<(Vec3, Dir3, f32)>,
    weight: f32,
    exited: Vec<Entity>,
) -> Vec<FieldRay> {
    field_rays_under(app, rays, weight, exited, Passage::Partial)
}

/// [`field_rays`] under `passage`.
fn field_rays_under(
    app: &mut App,
    rays: Vec<(Vec3, Dir3, f32)>,
    weight: f32,
    exited: Vec<Entity>,
    passage: Passage,
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
                            &within,
                            &|volume| health.contains(volume),
                            &exited,
                            origin,
                            dir,
                            strength,
                            weight,
                            passage,
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
/// HP of every fixture box: above any dose below, so no dose is clamped away.
const FIELD_HP: u32 = 10_000_000;

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

/// Every passage a cone shape may declare: the shipped one and its dial (playtest fork F6). An
/// invariant that holds under any passage is tested under each.
const PASSAGES: [Passage; 2] = [Passage::FirstSolid, Passage::Partial];

/// Each of `boxes`' dose from one cone of `directions` strata under `passage` and the field
/// [`FIELD`], of strength `strength`, thrown from the origin down `-Y` through every volume of the
/// world (volumes of no tank: one tank).
fn field_doses(
    app: &mut App,
    boxes: &[Entity],
    strength: f32,
    directions: usize,
    passage: Passage,
) -> Vec<f64> {
    field_doses_of(app, boxes, FIELD, strength, directions, passage)
}

/// [`field_doses`] under the field `field`.
fn field_doses_of(
    app: &mut App,
    boxes: &[Entity],
    field: FieldDescriptor,
    strength: f32,
    directions: usize,
    passage: Passage,
) -> Vec<f64> {
    let spread = ConeShape::new(AxisRule::ExitDirection, directions, passage)
        .unwrap()
        .spread(field)
        .unwrap();
    let rays = spread
        .directions(Dir3::NEG_Y)
        .map(|(dir, weight)| (Vec3::ZERO, dir, strength * weight))
        .collect();
    let mut dose = vec![0.0f64; boxes.len()];
    for ray in field_rays_under(app, rays, 1.0 / directions as f32, Vec::new(), passage) {
        for applied in ray.doses {
            if let Some(index) = boxes.iter().position(|b| *b == applied.address) {
                dose[index] += applied.amount as f64;
            }
        }
    }
    dose
}

/// CLASS INVARIANT 2, RESOLUTION STABILITY, under every passage: a box's dose converges as the
/// direction count grows — in the convergence regime the change from N to 4N shrinks with every
/// step — and at the shipped count it lies within a stated tolerance of the converged value.
/// Fixture: a soft box off the axis, a metre down.
///
/// The steps are taken from 384 directions up. Below that the strata the box's edges cut are noise
/// of a stratum's weight, not a trend, and the axis strata the box covers carry the field's
/// heaviest weights: MEASURED on this fixture under the first-solid passage, 96 → 384 → 1536
/// changed the dose by 0.016 then 0.055 (of ~1.83), then 0.027 and 0.004.
#[test]
fn a_box_dose_is_stable_under_resolution() {
    let (mut app, boxes) =
        field_world(|app| vec![below(app, Vec3::new(0.4, 0.05, 0.4), 1.0, 0.15, 0.0, true)]);
    let counts = [384, 1536, 6144, 24576];
    for passage in PASSAGES {
        let doses: Vec<f64> = counts
            .iter()
            .map(|&n| field_doses(&mut app, &boxes, 10.0, n, passage)[0])
            .collect();
        let changes: Vec<f64> = doses.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
        assert!(
            changes.windows(2).all(|w| w[1] <= w[0]),
            "{passage:?}: the change shrinks as N quadruples: doses {doses:?}, changes {changes:?}"
        );
        let converged = doses[doses.len() - 1];
        let shipped = ConeShape::shipped().directions_count();
        let at_shipped = field_doses(&mut app, &boxes, 10.0, shipped, passage)[0];
        // Fixture tolerance: a tenth of the converged dose at the shipped resolution.
        assert!(
            (at_shipped - converged).abs() <= 0.1 * converged,
            "{passage:?}, N = {shipped}: {at_shipped} against {converged}"
        );
    }
}

/// CLASS INVARIANT 3, HOMOGENEITY: under every passage, where no direction is exhausted, scaling
/// the strength `S` scales every dose by the same factor; under the partial passage, where a plate
/// exhausts directions, a dose never falls as the strength rises.
#[test]
fn doses_are_homogeneous_in_strength() {
    let (mut app, boxes) = field_world(|app| {
        vec![
            below(app, Vec3::new(0.3, 0.05, 0.3), 1.0, 0.1, 0.0, true),
            below(app, Vec3::new(0.2, 0.05, 0.2), 1.5, -0.2, 0.0, true),
        ]
    });
    for passage in PASSAGES {
        let one = field_doses(&mut app, &boxes, 10.0, FIELD_N, passage);
        let scaled = field_doses(&mut app, &boxes, 37.0, FIELD_N, passage);
        for (a, b) in one.iter().zip(&scaled) {
            assert!(*a > 0.0, "{passage:?}: the fixture boxes lie in the cone");
            assert!(
                (b / a - 3.7).abs() < 1.0e-5,
                "{passage:?}: {b} against 3.7 × {a}"
            );
        }
    }

    let (mut app, boxes) = field_world(|app| {
        below(app, Vec3::new(2.0, 0.01, 2.0), 0.5, 0.0, STEEL, false);
        vec![below(app, Vec3::new(0.4, 0.05, 0.4), 1.0, 0.1, 10.0, true)]
    });
    let mut last = 0.0;
    for strength in [1.0, 5.0, 9.0, 11.0, 20.0, 50.0, 200.0] {
        let dose = field_doses(&mut app, &boxes, strength, FIELD_N, Passage::Partial)[0];
        assert!(dose >= last, "S = {strength}: {dose} below {last}");
        last = dose;
    }
    assert!(last > 0.0, "a strong enough cone gets through the plate");
}

/// CLASS INVARIANT 4, INVERSE SQUARE, under every passage: strength is not spent in air, so a thin
/// unresisting box on the axis takes a quarter of the dose at twice the distance — the share of the
/// cone it covers. Fixture: a 0.1 m square; the ball's density across it falls by at most 2.1 % at
/// 1 m and a quarter of that at 2 m (DERIVED: `1 − p_Ω(θ)/p_Ω(0) ≈ 3λ²(3 + λ²)θ²/(2(1 + 3λ²))`,
/// at the corner's θ = 0.071 and 0.035), inside the tolerance.
#[test]
fn an_unshadowed_dose_falls_as_the_inverse_square() {
    for passage in PASSAGES {
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
            field_doses(&mut app, &boxes, 10.0, 16384, passage)[0]
        };
        let ratio = dose_at(2.0) / dose_at(1.0);
        assert!(
            (ratio - 0.25).abs() < 0.01,
            "{passage:?}: D(2r)/D(r) = {ratio}"
        );
    }
}

/// The pen cam's potency colour reads this resolver's dose, under every passage and on every
/// support: the intensity its law draws at distance `r` along a direction (`ρ s / (Ω r²)`,
/// `pen_cam::potency`, `Ω` the support's solid angle) times a thin unresisting face's area there
/// is `ρ` times the dose the face takes. The colour's promise — a box's face wholly at or above a
/// threshold takes at least that threshold's pool — relies on it; a pen cam law that drifts from
/// the dose law (a lost `Ω`, a ring's band taken as the cap it lies in, a square dropped, the
/// profile ignored) fails here.
/// Fixture: a ball's cap ([`FIELD`]) with a 0.1 m square face-on on its axis, and a ring's band
/// (`λ = 0.45`, half-width 10°) with a 0.2 m square face-on at its middle, each at 1.0 and 1.5 m,
/// 16384 directions. At 1.5 m the faces cover ~124 and ~146 strata, one stratum under 0.8 % of
/// the dose (DERIVED: `(A / r²) / Ω · N`), and the tolerance is three strata. MEASURED alike under
/// both passages: the ball's 0.5 % and 2.4 % off, the ring's 2.2 % and 1.1 %, ~1.2 % of the
/// ring's at 1.0 m its larger face's (DERIVED: `A / r²` over the square's exact solid angle).
#[test]
fn the_pen_cam_intensity_times_a_face_is_its_dose() {
    use crate::damage_cam::pen_cam::potency::{Potency, intensity};
    let potency = Potency::shipped();
    let ring = FieldDescriptor::ring(0.45, 10.0_f32.to_radians());
    let middle = libm::atan2(1.0, 0.45) as f32;
    let fields = [
        (FIELD, Vec3::NEG_Y, 0.1),
        (ring, Vec3::new(middle.sin(), -middle.cos(), 0.0), 0.2),
    ];
    for passage in PASSAGES {
        for (field, face, side) in fields {
            let spread = ConeShape::new(AxisRule::ExitDirection, 16384, passage)
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
                let dose = field_doses_of(&mut app, &boxes, field, 10.0, 16384, passage)[0];
                let e = potency.emission(10.0 * weight, field.inner_angle(), field.half_angle());
                let drawn = intensity(e, depth - 0.001) as f64 * (side * side) as f64;
                let ratio = drawn / (potency.rate as f64 * dose);
                assert!(
                    (ratio - 1.0).abs() < 0.025,
                    "{passage:?}, {field:?}, r = {depth}: I·A / ρD = {ratio}"
                );
            }
        }
    }
}

/// CLASS INVARIANT 6, FACTOR INDEPENDENCE, under every passage: a box's dose is the strength
/// arriving at it, never scaled by its own factor.
#[test]
fn a_box_dose_does_not_depend_on_its_own_factor() {
    for passage in PASSAGES {
        let dose_with = |factor: f32| {
            let (mut app, boxes) = field_world(|app| {
                vec![below(app, Vec3::new(0.3, 0.2, 0.3), 1.0, 0.1, factor, true)]
            });
            field_doses(&mut app, &boxes, 10.0, FIELD_N, passage)[0]
        };
        let soft = dose_with(10.0);
        assert!(soft > 0.0, "{passage:?}");
        assert_eq!(soft.to_bits(), dose_with(900.0).to_bits(), "{passage:?}");
    }
}

/// CLASS INVARIANT 7, SHADOWING IS MONOTONE, under every passage: adding any volume between the
/// origin and a box — armour or another box, thin or thick — never raises the box's dose.
#[test]
fn adding_a_volume_never_raises_a_dose() {
    for passage in PASSAGES {
        shadowing_is_monotone(passage);
    }
}

/// [`adding_a_volume_never_raises_a_dose`] under `passage`.
fn shadowing_is_monotone(passage: Passage) {
    let target = |app: &mut App| below(app, Vec3::new(0.4, 0.05, 0.4), 1.2, 0.1, 10.0, true);
    let (mut app, boxes) = field_world(|app| vec![target(app)]);
    let bare = field_doses(&mut app, &boxes, 10.0, FIELD_N, passage)[0];
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
        let shadowed = field_doses(&mut app, &boxes, 10.0, FIELD_N, passage)[0];
        assert!(
            shadowed <= bare,
            "{passage:?}, {size:?} at factor {factor}: {shadowed} above {bare}"
        );
    }
}

/// A cone doses the struck tank's boxes alone: a box of another tank inside the cone takes
/// nothing, a box of the struck tank beside it does. And square on, the cone's axis is the plate's
/// rear normal, which is the round's direction. Fixture: an 88 perforates 60 mm of steel square on;
/// two soft boxes sit below the plate either side of the path.
#[test]
fn a_cone_doses_the_struck_tank_alone() {
    let mut app = world_with_trimesh_plates(&[]);
    volume(
        &mut app,
        Vec3::new(2.0, 0.06, 2.0),
        Transform::from_translation(Vec3::new(DROP_FROM.x, 0.97, DROP_FROM.z)),
        STEEL,
        None,
    );
    let own = volume(
        &mut app,
        Vec3::new(0.5, 0.2, 0.6),
        Transform::from_translation(Vec3::new(DROP_FROM.x + 0.31, 0.54, DROP_FROM.z)),
        STEEL * 0.1,
        Some(1000),
    );
    let foreign = volume(
        &mut app,
        Vec3::new(0.5, 0.2, 0.6),
        Transform::from_translation(Vec3::new(DROP_FROM.x - 0.31, 0.54, DROP_FROM.z)),
        STEEL * 0.1,
        Some(1000),
    );
    let other = app.world_mut().spawn_empty().id();
    app.world_mut()
        .entity_mut(foreign)
        .insert(crate::damage::VolumeOf(other));
    app.insert_resource(RetainSpentShells(true));
    for _ in 0..8 {
        app.update();
    }
    let shell = spawn_round(&mut app, FIXTURE_88, DROP_FROM, Vec3::NEG_Y * 800.0);
    app.update();

    let hp = |entity: Entity| app.world().get::<ComponentHealth>(entity).unwrap().current;
    assert!(hp(own) < 1000, "the struck tank's box is dosed");
    assert_eq!(hp(foreign), 1000, "another tank's box is not");
    let marks = app
        .world()
        .get::<SpallMarks>(shell)
        .expect("the shell keeps its marks");
    let first = marks.cones.first().expect("the plate's exit throws");
    assert!(
        Vec3::from(first.axis).distance(Vec3::NEG_Y) < 1.0e-4,
        "square on, the axis is the rear normal and the direction: {:?}",
        first.axis
    );
}

/// THE HP QUANTUM GATE: a body exit whose cone's whole emission `S/2` converts to no HP point at
/// the field's rate `r` (HP points per reference-mm, `spall_cone::hp_per_reference_mm`) is not
/// thrown — no direction walked, no cone recorded, the crossing counting none — and one above it
/// is. Fixture: an 88 through `3/r` m of a factor-10 slab (spend ≈ `30/r` reference-mm, κ = 0.01,
/// so `S/2` ≈ `0.15/r` reference-mm, 0.15 of a point at `r`, DERIVED) and through 20 mm of steel.
#[test]
fn a_cone_below_the_hp_quantum_is_not_thrown() {
    let slab = 3.0 / crate::ballistics::spall_cone::hp_per_reference_mm() as f32;
    for (thickness, factor, thrown) in [(slab, 10.0, false), (0.02, STEEL, true)] {
        let mut app = world_with_trimesh_plates(&[]);
        volume(
            &mut app,
            Vec3::new(2.0, thickness, 2.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x, 1.0 - 0.5 * thickness, DROP_FROM.z)),
            factor,
            None,
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
            (spall.cones.len(), spall.gated, counted),
            if thrown { (1, 0, 1) } else { (0, 1, 0) },
            "factor {factor}: {:?}",
            spall
                .cones
                .iter()
                .map(|cone| cone.strength)
                .collect::<Vec<_>>()
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
    let undosed = cone.rays.iter().filter(|ray| !ray.deposited).count();
    assert_eq!(
        undosed,
        0,
        "every direction starts inside B, never left, and doses it ({} directions)",
        cone.rays.len()
    );
    assert!(app.world().get::<ComponentHealth>(b).unwrap().current < FIELD_HP);
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
    )
    .expect("fixture factors are legal");
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

/// FIRST SOLID: a direction stops where it enters the first volume. A box there takes its whole
/// `strength × weight` and nothing beyond it is dosed; armour there takes nothing and shadows every
/// box behind it; the plate the throw leaves stops nothing. Fixture slabs along the direction.
#[test]
fn a_first_solid_direction_stops_at_what_it_enters() {
    let walk = |armour_first: bool| {
        let mut slabs = vec![(-0.15, 0.05, 1.0), (0.9, 1.1, 0.0), (1.9, 2.1, 0.0)];
        if armour_first {
            slabs.push((0.45, 0.55, 1.0));
        }
        let (layers, volumes) = slab_layers(&slabs);
        let boxes = [volumes[1], volumes[2]];
        let layers: Vec<&walk::Layer> = layers.iter().collect();
        let (end, doses, _, _) = first_solid(
            &layers,
            BEHIND,
            Vec3::ZERO,
            Dir3::NEG_Y,
            3.0,
            0.25,
            &|volume| boxes.contains(&volume),
            &[volumes[0]],
        );
        let doses: Vec<Dose> = doses.into_iter().map(|(_, dose)| dose).collect();
        (end, doses, boxes)
    };
    let (end, doses, boxes) = walk(false);
    assert_eq!(doses.len(), 1, "{doses:?}");
    assert_eq!(doses[0].address, boxes[0]);
    assert_eq!(
        doses[0].amount, 0.75,
        "its whole strength, no attenuation in air"
    );
    assert!(
        (end.y + 0.9).abs() < 1.0e-4,
        "stopped at the near box's face: {end}"
    );
    let (end, doses, _) = walk(true);
    assert!(doses.is_empty(), "armour shadows the boxes: {doses:?}");
    assert!((end.y + 0.45).abs() < 1.0e-4, "stopped at the plate: {end}");
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

/// A direction thrown from inside a soft block starts inside it and pays for it: strength 10 into
/// a block of factor 150 exhausts 10/150 m in, and the box beyond the block takes nothing. Fixture:
/// the block spans 0.5 m above the origin to 1 m below it.
#[test]
fn a_direction_thrown_inside_a_block_pays_for_it() {
    let (mut app, boxes) = field_world(|app| {
        slab_below(app, 0.5, 1.5, 150.0, false);
        vec![slab_below(app, -1.4, 0.2, 0.0, true)]
    });
    let (end, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[]);
    assert_eq!(dose_to(&doses, boxes[0]), 0.0, "the box beyond is shadowed");
    assert!(
        (end.y + 10.0 / 150.0).abs() < 1.0e-4,
        "the direction exhausts 66.7 mm into the block: {end:?}"
    );
}

/// A box wholly inside another volume the direction crosses is entered, and dosed with the
/// strength that reaches its entry. Fixture: a soft volume of factor 1 over `[1, 3]` m, the box
/// over `[1.9, 2.1]` m inside it: 0.9 reference-mm is spent before it.
#[test]
fn a_box_inside_a_traversed_volume_is_dosed() {
    let (layers, volumes) = slab_layers(&[(1.0, 3.0, 1.0), (1.9, 2.1, 0.0)]);
    let layers: Vec<&walk::Layer> = layers.iter().collect();
    let (_, doses, _, _) = partial(
        &layers,
        BEHIND,
        Vec3::ZERO,
        Dir3::NEG_Y,
        10.0,
        1.0,
        &|volume| volume == volumes[1],
        &[],
    );
    let dose: f32 = doses
        .iter()
        .filter(|(_, dose)| dose.address == volumes[1])
        .map(|(_, dose)| dose.amount)
        .sum();
    assert!((dose - 9.1).abs() < 1.0e-4, "dosed at entry: {dose}");
}

/// A plate thinner than any nudge is still material: half a millimetre of steel costs 0.5
/// reference-mm, so a direction of strength 0.25 exhausts halfway through it and the box behind
/// takes nothing.
#[test]
fn a_direction_exhausts_halfway_through_a_thin_plate() {
    let (mut app, boxes) = field_world(|app| {
        slab_below(app, -1.0, 0.0005, 1000.0, false);
        vec![slab_below(app, -2.0, 0.2, 0.0, true)]
    });
    let (end, doses) = walk_direction(&mut app, Vec3::ZERO, 0.25, &[]);
    assert_eq!(dose_to(&doses, boxes[0]), 0.0, "the box behind is shadowed");
    assert!(
        (end.y + 1.000_25).abs() < 2.0e-6,
        "the direction exhausts halfway through the plate: {end:?}"
    );
}

/// CLASS INVARIANT, SHADOWING IS MONOTONE ALONG ONE DIRECTION: adding any volume anywhere along a
/// direction — before, around, inside or overlapping the volumes already there — never raises any
/// box's dose, a box sitting exactly on its exhaustion threshold included ([`field_direction`]
/// states the exact guarantee). Fixture: randomized stacks of overlapping volumes of random factor
/// along `-Y`.
#[test]
fn adding_material_along_a_direction_never_raises_a_dose() {
    let mut state = 0x2545_f491_u32;
    let mut unit = move || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (state >> 8) as f32 / (1u32 << 24) as f32
    };
    let factors = [0.0, 1.0, 20.0, 150.0, 1000.0];
    for trial in 0..16 {
        // (top, thickness, factor, box) of each volume; the first three are boxes.
        let mut stack: Vec<(f32, f32, f32, bool)> = (0..6)
            .map(|index| {
                let top = -0.2 - 3.0 * unit();
                let thickness = 0.001 + 1.2 * unit() * unit();
                let factor = factors[(unit() * factors.len() as f32) as usize % factors.len()];
                (top, thickness, factor, index < 3)
            })
            .collect();
        let doses = |stack: &[(f32, f32, f32, bool)]| {
            let (mut app, boxes) = field_world(|app| {
                stack
                    .iter()
                    .filter_map(|&(top, thickness, factor, hp)| {
                        let entity = slab_below(app, top, thickness, factor, hp);
                        hp.then_some(entity)
                    })
                    .collect()
            });
            let (_, doses) =
                walk_direction(&mut app, Vec3::ZERO, 4.0 + 20.0 * unit_free(trial), &[]);
            boxes
                .iter()
                .map(|b| dose_to(&doses, *b))
                .collect::<Vec<f32>>()
        };
        let before = doses(&stack);
        stack.push((
            -0.1 - 3.2 * unit(),
            0.001 + 2.0 * unit() * unit(),
            factors[(unit() * factors.len() as f32) as usize % factors.len()],
            false,
        ));
        let after = doses(&stack);
        for (index, (b, a)) in before.iter().zip(&after).enumerate() {
            assert!(
                a <= b,
                "trial {trial}, box {index}: adding {:?} raised the dose {b} to {a} (stack \
                 {stack:?})",
                stack.last()
            );
        }
    }

    // Exhaustion thresholds: stacks on a 1/1024 m grid, where every cost is exact in f64 and the
    // cost of reaching a box is exact in f32, thrown with exactly that strength — the box sits on
    // the threshold, undosed, and a strength one ulp higher doses it. The added material is a slab
    // off the grid or a sloped prism whose crossings are non-dyadic; neither may lift the box off
    // the threshold, nor raise any dose.
    let grid = |value: f32| (value * 1024.0).round() / 1024.0;
    for trial in 0..16 {
        let stack: Vec<(f32, f32, f32, bool)> = (0..6)
            .map(|index| {
                let top = grid(-0.2 - 3.0 * unit());
                let thickness = grid(0.01 + 1.2 * unit() * unit());
                let factor = factors[(unit() * factors.len() as f32) as usize % factors.len()];
                (top, thickness, factor, index < 3)
            })
            .collect();
        // `∫ max(factor)` from the origin down to `depth`, over the stack's own cuts.
        let cost_to = |depth: f64| -> f64 {
            let mut cuts: Vec<f64> = stack
                .iter()
                .flat_map(|&(top, thickness, _, _)| {
                    [-(top as f64), -(top as f64) + thickness as f64]
                })
                .filter(|t| *t < depth)
                .chain([0.0, depth])
                .collect();
            cuts.sort_by(f64::total_cmp);
            cuts.dedup();
            cuts.windows(2)
                .map(|w| {
                    let mid = 0.5 * (w[0] + w[1]);
                    let factor = stack
                        .iter()
                        .filter(|&&(top, thickness, _, _)| {
                            -(top as f64) <= mid && mid < -(top as f64) + thickness as f64
                        })
                        .fold(0.0f64, |most, &(_, _, factor, _)| most.max(factor as f64));
                    (w[1] - w[0]) * factor
                })
                .sum()
        };
        let Some((target, cost)) = (0..3)
            .map(|index| (index, cost_to(-(stack[index].0 as f64))))
            .filter(|(_, cost)| *cost > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
        else {
            continue;
        };
        let strength = cost as f32;
        assert_eq!(
            strength as f64, cost,
            "trial {trial}: the threshold is exact"
        );
        let doses =
            |stack: &[(f32, f32, f32, bool)], prism: Option<(f32, f32, f32)>, strength: f32| {
                let (mut app, boxes) = field_world(|app| {
                    if let Some((base, height, factor)) = prism {
                        sloped_prism(app, base, height, factor);
                    }
                    stack
                        .iter()
                        .filter_map(|&(top, thickness, factor, hp)| {
                            let entity = slab_below(app, top, thickness, factor, hp);
                            hp.then_some(entity)
                        })
                        .collect()
                });
                let (_, doses) = walk_direction(&mut app, Vec3::ZERO, strength, &[]);
                boxes
                    .iter()
                    .map(|b| dose_to(&doses, *b))
                    .collect::<Vec<f32>>()
            };
        let before = doses(&stack, None, strength);
        assert_eq!(
            before[target], 0.0,
            "trial {trial}: box {target} sits on the threshold (stack {stack:?})"
        );
        let above = doses(&stack, None, f32::from_bits(strength.to_bits() + 1));
        assert!(
            above[target] > 0.0,
            "trial {trial}: one ulp more strength doses box {target} (stack {stack:?})"
        );
        let factor = factors[(unit() * factors.len() as f32) as usize % factors.len()];
        let (stacked, prism) = if trial % 2 == 0 {
            let mut stacked = stack.clone();
            stacked.push((
                -0.1 - 3.2 * unit(),
                0.001 + 2.0 * unit() * unit(),
                factor,
                false,
            ));
            (stacked, None)
        } else {
            // Inside the target's shadow — between the origin and its entry — where there is room.
            let height = grid(0.01 + 1.5 * unit() * unit());
            let (lowest, highest) = (stack[target].0 - 0.2, -(height + 0.6));
            let base = if highest > lowest {
                grid(lowest + (highest - lowest) * unit())
            } else {
                grid(-(height + 0.6) - 2.5 * unit())
            };
            (stack.clone(), Some((base, height, factor)))
        };
        let after = doses(&stacked, prism, strength);
        for (index, (b, a)) in before.iter().zip(&after).enumerate() {
            assert!(
                a <= b,
                "threshold trial {trial}, box {index}: adding {:?} raised the dose {b} to {a} \
                 (stack {stack:?})",
                prism.map_or(stacked.last().copied(), |(base, height, factor)| Some((
                    base, height, factor, false
                )))
            );
        }
    }
}

/// A static trimesh volume of `factor` reference-mm per metre, of the closed shell `mesh`.
fn factored_mesh(app: &mut App, mesh: (Collider, BallisticSurfaces), factor: f32) -> Entity {
    app.world_mut()
        .spawn((
            Transform::IDENTITY,
            RigidBody::Static,
            mesh,
            CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(factor),
                substance: "fixture".to_string(),
            },
        ))
        .id()
}

/// A prism of `factor` whose top and bottom faces rise 0.4 per metre of `+X`, over `x ∈ [-0.5,
/// 0.75]`: it crosses the `-Y` axis at heights `base + 0.2` and `base + height + 0.2`, non-dyadic
/// though every vertex is exact in f32 for `base` and `height` on a dyadic grid.
fn sloped_prism(app: &mut App, base: f32, height: f32, factor: f32) -> Entity {
    let mesh = mapped_box_trimesh(|c| {
        Vec3::new(
            0.625 * c.x + 0.125,
            base + 0.5 * height * (c.y + 1.0) + 0.25 * (c.x + 1.0),
            0.5 * c.z,
        )
    });
    factored_mesh(app, mesh, factor)
}

/// A trial's strength, a function of the trial alone so both walks of a trial throw the same.
fn unit_free(trial: u32) -> f32 {
    ((trial.wrapping_mul(2_654_435_761) >> 8) as f32 / (1u32 << 24) as f32).fract()
}

/// Softer material inside a plate leaves the plate's cost bit-identical: strength 1 thrown along
/// `+X` from 0.125 m before a plate of factor 1 over `[0, 1]` m is exhausted exactly at its rear
/// face, with or without a factor-0.5 prism crossing the ray over `[0.1, 0.3]` m inside it, and the
/// box behind takes nothing either way. Fixture (DERIVED): the prism is bounded by the planes
/// `10x − y = 1` and `10x − y = 3`, its vertices exact in f32, so its crossings are the non-dyadic
/// `0.1` and `0.3` along the ray; charged boundary by boundary, the three pieces of the plate
/// leave `2^-53` of strength (DERIVED in f64), which reaches the box.
///
/// Softer material around the plate adds nothing inside it either: a factor-0.5 volume enclosing
/// the plate over `[-1/16, 17/16]` m charges only its own material outside the plate, and the box
/// still takes nothing. Charged at the enclosing factor inside the plate, the whole span would cost
/// 0.5625 and the box would take the rest (DERIVED).
#[test]
fn softer_material_inside_a_plate_leaves_its_cost_unchanged() {
    let dose_behind = |prism: bool, around: bool| {
        let (mut app, boxes) = field_world(|app| {
            volume(
                app,
                Vec3::new(1.0, 2.0, 2.0),
                Transform::from_translation(Vec3::new(0.5, 0.0, 0.0)),
                1.0,
                None,
            );
            if prism {
                // The cube's faces `x = ∓1` land on the planes `10x − y = 1` and `10x − y = 3`.
                let mesh = mapped_box_trimesh(|c| {
                    Vec3::new(0.25 * (c.y + 1.0), 2.5 * (c.y + 1.0) - c.x - 2.0, c.z)
                });
                factored_mesh(app, mesh, 0.5);
            }
            if around {
                volume(
                    app,
                    Vec3::new(1.125, 3.0, 3.0),
                    Transform::from_translation(Vec3::new(0.5, 0.0, 0.0)),
                    0.5,
                    None,
                );
            }
            vec![volume(
                app,
                Vec3::new(0.2, 2.0, 2.0),
                Transform::from_translation(Vec3::new(1.6, 0.0, 0.0)),
                0.0,
                Some(FIELD_HP),
            )]
        });
        let ray = field_rays(
            &mut app,
            vec![(Vec3::new(-0.125, 0.0, 0.0), Dir3::X, 1.0)],
            1.0,
            Vec::new(),
        )
        .remove(0);
        dose_to(
            &ray.doses
                .iter()
                .map(|dose| (dose.address, dose.amount))
                .collect::<Vec<_>>(),
            boxes[0],
        )
    };
    assert_eq!(dose_behind(false, false), 0.0, "the plate alone");
    assert_eq!(
        dose_behind(true, false),
        0.0,
        "the plate with softer material inside it"
    );
    assert_eq!(
        dose_behind(false, true),
        0.0,
        "the plate with softer material around it"
    );
}

/// A direction recognises the volume it is thrown from by identity, never by displacement: starting
/// a millimetre inside the box its throw leaves, it pays for that millimetre but does not dose the
/// box, and it doses the box it steps into with the strength left at its entry. Named as anything
/// but the volume the throw leaves, the same box is entered at the throw point and dosed. Fixture: a
/// box of factor 150 ending 1 mm along the direction, a box of factor 20 in contact behind it.
#[test]
fn a_direction_does_not_dose_the_volume_its_throw_leaves() {
    let (mut app, boxes) = field_world(|app| {
        vec![
            slab_below(app, 0.5, 0.501, 150.0, true),
            slab_below(app, -0.001, 0.5, 20.0, true),
        ]
    });
    let (_, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[boxes[0]]);
    assert_eq!(dose_to(&doses, boxes[0]), 0.0, "the throw's own box");
    let behind = dose_to(&doses, boxes[1]);
    assert!(
        (behind - (10.0 - 0.001 * 150.0)).abs() < 1.0e-4,
        "the box stepped into, at entry: {behind}"
    );
    let (_, doses) = walk_direction(&mut app, Vec3::ZERO, 10.0, &[]);
    assert_eq!(
        dose_to(&doses, boxes[0]),
        10.0,
        "entered at the throw point"
    );
}
