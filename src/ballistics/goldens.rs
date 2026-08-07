//! Frozen end-to-end shots at the REAL Tiger — CHARACTERIZATION, not specification.
//!
//! Every other ballistics test builds a synthetic plate and asserts a law. These four fire through
//! the whole shipped stack — `bake`'s glb extraction, the material registry, the per-primitive
//! trimeshes, the disc walk, the resolver, the damage deposit — at the bound tank at spawn pose, and
//! pin the OUTCOME TUPLE: which events fired, what perforated, who lost health.
//!
//! # Reading a red golden
//!
//! **A failure here does not mean the code is wrong. It means the physics moved.** These tests
//! describe what the simulator does today; they do not claim it is what it should do. So when one
//! goes red, diff it CONSCIOUSLY — decide whether the change was the point of the work you just did
//! — and only then re-pin. Silently re-baking the expected values converts the one instrument that
//! notices a whole-stack behaviour change into a rubber stamp.
//!
//! # What is pinned, and what is not
//!
//! Kinds, counts, flags and the sign of a health change: pinned. Exact floats where the value rides
//! on mesh triangulation or on a physics-settled pose: NOT pinned — those are asserted as
//! inequalities or as geometry-relative tolerances, because a golden that fails on the last bit of a
//! trimesh normal teaches people to delete goldens.
//!
//! # The shot lines
//!
//! Stated in TANK-LOCAL metres (the frame `fuzz`'s bless list uses), converted once at the fixture
//! seam. Local `-Z` is the front, `+Z` the rear, `+Y` up. The lines were measured off the bound
//! volumes' own bounding boxes, not guessed.

use std::time::Duration;

use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;

use super::fuzz::{PROBE_TANK_AT, probe_world};
use super::*;
use crate::damage::CrewStation;

/// One captured impact.
#[derive(Clone, Copy, Debug)]
struct Captured {
    surface: ImpactSurface,
    penetrated: bool,
    deflection: Option<Vec3>,
    normal: Vec3,
    position: Vec3,
}

#[derive(Resource, Default)]
struct ImpactLog(Vec<Captured>);

fn capture_impact(impact: On<Impact>, mut log: ResMut<ImpactLog>) {
    log.0.push(Captured {
        surface: impact.surface,
        penetrated: impact.penetrated,
        deflection: impact.deflection,
        normal: impact.normal,
        position: impact.position,
    });
}

#[derive(Resource, Default)]
struct DamageLog(Vec<f32>);

fn capture_damage(damage: On<ShellDamage>, mut log: ResMut<DamageLog>) {
    log.0.push(damage.amount);
}

/// The probe world plus the REAL march and the two sinks a shot's outcome is read from.
fn golden_world() -> App {
    let mut app = probe_world().expect("the bound Tiger builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(
        16,
    )))
    .init_resource::<RetainSpentShells>()
    .insert_resource(MgShortCircuit(false))
    .init_resource::<ImpactLog>()
    .init_resource::<DamageLog>()
    .add_observer(capture_impact)
    .add_observer(capture_damage)
    .add_systems(Update, integrate_projectiles);
    app
}

/// The shot identity every golden fires under — keyed, so the authority damage confirmation
/// (`ShellDamage`) is emitted and observable.
fn golden_shot() -> ShotId {
    ShotId {
        combatant: crate::CombatantId(1),
        weapon: 0,
        fire_tick: 100,
    }
}

/// Tank-local → world. The tank spawns unrotated at [`PROBE_TANK_AT`], so this is the whole frame
/// change; it is written as a function anyway, because a golden that hides its frame conversion is
/// a golden nobody can re-derive.
fn world_at(local: Vec3) -> Vec3 {
    PROBE_TANK_AT + local
}

/// One golden's whole outcome.
struct Outcome {
    impacts: Vec<Captured>,
    damage: Vec<f32>,
    /// Health left in each named volume, after.
    health: Vec<(String, f32)>,
    /// Shells still in flight when the fixture stopped.
    survivors: usize,
    /// Ricochet points the shell recorded along its whole flight.
    ricochets: usize,
}

