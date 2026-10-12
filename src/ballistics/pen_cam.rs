//! The pen cam recorder: scripted shots through the real march, read back as pen cam records
//! ([`crate::damage_cam::record`]).
//!
//! A scenario names a round from the bound tank's own weapons, where it is fired from and toward,
//! and the target's pose; [`record_scenario`] fires it at the bound tank in the probe world
//! (headless, no window, no input) and returns the record.
//!
//! The class-invariant tests at the end fire at generic fixture plates, not at the bound tank.

use std::time::Duration;

use avian3d::prelude::{CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
use bevy::time::TimeUpdateStrategy;
use serde::Deserialize;

use super::fuzz::{PROBE_TANK_AT, ProbeTank, probe_world, probe_world_posed};
use super::march_tests::{FIXTURE_88, fixture_round};
use super::*;
use crate::damage::VolumeOf;
use crate::damage_cam::record::{self, ShotFacts, StruckTank};
use crate::damage_cam::{PenetrationReport, PlateClass, ShotVerdict};
use crate::spec::VolumeSpec;
use crate::tank::Rig;

/// Fixed-step length the march is driven at (s), as the goldens drive it.
const STEP: Duration = Duration::from_millis(16);
/// Steps a shot is marched for: 8 × 16 ms carries a main-gun round ~100 m (DERIVED), the whole
/// tank and the run-out past it.
const STEPS: usize = 8;

/// One scripted shot. Positions are metres in the target's spawn frame: the probe tank's root at
/// the origin, the world's axes (`-Z` its front, `+Y` up). The target's pose turns the tank under
/// the shot; the shot itself does not move.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// The record's file stem.
    name: String,
    /// One of the bound tank's weapons, by its name in the tank file.
    weapon: String,
    /// Striking speed (m/s); the weapon's muzzle speed when absent.
    #[serde(default)]
    speed: Option<f32>,
    from: Vec3,
    toward: Vec3,
    /// The hull's yaw about `+Y` (degrees).
    #[serde(default)]
    hull_yaw_deg: f32,
    /// The turret's traverse from its spawn pose about the hull's `+Y` (degrees).
    #[serde(default)]
    turret_yaw_deg: f32,
}

/// The probe world posed for `scenario`, with the march driven on [`STEP`] and spent shells kept.
fn scenario_world(scenario: &Scenario) -> App {
    let pose = Transform::from_translation(PROBE_TANK_AT)
        .with_rotation(Quat::from_rotation_y(scenario.hull_yaw_deg.to_radians()));
    let mut app = probe_world_posed(pose).expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(MgShortCircuit(false))
        .add_systems(Update, integrate_projectiles);
    if scenario.turret_yaw_deg != 0.0 {
        let tank = app.world().resource::<ProbeTank>().0;
        let turret = app
            .world()
            .get::<Rig>(tank)
            .expect("the tank has a rig")
            .turret;
        let mut transform = app
            .world_mut()
            .get_mut::<Transform>(turret)
            .expect("the turret is posed");
        transform.rotation =
            Quat::from_rotation_y(scenario.turret_yaw_deg.to_radians()) * transform.rotation;
        // The turret's colliders follow its new pose before anything is cast.
        for _ in 0..4 {
            app.update();
        }
    }
    app
}

/// The turret's yaw relative to the hull (degrees).
fn turret_yaw(world: &World, tank: Entity) -> f32 {
    let rig = world.get::<Rig>(tank).expect("the tank has a rig");
    let rotation = |entity: Entity| {
        world
            .get::<GlobalTransform>(entity)
            .expect("a posed node")
            .to_scale_rotation_translation()
            .1
    };
    let relative = rotation(rig.hull).inverse() * rotation(rig.turret);
    relative.to_euler(EulerRot::YXZ).0.to_degrees()
}

