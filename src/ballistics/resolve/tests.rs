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
use crate::ballistics::{
    ComponentHealth, RetainSpentShells, SpallMarks, Verdict, drag_k, freeflight_step,
};

/// The fixture tick the plate worlds step by (`march_tests::plate_world`'s manual 16 ms).
const DT: f32 = 0.016;
/// Where every shot of this section starts: straight above the geometry, falling, so gravity and
/// drag change the speed but never the direction, and the gate's straight flight is the runtime's.
const DROP_FROM: Vec3 = Vec3::new(0.1, 3.0, -0.2);

/// The top of a target out of reach of every spall fragment thrown from a plate at `y = 1`: 6.5 m
/// below it, past the 6 m a fragment flies (`throw_spall_burst`'s range). Fixture data; each
/// arrangement also asserts that no fragment deposited ([`Agreement::transit_only`]).
const PAST_SPALL: f32 = -5.5;

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
}

/// The runtime's side: one tick of the real march, read back as impacts, and whether any spall
/// fragment the round threw deposited HP.
fn marched(app: &mut App, body: RoundBody, speed: f32) -> (Vec<(Verdict, Vec3)>, bool) {
    let shell = spawn_round(app, body, DROP_FROM, Vec3::NEG_Y * speed);
    app.update();
    let fragments = app.world().get::<SpallMarks>(shell).is_some_and(|marks| {
        marks
            .bursts
            .iter()
            .flat_map(|burst| &burst.fragments)
            .any(|fragment| fragment.deposited)
    });
    (app.world().resource::<Read>().0.clone(), fragments)
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
    /// The gate admitted the target: some crossing put a positive TRANSIT chord into it.
    admitted: bool,
    /// The runtime removed HP from the target, by any route.
    damaged: bool,
    /// A spall fragment the runtime threw deposited HP — a route the admission does not observe.
    fragments: bool,
    /// The round bounced off an HP-bearing volume and jarred it — a route the admission does not
    /// observe either.
    shock: bool,
}

impl Agreement {
    /// The runtime's only HP route was transit, the one the admission observes.
    fn transit_only(&self) -> bool {
        !self.fragments && !self.shock
    }
}

/// Build the arrangement twice, fly `body` through the runtime in one and through the gate's driver
/// in the other, and require that they agree on every impact, and — where transit was the runtime's
/// only HP route ([`Agreement::transit_only`]) — on whether the target was reached.
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
    let (read, fragments) = marched(&mut runtime, body, speed);
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
        admitted,
        damaged,
        fragments,
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
///
/// THE PARITY IS TRANSIT PARITY. The admission observes transit deposits; the runtime also removes
/// HP by a bounce's shock and by spall fragments. Every arrangement here is transit-only, asserted,
/// with targets placed past spall reach where a perforation could throw any;
/// [`shock_and_fragments_lie_outside_the_transit_parity`] shows the two routes outside it.
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
        target_below(app, PAST_SPALL, 6.0)
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
        target_below(app, PAST_SPALL, 6.0)
    });
    assert!(overmatched.transit_only());
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
        "through the metre with nothing left, then stopped at the target's face",
    );
    assert!(!exact.admitted, "no chord into the target");
    assert!(exact.transit_only());
}

/// SHOCK AND FRAGMENTS LIE OUTSIDE THE TRANSIT PARITY.
///
/// Two routes by which the runtime removes HP that the admission does not observe, each with the
/// round's own path kept clear of the HP-bearing box: a round that bounces off the box itself
/// jars it ([`Bounce::shock`]), and a perforation's spall reaches a box beside the round's path.
/// Both sides still agree on every impact; the box is damaged and not admitted. Fixture data.
#[test]
fn shock_and_fragments_lie_outside_the_transit_parity() {
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
            Some(100.0),
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

    // Fragments: an 88 perforates 60 mm of steel square on; an HP-bearing box sits 0.3 m below the
    // plate, beside the disc's path but inside the spall cone.
    let thrown = agree("fragments", FIXTURE_88, 800.0, |app| {
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
            Some(100.0),
        )
    });
    assert_eq!(
        thrown.impacts.first().map(|i| i.0),
        Some(Verdict::Through),
        "the plate is perforated: {:?}",
        thrown.impacts,
    );
    assert!(
        thrown.fragments && thrown.damaged && !thrown.admitted,
        "a fragment reaches the box beside the path, and that is not a transit route: fragments \
         {}, damaged {}, admitted {}",
        thrown.fragments,
        thrown.damaged,
        thrown.admitted,
    );
}