/// Fire one round from a tank-local origin along a tank-local direction, march it to rest, and
/// report everything the outcome tuple is read from.
fn fire(
    app: &mut App,
    local_origin: Vec3,
    local_direction: Vec3,
    round: (f32, f32, f32),
) -> Outcome {
    let (caliber, mass, speed) = round;
    let origin = world_at(local_origin);
    let direction = local_direction.normalize();
    app.world_mut().spawn((
        Projectile {
            velocity: direction * speed,
            caliber,
            mass,
            drag_k: drag_k(caliber, mass),
            disc: walk::DiscFrame::anchored(direction).expect("a golden fires along a real axis"),
        },
        DamageReport::default(),
        TerminalReport::default(),
        ShellPath {
            points: vec![origin],
            segment_starts: Vec::new(),
        },
        PenetrationMarks::default(),
        SpallMarks::default(),
        ShellReadout {
            speed,
            capability: capability(mass, speed),
        },
        Shot(golden_shot()),
        Transform::from_translation(origin).looking_to(direction, Vec3::Y),
    ));
    // Eight ticks at 16 ms: ~100 m of flight for a main-gun round, which is far more than the whole
    // tank plus the run-out past it.
    let mut ricochets = 0;
    for _ in 0..8 {
        app.update();
        ricochets = app
            .world_mut()
            .query::<&PenetrationMarks>()
            .iter(app.world())
            .map(|marks| marks.ricochets.len())
            .max()
            .unwrap_or(ricochets)
            .max(ricochets);
    }
    let health = app
        .world_mut()
        .query::<(&Name, &ComponentHealth)>()
        .iter(app.world())
        .map(|(name, hp)| (name.as_str().to_owned(), hp.current))
        .collect();
    let survivors = app
        .world_mut()
        .query::<&Projectile>()
        .iter(app.world())
        .count();
    Outcome {
        impacts: app.world().resource::<ImpactLog>().0.clone(),
        damage: app.world().resource::<DamageLog>().0.clone(),
        health,
        survivors,
        ricochets,
    }
}

impl Outcome {
    fn health_of(&self, volume: &str) -> f32 {
        self.health
            .iter()
            .find(|(name, _)| name == volume)
            .map(|(_, hp)| *hp)
            .unwrap_or_else(|| panic!("no volume named `{volume}` in the bound tank"))
    }
}

/// The Tiger's own 88 (`assets/tiger_1/tiger_1.tank.ron`, weapon `MainGun`).
const EIGHTY_EIGHT: (f32, f32, f32) = (0.088, 10.2, 773.0);
/// The Tiger's own 7.92 mm coax/hull MG.
const MG: (f32, f32, f32) = (0.0079, 0.0118, 755.0);

/// The driver's line: dead level, across the tank along `+X`, at the driver's own height.
///
/// `Hull_Side_Upper_L` is a flat vertical plate at local `x -1.631..-1.541`, `y 1.102..1.809`; the
/// `Driver` volume sits at `x -0.800..-0.348`, `y 0.534..1.652`, `z -1.962..-1.268` behind it. At
/// `y = 1.50` the line clears the sponson floor (`Hull_Side_Track_L`, up to `y 1.315`), the lower
/// side plate and the road wheels, so it crosses exactly one plate of armour before flesh.
///
/// A FLANK shot rather than the obvious head-on one through `Hull_UFP_Upper`, and deliberately so:
/// the front plate is sloped, and every sloped plate currently kills the round at its own exit face
/// (see [`a_sloped_plate_stops_the_round_at_its_own_exit_face`]). Pointing the reference golden at a
/// defective line would pin the defect as if it were the physics.
const DRIVER_LINE_FROM: Vec3 = Vec3::new(-6.0, 1.5, -1.6);
const DRIVER_LINE_DIR: Vec3 = Vec3::X;

