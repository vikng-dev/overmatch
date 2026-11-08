//! The track scorecard: one driving card that judges any track model the same way (the game's
//! tank driving physics: belt, suspension and their contact with the world; game state only).
//!
//! Run: `scripts/serial-cargo test --lib --features dev_ui,bitprobe track_scorecard --
//! --ignored --nocapture`. It writes `target/track-scorecard.json` and `target/track-scorecard.md`
//! under the crate root, then fails if a law check failed.
//!
//! Every scenario boots the shipped plugin on a fresh app and drives the shipped tank only
//! through `TankCommand`. It reads hull and world state: Avian body state, Avian's solver
//! contacts and construction facts. What belongs to one track model sits behind [`arm`], the
//! only file an arm edits.
//!
//! Each line is a **law check**, judged against a value computed from a law and the tank's
//! data, or an **observation**, reported for judgement by eye.
//!
//! The world every scenario stands on, met the same way by every arm:
//! - terrain is a height grid at the shipped cell spacing, with its heightfield collider;
//! - steps, walls, curbs, bars and trenches finer than a cell are static Avian bodies, made as
//!   the map makes a graybox block;
//! - rubble is dynamic pieces at and above the size floor.
//!
//! Fixture: every dimension and placement below is fixture data, chosen to pose its question.

mod arm;

use super::*;
use avian3d::prelude::*;
use serde_json::{Value, json};

use crate::Layer;
use crate::terrain_grid::{HeightGrid, TerrainExtent};
use crate::track::rig_geom::RigGeom;
use crate::track::side::Side;
use arm::WorldForce;

/// Fixture: 513 samples over 750 m, so a cell is the shipped grid's 1500 m / 1024.
const CARD_SAMPLES: usize = 513;
const CARD_WORLD_M: f32 = 750.0;
const DT: f32 = 1.0 / FIXED_TICKS_PER_SECOND as f32;
const TICKS: usize = FIXED_TICKS_PER_SECOND;
/// The shipped material the card's static fixtures and pieces are made of.
const FIXTURE_SUBSTANCE: &str = "Brick";

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Law,
    Observation,
}