/// A RAISED FLOOR CAN REACH A BOX AFTER A BOUNCE ON A ROUTE NO LISTED GUN TAKES.
///
/// The gate's findings are admitted by the floor probe — the least capable listed round, flown at
/// the speed where its capability is the floor — and its per-region report measures each listed
/// gun at its own speed. A raised floor flies that round faster than any gun fires it, and a bounce
/// separates the two: the graze bleeds its speed, a gun that overmatches the grazed plate bites in
/// and bends away instead of bouncing, and a gun that bounces with it has too little left after the
/// bleed. So "some listed gun admits every finding" is not an invariant of a raised floor. Generic
/// geometry: a thin plate grazed at 75°, a skin square to the bounce, and a box behind the skin.
#[test]
fn a_raised_floor_reaches_a_box_after_a_bounce_no_listed_gun_takes() {
    let laws = WalkLaws::default();
    let thickness = 0.008;
    let tilt = 75.0_f32.to_radians();
    let rotation = Quat::from_rotation_z(tilt);
    let normal = rotation * Vec3::Y;
    let struck = Vec3::new(DROP_FROM.x, 1.0, DROP_FROM.z);
    let bounced = Vec3::NEG_Y - 2.0 * Vec3::NEG_Y.dot(normal) * normal;
    let (probe, probe_speed) = fuzz::floor_probe(&fuzz::FuzzConfig {
        finding_floor: Some(200.0),
        ..default()
    });
    // Capability after a full-coverage bounce, which keeps `ricochet_bleed` of the speed.
    let after_bounce = |body: RoundBody, speed: f32| capability(body, speed * laws.ricochet_bleed);
    let bouncers: Vec<f32> = fuzz::PROBE_ROUNDS
        .iter()
        .filter(|round| round.caliber < laws.overmatch_ratio * thickness)
        .map(|round| after_bounce(round.body(), round.speed))
        .collect();
    let strongest_bouncer = bouncers.iter().copied().fold(0.0, f32::max);
    let probe_left = after_bounce(probe, probe_speed);
    assert!(
        probe.caliber < laws.overmatch_ratio * thickness && !bouncers.is_empty(),
        "the floor probe and some guns bounce off the plate rather than overmatch it",
    );
    assert!(
        strongest_bouncer < probe_left,
        "the raised floor keeps more after the bleed than any bouncing gun: {strongest_bouncer} \
         against {probe_left}",
    );
    // The skin costs more than any gun that bounces can spend, less than the probe can.
    let skin = 0.5 * (strongest_bouncer + probe_left) / STEEL;

    let mut app = world_with_trimesh_plates(&[]);
    volume(
        &mut app,
        Vec3::new(0.3, thickness, 0.3),
        Transform::from_translation(struck - normal * thickness * 0.5).with_rotation(rotation),
        STEEL,
        None,
    );
    let square = Quat::from_rotation_arc(Vec3::Y, -bounced);
    volume(
        &mut app,
        Vec3::new(0.4, skin, 0.4),
        Transform::from_translation(struck + bounced * 0.5).with_rotation(square),
        STEEL,
        None,
    );
    let target = volume(
        &mut app,
        Vec3::splat(0.3),
        Transform::from_translation(struck + bounced * 0.9).with_rotation(square),
        STEEL * 0.1,
        Some(100.0),
    );
    for _ in 0..8 {
        app.update();
    }
    let reaches = |app: &mut App, body: RoundBody, speed: f32| {
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
                    fuzz::drive(&context, body, speed, DROP_FROM, Vec3::NEG_Y, 5.0)
                },
            )
            .expect("the driver system runs")
            .unwrap_or_else(|failure| panic!("the driven flight failed to walk: {failure:?}"))
            .reaches(target)
    };
    assert!(
        reaches(&mut app, probe, probe_speed),
        "the raised floor's probe reaches the box after the bounce",
    );
    for round in fuzz::PROBE_ROUNDS {
        assert!(
            !reaches(&mut app, round.body(), round.speed),
            "{} reaches the box, so this is not the case",
            round.name,
        );
    }
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
/// nudge (DERIVED below), so [`MAX_STEP_CROSSINGS`] links span metres, not [`MAX_OVERRUN`].
///
/// Generic geometry, a single-sample round falling straight down so no bend moves it off the
/// chain: square entry faces (no normalization bend), exit faces sheared 65° (not opposed to the
/// next entry, so no ε-weld), and the chain starting a metre short of the budget's end. Fixture
/// data throughout.
#[test]
fn a_chain_of_held_continuations_stops_at_the_step_s_limits() {
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
    let top = DROP_FROM.y as f64 - budget as f64 + 1.0;
    let links = MAX_STEP_CROSSINGS + 100;
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
                    material_factor: STEEL * 1.0e-5,
                    substance: "fixture".to_string(),
                },
            ));
        }
    };

    let mut runtime = world_with_trimesh_plates(&[]);
    runtime.init_resource::<Read>().add_observer(read_impact);
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
        through, MAX_STEP_CROSSINGS,
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
    build(&mut gate);
    for _ in 0..8 {
        gate.update();
    }
    let failure = try_driven(&mut gate, body, 755.0)
        .expect_err("the gate's driver stops the chain at the same limit");
    assert!(
        matches!(
            failure.error,
            WalkError::DrainExhausted { crossings, .. } if crossings == MAX_STEP_CROSSINGS + 1
        ),
        "the driver names the exhausted count: {failure:?}",
    );
}