/// Fire `scenario` at the bound tank and read back its record.
fn record_scenario(scenario: &Scenario) -> PenetrationReport {
    let mut app = scenario_world(scenario);
    let tank = app.world().resource::<ProbeTank>().0;
    let spec = app
        .world()
        .resource::<crate::bake::TankBlueprint>()
        .spec
        .clone();
    let weapon = spec
        .weapons
        .get(&scenario.weapon)
        .unwrap_or_else(|| panic!("{}: no weapon `{}`", scenario.name, scenario.weapon));
    let body = RoundBody {
        mass: weapon.mass,
        caliber: weapon.caliber,
        nose_factor: weapon.nose.factor(),
    };
    let speed = scenario.speed.unwrap_or(weapon.speed);
    let origin = PROBE_TANK_AT + scenario.from;
    let direction = (scenario.toward - scenario.from).normalize();
    let struck = StruckTank::capture(app.world_mut(), tank).expect("the tank has a rig");
    let (projectile, readout) = fixture_round(body, direction * speed);
    let shell = app
        .world_mut()
        .spawn((
            projectile,
            readout,
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![origin],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            Transform::from_translation(origin).looking_to(direction, Vec3::Y),
        ))
        .id();
    for _ in 0..STEPS {
        app.update();
    }
    let facts = ShotFacts {
        launch: origin,
        projectile: scenario.weapon.clone(),
        caliber_mm: weapon.caliber * 1000.0,
        mass_kg: weapon.mass,
        vehicle: Some(crate::tank::TIGER_SPEC_PATH.to_owned()),
        turret_yaw_deg: turret_yaw(app.world(), tank),
    };
    record::report(app.world_mut(), &struck, shell, &facts)
        .unwrap_or_else(|| panic!("{}: the shot opens no pen cam", scenario.name))
}

/// A record is stated in the struck hull's frame: one shot fired at the bound tank at two hull
/// poses, turned with the tank, records the same verdict, plates, boxes and path. The shot down a
/// front plate and the 30° pose are fixture data; 1 mm is the tolerance on a position.
#[test]
fn a_record_is_stated_in_the_struck_hull_frame() {
    let shot = |yaw_deg: f32| {
        let turn = Quat::from_rotation_y(yaw_deg.to_radians());
        record_scenario(&Scenario {
            name: format!("hull yaw {yaw_deg}"),
            weapon: "MainGun".to_owned(),
            speed: None,
            from: turn * Vec3::new(-0.57, 1.45, -6.5),
            toward: turn * Vec3::new(-0.57, 1.45, 0.0),
            hull_yaw_deg: yaw_deg,
            turret_yaw_deg: 0.0,
        })
    };
    let (still, turned) = (shot(0.0), shot(30.0));
    let near = |a: [f32; 3], b: [f32; 3]| Vec3::from(a).distance(Vec3::from(b)) < 1.0e-3;
    assert_eq!(still.impact.verdict, turned.impact.verdict);
    assert!(near(still.impact.point, turned.impact.point));
    let names = |report: &PenetrationReport| -> Vec<String> {
        report
            .plates
            .iter()
            .map(|plate| plate.name.clone())
            .collect()
    };
    assert_eq!(names(&still), names(&turned));
    for (a, b) in still.plates.iter().zip(&turned.plates) {
        assert!(near(a.entry, b.entry), "{}", a.name);
    }
    for (a, b) in still.internals.iter().zip(&turned.internals) {
        assert_eq!(a.name, b.name);
        assert!(near(a.center, b.center), "{}", a.name);
    }
    assert_eq!(still.path.len(), turned.path.len());
    for (a, b) in still.path.iter().zip(&turned.path) {
        assert!(near(a.point, b.point), "{:?} vs {:?}", a.point, b.point);
    }
}

