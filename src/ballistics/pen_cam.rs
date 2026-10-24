//! The pen cam recorder: scripted shots through the real march, read back as pen cam records
//! ([`crate::damage_cam::record`]), and the scenario files the demo plays.
//!
//! A scenario names a round from the bound tank's own weapons, where it is fired from and toward,
//! and the target's pose; [`record_scenario`] fires it at the bound tank in the probe world
//! (headless, no window, no input) and returns the record. The scenario list and the records it
//! produces live in [`SCENARIO_DIR`]; [`bless_scenarios`], an ignored tool run on demand,
//! re-records every one.
//!
//! The class-invariant tests at the end fire at generic fixture plates, not at the bound tank.

use std::time::Duration;

use avian3d::prelude::{CollisionLayers, LayerMask, PhysicsPlugins, RigidBody};
use bevy::time::TimeUpdateStrategy;
use serde::Deserialize;

use super::fuzz::{PROBE_TANK_AT, ProbeTank, probe_world, probe_world_posed};
use super::march_tests::{FIXTURE_88, fixture_round, fixture_round_with};
use super::*;
use crate::damage::VolumeOf;
use crate::damage_cam::record::{self, ShotFacts, StruckTank};
use crate::damage_cam::shapes::{ComponentShape, TankAsset, TankShapes};
use crate::damage_cam::{PenetrationReport, PlateClass, RoundEnd, Shooter, ShotVerdict};
use crate::spec::VolumeSpec;
use crate::tank::{Rig, ServoIndex, ServoState, TankServos, restore_servo_truth};
use bevy::ecs::system::RunSystemOnce;
use std::collections::BTreeMap;

/// The scenario list and its records, relative to the asset root.
const SCENARIO_DIR: &str = "dev/pen_cam";
const SCENARIO_LIST: &str = "scenarios.ron";

/// Fixed-step length the march is driven at (s).
pub(super) const STEP: Duration = Duration::from_millis(16);
/// The shooter every scenario names: fixture data, a player name no match produced. The shooter
/// fires one of the bound tank's own weapons, so its vehicle is the bound tank's.
const FIXTURE_SHOOTER: &str = "Fixture Gunner";

/// Steps a shot is marched for: 8 × 16 ms carries a main-gun round ~100 m (DERIVED), the whole
/// tank and the run-out past it.
pub(super) const STEPS: usize = 8;

/// One scripted shot. Positions are metres in the target's spawn frame: the probe tank's root at
/// the origin, the world's axes (`-Z` its front, `+Y` up). The target's pose turns the tank under
/// the shot; the shot itself does not move.
#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct Scenario {
    /// The record's file stem.
    name: String,
    /// What the scenario shows, carried into the record's header.
    shows: String,
    /// One of the bound tank's weapons, by its name in the tank file.
    weapon: String,
    /// The round fired, by its round registry key, whether or not the weapon lists it; the weapon's
    /// default round when absent.
    #[serde(default)]
    round: Option<String>,
    /// Striking speed (m/s); the round's muzzle speed when absent.
    #[serde(default)]
    speed: Option<f32>,
    from: Vec3,
    toward: Vec3,
    /// Where the shooter's gun stood, as a fixture: the recorder fires from `from` and has no
    /// shooter's rig to read a muzzle off.
    #[serde(default)]
    muzzle: Option<Vec3>,
    /// The hull's yaw about `+Y` (degrees).
    #[serde(default)]
    hull_yaw_deg: f32,
    /// The rig's turret servo's angle (degrees), posed as the sim poses it.
    #[serde(default)]
    turret_yaw_deg: f32,
    /// The rig's gun servo's angle (degrees), posed as the sim poses it.
    #[serde(default)]
    gun_elevation_deg: f32,
    /// The verdict the shot earns on the bound tank today: a physics change that flips it fails by
    /// name.
    verdict: ShotVerdict,
}

/// The probe world posed for `scenario`, with the march driven on [`STEP`] and spent shells kept.
fn scenario_world(scenario: &Scenario) -> App {
    scenario_world_under(scenario, None)
}

/// [`scenario_world`] throwing `law`'s cones in place of the shipped shape's.
fn scenario_world_under(scenario: &Scenario, law: Option<&spall_cone::ConeShape>) -> App {
    let pose = Transform::from_translation(PROBE_TANK_AT)
        .with_rotation(Quat::from_rotation_y(scenario.hull_yaw_deg.to_radians()));
    let mut app = probe_world_posed(pose).expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(MgShortCircuit(false))
        .add_systems(Update, integrate_projectiles);
    if let Some(law) = law {
        app.insert_resource(spall_cone::SpallLaw(law.clone()));
    }
    if scenario.turret_yaw_deg != 0.0 || scenario.gun_elevation_deg != 0.0 {
        let tank = app.world().resource::<ProbeTank>().0;
        let rig = app.world().get::<Rig>(tank).expect("the tank has a rig");
        for (node, deg) in [
            (rig.turret, scenario.turret_yaw_deg),
            (rig.gun, scenario.gun_elevation_deg),
        ] {
            let slot = app
                .world()
                .get::<ServoIndex>(node)
                .expect("the rig's turret and gun are servos")
                .0;
            let angle = deg.to_radians();
            app.world_mut()
                .get_mut::<TankServos>(tank)
                .expect("the tank integrates its servos")
                .states[slot] = ServoState::test_new(angle, angle, 0.0);
        }
        // The sim's own pose law writes the nodes, and the colliders follow before anything is
        // cast.
        app.world_mut()
            .run_system_once(restore_servo_truth)
            .expect("the servos restore");
        for _ in 0..4 {
            app.update();
        }
    }
    app
}

/// Fire `scenario` at the bound tank and read back its record.
fn record_scenario(scenario: &Scenario) -> PenetrationReport {
    record_timed(scenario, None).0
}

/// [`record_scenario`] under `law`'s cones (the shipped shape's when `None`), the wall time (s) its
/// march steps took, and the cones its body exits did not throw, below the HP quantum
/// ([`SpallMarks::gated`]).
fn record_timed(
    scenario: &Scenario,
    law: Option<&spall_cone::ConeShape>,
) -> (PenetrationReport, f64, usize) {
    record_edited(scenario, law, |_| {})
}

/// The first scenario's record under `law`'s cones, with `edit` applied to the posed world before
/// the shot: what an edited world makes of a recorded shot.
pub(super) fn record_first_edited(
    law: Option<&spall_cone::ConeShape>,
    edit: impl FnOnce(&mut App),
) -> PenetrationReport {
    let scenario = scenarios().into_iter().next().expect("a scenario");
    record_edited(&scenario, law, edit).0
}

/// [`record_timed`] with `edit` applied to the posed world before the shot.
fn record_edited(
    scenario: &Scenario,
    law: Option<&spall_cone::ConeShape>,
    edit: impl FnOnce(&mut App),
) -> (PenetrationReport, f64, usize) {
    let mut app = scenario_world_under(scenario, law);
    edit(&mut app);
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
    let key = scenario
        .round
        .as_deref()
        .unwrap_or_else(|| weapon.default_round_key());
    let round = crate::rounds::RoundRegistry::shipped()
        .get(key)
        .unwrap_or_else(|err| panic!("{}: {err}", scenario.name));
    let body = round.body;
    let speed = scenario.speed.unwrap_or(round.muzzle_speed);
    let origin = PROBE_TANK_AT + scenario.from;
    let direction = (scenario.toward - scenario.from).normalize();
    let struck = StruckTank::capture(app.world_mut(), tank).expect("the tank has a rig");
    let (mut projectile, readout) = fixture_round_with(body, round.penetrator, direction * speed);
    projectile.wound = round.wound;
    projectile.fuze = super::burst::Fuze::of(round.burst);
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
    let started = std::time::Instant::now();
    for _ in 0..STEPS {
        app.update();
    }
    let marched = started.elapsed().as_secs_f64();
    let facts = ShotFacts {
        shooter: Some(Shooter {
            name: FIXTURE_SHOOTER.to_owned(),
            vehicle: crate::tank::TIGER_SPEC_PATH.to_owned(),
        }),
        launch: origin,
        muzzle: scenario.muzzle.map(|muzzle| PROBE_TANK_AT + muzzle),
        projectile: scenario.weapon.clone(),
        caliber_mm: body.caliber * 1000.0,
        mass_kg: body.mass,
        round: key.to_owned(),
        vehicle: Some(crate::tank::TIGER_SPEC_PATH.to_owned()),
    };
    let gated = app
        .world()
        .get::<SpallMarks>(shell)
        .map_or(0, |marks| marks.gated);
    let report = record::report(app.world_mut(), &struck, shell, &facts)
        .unwrap_or_else(|| panic!("{}: the shot opens no pen cam", scenario.name));
    (report, marched, gated)
}