/// GOLDEN 1 — the 88 through the driver's plate.
///
/// The reference shot of the whole model: a main-gun round meets one plate of rolled armour head on,
/// perforates it, and kills what is behind it. It also carries the §13.3 half of
/// "an arm does not ricochet an 88": the crewman the round crosses on the way through deflects
/// nothing, so the flight records no bounce at all.
#[test]
fn eighty_eight_through_the_driver_plate_perforates_and_wounds_him() {
    let mut app = golden_world();
    let before = app
        .world_mut()
        .query::<(&CrewStation, &ComponentHealth)>()
        .iter(app.world())
        .find(|(station, _)| **station == CrewStation::Driver)
        .map(|(_, hp)| hp.current)
        .expect("the bound tank has a driver");
    assert_eq!(before, 3.0, "the driver starts at his authored hp");

    let out = fire(&mut app, DRIVER_LINE_FROM, DRIVER_LINE_DIR, EIGHTY_EIGHT);

    let armor: Vec<&Captured> = out
        .impacts
        .iter()
        .filter(|hit| hit.surface == ImpactSurface::Armor)
        .collect();
    assert!(
        !armor.is_empty(),
        "PHYSICS CHANGED — diff consciously, don't just re-pin. The 88 fired at the driver's plate \
         produced no armour impact at all: {:?}",
        out.impacts
    );
    assert!(
        armor.iter().all(|hit| hit.penetrated),
        "PHYSICS CHANGED — diff consciously. Every armour read on this line should be a bite, not a \
         bounce and not a fail-closed stop: {armor:?}"
    );
    assert_eq!(
        armor.len(),
        2,
        "PHYSICS CHANGED — diff consciously. The 88 crosses the near side plate and the far one on \
         its way out: {armor:?}"
    );
    assert_eq!(
        out.survivors, 1,
        "PHYSICS CHANGED — diff consciously. 250 reference-mm of capability against ~90 mm of side \
         plate plus a crewman leaves the round flying"
    );
    assert_eq!(
        out.ricochets, 0,
        "PHYSICS CHANGED — diff consciously. Nothing on a head-on line deflects an 88, least of all \
         the crewman it crosses (§13.3's factor-weighted overmatch)"
    );
    assert_eq!(
        out.health_of("Driver"),
        0.0,
        "PHYSICS CHANGED — diff consciously. A 3 hp crewman crossed by an 88 is dead outright; he \
         started at {before}"
    );
    assert_eq!(
        out.damage.len(),
        1,
        "PHYSICS CHANGED — diff consciously. One damaging shot must confirm exactly once, got {:?}",
        out.damage
    );
}

/// GOLDEN 2 — the same line, a machine-gun round.
///
/// The other half of "an arm does not ricochet an 88", stated as its converse: identical geometry,
/// opposite outcome, decided by CAPABILITY alone (8 reference-mm against 250). The MG bites into the
/// same plate and dies inside it; the driver behind it is untouched, and no confirmation rides the
/// wire. The plate stops it — the crewman never gets a say either way.
#[test]
fn the_mg_burst_dies_in_the_driver_plate_and_leaves_him_untouched() {
    let mut app = golden_world();
    let out = fire(&mut app, DRIVER_LINE_FROM, DRIVER_LINE_DIR, MG);

    let armor: Vec<&Captured> = out
        .impacts
        .iter()
        .filter(|hit| hit.surface == ImpactSurface::Armor)
        .collect();
    assert_eq!(
        armor.len(),
        1,
        "PHYSICS CHANGED — diff consciously. The MG round should read the side plate exactly once: \
         {:?}",
        out.impacts
    );
    assert!(
        armor[0].penetrated,
        "PHYSICS CHANGED — diff consciously. `penetrated` means the round BIT STEEL (an embed reads \
         it too); an MG round on a head-on plate is not a ricochet"
    );
    assert_eq!(
        out.health_of("Driver"),
        3.0,
        "PHYSICS CHANGED — diff consciously. A 7.92 mm round stopped by 90 mm of plate cannot cost \
         the driver a single hit point"
    );
    assert!(
        out.damage.is_empty(),
        "PHYSICS CHANGED — diff consciously. Nothing was damaged, so nothing may be confirmed: {:?}",
        out.damage
    );
    assert_eq!(
        out.survivors, 0,
        "PHYSICS CHANGED — diff consciously. The MG round embeds; it must not come out the far side"
    );
}

