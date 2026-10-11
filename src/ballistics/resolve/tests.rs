//! The driver's two-stage crossing — the hand-off from the entrance corridor to the transit
//! corridor counts every entry exactly once — and the one mechanism: the runtime march and the
//! §13.6 gate's driver decide the same crossings.

use avian3d::prelude::{CollisionLayers, LayerMask, Position, RigidBody, Rotation};
use bevy::ecs::system::RunSystemOnce;

use super::*;
use crate::ballistics::march_tests::{box_trimesh, world_with_trimesh_plates};
use crate::ballistics::walk::FaceHit;
use crate::ballistics::{BallisticVolume, RoundBody};

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
                    material_factor: STEEL,
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
                            let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                            let context = ResolveContext {
                                world: &world,
                                colliders: &colliders,
                                armor: &armor,
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
                                capability: capability(ROUND, SPEED),
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
                            walk::finish(&transit, &request, &shot, &context.laws)?;
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
use crate::ballistics::{ComponentHealth, Verdict, advance_shell, drag_k};

/// The fixture tick the plate worlds step by (`march_tests::plate_world`'s manual 16 ms).
const DT: f32 = 0.016;
/// Where every shot of this section starts: straight above the geometry, falling, so gravity and
/// drag change the speed but never the direction, and the gate's straight flight is the runtime's.
const DROP_FROM: Vec3 = Vec3::new(0.1, 3.0, -0.2);

/// Every impact the runtime fired, as `(verdict, position)`.
#[derive(Resource, Default)]
struct Read(Vec<(Verdict, Vec3)>);

fn read_impact(impact: On<Impact>, mut read: ResMut<Read>) {
    read.0.push((impact.outcome.verdict(), impact.position));
}

/// A static trimesh volume of `factor` reference-mm per metre, optionally carrying HP.
fn volume(app: &mut App, size: Vec3, at: Transform, factor: f32, hp: Option<f32>) -> Entity {
    let mut entity = app.world_mut().spawn((
        at,
        RigidBody::Static,
        box_trimesh(size),
        CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
        BallisticVolume {
            material_factor: factor,
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
    let (_, stepped) = advance_shell(
        DROP_FROM,
        Vec3::NEG_Y * speed,
        drag_k(body.caliber, body.mass),
        DT,
    );
    let speed = stepped.length();
    (speed, speed * DT)
}

/// The gate's side: the same shot, through [`fuzz::drive`], for the runtime's first-tick budget.
fn driven(app: &mut App, body: RoundBody, speed: f32) -> Drive {
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
                let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    armor: &armor,
                    deposit: false,
                    laws: WalkLaws::default(),
                };
                fuzz::drive(&context, body, speed, DROP_FROM, Vec3::NEG_Y, budget)
            },
        )
        .expect("the driver system runs")
        .unwrap_or_else(|failure| panic!("the driven flight failed to walk: {failure:?}"))
}

/// The runtime's side: one tick of the real march, read back as impacts.
fn marched(app: &mut App, body: RoundBody, speed: f32) -> Vec<(Verdict, Vec3)> {
    spawn_round(app, body, DROP_FROM, Vec3::NEG_Y * speed);
    app.update();
    app.world().resource::<Read>().0.clone()
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
    /// The gate admitted the target: some crossing put a positive chord into it.
    admitted: bool,
}

/// Build the arrangement twice, fly `body` through the runtime in one and through the gate's driver
/// in the other, and require that they agree on every impact and on whether the target was reached.
fn agree(name: &str, body: RoundBody, speed: f32, build: impl Fn(&mut App) -> Entity) -> Agreement {
    let mut runtime = world_with_trimesh_plates(&[]);
    runtime.init_resource::<Read>().add_observer(read_impact);
    let target = build(&mut runtime);
    for _ in 0..8 {
        runtime.update();
    }
    runtime.world_mut().resource_mut::<Read>().0.clear();
    let read = marched(&mut runtime, body, speed);
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
    let admitted = flown.reaches(gate_target);

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
    assert_eq!(
        damaged, admitted,
        "{name}: the runtime damaged the target: {damaged}; the gate admitted it: {admitted}",
    );
    Agreement {
        impacts: read,
        admitted,
    }
}

/// A soft HP-bearing box `width` across, centred under the drop line with its top at `top`: the
/// target every arrangement aims at. Fixture data.
fn target_below(app: &mut App, top: f32, width: f32) -> Entity {
    volume(
        app,
        Vec3::new(width, 0.4, width),
        Transform::from_translation(Vec3::new(DROP_FROM.x, top - 0.2, DROP_FROM.z)),
        STEEL * 0.1,
        Some(100.0),
    )
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
#[test]
fn the_runtime_and_the_gate_decide_the_same_crossings() {
    // Stop: a plate thicker than the round can defeat.
    let stopped = agree("stop", FIXTURE_88, 800.0, |app| {
        tilted_plate(app, 0.6, 0.0, 1.0, STEEL);
        target_below(app, 0.0, 2.0)
    });
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

    // Partial coverage: a plate whose edge only part of an 88's ring overlaps; the axis misses it.
    let partial = agree("partial coverage", FIXTURE_88, 800.0, |app| {
        volume(
            app,
            Vec3::new(1.0, 0.02, 1.0),
            Transform::from_translation(Vec3::new(DROP_FROM.x + 0.5 + 0.02, 1.0, DROP_FROM.z)),
            STEEL,
            None,
        );
        target_below(app, 0.0, 2.0)
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
}

/// OVERMATCH CHARGES THE PROJECTION, AND THE GATE ADMITS THROUGH IT.
///
/// A round that overmatches an oblique plate is charged the plate's perpendicular projection, not
/// its slope chord (§4). A gate that admits by the straight slope cost sees armour the round never
/// pays for and closes a route the runtime takes. Fixture: an 88 against 25 mm at 80°; the slope
/// chord costs more than the round can spend, the projection far less (both asserted from the
/// fixture, so the arrangement cannot drift out of the case).
#[test]
fn overmatch_projection_is_admitted_as_the_runtime_charges_it() {
    let thickness = 0.025;
    let tilt = 80.0_f32.to_radians();
    let (speed, _) = first_tick(FIXTURE_88, 800.0);
    let capability_in = capability(FIXTURE_88, speed);
    let laws = WalkLaws::default();
    assert!(
        FIXTURE_88.caliber >= laws.overmatch_ratio * thickness,
        "the round overmatches"
    );
    assert!(
        thickness / tilt.cos() * STEEL > capability_in,
        "the slope chord costs more than the round can spend"
    );
    assert!(
        thickness * STEEL < capability_in,
        "the projection costs less"
    );
    let overmatched = agree("overmatch", FIXTURE_88, 800.0, |app| {
        tilted_plate(app, thickness, tilt, 1.0, STEEL);
        target_below(app, 0.0, 2.0)
    });
    assert!(
        overmatched
            .impacts
            .first()
            .is_some_and(|i| i.0 == Verdict::Through),
        "the overmatched plate is perforated: {:?}",
        overmatched.impacts,
    );
    assert!(
        overmatched.admitted,
        "and the gate admits the target behind it"
    );
}

/// A ROUND THAT SPENDS EXACTLY ITS CAPABILITY REACHES NOTHING BEHIND.
///
/// The crossing's cost equals the capability to the bit: the plate is perforated with no residual
/// (`walk::CostProfile::invert` stops only short of the whole profile), and the next crossing embeds
/// at its face with no chord into the target. The gate must agree, and a gate that admits by
/// `cost ≤ capability` does not. Fixture: a one-sample round, a metre of material whose factor is
/// the round's capability per metre, faces at exactly representable heights.
#[test]
fn an_exact_budget_crossing_terminates_before_the_target() {
    let (_, stepped) = advance_shell(
        DROP_FROM,
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
        "through the metre with nothing left, then stopped at the target's face",
    );
    assert!(!exact.admitted, "no chord into the target");
}