/// Every entity carrying a volume of a node the tank file tags external carries
/// [`ExternalArmour`], a mixed node's per-substance volumes included, and no other does.
#[test]
fn external_armour_marks_exactly_the_tagged_volumes() {
    let mut app = probe_world().expect("the bound tank builds");
    let spec = app
        .world()
        .resource::<crate::bake::TankBlueprint>()
        .spec
        .clone();
    let mut volumes = app
        .world_mut()
        .query::<(Entity, Has<ExternalArmour>, Option<&Name>, Option<&ChildOf>)>();
    let found: Vec<(Entity, bool, Option<String>, Option<Entity>)> = volumes
        .iter(app.world())
        .map(|(entity, external, name, parent)| {
            (
                entity,
                external,
                name.map(|name| name.as_str().to_owned()),
                parent.map(ChildOf::parent),
            )
        })
        .collect();
    let mut tagged = 0;
    for (entity, external, name, parent) in found {
        if app.world().get::<BallisticVolume>(entity).is_none() {
            assert!(!external, "{entity}: external armour on a non-volume");
            continue;
        }
        let node = name.or_else(|| {
            parent
                .and_then(|parent| app.world().get::<Name>(parent))
                .map(|name| name.as_str().to_owned())
        });
        let node = node.expect("a volume is named or under a named node");
        let expected = matches!(spec.volumes.get(&node), Some(VolumeSpec::External));
        assert_eq!(external, expected, "{node}");
        tagged += usize::from(external);
    }
    let declared = spec
        .volumes
        .values()
        .filter(|volume| matches!(volume, VolumeSpec::External))
        .count();
    assert!(
        tagged >= declared,
        "{tagged} tagged volumes for {declared} external entries"
    );
}

// ---------------------------------------------------------------------------------------------
// Class invariants on generic fixtures
// ---------------------------------------------------------------------------------------------

/// What a fixture volume is.
#[derive(Clone, Copy)]
enum Kind {
    Main,
    External,
    /// A box with this HP pool.
    Box(f32),
}

/// Reference-mm per metre: steel, and the box's soft substance. Fixture data.
const STEEL: f32 = 1000.0;
const SOFT: f32 = 10.0;

/// One fixture volume: name, full extents and centre (m), kind.
type FixtureVolume = (&'static str, Vec3, Vec3, Kind);

/// A static fixture tank owning `volumes`, each its own static cuboid on the `Armor` layer, and
/// the real march driven on [`STEP`] with spent shells kept.
fn fixture_world(volumes: &[FixtureVolume]) -> (App, Entity) {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin::default(),
        PhysicsPlugins::default(),
    ))
    .init_asset::<Mesh>()
    .insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
    .insert_resource(RetainSpentShells(true))
    .insert_resource(MgShortCircuit(false))
    .add_systems(Update, integrate_projectiles);
    while app.plugins_state() == bevy::app::PluginsState::Adding {
        std::thread::sleep(Duration::from_millis(1));
    }
    app.finish();
    app.cleanup();
    let tank = app
        .world_mut()
        .spawn((Transform::default(), RigidBody::Static))
        .id();
    app.world_mut().entity_mut(tank).insert(Rig {
        hull: tank,
        turret: tank,
        gun: tank,
        muzzle: tank,
    });
    for &(name, size, at, kind) in volumes {
        let factor = if matches!(kind, Kind::Box(_)) {
            SOFT
        } else {
            STEEL
        };
        let mut volume = app.world_mut().spawn((
            Name::new(name),
            Transform::from_translation(at),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(factor),
                substance: "fixture".to_owned(),
            },
            VolumeOf(tank),
        ));
        match kind {
            Kind::Main => {}
            Kind::External => {
                volume.insert(ExternalArmour);
            }
            Kind::Box(hp) => {
                volume.insert(ComponentHealth {
                    current: hp,
                    max: hp,
                });
            }
        }
    }
    for _ in 0..8 {
        app.update();
    }
    (app, tank)
}

/// Fire the fixture 88 from `from` along `-Z` at 800 m/s (fixture data) and read back its record.
fn fixture_shot(volumes: &[FixtureVolume], from: Vec3) -> PenetrationReport {
    let (mut app, tank) = fixture_world(volumes);
    let struck = StruckTank::capture(app.world_mut(), tank).expect("the fixture has a rig");
    let (projectile, readout) = fixture_round(FIXTURE_88, Vec3::NEG_Z * 800.0);
    let shell = app
        .world_mut()
        .spawn((
            projectile,
            readout,
            DamageReport::default(),
            TerminalReport::default(),
            ShellPath {
                points: vec![from],
                segment_starts: Vec::new(),
            },
            PenetrationMarks::default(),
            SpallMarks::default(),
            Transform::from_translation(from).looking_to(Vec3::NEG_Z, Vec3::Y),
        ))
        .id();
    for _ in 0..STEPS {
        app.update();
    }
    let facts = ShotFacts {
        launch: from,
        projectile: "fixture".to_owned(),
        caliber_mm: FIXTURE_88.caliber * 1000.0,
        mass_kg: FIXTURE_88.mass,
        vehicle: None,
        turret_yaw_deg: 0.0,
    };
    let report = record::report(app.world_mut(), &struck, shell, &facts).expect("a pen cam opens");
    assert_eq!(
        ShotVerdict::of_path(&report.steps()),
        Some(report.impact.verdict),
        "the record's own path reads its verdict"
    );
    report
}