/// GOLDEN 3 — a rear shot into the engine.
///
/// `Engine` sits at local `x ±0.357`, `y 0.458..1.717`, `z 1.379..2.657`, behind `Hull_Rear`
/// (`z 2.579..2.831`). The point is the SUBSTANCE: the engine is `EngineBlock` at factor 150, a
/// sixth of steel, so a round that would be stopped by an equal chord of armour walks through it —
/// and the block, which has hit points, is what pays.
#[test]
fn a_rear_shot_soaks_in_the_engine_block() {
    let mut app = golden_world();
    let factor = app
        .world_mut()
        .query::<(&Name, &BallisticVolume)>()
        .iter(app.world())
        .find(|(name, _)| name.as_str() == "Engine")
        .map(|(_, volume)| (volume.material_factor, volume.substance.clone()))
        .expect("the bound tank has an engine volume");
    assert_eq!(
        factor,
        (150.0, "EngineBlock".to_owned()),
        "PHYSICS CHANGED — diff consciously. The engine's resistance comes from the substance \
         registry (`assets/materials/materials.ron`), and this golden is written against 150"
    );

    let out = fire(
        &mut app,
        Vec3::new(0.0, 1.2, 8.0),
        Vec3::NEG_Z,
        EIGHTY_EIGHT,
    );

    assert!(
        out.impacts
            .iter()
            .any(|hit| hit.surface == ImpactSurface::Armor && hit.penetrated),
        "PHYSICS CHANGED — diff consciously. The rear plate should be bitten: {:?}",
        out.impacts
    );
    assert_eq!(
        out.health_of("Engine"),
        0.0,
        "PHYSICS CHANGED — diff consciously. A metre of engine block at factor 150 is 150 \
         reference-mm of transit damage against a 10 hp pool — the block is destroyed outright"
    );
    assert!(
        !out.damage.is_empty(),
        "PHYSICS CHANGED — diff consciously. Destroying the engine must confirm damage"
    );
}

/// GOLDEN 4 — a grazing MG round off the hull side.
///
/// `Hull_Side_Upper_L` is a flat vertical plate at local `x -1.631..-1.541`, spanning
/// `y 1.102..1.809`. The line meets it at 75° from its normal — past the ricochet angle (1.221 rad ≈
/// 70°) and nowhere near overmatched (7.9 mm against 3 × 80 mm) — so §4's bounce fires.
///
/// The disc degenerates to its axis below 20 mm calibre (`resolve::DISC_MIN_CALIBER`), so η is
/// exactly 1 here and §13.5's η-scaled blend must return the CLASSIC specular reflection verbatim.
/// That is what the direction assertion pins: full coverage, full angle, no partial graze.
#[test]
fn a_grazing_mg_round_ricochets_off_the_hull_side_at_full_angle() {
    let mut app = golden_world();
    let direction = Vec3::new(0.26, 0.0, 0.966).normalize();
    let out = fire(&mut app, Vec3::new(-3.0, 1.5, -6.0), direction, MG);

    let bounce = out
        .impacts
        .iter()
        .find(|hit| hit.deflection.is_some())
        .unwrap_or_else(|| {
            panic!(
                "PHYSICS CHANGED — diff consciously. A 75° MG graze on the hull side must deflect: \
                 {:?}",
                out.impacts
            )
        });
    assert_eq!(
        bounce.surface,
        ImpactSurface::Armor,
        "PHYSICS CHANGED — diff consciously. The bounce is off armour"
    );
    assert!(
        !bounce.penetrated,
        "PHYSICS CHANGED — diff consciously. A ricochet bit no steel"
    );
    assert!(
        (bounce.position.x - world_at(Vec3::ZERO).x).abs() > 1.4,
        "PHYSICS CHANGED — diff consciously. The bounce should land on the hull FLANK (local |x| ≈ \
         1.6), got {:?}",
        bounce.position - world_at(Vec3::ZERO)
    );

    let out_dir = bounce.deflection.expect("checked above");
    let normal = bounce.normal.normalize();
    let specular = direction - 2.0 * direction.dot(normal) * normal;
    assert!(
        out_dir.normalize().distance(specular.normalize()) < 1.0e-3,
        "PHYSICS CHANGED — diff consciously. At η = 1 the outgoing direction IS the specular \
         reflection, returned bit-for-bit (§13.5, ruled 2026-08-07): expected {specular:?}, got \
         {out_dir:?}"
    );
    assert_eq!(
        out.ricochets, 1,
        "PHYSICS CHANGED — diff consciously. Exactly one bounce on this line"
    );
    assert!(
        out.damage.is_empty(),
        "PHYSICS CHANGED — diff consciously. A bounce off a plate with no hit points damages nothing"
    );
}