/// THE STEP REFUSES THE CROSSING THAT WOULD CARRY IT PAST ITS OVERRUN, before anything is charged.
#[test]
fn a_step_refuses_the_crossing_past_its_overrun() {
    let mut budget = Budget::new(1.0);
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
    let mut budget = Budget::new(1.0);
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
                let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    armor: &armor,
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
                let decision = decide(&context, &|_| true, ROUND, &flight, contact)?;
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
                let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    armor: &armor,
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
                material_factor: STEEL,
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
                let armor = SpatialQueryFilter::from_mask(crate::Layer::Armor);
                let context = ResolveContext {
                    world: &world,
                    colliders: &colliders,
                    armor: &armor,
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
/// It refuses today: the start's own line crosses the lower face transversely in exact arithmetic,
/// but the walk reads that crossing's orientation as a tangent and drops it (see
/// [`a_crossing_the_collector_admits_is_never_read_as_a_tangent`]), so the probe does not close.
#[test]
fn a_start_is_certified_on_the_ray_its_corridor_resumes_along() {
    let (mut app, built) = settled(grazed_prism);
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

/// A START A HAIR INSIDE A GRAZED FACE IS CERTIFIED INSIDE IT.
///
/// The grazed prism, certified one f32 step off the lower face's own direction, `(0.6000001, 0.8,
/// 0)`: the start's line still runs within a micron of the face across the whole prism, now
/// crossing it transversely (DERIVED: ~0.15 m ahead of the start), and its orientation survives
/// f32. A probe line formed metres behind the start loses the start's `2⁻²⁶` and crosses the face
/// behind it instead, reading the start outside; the corridor resumed along the same direction then
/// meets the exit with no entry owned. Read on the start's own line, the start is inside and the
/// resumed corridor pairs.
#[test]
fn a_start_a_hair_inside_a_grazed_face_is_certified_inside_it() {
    let (mut app, built) = settled(grazed_prism);
    let swept = Vec3::new(0.600_000_1, 0.8, 0.0);
    let starts = resumed(&mut app, GRAZED_START, swept, swept, 0.0);
    assert_eq!(
        inside_volumes(&starts.seeds[0]),
        built,
        "the start lies inside the prism",
    );
}

/// A CROSSING THE COLLECTOR ADMITS IS NEVER READ AS A TANGENT.
///
/// The collector admits a triangle whenever the exact crossing determinant is non-zero, but the
/// walk (`walk::canonical_crossings`) signs it by the f32 dot of the axis with the face's normalized
/// normal, and drops it as a tangent when that dot is exactly zero. Along `(0.6, 0.8, 0)` in f32 the
/// prism's lower-face normal `(0.8, −0.6, 0)` dots to exactly zero, while the exact determinant does
/// not vanish (DERIVED: `0.6f·8 − 0.8f·6 ≈ 1.2e-7`), so the start's own line exits through that face
/// and the walk never sees the exit: the probe ends with the prism open, `IncompleteCorridor`.
#[test]
#[ignore = "open defect: a transverse crossing's orientation is read from an f32 dot that can \
            cancel to zero, so the walk drops a crossing the collector admitted; the certificate \
            fails closed on it"]
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
                    material_factor: STEEL,
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
