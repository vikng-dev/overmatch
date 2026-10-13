//! The pen cam recorder: scripted shots through the real march, read back as pen cam records
//! ([`crate::damage_cam::record`]), and the scenario files the demo plays.
//!
//! A scenario names a round from the bound tank's own weapons, where it is fired from and toward,
//! and the target's pose; [`record_scenario`] fires it at the bound tank in the probe world
//! (headless, no window, no input) and returns the record. The scenario list and the records it
//! produces live in [`SCENARIO_DIR`]; [`the_pen_cam_scenarios_are_current`] re-records every one
//! and fails when a checked-in file differs, so a physics change refreshes them deliberately:
//! `PEN_CAM_BLESS=1 cargo test --lib pen_cam` rewrites them, and the diff is the review.
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
use crate::damage_cam::shapes::{ComponentShape, TankAsset, TankShapes};
use crate::damage_cam::{PenetrationReport, PlateClass, RoundEnd, Shooter, ShotVerdict};
use crate::spec::VolumeSpec;
use crate::tank::{Rig, ServoIndex, ServoState, TankServos, restore_servo_truth};
use bevy::ecs::system::RunSystemOnce;
use std::collections::BTreeMap;

/// The scenario list and its records, relative to the asset root.
const SCENARIO_DIR: &str = "dev/pen_cam";
const SCENARIO_LIST: &str = "scenarios.ron";

/// Fixed-step length the march is driven at (s), as the goldens drive it.
const STEP: Duration = Duration::from_millis(16);
/// The shooter every scenario names: fixture data, a player name no match produced. The shooter
/// fires one of the bound tank's own weapons, so its vehicle is the bound tank's.
const FIXTURE_SHOOTER: &str = "Fixture Gunner";

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
    /// What the scenario shows, carried into the record's header.
    shows: String,
    /// One of the bound tank's weapons, by its name in the tank file.
    weapon: String,
    /// Striking speed (m/s); the weapon's muzzle speed when absent.
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
    let pose = Transform::from_translation(PROBE_TANK_AT)
        .with_rotation(Quat::from_rotation_y(scenario.hull_yaw_deg.to_radians()));
    let mut app = probe_world_posed(pose).expect("the bound tank builds");
    app.insert_resource(TimeUpdateStrategy::ManualDuration(STEP))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(MgShortCircuit(false))
        .add_systems(Update, integrate_projectiles);
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
    let round = weapon
        .resolved_round()
        .unwrap_or_else(|err| panic!("{}: {err}", scenario.name));
    let body = round.body;
    let speed = scenario.speed.unwrap_or(round.muzzle_speed);
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
        shooter: Some(Shooter {
            name: FIXTURE_SHOOTER.to_owned(),
            vehicle: crate::tank::TIGER_SPEC_PATH.to_owned(),
        }),
        launch: origin,
        muzzle: scenario.muzzle.map(|muzzle| PROBE_TANK_AT + muzzle),
        projectile: scenario.weapon.clone(),
        caliber_mm: body.caliber * 1000.0,
        mass_kg: body.mass,
        round: weapon.round.clone(),
        vehicle: Some(crate::tank::TIGER_SPEC_PATH.to_owned()),
    };
    record::report(app.world_mut(), &struck, shell, &facts)
        .unwrap_or_else(|| panic!("{}: the shot opens no pen cam", scenario.name))
}