/// The distance along `direction` (unit) from `origin` to the nearest triangle of `shape`, if the
/// ray meets one (Möller–Trumbore).
fn ray_hit(shape: &ComponentShape, origin: Vec3, direction: Vec3) -> Option<f32> {
    shape
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|tri| {
            let [a, b, c] = [0, 1, 2].map(|k| shape.positions[tri[k] as usize]);
            let (e1, e2) = (b - a, c - a);
            let p = direction.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1.0e-12 {
                return None;
            }
            let s = origin - a;
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = direction.dot(q) / det;
            let t = e2.dot(q) / det;
            (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0).then_some(t)
        })
        .min_by(f32::total_cmp)
}

/// A shot across an elevated gun is recorded with the gun servo at the sim's own angle, and the
/// record's servo pose draws the struck component where the path crosses it: the line's recorded
/// entry lies on that component's mesh posed by the record, and not on its mesh at rest. The
/// component is whichever under the gun's servo the elevation moves farthest; the 12° elevation
/// and the 4 m stand-off are fixture data. The entry is the march's mean over the round's calibre
/// disc, so on a face curved across the disc it sits off the centre line's hit by up to the face's
/// sagitta over the disc, at most half a calibre (DERIVED): that is the tolerance on the entry, and
/// the mesh at rest must miss by more than two calibres.
#[test]
fn an_elevated_gun_shot_draws_the_barrel_where_the_path_crosses_it() {
    const ELEVATION_DEG: f32 = 12.0;
    let posed_scenario = |from: Vec3, toward: Vec3| Scenario {
        name: "elevated gun".to_owned(),
        shows: String::new(),
        weapon: "MainGun".to_owned(),
        round: None,
        speed: None,
        from,
        toward,
        muzzle: None,
        hull_yaw_deg: 0.0,
        turret_yaw_deg: 0.0,
        gun_elevation_deg: ELEVATION_DEG,
        verdict: ShotVerdict::Damaged,
    };
    let app = scenario_world(&posed_scenario(Vec3::X, Vec3::ZERO));
    let tank = app.world().resource::<ProbeTank>().0;
    let rig = app.world().get::<Rig>(tank).expect("the tank has a rig");
    let gun = app
        .world()
        .get::<Name>(rig.gun)
        .expect("the gun servo is named")
        .as_str()
        .to_owned();
    let hull = *app
        .world()
        .get::<GlobalTransform>(rig.hull)
        .expect("a posed hull");
    let asset = TankAsset::load(crate::tank::TIGER_SPEC_PATH).expect("the bound tank loads");
    let at_rest = asset.shapes(&BTreeMap::new());
    let elevated = asset.shapes(&BTreeMap::from([(gun.clone(), ELEVATION_DEG)]));
    let spec = app
        .world()
        .resource::<crate::bake::TankBlueprint>()
        .spec
        .clone();
    let (component, _) = spec
        .volumes
        .iter()
        .filter(|(_, volume)| volume.component().is_some())
        .map(|(name, _)| {
            let moved = at_rest
                .get(name)
                .expect("a component resolves")
                .center()
                .distance(elevated.get(name).expect("a component resolves").center());
            (name.clone(), moved)
        })
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .expect("the tank has components");
    let target = elevated.get(&component).unwrap().center();
    let across = Vec3::X * 4.0;
    let world = |hull_local: Vec3| hull.transform_point(hull_local) - PROBE_TANK_AT;
    let report = record_scenario(&posed_scenario(world(target + across), world(target)));

    let recorded = report.servo_deg.get(&gun).copied();
    assert!(
        recorded.is_some_and(|deg| (deg - ELEVATION_DEG).abs() < 1.0e-4),
        "the record holds the gun servo's own angle: {recorded:?}"
    );
    let entered = report
        .doses
        .iter()
        .find(|dose| dose.volume == component)
        .and_then(|dose| dose.entered)
        .map(Vec3::from)
        .unwrap_or_else(|| panic!("the line enters {component}: {:?}", report.doses));
    let origin = Vec3::from(report.origin);
    let direction = (entered - origin).normalize();
    let drawn = |shapes: &TankShapes| {
        ray_hit(shapes.get(&component).unwrap(), origin, direction).map(|t| origin + direction * t)
    };
    let calibre = report.projectile.caliber_mm * 1.0e-3;
    let posed_hit = drawn(&asset.shapes(&report.servo_deg));
    assert!(
        posed_hit.is_some_and(|hit| hit.distance(entered) <= 0.5 * calibre),
        "{component} is drawn where the path enters it, {entered}: met at {posed_hit:?}"
    );
    let rest_hit = drawn(&at_rest);
    assert!(
        rest_hit.is_none_or(|hit| hit.distance(entered) > 2.0 * calibre),
        "{component} at rest is met where the elevated one is: {rest_hit:?}"
    );
}

/// A record as its file holds it.
fn render(scenario: &Scenario, report: &PenetrationReport) -> String {
    let body = ron::ser::to_string_pretty(report, ron::ser::PrettyConfig::default())
        .expect("a record serializes");
    format!(
        "// {}: {}\n// Recorded by the real march from `{SCENARIO_LIST}` (`ballistics::pen_cam`); do \
         not edit.\n// Re-record with `PEN_CAM_BLESS=1 cargo test --lib pen_cam::bless -- --ignored`.\n\
         {body}\n",
        scenario.name, scenario.shows
    )
}