type Row = Vec<(&'static str, Value)>;

struct Line {
    id: &'static str,
    title: &'static str,
    kind: Kind,
    /// `None` for an observation.
    pass: Option<bool>,
    rows: Vec<Row>,
    note: String,
}

#[derive(Default)]
struct Card {
    lines: Vec<Line>,
}

impl Card {
    fn push(&mut self, line: Line) {
        println!(
            "track scorecard: {} done ({} rows, pass {:?})",
            line.id,
            line.rows.len(),
            line.pass
        );
        self.lines.push(line);
        self.write();
    }

    /// Writes the card so far, so a later line's failure keeps the lines already run.
    fn write(&self) {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
        std::fs::create_dir_all(&dir).expect("the target directory exists or can be made");
        let json = serde_json::to_string_pretty(&self.json()).expect("the card serializes");
        std::fs::write(dir.join("track-scorecard.json"), json).expect("the card's JSON writes");
        std::fs::write(dir.join("track-scorecard.md"), self.markdown())
            .expect("the card's table writes");
    }

    fn json(&self) -> Value {
        Value::Array(
            self.lines
                .iter()
                .map(|line| {
                    let kind = match line.kind {
                        Kind::Law => "law",
                        Kind::Observation => "observation",
                    };
                    json!({
                        "id": line.id,
                        "title": line.title,
                        "kind": kind,
                        "pass": line.pass,
                        "note": line.note,
                        "rows": line.rows.iter().map(|row| {
                            row.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect::<serde_json::Map<_, _>>()
                        }).collect::<Vec<_>>(),
                    })
                })
                .collect(),
        )
    }

    fn markdown(&self) -> String {
        let mut out = String::from("# Track scorecard\n");
        for line in &self.lines {
            let kind = match (line.kind, line.pass) {
                (Kind::Law, Some(true)) => "law check: PASS",
                (Kind::Law, Some(false)) => "law check: FAIL",
                (Kind::Law, None) => "law check: not claimed by this arm",
                (Kind::Observation, _) => "observation",
            };
            out += &format!("\n## {} ({kind})\n\n{}\n", line.title, line.note);
            let Some(first) = line.rows.first() else {
                continue; // A line with no rows says why in its note.
            };
            out += "\n|";
            for (key, _) in first {
                out += &format!(" {key} |");
            }
            out += "\n|";
            for _ in first {
                out += "---|";
            }
            for row in &line.rows {
                out += "\n|";
                for (_, value) in row {
                    out += &format!(" {} |", cell(value));
                }
            }
            out += "\n";
        }
        out
    }
}

fn cell(value: &Value) -> String {
    match value {
        Value::Number(n) if n.is_f64() => {
            let x = n.as_f64().expect("an f64 number");
            if x != 0.0 && (x.abs() >= 1e5 || x.abs() < 1e-3) {
                format!("{x:.3e}")
            } else {
                format!("{x:.4}")
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The hull's state, read from Avian.
#[derive(Clone, Copy)]
struct Hull {
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    angular: Vec3,
}

impl Hull {
    fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }
    fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }
    /// Nose-up positive (rad).
    fn pitch(&self) -> f32 {
        self.forward().y.clamp(-1.0, 1.0).asin()
    }
    /// Right side up positive (rad).
    fn roll(&self) -> f32 {
        (self.rotation * Vec3::X).y.clamp(-1.0, 1.0).asin()
    }
    fn speed(&self) -> f32 {
        self.velocity.dot(self.forward())
    }
    /// Body-frame yaw rate (rad/s), positive turning left.
    fn yaw_rate(&self) -> f32 {
        self.angular.dot(self.up())
    }
}

/// One booted scenario: the shipped plugin, the controlled tank alone, and the fixtures.
struct Bed {
    app: BootedSim,
    tank: Entity,
    rest_y: f32,
    pieces: Vec<Entity>,
}

/// The tank the game spawns, parsed from the file the game loads.
fn shipped_spec() -> crate::spec::TankSpec {
    let path = crate::tank::TANK_SPEC_PATH;
    let text = std::fs::read_to_string(crate::assets::asset_root().join(path))
        .unwrap_or_else(|err| panic!("the shipped tank spec {path} must read: {err}"));
    crate::provenance::from_shipped_str(path, &text).expect("the shipped tank spec parses")
}

fn card_grid(height: impl Fn(f32, f32) -> f32) -> HeightGrid {
    let half = CARD_WORLD_M / 2.0;
    let cell = CARD_WORLD_M / (CARD_SAMPLES - 1) as f32;
    let samples: Vec<f32> = (0..CARD_SAMPLES * CARD_SAMPLES)
        .map(|k| {
            let (i, j) = (k % CARD_SAMPLES, k / CARD_SAMPLES);
            height(-half + i as f32 * cell, -half + j as f32 * cell)
        })
        .collect();
    let low = samples.iter().copied().fold(f32::INFINITY, f32::min);
    let high = samples.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let extent = TerrainExtent {
        world_size_m: CARD_WORLD_M,
        height_offset_m: low,
        // A flat plane still needs a positive decode span to admit its fixture manifest.
        height_span_m: (high - low).max(1.0),
    };
    HeightGrid::new(samples.into(), CARD_SAMPLES as u32, extent)
}

fn flat() -> HeightGrid {
    card_grid(|_, _| 0.0)
}

impl Bed {
    fn new(grid: HeightGrid) -> Self {
        let mut app = booted_driving_grid(grid);
        app.init_resource::<crate::cost::stages::Stages>();
        let world = app.world_mut();
        let tank = world
            .query_filtered::<Entity, (With<Tank>, With<Controlled>)>()
            .single(world)
            .expect("one controlled tank");
        let others: Vec<Entity> = world
            .query_filtered::<Entity, (With<Tank>, Without<Controlled>)>()
            .iter(world)
            .collect();
        for other in others {
            world.despawn(other);
        }
        let rest_y = world.resource::<RigGeom>().hull_rest_y;
        Self {
            app,
            tank,
            rest_y,
            pieces: Vec::new(),
        }
    }

    fn on_ground(ground: Option<crate::ground_field::ContactGround>, grid: HeightGrid) -> Self {
        let mut bed = Self::new(grid);
        if let Some(ground) = ground {
            arm::set_ground(&mut bed.app, ground);
        }
        bed
    }

    /// Place the hull at rest over `ground`, before the clock starts: every arm then begins from
    /// its own spawn state.
    fn place(&mut self, ground: Vec3, rotation: Quat) {
        let at = ground + rotation * Vec3::Y * self.rest_y;
        let mut e = self.app.world_mut().entity_mut(self.tank);
        *e.get_mut::<Transform>().expect("tank transform") =
            Transform::from_translation(at).with_rotation(rotation);
        e.get_mut::<Position>().expect("tank position").0 = at;
        e.get_mut::<Rotation>().expect("tank rotation").0 = rotation;
        e.get_mut::<LinearVelocity>().expect("tank velocity").0 = Vec3::ZERO;
        e.get_mut::<AngularVelocity>().expect("tank spin").0 = Vec3::ZERO;
    }

    fn start(&mut self) {
        // One frozen update registers the fixtures before the first tick reads the world.
        self.app.update();
        start_fixed_clock(&mut self.app);
    }

    fn tick(&mut self, throttle: f32, steer: f32) {
        drive_tick(&mut self.app, self.tank, throttle, steer);
    }

    fn hull(&self) -> Hull {
        let world = self.app.world();
        Hull {
            position: world.get::<Position>(self.tank).expect("tank position").0,
            rotation: world.get::<Rotation>(self.tank).expect("tank rotation").0,
            velocity: world.get::<LinearVelocity>(self.tank).expect("velocity").0,
            angular: world.get::<AngularVelocity>(self.tank).expect("spin").0,
        }
    }

    fn mass(&self) -> f32 {
        self.app
            .world()
            .get::<ComputedMass>(self.tank)
            .expect("tank mass")
            .value()
    }

    fn weight(&self) -> f32 {
        self.mass() * crate::track::derive::G
    }

    fn center_of_mass(&self) -> Vec3 {
        let hull = self.hull();
        let local = self
            .app
            .world()
            .get::<ComputedCenterOfMass>(self.tank)
            .expect("tank centre of mass")
            .0;
        hull.position + hull.rotation * local
    }

    /// Translational plus rotational kinetic energy of the hull (J).
    fn kinetic_energy(&self) -> f32 {
        let hull = self.hull();
        let inertia = self
            .app
            .world()
            .get::<ComputedAngularInertia>(self.tank)
            .expect("tank inertia")
            .value();
        let w = hull.rotation.inverse() * hull.angular;
        0.5 * self.mass() * hull.velocity.length_squared() + 0.5 * w.dot(inertia.mul_vec3(w))
    }

    fn geom(&self) -> &RigGeom {
        self.app.world().resource::<RigGeom>()
    }

    /// A static block as the map makes a graybox: centre and full size (m).
    fn spawn_static(&mut self, center: Vec3, size: Vec3) -> Entity {
        let friction = crate::substances::SubstanceRegistry::shipped_static()
            .contact_friction(FIXTURE_SUBSTANCE)
            .expect("the fixture substance states a friction");
        self.app
            .world_mut()
            .spawn((
                Name::new("track scorecard fixture"),
                Transform::from_translation(center),
                RigidBody::Static,
                Collider::cuboid(size.x, size.y, size.z),
                CollisionLayers::new([Layer::Terrain], LayerMask::ALL),
                friction,
            ))
            .id()
    }

    /// A dynamic rubble piece of the fixture substance: centre and full size (m).
    fn spawn_piece(&mut self, center: Vec3, size: Vec3) -> Entity {
        let registry = crate::substances::SubstanceRegistry::shipped_static();
        let density = registry
            .hard(FIXTURE_SUBSTANCE)
            .expect("the fixture substance is a hard substance")
            .density()
            .expect("the fixture substance states a density") as f32;
        let mass = density * size.x * size.y * size.z;
        let friction = registry
            .contact_friction(FIXTURE_SUBSTANCE)
            .expect("the fixture substance states a friction");
        let shape = Cuboid::new(size.x, size.y, size.z);
        let piece = self
            .app
            .world_mut()
            .spawn((
                crate::debris::DebrisPiece,
                Transform::from_translation(center),
                Position(center),
                Rotation::default(),
                RigidBody::Dynamic,
                Collider::cuboid(size.x, size.y, size.z),
                Mass(mass),
                NoAutoMass,
                AngularInertia::from_shape(&shape, mass),
                NoAutoAngularInertia,
                CenterOfMass(Vec3::ZERO),
                NoAutoCenterOfMass,
                friction,
                crate::construction::breakage::piece_layers(),
            ))
            .id();
        self.pieces.push(piece);
        piece
    }

    /// Every force on the hull from a world body this tick: the arm's own channel plus Avian's
    /// solver normal impulses between the tank's colliders and other bodies.
    fn world_forces(&self) -> Vec<WorldForce> {
        let mut out = arm::belt_forces(&self.app, self.tank);
        let graph = self.app.world().resource::<ContactGraph>();
        for pair in graph.iter_active_touching() {
            // Avian reports the impulse on body 1 against the normal, which points 1 → 2.
            let (sign, other) = if pair.body1 == Some(self.tank) {
                (-1.0, pair.body2.unwrap_or(pair.collider2))
            } else if pair.body2 == Some(self.tank) {
                (1.0, pair.body1.unwrap_or(pair.collider1))
            } else {
                continue;
            };
            for manifold in &pair.manifolds {
                for point in &manifold.points {
                    out.push(WorldForce {
                        body: Some(other),
                        point: point.point,
                        force: manifold.normal * (sign * point.normal_impulse / DT),
                    });
                }
            }
        }
        out
    }

    /// Avian solver contacts with the tank as a party: (pairs, manifolds, points).
    fn solver_contacts(&self) -> (usize, usize, usize) {
        let graph = self.app.world().resource::<ContactGraph>();
        graph
            .iter_active_touching()
            .filter(|pair| pair.body1 == Some(self.tank) || pair.body2 == Some(self.tank))
            .fold((0, 0, 0), |(pairs, manifolds, points), pair| {
                (
                    pairs + 1,
                    manifolds + pair.manifolds.len(),
                    points + pair.manifolds.iter().map(|m| m.points.len()).sum::<usize>(),
                )
            })
    }

    /// The names of the tank's colliders in solver contact with `body` this tick.
    fn touching_colliders(&self, body: Entity) -> Vec<String> {
        let world = self.app.world();
        let name = |collider: Entity| {
            world
                .get::<Name>(collider)
                .or_else(|| {
                    world
                        .get::<ChildOf>(collider)
                        .and_then(|parent| world.get::<Name>(parent.parent()))
                })
                .map_or_else(|| format!("{collider:?}"), |n| n.as_str().to_owned())
        };
        let mut out = Vec::new();
        for pair in world.resource::<ContactGraph>().iter_active_touching() {
            if pair.body1 == Some(self.tank) && pair.body2.unwrap_or(pair.collider2) == body {
                out.push(name(pair.collider1));
            } else if pair.body2 == Some(self.tank) && pair.body1.unwrap_or(pair.collider1) == body
            {
                out.push(name(pair.collider2));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// World support on the pieces from everything except the tank and each other (N, up).
    fn world_support_on_pieces(&self) -> f32 {
        let graph = self.app.world().resource::<ContactGraph>();
        let is_piece = |e: Option<Entity>| e.is_some_and(|e| self.pieces.contains(&e));
        let mut up = 0.0;
        for pair in graph.iter_active_touching() {
            let (b1, b2) = (pair.body1, pair.body2);
            if b1 == Some(self.tank) || b2 == Some(self.tank) || is_piece(b1) == is_piece(b2) {
                continue;
            }
            let sign = if is_piece(b1) { -1.0 } else { 1.0 };
            for manifold in &pair.manifolds {
                for point in &manifold.points {
                    up += manifold.normal.y * sign * point.normal_impulse / DT;
                }
            }
        }
        up
    }

    fn piece_weight(&self) -> f32 {
        self.pieces
            .iter()
            .map(|&p| {
                self.app
                    .world()
                    .get::<ComputedMass>(p)
                    .expect("piece mass")
                    .value()
            })
            .sum::<f32>()
            * crate::track::derive::G
    }

    fn max_piece_speed(&self) -> f32 {
        self.pieces
            .iter()
            .map(|&p| {
                self.app
                    .world()
                    .get::<LinearVelocity>(p)
                    .map_or(0.0, |v| v.0.length())
            })
            .fold(0.0, f32::max)
    }

    /// The deepest point of the rest-pose belt bottom inside any piece (m): the track's own
    /// outline, from construction, sampled under every route circle at the shoe's three columns.
    fn sink_into_pieces(&self) -> f32 {
        let world = self.app.world();
        let geom = self.geom();
        let hull = self.hull();
        let outer = geom.model.pin_to_outer;
        let mut points = Vec::new();
        for side in Side::ALL {
            for &(centre, radius) in geom.rest.get(side) {
                for step in 0..=8 {
                    let angle = std::f32::consts::PI * (1.0 + step as f32 / 8.0);
                    for (x, _) in geom.grip_columns(side) {
                        let local = Vec3::new(
                            x,
                            centre.y + (radius + outer) * angle.sin(),
                            centre.x + (radius + outer) * angle.cos(),
                        );
                        points.push(hull.position + hull.rotation * local);
                    }
                }
            }
        }
        let mut deepest = 0.0_f32;
        for &piece in &self.pieces {
            let (Some(shape), Some(at), Some(rot)) = (
                world.get::<Collider>(piece),
                world.get::<Position>(piece),
                world.get::<Rotation>(piece),
            ) else {
                continue; // A piece the destruction model removed has no depth.
            };
            for &point in &points {
                let (surface, inside) = shape.project_point(*at, *rot, point, false);
                if inside {
                    deepest = deepest.max(surface.distance(point));
                }
            }
        }
        deepest
    }

    /// This tick's production `Work` counts.
    fn take_work(&self) -> Vec<(String, f64)> {
        let counts = serde_json::to_value(
            self.app
                .world()
                .resource::<crate::cost::stages::Stages>()
                .take_counts(),
        )
        .expect("work counts serialize");
        counts
            .as_object()
            .expect("work counts are a map")
            .iter()
            .map(|(k, v)| (k.clone(), v.as_f64().expect("a count is a number")))
            .collect()
    }
}

/// Mean and root-mean-square deviation of a series.
fn mean_rms(values: &[f32]) -> (f32, f32) {
    let n = values.len().max(1) as f32;
    let mean = values.iter().sum::<f32>() / n;
    let rms = (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n).sqrt();
    (mean, rms)
}

fn deg(rad: f32) -> f64 {
    f64::from(rad.to_degrees())
}

fn f(x: f32) -> Value {
    json!(f64::from(x))
}

fn grounds() -> Vec<(String, crate::ground_types::GroundType)> {
    crate::ground_types::GroundTypes::load_shipped()
        .types
        .into_iter()
        .collect()
}

fn base_ground() -> crate::ground_field::ContactGround {
    crate::ground_types::GroundTypes::load_shipped()
        .get(crate::terrain_blend::BASE_GROUND)
        .contact()
}

/// Line 1. The static friction law (the owner's decision 3): a parked tank holds while gravity's
/// pull along the slope stays within its grip and brake capacity, and moves once it exceeds
/// either. Per ground, `limit = min(atan μ, asin(2 F_brake k_static / W))` from the ground's grip
/// and the tank's brake data; the slopes sit a tenth of the limit below and above it.
fn slope_hold(card: &mut Card) {
    let spec = shipped_spec();
    let tr = &spec.track.powertrain.transmission;
    let weight = spec.mass * crate::track::derive::G;
    let brake_limit = (2.0 * tr.brake_force * tr.brake_static_factor / weight)
        .min(1.0)
        .asin();
    // Resolution: 0.1 % of a tick's gravitational speed permits force and pose roundoff.
    let precision = crate::track::derive::G * DT * 1e-3;
    let mut rows = Vec::new();
    let mut pass = true;
    for (name, ground) in grounds() {
        let ground_limit = ground.grip.value.atan();
        let limit = ground_limit.min(brake_limit);
        for above in [false, true] {
            if limit == 0.0 && !above {
                continue; // Zero grip has no holding slope.
            }
            let margin = if limit > 0.0 {
                (limit * 0.1).min((std::f32::consts::FRAC_PI_2 - limit) * 0.5)
            } else {
                1.0_f32.to_radians()
            };
            let angle = limit + if above { margin } else { -margin };
            // The plane falls toward +z; the tank faces uphill, toward −z.
            let mut bed =
                Bed::on_ground(Some(ground.contact()), card_grid(|_, z| -z * angle.tan()));
            let rotation = Quat::from_rotation_x(angle);
            let downhill = rotation * Vec3::Z;
            let start = Vec3::new(0.0, 0.0, 0.0);
            bed.place(start, rotation);
            bed.start();
            let begin = bed.hull().position;
            let seconds = if above { 8 } else { 60 };
            let mut late_start = 0.0;
            let mut late_speed = 0.0_f32;
            let mut last = 0.0;
            for tick in 0..seconds * TICKS {
                bed.tick(0.0, 0.0);
                let hull = bed.hull();
                last = (hull.position - begin).dot(downhill);
                if tick + 1 == (seconds - 1) * TICKS {
                    late_start = last;
                }
                if tick >= (seconds - 1) * TICKS {
                    late_speed = late_speed.max(hull.velocity.dot(downhill).abs());
                }
            }
            let travel = last - late_start;
            let held = travel.abs() < precision && late_speed < precision;
            let moved = travel > precision && late_speed > precision;
            let ok = if above { moved } else { held };
            pass &= ok;
            rows.push(vec![
                ("ground", json!(name)),
                ("side", json!(if above { "above" } else { "below" })),
                ("slope_deg", json!(deg(angle))),
                ("grip_limit_deg", json!(deg(ground_limit))),
                ("brake_limit_deg", json!(deg(brake_limit))),
                (
                    "binding",
                    json!(if ground_limit < brake_limit {
                        "grip"
                    } else {
                        "brake"
                    }),
                ),
                ("settle_m", f(last)),
                ("last_second_travel_m", f(travel)),
                ("last_second_speed_m_s", f(late_speed)),
                ("pass", json!(ok)),
            ]);
        }
    }
    card.push(Line {
        id: "slope",
        title: "1. Holding on a slope",
        kind: Kind::Law,
        pass: Some(pass),
        rows,
        note: format!(
            "Parked (spawn state, park lever on), no input. Below: 60 s, holds iff the last \
             second's travel and speed are under {precision:.2e} (m, m/s; 0.1 % of g·dt). \
             Above: 8 s, moves iff both exceed it. Limits from the ground's grip and the tank's \
             brake data; settle_m (the spring-back and any creep) is an observation."
        ),
    });
}

/// Line 2. Turning: achieved over geared radius, yaw rate, and the yaw-resisting moment against
/// Steeds' μWL/4 (uniform pressure; L the construction's road-wheel span).
fn turning(card: &mut Card) {
    let feel = crate::track::drive::drive_feel();
    let wide = 0.5 * (feel.wide_on.0 + feel.tight_on.0);
    let ground = base_ground();
    let mut rows = Vec::new();
    // Fixture: two run-ups give two gears; a pivot is the zero-radius end.
    for (label, run_up_s, steer, throttle) in [
        ("wide, short run-up", 3, wide, 1.0),
        ("tight, short run-up", 3, 1.0, 1.0),
        ("wide, long run-up", 10, wide, 1.0),
        ("tight, long run-up", 10, 1.0, 1.0),
        ("neutral pivot", 0, 1.0, 0.0),
    ] {
        let mut bed = Bed::on_ground(Some(ground), flat());
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        for _ in 0..run_up_s * TICKS {
            bed.tick(throttle, 0.0);
        }
        let hold = 12 * TICKS;
        let window = 3 * TICKS;
        let (mut speeds, mut yaws, mut m_long, mut m_lat) = (vec![], vec![], vec![], vec![]);
        let mut gears = std::collections::BTreeSet::new();
        for tick in 0..hold {
            bed.tick(throttle, steer);
            if tick < hold - window {
                continue;
            }
            let hull = bed.hull();
            speeds.push(hull.speed());
            yaws.push(hull.yaw_rate());
            let state = bed
                .app
                .world()
                .get::<crate::track::sim::TankTransmission>(bed.tank)
                .expect("a tank carries its transmission")
                .0;
            gears.insert((state.gear, state.reverse, state.steer_step));
            let com = bed.center_of_mass();
            let (fwd, lat, up) = (hull.forward(), hull.rotation * Vec3::X, hull.up());
            let (mut long, mut side) = (0.0, 0.0);
            for force in bed.world_forces() {
                let arm = force.point - com;
                long += arm.cross(fwd * force.force.dot(fwd)).dot(up);
                side += arm.cross(lat * force.force.dot(lat)).dot(up);
            }
            m_long.push(long);
            m_lat.push(side);
        }
        let (v, _) = mean_rms(&speeds);
        let (w, w_rms) = mean_rms(&yaws);
        let (long, _) = mean_rms(&m_long);
        let (side, _) = mean_rms(&m_lat);
        let world = bed.app.world();
        let trans = world.resource::<crate::track::sim::TrackGear>().trans();
        let geom = world.resource::<RigGeom>();
        let span = 0.5
            * (span_of(geom.road_wheels.get(Side::Left))
                + span_of(geom.road_wheels.get(Side::Right)));
        let steeds = ground.grip * bed.weight() * span / 4.0;
        let &(gear, reverse, detent) = gears.last().expect("the window sampled");
        let geared = if gear == 0 || detent == 0 {
            f32::NAN
        } else {
            let (tight, wide) = trans.steering_radii(reverse, gear);
            if detent == 2 { tight } else { wide }
        };
        let radius = if w.abs() > 1e-4 {
            v / w.abs()
        } else {
            f32::INFINITY
        };
        rows.push(vec![
            ("run", json!(label)),
            ("steer", f(steer)),
            ("states_in_window", json!(format!("{gears:?}"))),
            ("speed_m_s", f(v)),
            ("yaw_rate_rad_s", f(w)),
            ("yaw_rate_rms", f(w_rms)),
            ("radius_m", f(radius)),
            ("geared_radius_m", f(geared)),
            ("radius_over_geared", f(radius / geared)),
            ("moment_longitudinal_n_m", f(long)),
            ("moment_lateral_n_m", f(side)),
            ("steeds_n_m", f(steeds)),
            ("lateral_over_steeds", f(side.abs() / steeds)),
        ]);
    }
    card.push(Line {
        id: "turning",
        title: "2. Turning",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "Base ground ({}), full throttle run-up then a 12 s steer hold; means over the last \
             3 s. states_in_window lists (gear, reverse, detent). Moments are about the hull's up \
             axis through its centre of mass, from every world force on the hull split into its \
             longitudinal and lateral parts; in a steady turn they cancel, and the lateral part \
             is the resisting moment Steeds' μWL/4 estimates (DERIVED: μ the ground's grip, W the \
             hull weight, L the mean road-wheel span).",
            crate::terrain_blend::BASE_GROUND
        ),
    });
}

fn span_of(wheels: &[Vec3]) -> f32 {
    let (lo, hi) = wheels
        .iter()
        .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), w| {
            (lo.min(w.z), hi.max(w.z))
        });
    hi - lo
}

/// Line 3. Acceleration and top speed per ground: full throttle from rest on level ground.
fn acceleration(card: &mut Card) {
    let mut rows = Vec::new();
    for (name, ground) in grounds() {
        let mut bed = Bed::on_ground(Some(ground.contact()), flat());
        bed.place(Vec3::new(0.0, 0.0, 330.0), Quat::IDENTITY);
        bed.start();
        let marks = [2, 5, 10, 20, 40];
        let mut at = Vec::new();
        let mut to_5 = f32::NAN;
        let mut last_second = Vec::new();
        for tick in 1..=40 * TICKS {
            bed.tick(1.0, 0.0);
            let speed = bed.hull().speed();
            if to_5.is_nan() && speed >= 5.0 {
                to_5 = tick as f32 * DT;
            }
            if marks.iter().any(|&s| s * TICKS == tick) {
                at.push(speed);
            }
            if tick > 39 * TICKS {
                last_second.push(speed);
            }
        }
        let (top, spread) = mean_rms(&last_second);
        let gear = bed
            .app
            .world()
            .get::<crate::track::sim::TankTransmission>(bed.tank)
            .expect("transmission")
            .0
            .gear;
        rows.push(vec![
            ("ground", json!(name)),
            ("grip", f(ground.grip.value)),
            ("rolling_resistance", f(ground.rolling_resistance.value)),
            ("v_2s", f(at[0])),
            ("v_5s", f(at[1])),
            ("v_10s", f(at[2])),
            ("v_20s", f(at[3])),
            ("v_40s", f(at[4])),
            ("time_to_5_m_s", f(to_5)),
            ("top_speed_m_s", f(top)),
            ("top_speed_rms", f(spread)),
            ("final_gear", json!(gear)),
        ]);
    }
    card.push(Line {
        id: "acceleration",
        title: "3. Acceleration and top speed per ground",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: "Full throttle from rest, 40 s on level ground; top speed is the last second's mean."
            .into(),
    });
}

/// The hull-local z of the tank's front-most belt point (m, forward is −z).
fn front_z(geom: &RigGeom) -> f32 {
    Side::ALL
        .iter()
        .flat_map(|&side| geom.rest.get(side).iter())
        .map(|&(c, r)| c.x - r - geom.model.pin_to_outer)
        .fold(f32::INFINITY, f32::min)
}

/// Line 4. Climbing (the owner's decision 1): a step-height sweep from a fixed approach, and the
/// wall taller than the run-up.
fn climbing(card: &mut Card) {
    let ground = base_ground();
    let mut rows = Vec::new();
    let mut highest = 0.0_f32;
    // Fixture: heights across and beyond the running gear's reach.
    for height in [0.2, 0.4, 0.6, 0.8, 1.0, 1.2] {
        let mut bed = Bed::on_ground(Some(ground), flat());
        let front = front_z(bed.geom());
        // Fixture: the step face 6 m ahead of the belt's front point; a 20 × 10 m plateau.
        let face = front - 6.0;
        bed.spawn_static(
            Vec3::new(0.0, (height - 1.0) / 2.0, face - 10.0),
            Vec3::new(10.0, height + 1.0, 20.0),
        );
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        let begin = bed.hull().position;
        let (mut max_pitch, mut max_rise, mut crossed_at) = (0.0_f32, 0.0_f32, f32::NAN);
        for tick in 0..15 * TICKS {
            bed.tick(1.0, 0.0);
            let hull = bed.hull();
            max_pitch = max_pitch.max(hull.pitch());
            max_rise = max_rise.max(hull.position.y - begin.y);
            if hull.position.z < face - 3.0 && hull.position.y - begin.y > 0.8 * height {
                crossed_at = tick as f32 * DT;
                break;
            }
        }
        let climbed = !crossed_at.is_nan();
        if climbed {
            highest = highest.max(height);
        }
        let hull = bed.hull();
        rows.push(vec![
            ("case", json!(format!("step {height} m"))),
            ("climbed", json!(climbed)),
            ("time_s", f(crossed_at)),
            ("max_rise_m", f(max_rise)),
            ("max_pitch_deg", json!(deg(max_pitch))),
            ("end_z_past_face_m", f(face - hull.position.z)),
            ("upward_force_from_obstacle_n", Value::Null),
            ("touching", Value::Null),
        ]);
    }

    // The owner's wall case: a wall taller than the run-up, pushed at full throttle.
    let mut bed = Bed::on_ground(Some(ground), flat());
    let front = front_z(bed.geom());
    let face = front - 2.0;
    // Fixture: 4 m tall, 2 m thick, 10 m wide; no part of the tank reaches over it.
    let wall = bed.spawn_static(Vec3::new(0.0, 1.5, face - 1.0), Vec3::new(10.0, 5.0, 2.0));
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    let begin = bed.hull().position;
    let (mut max_pitch, mut max_rise, mut max_up) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut late_up = Vec::new();
    let mut touching = std::collections::BTreeSet::new();
    let mut belt_touch = false;
    for tick in 0..8 * TICKS {
        bed.tick(1.0, 0.0);
        let hull = bed.hull();
        max_pitch = max_pitch.max(hull.pitch());
        max_rise = max_rise.max(hull.position.y - begin.y);
        let forces = bed.world_forces();
        let up: f32 = forces
            .iter()
            .filter(|f| f.body == Some(wall))
            .map(|f| f.force.y)
            .sum();
        belt_touch |= arm::belt_forces(&bed.app, bed.tank)
            .iter()
            .any(|f| f.body == Some(wall));
        touching.extend(bed.touching_colliders(wall));
        max_up = max_up.max(up);
        if tick >= 6 * TICKS {
            late_up.push(up);
        }
    }
    let (late, _) = mean_rms(&late_up);
    rows.push(vec![
        ("case", json!("wall 4 m, front wheel first")),
        ("climbed", json!(false)),
        ("time_s", Value::Null),
        ("max_rise_m", f(max_rise)),
        ("max_pitch_deg", json!(deg(max_pitch))),
        ("end_z_past_face_m", f(face - bed.hull().position.z)),
        (
            "upward_force_from_obstacle_n",
            json!(format!("peak {max_up:.0}, last 2 s mean {late:.0}")),
        ),
        (
            "touching",
            json!(format!(
                "belt channel {belt_touch}; solver colliders {:?}",
                touching
            )),
        ),
    ]);
    card.push(Line {
        id: "climbing",
        title: "4. Climbing",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "Full throttle from rest. Steps: face 6 m ahead of the belt's front point, climbed iff \
             the hull centre passes 3 m beyond the face risen by 0.8 × the step within 15 s. \
             Highest step climbed: {highest} m. Wall: face 2 m ahead, 8 s of push; the upward \
             force is the wall's vertical force on the hull through every channel the card reads \
             (the arm's own and Avian's solver normals), and touching names what met it."
        ),
    });
}

/// Line 5. Trenches narrower and wider than the road-wheel span, cut between two plateaus.
fn trenches(card: &mut Card) {
    let ground = base_ground();
    let mut rows = Vec::new();
    let probe = Bed::on_ground(Some(ground), flat());
    let geom = probe.geom();
    let span = 0.5
        * (span_of(geom.road_wheels.get(Side::Left)) + span_of(geom.road_wheels.get(Side::Right)));
    let front = front_z(geom);
    drop(probe);
    // Fixture: 2 m deep plateaus, the near edge 6 m ahead of the belt's front point.
    let top = 2.0;
    for share in [0.25, 0.5, 0.75, 1.25] {
        let width = share * span;
        let mut bed = Bed::on_ground(Some(ground), flat());
        let near = front - 6.0;
        let far = near - width;
        bed.spawn_static(
            Vec3::new(0.0, top / 2.0, (near + 40.0) / 2.0),
            Vec3::new(12.0, top, 40.0 - near),
        );
        bed.spawn_static(
            Vec3::new(0.0, top / 2.0, far - 20.0),
            Vec3::new(12.0, top, 40.0),
        );
        bed.place(Vec3::new(0.0, top, 0.0), Quat::IDENTITY);
        bed.start();
        let begin = bed.hull().position;
        let (mut sink, mut max_pitch, mut min_pitch) = (0.0_f32, 0.0_f32, 0.0_f32);
        let mut crossed_at = f32::NAN;
        for tick in 0..15 * TICKS {
            bed.tick(1.0, 0.0);
            let hull = bed.hull();
            sink = sink.max(begin.y - hull.position.y);
            max_pitch = max_pitch.max(hull.pitch());
            min_pitch = min_pitch.min(hull.pitch());
            if hull.position.z < far - 6.0 && hull.position.y > begin.y - 0.5 * bed.rest_y {
                crossed_at = tick as f32 * DT;
                break;
            }
        }
        rows.push(vec![
            ("width_m", f(width)),
            ("width_over_span", f(share)),
            ("crossed", json!(!crossed_at.is_nan())),
            ("time_s", f(crossed_at)),
            ("max_sink_m", f(sink)),
            (
                "pitch_range_deg",
                json!(format!("{:.1} .. {:.1}", deg(min_pitch), deg(max_pitch))),
            ),
            ("end_z_past_far_edge_m", f(far - bed.hull().position.z)),
        ]);
    }
    card.push(Line {
        id: "trenches",
        title: "5. Trenches",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "Road-wheel span {span:.3} m (construction). A {top} m deep cut between two static \
             plateaus; full throttle from rest, the near edge 6 m ahead. Crossed iff the hull \
             centre passes 6 m beyond the far edge within half a rest height of the plateau's \
             rest level, within 15 s. Sink is the hull's deepest drop below its start."
        ),
    });
}

/// Line 6. A curb under one track's outer edge, driven along; and one track half off a ledge.
fn curb_and_ledge(card: &mut Card) {
    let ground = base_ground();
    let mut rows = Vec::new();

    // A 0.15 m curb, 0.3 m wide, centred under the right shoe's outer edge, from 5 m ahead.
    let mut bed = Bed::on_ground(Some(ground), flat());
    let edge = bed.geom().grip_columns(Side::Right)[2].0;
    let front = front_z(bed.geom());
    let start = front - 5.0;
    bed.spawn_static(
        Vec3::new(edge, 0.0, start - 30.0),
        Vec3::new(0.3, 0.3, 60.0),
    );
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    let begin = bed.hull().position;
    let (mut rolls, mut heights) = (vec![], vec![]);
    for tick in 0..14 * TICKS {
        bed.tick(0.3, 0.0);
        if tick >= 9 * TICKS {
            let hull = bed.hull();
            rolls.push(hull.roll());
            heights.push(hull.position.y - begin.y);
        }
    }
    let (roll, roll_rms) = mean_rms(&rolls);
    let (rise, _) = mean_rms(&heights);
    let hull = bed.hull();
    rows.push(vec![
        (
            "case",
            json!("curb 0.15 m under right outer edge, 0.3 throttle"),
        ),
        ("roll_deg", json!(deg(roll))),
        ("roll_rms_deg", json!(deg(roll_rms))),
        ("hull_rise_m", f(rise)),
        ("lateral_drift_m", f(hull.position.x - begin.x)),
        ("travel_m", f(begin.z - hull.position.z)),
    ]);

    drop(bed); // Releases the boot lease the next scenario takes.

    // A 1 m ledge whose edge runs under the right shoe's centre line; parked there.
    let mut bed = Bed::on_ground(Some(ground), flat());
    let centre = bed.geom().grip_columns(Side::Right)[1].0;
    let top = 1.0;
    bed.spawn_static(
        Vec3::new(centre - 10.0, top / 2.0, 0.0),
        Vec3::new(20.0, top, 20.0),
    );
    bed.place(Vec3::new(0.0, top, 0.0), Quat::IDENTITY);
    bed.start();
    let begin = bed.hull().position;
    let (mut rolls, mut heights) = (vec![], vec![]);
    for tick in 0..6 * TICKS {
        bed.tick(0.0, 0.0);
        if tick >= 4 * TICKS {
            let hull = bed.hull();
            rolls.push(hull.roll());
            heights.push(hull.position.y - begin.y);
        }
    }
    let (roll, roll_rms) = mean_rms(&rolls);
    let (rise, _) = mean_rms(&heights);
    let hull = bed.hull();
    rows.push(vec![
        ("case", json!("right track half off a 1 m ledge, parked")),
        ("roll_deg", json!(deg(roll))),
        ("roll_rms_deg", json!(deg(roll_rms))),
        ("hull_rise_m", f(rise)),
        ("lateral_drift_m", f(hull.position.x - begin.x)),
        ("travel_m", f(begin.z - hull.position.z)),
    ]);
    card.push(Line {
        id: "curb_ledge",
        title: "6. Curb under one edge, half off a ledge",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: "Curb: means over seconds 9 to 14. Ledge: placed with the edge under the right \
               shoe's centre line, means over seconds 4 to 6. Rise is the hull centre against \
               its rest height over the supporting level."
            .into(),
    });
}

/// Line 7. Ride over washboard bars (observation), and the passivity law after a kick: with no
/// input, the hull's oscillation energy must fall.
fn ride(card: &mut Card) {
    let ground = base_ground();
    let mut rows = Vec::new();
    // Fixture: 30 bars 0.08 m tall, 0.4 m long, 1.2 m apart, from 40 m ahead.
    let mut bed = Bed::on_ground(Some(ground), flat());
    let front = front_z(bed.geom());
    let first = front - 40.0;
    for bar in 0..30 {
        bed.spawn_static(
            Vec3::new(0.0, 0.0, first - bar as f32 * 1.2),
            Vec3::new(10.0, 0.16, 0.4),
        );
    }
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    let (mut pitch, mut roll, mut vy, mut speed) = (vec![], vec![], vec![], vec![]);
    let last = first - 29.0 * 1.2;
    for _ in 0..40 * TICKS {
        bed.tick(1.0, 0.0);
        let hull = bed.hull();
        let z_front = hull.position.z + front;
        if z_front < first && hull.position.z - front > last {
            pitch.push(hull.pitch());
            roll.push(hull.roll());
            vy.push(hull.velocity.y);
            speed.push(hull.speed());
        }
        if hull.position.z - front < last - 5.0 {
            break;
        }
    }
    let (_, pitch_rms) = mean_rms(&pitch);
    let (_, roll_rms) = mean_rms(&roll);
    let (_, vy_rms) = mean_rms(&vy);
    let (v, _) = mean_rms(&speed);
    rows.push(vec![
        ("case", json!("washboard, full throttle")),
        ("samples", json!(pitch.len())),
        ("speed_m_s", f(v)),
        ("pitch_rms_deg", json!(deg(pitch_rms))),
        ("roll_rms_deg", json!(deg(roll_rms))),
        ("vertical_speed_rms_m_s", f(vy_rms)),
        ("energy_by_half_second_j", Value::Null),
    ]);

    drop(bed); // Releases the boot lease the next scenario takes.

    // The kick: parked on level ground, a pitch and heave impulse; 4 s with no input.
    let mut bed = Bed::on_ground(Some(ground), flat());
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    for _ in 0..TICKS {
        bed.tick(0.0, 0.0);
    }
    {
        let mut e = bed.app.world_mut().entity_mut(bed.tank);
        let rotation = e.get::<Rotation>().expect("rotation").0;
        // Fixture: 0.3 rad/s about the hull's lateral axis and 0.3 m/s upward.
        e.get_mut::<AngularVelocity>().expect("spin").0 = rotation * Vec3::X * 0.3;
        e.get_mut::<LinearVelocity>().expect("velocity").0 = Vec3::Y * 0.3;
    }
    let kick = bed.kinetic_energy();
    let half = TICKS / 2;
    let mut peaks = [0.0_f32; 8];
    for tick in 0..8 * half {
        bed.tick(0.0, 0.0);
        peaks[tick / half] = peaks[tick / half].max(bed.kinetic_energy());
    }
    let passive = peaks[7] < peaks[0] && peaks[6] < peaks[1];
    rows.push(vec![
        ("case", json!("parked, pitch + heave kick")),
        ("samples", json!(8 * half)),
        ("speed_m_s", f(0.0)),
        ("pitch_rms_deg", Value::Null),
        ("roll_rms_deg", Value::Null),
        ("vertical_speed_rms_m_s", Value::Null),
        (
            "energy_by_half_second_j",
            json!(format!(
                "kick {kick:.1}; peaks {}",
                peaks
                    .iter()
                    .map(|p| format!("{p:.2}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        ),
    ]);
    card.push(Line {
        id: "ride",
        title: "7. Ride, and energy decay after a kick",
        kind: Kind::Law,
        pass: Some(passive),
        rows,
        note: "Washboard (observation): RMS over the ticks the belt spans the bars. Kick (law \
               check, passivity): parked with no input the system takes no energy in, so the \
               peak hull kinetic energy in the last second must fall below the first second's \
               (each of the last two half-second peaks below its counterpart). The decay rate is \
               an observation."
            .into(),
    });
}

/// Line 8. Rubble: drive onto a pile of pieces and park on it.
fn rubble(card: &mut Card, work_rows: &mut Vec<Row>) {
    let ground = base_ground();
    let mut bed = Bed::on_ground(Some(ground), flat());
    let front = front_z(bed.geom());
    // Fixture: 0.8 × 0.5 × 0.6 m pieces (cube root of volume 0.62 m, above the 0.5 m floor), a
    // 5 × 6 bottom layer and a 4 × 5 top layer, the pile's near edge 6 m ahead.
    let size = Vec3::new(0.8, 0.5, 0.6);
    let gap = 0.05;
    let near = front - 6.0;
    for (layer, (nx, nz)) in [(5, 6), (4, 5)].into_iter().enumerate() {
        for i in 0..nx {
            for k in 0..nz {
                let x = (i as f32 - (nx - 1) as f32 / 2.0) * (size.x + gap);
                let z = near - (layer as f32 * 0.5 + k as f32) * (size.z + gap) - size.z / 2.0;
                let y = size.y * (layer as f32 + 0.5) + 0.002;
                bed.spawn_piece(Vec3::new(x, y, z), size);
            }
        }
    }
    let pile_centre = near - 3.0 * (size.z + gap);
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    // Let the pile settle with the tank parked, clear of it.
    for _ in 0..TICKS {
        bed.tick(0.0, 0.0);
    }
    let begin = bed.hull().position;
    let mut drive_piece_speed = 0.0_f32;
    let mut reached = f32::NAN;
    for tick in 0..15 * TICKS {
        bed.tick(0.6, 0.0);
        drive_piece_speed = drive_piece_speed.max(bed.max_piece_speed());
        if bed.hull().position.z < pile_centre {
            reached = tick as f32 * DT;
            break;
        }
    }
    // Brake to a stop, then park.
    for _ in 0..2 * TICKS {
        let moving = bed.hull().speed() > crate::track::transmission::PARK_ENGAGE_SPEED;
        bed.tick(if moving { -1.0 } else { 0.0 }, 0.0);
    }
    let (mut ys, mut vys, mut sinks, mut ratios) = (vec![], vec![], vec![], vec![]);
    let mut park_piece_speed = 0.0_f32;
    let mut tank_support = vec![];
    let mut work: std::collections::BTreeMap<String, f64> = Default::default();
    bed.take_work();
    let park = 4 * TICKS;
    for _ in 0..park {
        bed.tick(0.0, 0.0);
        let hull = bed.hull();
        ys.push(hull.position.y - begin.y);
        vys.push(hull.velocity.y);
        sinks.push(bed.sink_into_pieces());
        park_piece_speed = park_piece_speed.max(bed.max_piece_speed());
        let from_pieces: f32 = bed
            .world_forces()
            .iter()
            .filter(|f| f.body.is_some_and(|b| bed.pieces.contains(&b)))
            .map(|f| f.force.y)
            .sum();
        tank_support.push(from_pieces);
        let pressed = bed.world_support_on_pieces() - bed.piece_weight();
        if from_pieces > 0.0 {
            ratios.push(pressed / from_pieces);
        }
        accumulate_work(&bed, &mut work);
    }
    let (rise, jitter) = mean_rms(&ys);
    let (_, vy_rms) = mean_rms(&vys);
    let (sink, _) = mean_rms(&sinks);
    let sink_max = sinks.iter().copied().fold(0.0, f32::max);
    let (support, _) = mean_rms(&tank_support);
    let (ratio, ratio_rms) = mean_rms(&ratios);
    let weight = bed.weight();
    // Resolution: the reaction law holds to 5 % of the load, a margin for solver iteration error.
    let symmetric = !ratios.is_empty() && (ratio - 1.0).abs() < 0.05;
    let row = vec![
        ("reached_pile_s", f(reached)),
        ("hull_rise_m", f(rise)),
        ("vertical_jitter_rms_m", f(jitter)),
        ("vertical_speed_rms_m_s", f(vy_rms)),
        ("sink_mean_m", f(sink)),
        ("sink_max_m", f(sink_max)),
        ("piece_speed_max_drive_m_s", f(drive_piece_speed)),
        ("piece_speed_max_parked_m_s", f(park_piece_speed)),
        ("support_from_pieces_share_of_weight", f(support / weight)),
        ("reaction_ratio", f(ratio)),
        ("reaction_ratio_rms", f(ratio_rms)),
        ("reaction_symmetric", json!(symmetric)),
    ];
    work_rows.push(work_row("parked on the rubble pile", &work, park));
    card.push(Line {
        id: "rubble",
        title: "8. Rubble",
        kind: Kind::Law,
        pass: arm::CLAIMS_PIECE_REACTION.then_some(symmetric),
        rows: vec![row],
        note: format!(
            "{} pieces of {FIXTURE_SUBSTANCE} (shipped density and friction). Drive at 0.6 \
             throttle until the hull centre is over the pile, brake, park 4 s; parked values \
             are over those 4 s. Rise is against the hull's start on level ground. Sink is the \
             rest-pose belt bottom's depth inside pieces. The reaction ratio is (world support \
             on the pieces − their weight) / (the pieces' support on the hull): 1 when every \
             piece under load takes the equal and opposite reaction, 0 when the pieces hold the \
             tank up unloaded. It is a law check only where the arm claims it (this arm: {}).",
            bed.pieces.len(),
            if arm::CLAIMS_PIECE_REACTION {
                "claims it"
            } else {
                "does not"
            }
        ),
    });
}

fn accumulate_work(bed: &Bed, work: &mut std::collections::BTreeMap<String, f64>) {
    for (name, count) in bed.take_work() {
        *work.entry(name).or_default() += count;
    }
    for (name, count) in arm::work(&bed.app, bed.tank) {
        *work.entry(name.to_owned()).or_default() += count;
    }
    let (pairs, manifolds, points) = bed.solver_contacts();
    for (name, count) in [
        ("solver_pairs", pairs),
        ("solver_manifolds", manifolds),
        ("solver_points", points),
    ] {
        *work.entry(name.to_owned()).or_default() += count as f64;
    }
}

fn work_row(case: &str, work: &std::collections::BTreeMap<String, f64>, ticks: usize) -> Row {
    let per_tick = |name: &str| f(work.get(name).copied().unwrap_or(0.0) as f32 / ticks as f32);
    let mut row: Row = vec![("case", json!(case))];
    for name in WORK_COLUMNS {
        row.push((name, per_tick(name)));
    }
    row
}

/// The work columns: the production counts the belt adds, the arm's own, and the solver's.
const WORK_COLUMNS: [&str; 15] = [
    "belt_near_colliders",
    "belt_candidates",
    "belt_slab_checks",
    "belt_slab_aabb_passes",
    "belt_overlaps",
    "belt_dynamic_contacts",
    "belt_recipients",
    "belt_dispatches",
    "belt_triangulations",
    "belt_contacts",
    "belt_force_applications",
    "solver_pairs",
    "solver_manifolds",
    "solver_points",
    "contact_points",
];

/// Line 9, open ground: full throttle on level ground, counted over seconds 5 to 10.
fn work_open_ground(work_rows: &mut Vec<Row>) {
    let mut bed = Bed::on_ground(Some(base_ground()), flat());
    bed.place(Vec3::new(0.0, 0.0, 300.0), Quat::IDENTITY);
    bed.start();
    for _ in 0..5 * TICKS {
        bed.tick(1.0, 0.0);
    }
    bed.take_work();
    let mut work = Default::default();
    for _ in 0..5 * TICKS {
        bed.tick(1.0, 0.0);
        accumulate_work(&bed, &mut work);
    }
    work_rows.insert(0, work_row("driving on open ground", &work, 5 * TICKS));
}

/// The whole card, on whichever track model this branch builds.
#[test]
#[ignore = "the track scorecard, run on demand"]
fn track_scorecard() {
    let mut card = Card::default();
    let mut work_rows = Vec::new();
    slope_hold(&mut card);
    turning(&mut card);
    acceleration(&mut card);
    climbing(&mut card);
    trenches(&mut card);
    curb_and_ledge(&mut card);
    ride(&mut card);
    rubble(&mut card, &mut work_rows);
    work_open_ground(&mut work_rows);
    card.push(Line {
        id: "work",
        title: "9. Work per tank per tick",
        kind: Kind::Observation,
        pass: None,
        rows: work_rows,
        note: "MEASURED counts per tick for the one tank in the world: the production `Work` \
               counters, the arm's own counts (`arm::work`) and Avian's solver contacts with the \
               tank as a party. No timing."
            .into(),
    });
    println!("{}", card.markdown());
    let failed: Vec<&str> = card
        .lines
        .iter()
        .filter(|line| line.pass == Some(false))
        .map(|line| line.id)
        .collect();
    assert!(failed.is_empty(), "law checks failed: {failed:?}");
}