/// The head-on line every fixture fires down.
const LINE_FROM: Vec3 = Vec3::new(0.0, 2.0, 2.0);
/// Plate faces 3 m square, centred on the line's height. A 50 mm steel plate is perforated by the
/// fixture 88 and a 500 mm one stops it (fixture data, as the march's own plate tests use them).
const FACE: f32 = 3.0;
const THIN: f32 = 0.05;
const THICK: f32 = 0.5;

fn plate(name: &'static str, thickness: f32, z: f32, kind: Kind) -> FixtureVolume {
    (
        name,
        Vec3::new(FACE, FACE, thickness),
        Vec3::new(0.0, 2.0, z),
        kind,
    )
}

/// A round that perforates external armour and is stopped by the main armour behind it did not
/// penetrate.
#[test]
fn perforating_a_skirt_and_stopping_in_the_hull_is_a_non_pen() {
    let report = fixture_shot(
        &[
            plate("skirt", 0.01, 0.0, Kind::External),
            plate("hull", THICK, -0.8, Kind::Main),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::NonPen);
    let classes: Vec<(PlateClass, bool)> = report
        .plates
        .iter()
        .map(|plate| (plate.class, plate.exit.is_some()))
        .collect();
    assert_eq!(
        classes,
        [(PlateClass::External, true), (PlateClass::Main, false)]
    );
}

/// Perforating main armour is a penetration even when a far plate then stops the round.
#[test]
fn through_main_armour_then_stopped_in_a_far_plate_is_a_penetration() {
    let report = fixture_shot(
        &[
            plate("front", THIN, 0.0, Kind::Main),
            plate("rear", THICK, -2.0, Kind::Main),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    let exits: Vec<bool> = report.plates.iter().map(|p| p.exit.is_some()).collect();
    assert_eq!(exits, [true, false]);
}

/// A round that perforates main armour and leaves is a penetration.
#[test]
fn through_and_out_is_a_penetration() {
    let report = fixture_shot(&[plate("front", THIN, 0.0, Kind::Main)], LINE_FROM);
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    let last = report.path.last().expect("a path");
    assert!(last.point[2] < -THIN, "the path leaves past the plate");
}

/// A penetration that doses nothing is still a penetration: the box beside the line is recorded
/// at the shot, undosed.
#[test]
fn a_penetration_with_no_dose_is_a_penetration() {
    let report = fixture_shot(
        &[
            plate("front", THIN, 0.0, Kind::Main),
            (
                "box",
                Vec3::splat(0.5),
                Vec3::new(3.0, 2.0, -1.0),
                Kind::Box(3.0),
            ),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    assert_eq!(report.internals.len(), 1, "the box is recorded");
    assert_eq!(report.internals[0].count, 3.0);
}

/// A box on the line takes the line's dose and the spall cone's, and the record names both.
#[test]
fn a_box_behind_a_perforated_plate_is_dosed_by_line_and_cone() {
    let report = fixture_shot(
        &[
            plate("front", THIN, 0.0, Kind::Main),
            (
                "box",
                Vec3::splat(0.5),
                Vec3::new(0.0, 2.0, -1.0),
                Kind::Box(1000.0),
            ),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    let dose = report.doses.first().expect("the box is dosed");
    assert_eq!(dose.volume, "box");
    assert!(dose.line > 0.0 && dose.field > 0.0, "{dose:?}");
    assert!(!report.spall.is_empty(), "the plate's exit throws a cone");
}