fn scenarios() -> Vec<Scenario> {
    let path = crate::assets::asset_root()
        .join(SCENARIO_DIR)
        .join(SCENARIO_LIST);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    ron::de::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Re-records every scenario file from today's march (ignored; a tool, run on demand):
/// `PEN_CAM_BLESS=1 cargo test --lib pen_cam::bless -- --ignored` writes them, and the diff is the
/// review; without `PEN_CAM_BLESS` it records and checks but writes nothing. A record is written
/// only when its scenario earns its stated verdict, its own path reads that verdict, it is
/// self-consistent, and its round is a registry key.
#[test]
#[ignore = "a tool: PEN_CAM_BLESS=1 re-records the scenario files"]
fn bless_scenarios() {
    let dir = crate::assets::asset_root().join(SCENARIO_DIR);
    let bless = std::env::var_os("PEN_CAM_BLESS").is_some();
    for scenario in &scenarios() {
        let report = record_scenario(scenario);
        assert_eq!(
            report.impact.verdict, scenario.verdict,
            "PHYSICS CHANGED — {} earns a different verdict; diff consciously before re-recording",
            scenario.name
        );
        assert_eq!(
            ShotVerdict::of_path(&report.steps()),
            Some(report.impact.verdict),
            "{}: the record's own path reads a different verdict",
            scenario.name
        );
        crate::damage_cam::tests::assert_self_consistent(&scenario.name, &report);
        assert!(
            crate::rounds::RoundRegistry::shipped()
                .get(&report.round)
                .is_ok(),
            "{}: the record's round {} is not a registry key",
            scenario.name,
            report.round
        );
        if bless {
            let path = dir.join(format!("{}.ron", scenario.name));
            std::fs::write(&path, render(scenario, &report))
                .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        }
    }
}

/// The server cost of one cone throw under the shipped law at each of [`COST_DIRECTIONS`]
/// (ignored; run it on demand), and the cones the HP quantum kept from being thrown. Each
/// penetrating scenario's march steps are timed under the shipped shape at `N` directions and at
/// one; the throw's cost is the difference over the cones thrown, scaled from `N − 1` to `N`
/// directions. The best of [`COST_RUNS`] runs is taken.
/// `cargo test --release --lib --features dev_tools pen_cam::spall_law_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, run on demand"]
fn spall_law_cost() {
    let best = |scenario: &Scenario, law: &spall_cone::ConeShape| {
        (0..COST_RUNS)
            .map(|_| {
                let (report, marched, _) = record_timed(scenario, Some(law));
                (marched, report.spall.len())
            })
            .fold((f64::INFINITY, 0), |(t, _), (s, cones)| (t.min(s), cones))
    };
    let one = spall_cone::ConeShape::shipped_at(1);
    let penetrating: Vec<Scenario> = scenarios()
        .into_iter()
        .filter(|s| s.verdict == ShotVerdict::Penetration)
        .collect();
    for directions in COST_DIRECTIONS {
        let law = spall_cone::ConeShape::shipped_at(directions);
        let (mut extra, mut cones) = (0.0, 0);
        for scenario in &penetrating {
            let (full, thrown) = best(scenario, &law);
            let (single, _) = best(scenario, &one);
            extra += full - single;
            cones += thrown;
        }
        let n = directions as f64;
        println!(
            "N = {directions}: {cones} cones, {:.3} ms per throw",
            1000.0 * extra / cones.max(1) as f64 * n / (n - 1.0).max(1.0)
        );
    }
    let (mut thrown, mut gated) = (0, 0);
    for scenario in &scenarios() {
        let (report, _, kept) = record_timed(scenario, None);
        thrown += report.spall.len();
        gated += kept;
    }
    println!(
        "across {} scenarios: {thrown} cones thrown, {gated} below the HP quantum not thrown",
        scenarios().len()
    );
}

/// The direction counts the throw's cost is measured at.
const COST_DIRECTIONS: [usize; 4] = [24, 64, 128, 256];

/// The server cost of one burst field's throw at each of [`BURST_COST_DIRECTIONS`] (ignored; run
/// it on demand). The first scenario's line is fired with every burst round of the registry, its
/// march timed with both burst laws at `N` directions and at one; the throw's cost is the
/// difference, scaled from `N − 1` to `N` directions, the best of [`COST_RUNS`] runs.
/// `cargo test --release --lib --features dev_tools pen_cam::burst_law_cost -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, run on demand"]
fn burst_law_cost() {
    let line = scenarios().into_iter().next().expect("a scenario");
    let registry = crate::rounds::RoundRegistry::shipped();
    let bursting: Vec<&str> = registry
        .keys()
        .filter(|key| registry.get(key).is_ok_and(|round| round.burst.is_some()))
        .collect();
    assert!(!bursting.is_empty(), "the registry holds a burst round");
    for key in bursting {
        let scenario = Scenario {
            round: Some(key.to_owned()),
            ..line.clone()
        };
        let best = |directions: usize| {
            (0..COST_RUNS)
                .map(|_| {
                    let fields = spall_cone::BurstFields::shipped_at(directions, directions);
                    record_edited(&scenario, None, |app| {
                        app.insert_resource(spall_cone::BurstLaw(fields));
                    })
                    .1
                })
                .fold(f64::INFINITY, f64::min)
        };
        let one = best(1);
        for directions in BURST_COST_DIRECTIONS {
            let n = directions as f64;
            println!(
                "{key}: N = {directions}: {:.3} ms per throw",
                1000.0 * (best(directions) - one) * n / (n - 1.0)
            );
        }
    }
}

/// The direction counts the burst's throw cost is measured at.
const BURST_COST_DIRECTIONS: [usize; 3] = [64, 256, 684];

/// The runs a cost is the best of.
const COST_RUNS: usize = 9;

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
pub(super) enum Kind {
    Main,
    External,
    /// A box with this HP pool, of a soft hard substance: a layer of the walk.
    Box(u32),
    /// A box with this HP pool, of steel: an exposed component a round can glance off.
    SteelBox(u32),
    /// A witness (ADR-0040) with this HP pool: no layer of the walk.
    Witness(u32),
}

/// Reference-mm per metre: steel, and the box's soft substance. Fixture data.
const STEEL: f32 = 1000.0;
const SOFT: f32 = 10.0;

/// One fixture volume: name, full extents and centre (m), kind.
pub(super) type FixtureVolume = (&'static str, Vec3, Vec3, Kind);

/// Static fixture tanks, each volume its own static cuboid turned by its rotation, on the `Armor`
/// layer (a witness on the `Witness` layer) and owned by the tank its index names, and the real
/// march driven on [`STEP`] with spent shells kept.
pub(super) fn fixture_tanks(volumes: &[(FixtureVolume, usize, Quat)]) -> (App, Vec<Entity>) {
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
    let count = volumes
        .iter()
        .map(|(_, owner, _)| owner + 1)
        .max()
        .unwrap_or(1);
    let tanks: Vec<Entity> = (0..count)
        .map(|_| {
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
            tank
        })
        .collect();
    for &((name, size, at, kind), owner, turn) in volumes {
        if let Kind::Witness(hp) = kind {
            app.world_mut().spawn((
                Name::new(name),
                Transform::from_translation(at).with_rotation(turn),
                RigidBody::Static,
                Collider::cuboid(size.x, size.y, size.z),
                CollisionLayers::new([crate::Layer::Witness], LayerMask::ALL),
                WitnessVolume {
                    substance: "fixture".to_owned(),
                },
                WitnessSurface,
                VolumeOf(tanks[owner]),
                ComponentHealth {
                    current: hp,
                    max: hp,
                },
            ));
            continue;
        }
        let factor = if matches!(kind, Kind::Box(_)) {
            SOFT
        } else {
            STEEL
        };
        let mut volume = app.world_mut().spawn((
            Name::new(name),
            Transform::from_translation(at).with_rotation(turn),
            RigidBody::Static,
            Collider::cuboid(size.x, size.y, size.z),
            CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
            BallisticVolume {
                material: walk::VolumeMaterial::fixture(factor),
                substance: "fixture".to_owned(),
            },
            VolumeOf(tanks[owner]),
        ));
        match kind {
            Kind::Main | Kind::Witness(_) => {}
            Kind::External => {
                volume.insert(ExternalArmour);
            }
            Kind::Box(hp) | Kind::SteelBox(hp) => {
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
    (app, tanks)
}

/// Fire the fixture 88 from `from` along `-Z` at 800 m/s (fixture data) and read back its record.
fn fixture_shot(volumes: &[FixtureVolume], from: Vec3) -> PenetrationReport {
    fixture_shot_along(volumes, from, Vec3::NEG_Z)
}

/// Fire the fixture 88 from `from` along `direction` at 800 m/s (fixture data), read back its
/// record, and check the record's own path and the march's logged path read its verdict.
fn fixture_shot_along(volumes: &[FixtureVolume], from: Vec3, direction: Vec3) -> PenetrationReport {
    let owned: Vec<(FixtureVolume, usize, Quat)> = volumes
        .iter()
        .map(|&volume| (volume, 0, Quat::IDENTITY))
        .collect();
    fixture_shot_at(&owned, from, direction, 0)
}

/// [`fixture_shot_along`] among several fixture tanks, recorded against tank `struck`.
fn fixture_shot_at(
    volumes: &[(FixtureVolume, usize, Quat)],
    from: Vec3,
    direction: Vec3,
    struck: usize,
) -> PenetrationReport {
    fixture_fire(volumes, from, direction, struck, |_| {}).report
}

/// One fixture shot as [`fixture_fire`] read it back.
pub(super) struct FixtureFired {
    pub report: PenetrationReport,
    /// The march's armour log, in march order.
    pub log: Vec<ArmourMark>,
    /// The shell where the march left it: position, and velocity while it still flies.
    pub at: Vec3,
    pub velocity: Option<Vec3>,
    /// While it still flies, the shell's round: its body, its whole mass, its fuze.
    pub flying: Option<(RoundBody, f32, burst::Fuze)>,
    /// The burst field the shell threw, if it burst.
    pub burst: Option<BurstField>,
    /// Every fixture volume's name by entity.
    pub names: BTreeMap<Entity, String>,
}

/// [`fixture_shot_at`] with the fixture 88's flight state edited by `round` before it is fired,
/// read back with the march's log and the shell's final state.
pub(super) fn fixture_fire(
    volumes: &[(FixtureVolume, usize, Quat)],
    from: Vec3,
    direction: Vec3,
    struck: usize,
    round: impl FnOnce(&mut Projectile),
) -> FixtureFired {
    fixture_fire_with(
        volumes,
        from,
        direction * 800.0,
        struck,
        FIXTURE_88,
        round,
        |_| {},
    )
}

/// [`fixture_fire`] of a round of `body` fired at `velocity`, its flight state edited by `round`,
/// into the fixture world as `setup` leaves it. The round leaves the muzzle at `from`.
pub(super) fn fixture_fire_with(
    volumes: &[(FixtureVolume, usize, Quat)],
    from: Vec3,
    velocity: Vec3,
    struck: usize,
    body: RoundBody,
    round: impl FnOnce(&mut Projectile),
    setup: impl FnOnce(&mut App),
) -> FixtureFired {
    let (mut app, tanks) = fixture_tanks(volumes);
    setup(&mut app);
    let direction = velocity.normalize();
    let tank = tanks[struck];
    let struck = StruckTank::capture(app.world_mut(), tank).expect("the fixture has a rig");
    let (mut projectile, readout) = fixture_round(body, velocity);
    round(&mut projectile);
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
            Transform::from_translation(from).looking_to(direction, Vec3::Y),
        ))
        .id();
    for _ in 0..STEPS {
        app.update();
    }
    let logged = ShotVerdict::of_path(&record::path_steps(app.world(), shell, Some(tank)));
    let facts = ShotFacts {
        shooter: None,
        launch: from,
        muzzle: Some(from),
        projectile: "fixture".to_owned(),
        caliber_mm: body.caliber * 1000.0,
        mass_kg: body.mass,
        round: "fixture".to_owned(),
        vehicle: None,
    };
    let report = record::report(app.world_mut(), &struck, shell, &facts).expect("a pen cam opens");
    assert_eq!(
        ShotVerdict::of_path(&report.steps()),
        Some(report.impact.verdict),
        "the record's own path reads its verdict"
    );
    assert_eq!(
        logged,
        Some(report.impact.verdict),
        "the logged path reads its verdict"
    );
    let world = app.world_mut();
    let log = world
        .get::<PenetrationMarks>(shell)
        .expect("a retained shell keeps its marks")
        .log
        .clone();
    let at = world
        .get::<Transform>(shell)
        .expect("a retained shell")
        .translation;
    let velocity = world
        .get::<Projectile>(shell)
        .map(|projectile| projectile.velocity);
    let flying = world
        .get::<Projectile>(shell)
        .map(|projectile| (projectile.body(), projectile.mass, projectile.fuze));
    let names = world
        .query_filtered::<(Entity, &Name), With<VolumeOf>>()
        .iter(world)
        .map(|(entity, name)| (entity, name.as_str().to_owned()))
        .collect();
    let burst = world
        .get_mut::<SpallMarks>(shell)
        .and_then(|mut marks| marks.burst.take());
    FixtureFired {
        report,
        log,
        at,
        velocity,
        flying,
        burst,
        names,
    }
}

/// The head-on line every fixture fires down.
pub(super) const LINE_FROM: Vec3 = Vec3::new(0.0, 2.0, 2.0);
/// Plate faces 3 m square, centred on the line's height. A 50 mm steel plate is perforated by the
/// fixture 88 and a 500 mm one stops it (fixture data, as the march's own plate tests use them).
const FACE: f32 = 3.0;
pub(super) const THIN: f32 = 0.05;
pub(super) const THICK: f32 = 0.5;

pub(super) fn plate(name: &'static str, thickness: f32, z: f32, kind: Kind) -> FixtureVolume {
    (
        name,
        Vec3::new(FACE, FACE, thickness),
        Vec3::new(0.0, 2.0, z),
        kind,
    )
}

/// A round that perforates external armour and is stopped by the main armour behind it did not
/// penetrate; nor did one through external armour alone that flies on, dosing nothing.
#[test]
fn perforating_a_skirt_and_stopping_in_the_hull_is_a_non_pen() {
    let report = fixture_shot(&[plate("skirt", 0.01, 0.0, Kind::External)], LINE_FROM);
    assert_eq!(report.impact.verdict, ShotVerdict::NonPen);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    let last = report.path.last().expect("a path");
    assert!(last.point[2] < -0.01, "the path flies on past the skirt");

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
    assert_eq!(
        report.end,
        RoundEnd::Rest {
            volume: "hull".to_owned()
        },
        "the round's end names the plate it rests in"
    );
}

/// A round stopped inside a component rests in it, by name, however close to its face it stopped.
#[test]
fn a_round_stopped_inside_a_component_rests_in_it() {
    let report = fixture_shot(
        &[plate("component", THICK, 0.0, Kind::SteelBox(10_000))],
        LINE_FROM,
    );
    assert_eq!(
        report.end,
        RoundEnd::Rest {
            volume: "component".to_owned()
        }
    );
    assert!(report.plates.is_empty(), "a component is not a plate");
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
    // The plate's exit is its disc-mean chord's end, the round's the crossing's exit on the line:
    // the same face, 1 mm apart at most (fixture tolerance).
    let exit = Vec3::from(report.plates[0].exit.expect("perforated"));
    let RoundEnd::Left { at } = report.end else {
        panic!("it leaves the tank: {:?}", report.end);
    };
    assert!(
        Vec3::from(at).distance(exit) < 1.0e-3,
        "it leaves where it came out of the plate: {at:?} vs {exit}"
    );
    assert!(
        report.path.iter().any(|p| p.point == at),
        "where it left is a point of its path"
    );
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
                Kind::Box(30),
            ),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    assert_eq!(report.components.len(), 1, "the box is recorded");
    assert_eq!(report.components[0].count, 30);
    assert_eq!(report.components[0].hp, 30, "whole at the shot");
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
                Kind::Box(10_000),
            ),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    let dose = report.doses.first().expect("the box is dosed");
    assert_eq!(dose.volume, "box");
    assert!(dose.line > 0.0 && dose.field > 0.0, "{dose:?}");
    // The box's near face: 0.25 m in front of its centre at z = -1, fixture data.
    let entered = dose.entered.expect("the line entered the box");
    assert!((entered[2] + 0.75).abs() < 1.0e-3, "{entered:?}");
    assert!(!report.spall.is_empty(), "the plate's exit throws a cone");
}

/// A record is stated in the struck hull's frame: one shot through a plate into a box behind it,
/// fired at a fixture tank at two hull poses, moved and turned with the tank, records the same
/// verdict, plates, components, path and muzzle. The hull is its own entity, so the frame comes
/// from the hull's pose and not the tank root's. The second pose, turned 30° and moved 3 m aside,
/// 0.5 m up and 2 m back, is fixture data; 1 mm is the tolerance on a position.
#[test]
fn a_record_is_stated_in_the_struck_hull_frame() {
    let shot = |hull: Transform| {
        let volumes = [
            plate("front", THIN, 0.0, Kind::Main),
            (
                "box",
                Vec3::splat(0.5),
                Vec3::new(0.0, 2.0, -1.0),
                Kind::Box(10_000),
            ),
        ]
        .map(|(name, size, at, kind)| {
            (
                (name, size, hull.transform_point(at), kind),
                0,
                hull.rotation,
            )
        });
        fixture_fire_with(
            &volumes,
            hull.transform_point(LINE_FROM),
            hull.rotation * Vec3::NEG_Z * 800.0,
            0,
            FIXTURE_88,
            |_| {},
            |app| {
                let world = app.world_mut();
                let tank = world
                    .query_filtered::<Entity, With<Rig>>()
                    .single(world)
                    .expect("one fixture tank");
                let posed = world.spawn((hull, GlobalTransform::from(hull))).id();
                world.get_mut::<Rig>(tank).expect("the fixture rig").hull = posed;
            },
        )
        .report
    };
    let moved = Transform::from_rotation(Quat::from_rotation_y(30.0_f32.to_radians()))
        .with_translation(Vec3::new(3.0, 0.5, -2.0));
    let (still, turned) = (shot(Transform::IDENTITY), shot(moved));
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
    assert_eq!(still.components, turned.components);
    assert!(near(still.origin, turned.origin));
    let (Some(still_muzzle), Some(turned_muzzle)) = (still.muzzle, turned.muzzle) else {
        panic!("both records carry the muzzle");
    };
    assert!(
        near(still_muzzle, turned_muzzle),
        "{still_muzzle:?} vs {turned_muzzle:?}"
    );
    assert_eq!(still.path.len(), turned.path.len());
    for (a, b) in still.path.iter().zip(&turned.path) {
        assert!(near(a.point, b.point), "{:?} vs {:?}", a.point, b.point);
    }
}

/// A component on the line with no armour in front of it is damaged by the line through it, and
/// the round flies on.
#[test]
fn an_exposed_component_dosed_with_no_armour_crossed_is_damaged() {
    let report = fixture_shot(
        &[(
            "box",
            Vec3::splat(0.5),
            Vec3::new(0.0, 2.0, 0.0),
            Kind::Box(10_000),
        )],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Damaged);
    assert!(report.plates.is_empty(), "{:?}", report.plates);
    let dose = report.doses.first().expect("the box is dosed");
    assert!(dose.line > 0.0, "{dose:?}");
}

/// A component clipped on the way into armour that stops the round is damaged, not a non-pen.
#[test]
fn a_component_clipped_then_stopped_in_armour_is_damaged() {
    let report = fixture_shot(
        &[
            (
                "barrel",
                Vec3::splat(0.2),
                Vec3::new(0.0, 2.0, 0.5),
                Kind::Box(10_000),
            ),
            plate("mantlet", THICK, 0.0, Kind::Main),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Damaged);
    let exits: Vec<bool> = report.plates.iter().map(|p| p.exit.is_some()).collect();
    assert_eq!(exits, [false], "the armour stops the round");
    assert_eq!(report.doses.len(), 1, "{:?}", report.doses);
    assert_eq!(report.doses[0].volume, "barrel");
}

/// A round glancing off a component jars its shock into it: damage, not a ricochet. The 75° graze
/// is fixture data.
#[test]
fn a_bounce_that_shocks_a_component_is_damaged_not_a_ricochet() {
    let graze = Vec3::new(
        75.0_f32.to_radians().sin(),
        0.0,
        -75.0_f32.to_radians().cos(),
    );
    let report = fixture_shot_along(
        &[plate("component", 0.10, 0.0, Kind::SteelBox(10_000))],
        Vec3::new(-1.0, 2.0, 0.6),
        graze,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Damaged);
    assert_eq!(report.ricochets.len(), 1, "the round bounces away");
    let dose = report
        .doses
        .first()
        .expect("the bounce shocks the component");
    assert!(dose.line > 0.0 && dose.field == 0.0, "{dose:?}");
}

/// The same glance off armour doses nothing and is a ricochet.
#[test]
fn a_bounce_off_armour_that_doses_nothing_is_a_ricochet() {
    let graze = Vec3::new(
        75.0_f32.to_radians().sin(),
        0.0,
        -75.0_f32.to_radians().cos(),
    );
    let report = fixture_shot_along(
        &[plate("plate", 0.10, 0.0, Kind::Main)],
        Vec3::new(-1.0, 2.0, 0.6),
        graze,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Ricochet);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    assert_eq!(
        report.end,
        RoundEnd::Left {
            at: report.ricochets[0].point
        },
        "it leaves off the face it bounced from"
    );
}

/// A round's outcome is recorded against the selected tank alone: through tank A's plate and
/// stopped in tank B's behind it, A's record has the round leave A where it came out of A's plate,
/// lists A's plate only, and starts its clock at A; B's record has it rest in B's plate, its first
/// contact and its clock at B. The plates' places are fixture data.
#[test]
fn a_record_holds_the_selected_tanks_outcome_alone() {
    let volumes = [
        (plate("front", THIN, 0.0, Kind::Main), 0, Quat::IDENTITY),
        (plate("far", THICK, -3.0, Kind::Main), 1, Quat::IDENTITY),
    ];
    let a = fixture_shot_at(&volumes, LINE_FROM, Vec3::NEG_Z, 0);
    assert_eq!(a.impact.verdict, ShotVerdict::Penetration);
    let names: Vec<&str> = a.plates.iter().map(|plate| plate.name.as_str()).collect();
    assert_eq!(names, ["front"]);
    let exit = Vec3::from(a.plates[0].exit.expect("perforated"));
    let RoundEnd::Left { at } = a.end else {
        panic!("A's record has the round leave A: {:?}", a.end);
    };
    assert!(Vec3::from(at).distance(exit) < 1.0e-3, "{at:?} vs {exit}");
    let b = fixture_shot_at(&volumes, LINE_FROM, Vec3::NEG_Z, 1);
    assert_eq!(b.impact.verdict, ShotVerdict::NonPen);
    assert_eq!(
        b.end,
        RoundEnd::Rest {
            volume: "far".to_owned()
        }
    );
    // B's near face, at z = -3 + 0.25, fixture data.
    assert!(
        (b.impact.point[2] + 2.75).abs() < 1.0e-3,
        "{:?}",
        b.impact.point
    );
    let contact = b
        .path
        .iter()
        .find(|node| (node.point[2] + 2.75).abs() < 1.0e-3)
        .expect("B's contact is a node of B's path");
    assert_eq!(contact.t, 0.0, "B's clock starts at B");
}

/// Where two tanks' plates overlap inside one crossing, both tanks' records carry the crossing's
/// one entrance face: A's plate square to the line, B's turned 30° about `+Y` and entered inside
/// A's, and B's record reads A's normal and incidence with its own impact point. The plates' places
/// and the turn are fixture data; B's front face crosses the line at
/// z = -0.3 + 0.25 / cos 30° = -0.011, inside A's plate (DERIVED). 0.5° is the tolerance on a
/// direction.
#[test]
fn a_shared_crossing_carries_its_entrance_face_to_every_tank() {
    let turn = Quat::from_rotation_y(30.0_f32.to_radians());
    let volumes = [
        (plate("front", THIN, 0.0, Kind::Main), 0, Quat::IDENTITY),
        (plate("turned", THICK, -0.3, Kind::Main), 1, turn),
    ];
    let degrees = |a: [f32; 3], b: Vec3| Vec3::from(a).angle_between(b).to_degrees();
    let a = fixture_shot_at(&volumes, LINE_FROM, Vec3::NEG_Z, 0);
    let b = fixture_shot_at(&volumes, LINE_FROM, Vec3::NEG_Z, 1);
    assert!(
        b.impact.point[2] < THIN / 2.0 - 1.0e-3,
        "B's impact is inside A's plate: {:?}",
        b.impact.point
    );
    for (tank, report) in [("A", &a), ("B", &b)] {
        assert!(
            degrees(report.impact.normal, Vec3::Z) < 0.5,
            "{tank} carries the crossing's normal: {:?}",
            report.impact.normal
        );
        assert!(
            report.impact.incidence_deg < 0.5,
            "{tank} carries the crossing's incidence: {}",
            report.impact.incidence_deg
        );
    }
}

/// A round that glances off one plate and then stops in another rests in the second: the recorder's
/// end is the last mark's, not the bounce's. The 75° graze and the far plate's place are fixture
/// data.
#[test]
fn a_bounce_then_a_stop_rests_where_it_stopped() {
    let graze = Vec3::new(
        75.0_f32.to_radians().sin(),
        0.0,
        -75.0_f32.to_radians().cos(),
    );
    let report = fixture_shot_along(
        &[
            plate("plate", 0.10, 0.0, Kind::Main),
            (
                "wall",
                Vec3::new(THICK, FACE, FACE),
                Vec3::new(2.5, 2.0, 0.5),
                Kind::Main,
            ),
        ],
        Vec3::new(-1.0, 2.0, 0.6),
        graze,
    );
    assert_eq!(report.ricochets.len(), 1, "it glances off the plate");
    assert_eq!(report.ricochets[0].volume, "plate");
    assert_eq!(
        report.end,
        RoundEnd::Rest {
            volume: "wall".to_owned()
        }
    );
    assert_eq!(report.impact.verdict, ShotVerdict::NonPen);
}

/// The record's muzzle is read off the shooter's rig: the bound tank's main gun, turned and
/// elevated, fires from where its muzzle node stands. Fixture angles; 1 mm is the tolerance.
#[test]
fn the_muzzle_is_read_off_the_shooters_rig() {
    let mut app = scenario_world(&Scenario {
        name: "muzzle".to_owned(),
        shows: String::new(),
        weapon: "MainGun".to_owned(),
        round: None,
        speed: None,
        from: Vec3::ZERO,
        toward: Vec3::NEG_Z,
        muzzle: None,
        hull_yaw_deg: 20.0,
        turret_yaw_deg: 35.0,
        gun_elevation_deg: 8.0,
        verdict: ShotVerdict::Penetration,
    });
    let tank = app.world().resource::<ProbeTank>().0;
    let muzzle = app.world().get::<Rig>(tank).expect("a rig").muzzle;
    let node = app
        .world()
        .get::<GlobalTransform>(muzzle)
        .expect("the muzzle is placed")
        .translation();
    let read = record::muzzle_of(app.world_mut(), tank, "MainGun").expect("the main gun");
    assert!(
        read.distance(node) < 1.0e-3,
        "read {read}, the node at {node}"
    );
    assert!(
        read.distance(PROBE_TANK_AT) > 1.0,
        "the muzzle is not the tank's root"
    );
    assert!(record::muzzle_of(app.world_mut(), tank, "NoSuchGun").is_none());
}

/// The muzzle is the tick's truth before the globals catch up: with the turret turned and its node
/// posed by the sim's law, but no propagation run, the read is where the muzzle stands once the
/// globals are propagated, not where the stale globals put it. The angles are fixture data; the
/// stale globals must sit more than 10 cm off, so the check can tell them apart; 1 mm is the
/// tolerance.
#[test]
fn the_muzzle_is_read_before_the_globals_catch_up() {
    let mut app = scenario_world(&Scenario {
        name: "muzzle".to_owned(),
        shows: String::new(),
        weapon: "MainGun".to_owned(),
        round: None,
        speed: None,
        from: Vec3::ZERO,
        toward: Vec3::NEG_Z,
        muzzle: None,
        hull_yaw_deg: 20.0,
        turret_yaw_deg: 35.0,
        gun_elevation_deg: 8.0,
        verdict: ShotVerdict::Penetration,
    });
    let tank = app.world().resource::<ProbeTank>().0;
    let rig = app.world().get::<Rig>(tank).expect("a rig");
    let (turret, muzzle) = (rig.turret, rig.muzzle);
    let slot = app
        .world()
        .get::<ServoIndex>(turret)
        .expect("the turret is a servo")
        .0;
    let angle = 80.0_f32.to_radians();
    app.world_mut()
        .get_mut::<TankServos>(tank)
        .expect("the tank integrates its servos")
        .states[slot] = ServoState::test_new(angle, angle, 0.0);
    app.world_mut()
        .run_system_once(restore_servo_truth)
        .expect("the servos restore");
    let global = |app: &App| {
        app.world()
            .get::<GlobalTransform>(muzzle)
            .expect("the muzzle is placed")
            .translation()
    };
    let stale = global(&app);
    let read = record::muzzle_of(app.world_mut(), tank, "MainGun").expect("the main gun");
    assert!(
        read.distance(stale) > 0.1,
        "the turn moves the muzzle off the stale globals"
    );
    app.update();
    let caught_up = global(&app);
    assert!(
        read.distance(caught_up) < 1.0e-3,
        "read {read}, the propagated node at {caught_up}"
    );
}

// ---------------------------------------------------------------------------------------------
// The burst on generic fixtures (design doc §5a, "The burst")
// ---------------------------------------------------------------------------------------------

/// The fixture 88 carrying a burst of `shape` ([`burst::fixture_inputs`]).
fn fixture_burst(shape: burst::BurstShape) -> burst::Burst {
    burst::Burst::derived(
        FIXTURE_88,
        burst::FIXTURE_STEEL,
        &burst::fixture_inputs(shape),
    )
    .expect("the fixture burst derives")
}

/// [`fixture_fire`] down the head-on line through tank 0's `volumes`, the round fused by `fuze`.
fn fire_fused(volumes: &[FixtureVolume], fuze: burst::Fuze) -> FixtureFired {
    let owned: Vec<(FixtureVolume, usize, Quat)> = volumes
        .iter()
        .map(|&volume| (volume, 0, Quat::IDENTITY))
        .collect();
    fixture_fire(&owned, LINE_FROM, Vec3::NEG_Z, 0, |projectile| {
        projectile.fuze = fuze;
    })
}

/// Every crossing's end in the log, in march order.
fn crossing_ends(log: &[ArmourMark]) -> Vec<CrossingEnd> {
    log.iter()
        .filter_map(|mark| match mark {
            ArmourMark::Crossing { end, .. } => Some(*end),
            _ => None,
        })
        .collect()
}

/// Every burst in the log: where, at what speed, and the nose piece's mass.
fn bursts(log: &[ArmourMark]) -> Vec<(Vec3, f32, Option<f32>)> {
    log.iter()
        .filter_map(|mark| match mark {
            ArmourMark::Burst {
                at, speed, nose, ..
            } => Some((*at, *speed, *nose)),
            _ => None,
        })
        .collect()
}

/// The exit point and speed of a crossing the round left.
fn exit_of(end: CrossingEnd) -> (Vec3, f32) {
    match end {
        CrossingEnd::Exit { at, speed } => (at, speed),
        CrossingEnd::Rest { at } => panic!("the round rests at {at}"),
    }
}

/// Metres a burst may lie off `speed × delay` past the exit: the f32 path the fuze sums over the
/// march's legs, and gravity's bend of the ~10 m it flies (DERIVED < 1 mm). Fixture tolerance.
const BURST_TOLERANCE: f32 = 1.0e-3;

/// An armed round bursts `exit speed × delay` of path past the exit of the body that armed it: the
/// field's origin, and for the ring the nose piece's start. The ball ends the round there; the ring's
/// nose flies on from there at the burst's speed as an inert round of the derived nose mass and the
/// round's calibre and nose, nothing else of the round. A mutant that counts the delay from the
/// entrance, flies the nose at the whole round's mass, or lets the ball fly on fails here. The 50 mm
/// plate spends 50 reference-mm across, over the fixture threshold (fixture data).
#[test]
fn an_armed_round_bursts_its_delay_past_the_arming_exit() {
    for shape in [burst::BurstShape::Ball, burst::BurstShape::Ring] {
        let burst = fixture_burst(shape);
        let fired = fire_fused(
            &[plate("front", THIN, 0.0, Kind::Main)],
            burst::Fuze::of(Some(burst)),
        );
        let ends = crossing_ends(&fired.log);
        assert_eq!(ends.len(), 1, "{shape:?}: one crossing");
        let (exit, exit_speed) = exit_of(ends[0]);
        let found = bursts(&fired.log);
        assert_eq!(found.len(), 1, "{shape:?}: one burst");
        let (at, speed, nose) = found[0];
        let past = at.distance(exit);
        assert!(
            (past - exit_speed * burst.delay).abs() < BURST_TOLERANCE,
            "{shape:?}: the burst lies {past} m past the exit, not {} m",
            exit_speed * burst.delay
        );
        assert!(
            speed > 0.0 && speed <= exit_speed,
            "{shape:?}: it bursts in flight, at {speed} m/s"
        );
        match burst.option {
            burst::BurstOption::Ball => {
                assert_eq!(nose, None, "the ball leaves no nose piece");
                assert!(
                    matches!(fired.log.last(), Some(ArmourMark::Burst { .. })),
                    "the ball ends the round at the burst"
                );
                assert_eq!(fired.flying, None, "the ball ends the round");
                assert!(fired.at.distance(at) < BURST_TOLERANCE, "{}", fired.at);
            }
            burst::BurstOption::Ring { nose: piece } => {
                assert_eq!(nose, Some(piece.mass), "the mark carries the nose's mass");
                assert_eq!(
                    piece,
                    RoundBody {
                        mass: piece.mass,
                        ..FIXTURE_88
                    }
                );
                assert!(piece.mass < FIXTURE_88.mass);
                let (body, mass, fuze) = fired.flying.expect("the ring's nose flies on");
                assert_eq!(body, piece, "the nose flies as its own body");
                assert_eq!(mass, piece.mass, "the nose carries its own mass");
                assert_eq!(fuze, burst::Fuze::Inert, "the nose bursts no more");
                assert!(fired.at.z < at.z, "the nose flies on past the burst");
            }
        }
    }
}

/// A body whose spend is under the threshold leaves the round unarmed, and a round without a burst
/// never bursts: either flies on through the plate as a solid shot. A mutant that arms on any
/// exit, or bursts an inert fuze, fails here. The 20 mm plate spends 20 reference-mm across, under
/// the fixture threshold (fixture data).
#[test]
fn an_unarmed_or_inert_round_flies_on_as_a_solid_shot() {
    let burst = fixture_burst(burst::BurstShape::Ball);
    for (thickness, fuze) in [
        (0.02, burst::Fuze::of(Some(burst))),
        (THIN, burst::Fuze::Inert),
    ] {
        let fired = fire_fused(&[plate("front", thickness, 0.0, Kind::Main)], fuze);
        let spent: f32 = fired
            .log
            .iter()
            .filter_map(|mark| match mark {
                ArmourMark::Crossing { volumes, .. } => {
                    Some(volumes.iter().map(|volume| volume.cost).sum::<f32>())
                }
                _ => None,
            })
            .sum();
        if fuze != burst::Fuze::Inert {
            assert!(spent < burst.arming_spend, "the fixture spends {spent}");
        }
        assert_eq!(bursts(&fired.log), [], "{thickness} m plate");
        let (_, _, left) = fired.flying.expect("the round flies on");
        assert_eq!(left, fuze, "the fuze is as fired");
    }
}

/// A body met before the delay ends restarts it from that body's exit at that exit's speed,
/// whatever that body's spend: the burst lies the delay past the LAST body. A mutant that ignores
/// the second plate bursts ~3 m short of where this one reads. The 10 mm second plate, under the
/// threshold, and its place inside the first delay are fixture data.
#[test]
fn a_body_met_before_the_burst_restarts_the_delay() {
    let burst = fixture_burst(burst::BurstShape::Ball);
    let fired = fire_fused(
        &[
            plate("front", THIN, 0.0, Kind::Main),
            plate("second", 0.01, -3.0, Kind::Main),
        ],
        burst::Fuze::of(Some(burst)),
    );
    let ends = crossing_ends(&fired.log);
    assert_eq!(ends.len(), 2, "the round crosses both plates");
    let (first, first_speed) = exit_of(ends[0]);
    assert!(
        first_speed * burst.delay > first.z + 3.0,
        "the second plate lies inside the first delay"
    );
    let (exit, exit_speed) = exit_of(ends[1]);
    let found = bursts(&fired.log);
    assert_eq!(found.len(), 1);
    let past = found[0].0.distance(exit);
    assert!(
        (past - exit_speed * burst.delay).abs() < BURST_TOLERANCE,
        "the burst lies {past} m past the second exit, not {} m",
        exit_speed * burst.delay
    );
}

/// An armed round stopped before its delay ends bursts where it rests, at no speed, when the delay
/// ends: the delay's seconds less the path from the arming exit to the rest point at the exit's
/// speed, after the record has it arrive there. The ring then throws no nose piece on, and either
/// ends the round. The back plate stands 1.7 m behind the front, a crossing of its own, then behind
/// a 200 mm soft box filling the space from the front, one crossing that exits the front and rests
/// in the back; the 500 mm plate stops the fixture 88 inside the delay (fixture data). A mutant that drops the burst of a round at rest, flies a nose on from it, times
/// it on arrival, or flies none of the path to the rest point in either crossing fails here.
#[test]
fn an_armed_round_at_rest_bursts_where_it_rests() {
    for shape in [burst::BurstShape::Ball, burst::BurstShape::Ring] {
        let burst = fixture_burst(shape);
        // The front plate's exit speed, read where its crossing ends there.
        let mut exit_speed = None;
        let front = plate("front", THIN, 0.0, Kind::Main);
        let filler = plate("filler", 0.2, -0.5 * THIN - 0.1, Kind::Box(100_000));
        for (volumes, crossings) in [
            (vec![front, plate("back", THICK, -2.0, Kind::Main)], 2),
            (
                vec![
                    front,
                    filler,
                    plate("back", THICK, -0.5 * THIN - 0.2 - 0.5 * THICK, Kind::Main),
                ],
                1,
            ),
        ] {
            let fired = fire_fused(&volumes, burst::Fuze::of(Some(burst)));
            let ends = crossing_ends(&fired.log);
            assert_eq!(ends.len(), crossings, "{shape:?}: {ends:?}");
            let rest = match ends.last() {
                Some(CrossingEnd::Rest { at }) => *at,
                other => panic!("{shape:?}: the back plate stops the round: {other:?}"),
            };
            if let CrossingEnd::Exit { speed, .. } = ends[0] {
                exit_speed = Some(speed);
            }
            let exit_speed = exit_speed.expect("the far back plate is fired first");
            let exit = fired
                .log
                .iter()
                .flat_map(|mark| match mark {
                    ArmourMark::Crossing { volumes, .. } => volumes.as_slice(),
                    _ => &[],
                })
                .find(|crossed| fired.names[&crossed.volume] == "front")
                .expect("the round crosses the front plate")
                .end;
            let found = bursts(&fired.log);
            assert_eq!(found.len(), 1, "{shape:?}");
            let (at, speed, _) = found[0];
            assert_eq!(speed, 0.0, "{shape:?}: at rest");
            assert!(
                at.distance(rest) < BURST_TOLERANCE,
                "{shape:?}: {at} vs {rest}"
            );
            let left = burst.delay - rest.distance(exit) / exit_speed;
            let after = fired
                .log
                .iter()
                .find_map(|mark| match mark {
                    ArmourMark::Burst { after, .. } => Some(*after),
                    _ => None,
                })
                .expect("the burst's mark");
            assert!(left > 0.0, "{shape:?}: it stops inside the delay");
            assert!(
                (after - left).abs() < BURST_TOLERANCE / exit_speed,
                "{shape:?}, {crossings} crossings: it waits {after} s, not {left} s"
            );
            let recorded = fired.report.burst.as_ref().expect("the record's burst");
            let arrived = fired.report.path.last().expect("a path").t;
            assert!(
                (recorded.t - (arrived + after)).abs() < 1.0e-6,
                "{shape:?}: recorded at {} s, arrived at {arrived} s",
                recorded.t
            );
            assert_eq!(fired.flying, None, "{shape:?}: nothing flies on");
            let field = fired.burst.expect("the burst throws its field").field;
            assert_eq!(field.ratio, 0.0, "{shape:?}: thrown at rest");
        }
    }
}

/// A burst throws its field from the burst point about the flight direction, at the carried speed:
/// its strength is the burst's driven-mass energy at the shipped rate, its field the round's burst
/// shape at `λ = v_c / v_s`, and direction `k` of the shape's `N` carries `S·f_k` of that field's
/// spread, to the bit. A mutant that throws from the exit, at the exit speed, at `v_s / v_c`, at
/// another strength or under the other shape fails here; the record and the pen cam rely on it.
#[test]
fn a_burst_throws_its_field_from_where_it_bursts() {
    let fields = spall_cone::BurstFields::shipped();
    for shape in [burst::BurstShape::Ball, burst::BurstShape::Ring] {
        let burst = fixture_burst(shape);
        let fired = fire_fused(
            &[plate("front", THIN, 0.0, Kind::Main)],
            burst::Fuze::of(Some(burst)),
        );
        let (at, speed, _) = bursts(&fired.log)[0];
        let thrown = fired.burst.expect("the burst throws its field");
        assert_eq!(thrown.origin, at, "{shape:?}: from the burst point");
        assert!(
            thrown.axis.dot(Vec3::NEG_Z) > 0.9999,
            "{shape:?}: about the flight direction, {:?}",
            thrown.axis
        );
        assert_eq!(thrown.strength, fields.strength(burst.energy), "{shape:?}");
        let (field, law) = fields.field(shape, speed / burst.sideways_speed);
        assert_eq!(thrown.field, field, "{shape:?}");
        let spread = law.spread(field).unwrap();
        assert_eq!(thrown.rays.len(), spread.directions_count(), "{shape:?}");
        for (ray, [.., weight]) in thrown.rays.iter().zip(spread.strata()) {
            assert_eq!(ray.strength, thrown.strength * weight, "{shape:?}");
        }
    }
}

/// A burst of `shape` fired through the fixture's 50 mm front plate with crew boxes at `angles`
/// off the flight axis, each named, 0.4 m on a side with a 30 HP pool, 1.5 m from the burst point
/// toward `+Y` (fixture data): the shot, and the burst's point and carried speed. `angles` is given
/// the field's brightest angle, the ball's axis or the ring's band centre `atan(v_s / v_c)`, first
/// measured with no box present; the boxes move no burst.
fn fire_burst_at_boxes(
    shape: burst::BurstShape,
    angles: impl Fn(f32) -> Vec<(&'static str, f32)>,
) -> (FixtureFired, Vec3, f32) {
    let burst = fixture_burst(shape);
    let front = plate("front", THIN, 0.0, Kind::Main);
    let probe = fire_fused(&[front], burst::Fuze::of(Some(burst)));
    let (at, speed, _) = bursts(&probe.log)[0];
    let brightest = match shape {
        burst::BurstShape::Ball => 0.0,
        burst::BurstShape::Ring => (burst.sideways_speed / speed).atan(),
    };
    let mut volumes = vec![front];
    for (name, angle) in angles(brightest) {
        let centre = at + 1.5 * (Vec3::NEG_Z * angle.cos() + Vec3::Y * angle.sin());
        volumes.push((name, Vec3::splat(0.4), centre, Kind::Witness(30)));
    }
    let fired = fire_fused(&volumes, burst::Fuze::of(Some(burst)));
    let moved = bursts(&fired.log)[0].0.distance(at);
    assert!(
        moved < BURST_TOLERANCE,
        "{shape:?}: the boxes move the burst {moved} m"
    );
    (fired, at, speed)
}

/// The field leans with the carried speed: the burst doses a crew box where it is brightest (the
/// ball's axis; the ring's band centre, `atan(v_s / v_c)` off it), and none past its rim, behind
/// the burst at 150° from the axis. A mutant that inverts the ratio throws the ball over the whole
/// sphere and moves the ring's band, and fails here. The plate's cone reaches both boxes, so the
/// burst's own rays are read.
#[test]
fn the_burst_field_leans_with_the_carried_speed() {
    for shape in [burst::BurstShape::Ball, burst::BurstShape::Ring] {
        let (fired, ..) = fire_burst_at_boxes(shape, |brightest| {
            vec![("lit", brightest), ("behind", 150.0_f32.to_radians())]
        });
        let thrown = fired.burst.expect("the burst throws its field");
        let dosed: Vec<&str> = thrown
            .rays
            .iter()
            .flat_map(|ray| &ray.doses)
            .map(|(volume, _)| fired.names[volume].as_str())
            .collect();
        assert!(dosed.contains(&"lit"), "{shape:?}: {dosed:?}");
        assert!(!dosed.contains(&"behind"), "{shape:?}: {dosed:?}");
    }
}

/// The record carries the burst as the march threw it: its point on the path, the carried speed,
/// the ring's nose piece's mass, and its field with every direction's strength and doses, in the
/// hull frame; a box's field dose sums its hits from every field, the burst's after the cones',
/// and a box the burst empties is lost at the burst's time. The ball ends the round at the burst
/// point; the ring's nose piece runs the path on past it. A mutant that leaves the burst's
/// deposits out of the ledger, the field out of the record, or the ring's round ending at the
/// burst fails here; the wire and the pen cam read the record.
#[test]
fn the_record_carries_the_burst() {
    for shape in [burst::BurstShape::Ball, burst::BurstShape::Ring] {
        let (fired, at, speed) = fire_burst_at_boxes(shape, |brightest| vec![("lit", brightest)]);
        let report = &fired.report;
        crate::damage_cam::tests::assert_self_consistent("fixture", report);
        assert_eq!(report.unresolved(), Vec::<String>::new(), "{shape:?}");
        let recorded = report.burst.as_ref().expect("the record carries the burst");
        assert_eq!(recorded.speed_mps, speed, "{shape:?}");
        assert_eq!(recorded.nose_kg, bursts(&fired.log)[0].2, "{shape:?}");
        // The fixture tank's hull stands at the world origin, unturned.
        assert!(
            Vec3::from(recorded.point).distance(at) < 1.0e-6,
            "{shape:?}"
        );
        let thrown = fired.burst.as_ref().expect("the burst throws its field");
        let field = recorded
            .field
            .as_ref()
            .expect("the record carries its field");
        assert_eq!(field.field, thrown.field, "{shape:?}");
        assert_eq!(field.strength, thrown.strength, "{shape:?}");
        assert_eq!(field.fragments.len(), thrown.rays.len(), "{shape:?}");
        for (fragment, ray) in field.fragments.iter().zip(&thrown.rays) {
            assert_eq!(fragment.strength, ray.strength, "{shape:?}");
            let hits: Vec<(&str, f32)> = fragment
                .hits
                .iter()
                .map(|hit| (hit.volume.as_str(), hit.damage))
                .collect();
            let doses: Vec<(&str, f32)> = ray
                .doses
                .iter()
                .map(|(volume, dose)| (fired.names[volume].as_str(), *dose))
                .collect();
            assert_eq!(hits, doses, "{shape:?}");
        }
        let lit = report
            .doses
            .iter()
            .find(|dose| dose.volume == "lit")
            .expect("the burst doses the lit box");
        let hits = report
            .fields()
            .flat_map(|cone| &cone.fragments)
            .flat_map(|fragment| &fragment.hits)
            .filter(|hit| hit.volume == "lit")
            .fold(0.0, |sum, hit| sum + hit.damage);
        assert_eq!(lit.field, hits, "{shape:?}");
        let emptied = report
            .consequences
            .iter()
            .find(|consequence| consequence.volume == "lit")
            .expect("the burst knocks out the lit box");
        assert_eq!(emptied.t, recorded.t, "{shape:?}: at the burst");
        match shape {
            burst::BurstShape::Ball => {
                assert_eq!(report.end, RoundEnd::Burst { at: recorded.point })
            }
            burst::BurstShape::Ring => {
                assert!(
                    !matches!(report.end, RoundEnd::Burst { .. }),
                    "{:?}",
                    report.end
                );
                let last = Vec3::from(report.path.last().unwrap().point);
                assert!(last.z < at.z, "the nose runs the path on: {last}");
            }
        }
    }
}