/// A record is stated in the struck hull's frame: one shot fired at the bound tank at two hull
/// poses, turned with the tank, records the same verdict, plates, components, path and muzzle. The
/// shot down a front plate, its muzzle 40 m back and the 30° pose are fixture data; 1 mm is the
/// tolerance on a position.
#[test]
fn a_record_is_stated_in_the_struck_hull_frame() {
    let shot = |yaw_deg: f32| {
        let turn = Quat::from_rotation_y(yaw_deg.to_radians());
        record_scenario(&Scenario {
            name: format!("hull yaw {yaw_deg}"),
            shows: String::new(),
            weapon: "MainGun".to_owned(),
            speed: None,
            from: turn * Vec3::new(-0.57, 1.45, -6.5),
            toward: turn * Vec3::new(-0.57, 1.45, 0.0),
            muzzle: Some(turn * Vec3::new(-0.57, 1.45, -40.0)),
            hull_yaw_deg: yaw_deg,
            turret_yaw_deg: 0.0,
            gun_elevation_deg: 0.0,
            verdict: ShotVerdict::Penetration,
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
         not edit.\n// Re-record with `PEN_CAM_BLESS=1 cargo test --lib pen_cam`.\n{body}\n",
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

/// A checked-in record matches today's march when every fact but a float is exact and every float
/// is within [`RECORD_REL_TOLERANCE`] of its recorded value, or within [`RECORD_ABS_TOLERANCE`] of
/// it. The march is server-only, so it need not be bit-identical across platforms: a record is a
/// view fixture, blessed on one platform and checked on all, and libm's transcendentals differ by
/// an f32 ulp between them.
///
/// DERIVED: 75x the largest cross-platform spread MEASURED between macOS-blessed records and a
/// Linux march, 1.33e-7 relative (about one f32 ulp).
const RECORD_REL_TOLERANCE: f64 = 1e-5;
/// The floor for a value whose recorded magnitude is a cancellation residue (records hold values
/// near 1e-8). DERIVED: one f32 ulp at the largest magnitude a record holds (1000, ulp 6.1e-5),
/// with headroom.
const RECORD_ABS_TOLERANCE: f64 = 1e-4;

/// A record's text, split into float literals and the exact text between them. An integer, or
/// digits inside a name (`s3_ricochet`, `88mm_AP`), stay in the exact text, a [`Token::Fact`].
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Fact(&'a str),
    Float(f64),
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let (mut text_from, mut i) = (0, 0);
    while i < bytes.len() {
        let starts_number = (bytes[i].is_ascii_digit()
            || (bytes[i] == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit)))
            && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || b"_.".contains(&bytes[i - 1])));
        if !starts_number {
            i += 1;
            continue;
        }
        let mut end = i + 1;
        while end < bytes.len()
            && (bytes[end].is_ascii_digit()
                || b".eE".contains(&bytes[end])
                || (b"+-".contains(&bytes[end]) && b"eE".contains(&bytes[end - 1])))
        {
            end += 1;
        }
        let literal = &text[i..end];
        match literal.parse::<f64>() {
            Ok(value) if literal.contains(['.', 'e', 'E']) => {
                if text_from < i {
                    out.push(Token::Fact(&text[text_from..i]));
                }
                out.push(Token::Float(value));
                text_from = end;
            }
            _ => {}
        }
        i = end;
    }
    if text_from < bytes.len() {
        out.push(Token::Fact(&text[text_from..]));
    }
    out
}

/// The largest float difference above [`RECORD_ABS_TOLERANCE`] between two records whose text
/// matches exactly everywhere else.
#[derive(Debug, Default)]
struct Spread {
    abs: f64,
    rel: f64,
    /// The line of the checked-in record holding the largest relative difference.
    worst: String,
}

/// How far `today` departs from `recorded`: `Err` names the first difference in anything but a
/// float literal (a verdict, a name, a plate or path point gained or lost).
fn spread(recorded: &str, today: &str) -> Result<Spread, String> {
    let (a, b) = (tokens(recorded), tokens(today));
    let mut spread = Spread::default();
    let mut at = 0;
    for (index, pair) in a.iter().zip(&b).enumerate() {
        match pair {
            (Token::Float(x), Token::Float(y)) => {
                let abs = (x - y).abs();
                let rel = if abs == 0.0 {
                    0.0
                } else {
                    abs / x.abs().max(y.abs())
                };
                if abs <= RECORD_ABS_TOLERANCE {
                    continue;
                }
                spread.abs = spread.abs.max(abs);
                if rel > spread.rel {
                    spread.rel = rel;
                    at = index;
                }
            }
            (x, y) if x == y => {}
            (Token::Fact(x), Token::Fact(y)) => {
                let (x, y) = x
                    .lines()
                    .zip(y.lines())
                    .find(|(x, y)| x != y)
                    .unwrap_or((x, y));
                return Err(format!("{:?} became {:?}", x.trim(), y.trim()));
            }
            (x, y) => return Err(format!("{x:?} became {y:?}")),
        }
    }
    if a.len() != b.len() {
        return Err(format!("{} tokens became {}", a.len(), b.len()));
    }
    let line_start = a[..at]
        .iter()
        .rev()
        .find_map(|token| match token {
            Token::Fact(text) => text.rsplit('\n').next(),
            Token::Float(_) => None,
        })
        .unwrap_or_default();
    spread.worst = format!("{}{:?}", line_start.trim_start(), a.get(at));
    Ok(spread)
}

/// Every scenario earns its stated verdict, its record is self-consistent and agrees with its own
/// path, and the checked-in record is what the march records today: every discrete fact exact,
/// every float within [`RECORD_REL_TOLERANCE`] or [`RECORD_ABS_TOLERANCE`]. No record is left
/// without a scenario.
#[test]
fn the_pen_cam_scenarios_are_current() {
    let dir = crate::assets::asset_root().join(SCENARIO_DIR);
    let bless = std::env::var_os("PEN_CAM_BLESS").is_some();
    let list = scenarios();
    let mut stale = Vec::new();
    for scenario in &list {
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
        let text = render(scenario, &report);
        let path = dir.join(format!("{}.ron", scenario.name));
        if bless {
            std::fs::write(&path, &text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        } else {
            let recorded = std::fs::read_to_string(&path).unwrap_or_default();
            match spread(&recorded, &text) {
                Ok(spread) if spread.rel <= RECORD_REL_TOLERANCE => {}
                Ok(spread) => stale.push(format!("{}: {spread:?}", scenario.name)),
                Err(changed) => stale.push(format!("{}: {changed}", scenario.name)),
            }
        }
    }
    let mut orphans: Vec<String> = std::fs::read_dir(&dir)
        .expect("the scenario directory exists")
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|file| file.ends_with(".ron") && file != SCENARIO_LIST)
        .filter(|file| !list.iter().any(|s| format!("{}.ron", s.name) == *file))
        .collect();
    orphans.sort();
    assert!(orphans.is_empty(), "records with no scenario: {orphans:?}");
    assert!(
        stale.is_empty(),
        "stale pen cam records {stale:?}: the march records them differently today. Re-record with \
         `PEN_CAM_BLESS=1 cargo test --lib pen_cam` and review the diff"
    );
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
    /// A box with this HP pool, of a soft substance.
    Box(f32),
    /// A box with this HP pool, of steel: an exposed component a round can glance off.
    SteelBox(f32),
}

/// Reference-mm per metre: steel, and the box's soft substance. Fixture data.
const STEEL: f32 = 1000.0;
const SOFT: f32 = 10.0;

/// One fixture volume: name, full extents and centre (m), kind.
type FixtureVolume = (&'static str, Vec3, Vec3, Kind);

/// Static fixture tanks, each volume its own static cuboid turned by its rotation, on the `Armor`
/// layer and owned by the tank its index names, and the real march driven on [`STEP`] with spent
/// shells kept.
fn fixture_tanks(volumes: &[(FixtureVolume, usize, Quat)]) -> (App, Vec<Entity>) {
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
            Kind::Main => {}
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
    let (mut app, tanks) = fixture_tanks(volumes);
    let tank = tanks[struck];
    let struck = StruckTank::capture(app.world_mut(), tank).expect("the fixture has a rig");
    let (projectile, readout) = fixture_round(FIXTURE_88, direction * 800.0);
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
        muzzle: None,
        projectile: "fixture".to_owned(),
        caliber_mm: FIXTURE_88.caliber * 1000.0,
        mass_kg: FIXTURE_88.mass,
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
        &[plate("component", THICK, 0.0, Kind::SteelBox(1000.0))],
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
                Kind::Box(3.0),
            ),
        ],
        LINE_FROM,
    );
    assert_eq!(report.impact.verdict, ShotVerdict::Penetration);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    assert_eq!(report.components.len(), 1, "the box is recorded");
    assert_eq!(report.components[0].count, 3.0);
    assert_eq!(report.components[0].hp, 3.0, "whole at the shot");
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
    // The box's near face: 0.25 m in front of its centre at z = -1, fixture data.
    let entered = dose.entered.expect("the line entered the box");
    assert!((entered[2] + 0.75).abs() < 1.0e-3, "{entered:?}");
    assert!(!report.spall.is_empty(), "the plate's exit throws a cone");
}

/// A shot through external armour alone that flies on is a non-pen: no armour perforated, nothing
/// dosed, no bounce.
#[test]
fn through_external_armour_alone_is_a_non_pen() {
    let report = fixture_shot(&[plate("skirt", 0.01, 0.0, Kind::External)], LINE_FROM);
    assert_eq!(report.impact.verdict, ShotVerdict::NonPen);
    assert!(report.doses.is_empty(), "{:?}", report.doses);
    let last = report.path.last().expect("a path");
    assert!(last.point[2] < -0.01, "the path flies on past the skirt");
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
            Kind::Box(1000.0),
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
                Kind::Box(1000.0),
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
        &[plate("component", 0.10, 0.0, Kind::SteelBox(1000.0))],
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