/// KNOWN DEFECT, pinned so its fix is noticed — NOT a statement of intended physics.
///
/// A head-on 88 into `Hull_UFP_Upper` (the driver's front plate) bites in, and then the round STOPS
/// DEAD INSIDE THE PLATE. The march's second corridor starts one [`super::MARCH_EPS`] past the
/// perforation exit and the collector re-reports the exit face it has just left: `collect::admit`
/// clamps any UNSEEDED crossing behind the corridor origin to `t = 0` ("the ray is sitting on the
/// face") with no bound on how far behind, so the walk is handed an exit for a primitive it is not
/// inside — `WalkError::UnexpectedExit { t: 0.0 }` — and fails closed.
///
/// It bites SLOPED plates only, which is why the flank goldens above are green: a plate square to
/// the ray has exit-face triangles of zero extent along it, so the collector's prune drops them
/// before `admit` is ever consulted. On a slope the same triangles reach forward past the corridor
/// origin, survive the prune, and cross at `t ≈ −MARCH_EPS`. Measured 2026-08-07: 40 / 40 head-on
/// lines across the whole front plate, and 4 / 20 flank lines that clip the sloped sponson floor.
///
/// FAIL-CLOSED, so it is a stopped round rather than free penetration — the §13 doctrine working as
/// designed. The consequence is not an immune tank: the crossing that DID resolve still deposits its
/// transit damage, so the driver behind the plate still dies. What is lost is everything past that
/// contact — the exit, the spall cone, every volume deeper in, and the round itself.
///
/// WHEN THIS TEST FAILS: the defect is fixed. Delete it, and point
/// [`eighty_eight_through_the_driver_plate_perforates_and_wounds_him`] back at the head-on line —
/// `(-0.57, 1.45, -6.5)` along `+Z`, which crosses `Hull_UFP_Upper` and then the driver.
#[test]
fn a_sloped_plate_stops_the_round_at_its_own_exit_face() {
    let mut app = golden_world();
    let out = fire(
        &mut app,
        Vec3::new(-0.57, 1.45, -6.5),
        Vec3::Z,
        EIGHTY_EIGHT,
    );

    let stopped = out.impacts.iter().any(|hit| {
        hit.surface == ImpactSurface::Armor && !hit.penetrated && hit.deflection.is_none()
    });
    assert!(
        stopped,
        "the sloped-plate exit-face defect is GONE — see this test's doc comment for what to do \
         about it: {:?}",
        out.impacts
    );
    assert_eq!(
        out.survivors, 0,
        "the fail-closed round is despawned at the contact it could not resolve"
    );
    assert_eq!(
        out.impacts.len(),
        2,
        "the shape of the defect: one honest bite, then the stop — never an exit: {:?}",
        out.impacts
    );
}
