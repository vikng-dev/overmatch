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
use crate::contact_transfer::ContactTransfer;
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

    /// Every solver contact point of the last physics step, with its mean force on `other` over
    /// the step (N, world, normal and friction together): the game's one reading of what the
    /// solver moved, `contact_transfer::ContactTransfers`.
    fn solver_points(&self) -> Vec<(ContactTransfer, Vec3)> {
        let transfers = self
            .app
            .world()
            .resource::<crate::contact_transfer::ContactTransfers>();
        let dt = transfers.dt();
        transfers
            .solver_groups()
            .flatten()
            .map(|point| (*point, point.impulse / dt))
            .collect()
    }

    /// Every solver contact between the tank and another body this tick, as (that body, the
    /// contact normal, the force on the hull).
    fn tank_solver_forces(&self) -> Vec<(WorldForce, Vec3)> {
        let mut out = Vec::new();
        for (point, force) in self.solver_points() {
            // The transfer moves `force` into `other`; the reference takes its opposite.
            let (other, force) = if point.other == self.tank {
                (point.reference, force)
            } else if point.reference == self.tank {
                (point.other, -force)
            } else {
                continue;
            };
            let world = WorldForce {
                body: Some(other),
                point: point.point,
                force,
            };
            out.push((world, point.normal));
        }
        out
    }

    /// Every force on the hull from a world body this tick: the arm's own channel plus every
    /// solver contact between the tank and another body.
    fn world_forces(&self) -> Vec<WorldForce> {
        let mut out = arm::belt_forces(&self.app, self.tank);
        out.extend(
            self.tank_solver_forces()
                .into_iter()
                .map(|(force, _)| force),
        );
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

    /// The role of one of the tank's colliders, from what spawned it: the arm names the colliders
    /// it spawns; a remaining collider on the vehicle layer is a hull proxy, spawned from the
    /// tank's authored proxy node, which names it.
    fn role(&self, collider: Entity) -> String {
        let world = self.app.world();
        if let Some(role) = arm::collider_role(world, collider) {
            return role.to_owned();
        }
        let vehicle = world
            .get::<CollisionLayers>(collider)
            .is_some_and(|layers| layers.memberships.has_all(Layer::Vehicle));
        let node = world
            .get::<ChildOf>(collider)
            .and_then(|parent| world.get::<Name>(parent.parent()))
            .map_or_else(String::new, |name| format!(" {}", name.as_str()));
        if vehicle {
            format!("hull proxy{node}")
        } else {
            format!("unclassified tank collider{node}")
        }
    }

    /// The roles of the tank's colliders in solver contact with `body` this tick.
    fn touching_colliders(&self, body: Entity) -> Vec<String> {
        let mut out = Vec::new();
        for (point, _) in self.solver_points() {
            if point.reference == self.tank && point.other == body {
                out.push(self.role(point.reference_collider));
            } else if point.other == self.tank && point.reference == body {
                let collider = point
                    .other_collider
                    .expect("a solver contact names both of its colliders");
                out.push(self.role(collider));
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// What of the tank meets `body` this tick: `belt` for the arm's own channel, then the roles
    /// of the solver colliders; `None` while nothing does.
    fn first_touch(&self, body: Entity) -> Option<String> {
        let belt = arm::belt_forces(&self.app, self.tank)
            .iter()
            .any(|f| f.body == Some(body));
        let mut parts = self.touching_colliders(body);
        if belt {
            parts.insert(0, "belt".to_owned());
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    fn awake_pieces(&self) -> Vec<Entity> {
        self.pieces
            .iter()
            .copied()
            .filter(|&p| self.app.world().get::<Sleeping>(p).is_none())
            .collect()
    }

    /// The world's upward support on the awake pieces from everything except the tank and each
    /// other (N). A sleeping piece leaves the solver and reads nothing: whatever it carries, it
    /// carries as if fixed.
    fn world_support_on_pieces(&self) -> f32 {
        let awake = self.awake_pieces();
        let on = |e: Entity| awake.contains(&e);
        let mut up = 0.0;
        for (point, force) in self.solver_points() {
            if point.reference == self.tank || point.other == self.tank {
                continue;
            }
            match (on(point.reference), on(point.other)) {
                (false, true) if !self.pieces.contains(&point.reference) => up += force.y,
                (true, false) if !self.pieces.contains(&point.other) => up -= force.y,
                _ => {} // Between pieces, or with a sleeping piece: internal to the pile.
            }
        }
        up
    }

    fn awake_piece_weight(&self) -> f32 {
        self.awake_pieces()
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
        let step = bed.spawn_static(
            Vec3::new(0.0, (height - 1.0) / 2.0, face - 10.0),
            Vec3::new(10.0, height + 1.0, 20.0),
        );
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        let begin = bed.hull().position;
        let (mut max_pitch, mut max_rise, mut crossed_at) = (0.0_f32, 0.0_f32, f32::NAN);
        let mut first_touch = None;
        for tick in 0..15 * TICKS {
            bed.tick(1.0, 0.0);
            if first_touch.is_none() {
                first_touch = bed.first_touch(step);
            }
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
            ("first_touch", json!(first_touch)),
        ]);
    }

    card.push(Line {
        id: "climbing",
        title: "4. Climbing",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "Full throttle from rest. Steps: face 6 m ahead of the belt's front point, climbed iff \
             the hull centre passes 3 m beyond the face risen by 0.8 × the step within 15 s. \
             Highest step climbed: {highest} m. first_touch names what of the tank met the \
             obstacle on the first tick any did: `belt` for the arm's own channel, then each \
             solver collider's role."
        ),
    });
}

/// Fixture: the walls' height above the ground (m), taller than anything of the tank reaches.
const WALL_HEIGHT: f32 = 4.0;

/// Fixture: how far a one-lane wall reaches past each edge of the shoe (m). It covers the
/// lateral drift of a 2 m approach (line 6 measures under 1 mm over 10 m) with room to spare,
/// and keeps the wall inside the gap between the lanes.
const LANE_MARGIN: f32 = 0.05;

/// The belt lanes, as (low x, high x) in the hull frame, sorted by x: each side's shoe span from
/// its grip columns, widened by [`LANE_MARGIN`].
fn lanes(geom: &RigGeom) -> [(Side, f32, f32); 2] {
    let lane = |side: Side| {
        let xs = geom.grip_columns(side).map(|(x, _)| x);
        let lo = xs.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        (side, lo - LANE_MARGIN, hi + LANE_MARGIN)
    };
    let mut out = [lane(Side::Left), lane(Side::Right)];
    out.sort_by(|a, b| a.1.total_cmp(&b.1));
    out
}

/// One wall push: a wall [`WALL_HEIGHT`] tall and 2 m thick, its face 2 m ahead of the belt's
/// front point, spanning `span` (x low, x high, hull frame; `None` for the full-width wall),
/// pushed at full throttle for 8 s.
fn wall_push(case: &str, span: Option<(f32, f32)>) -> Row {
    let mut bed = Bed::on_ground(Some(base_ground()), flat());
    let front = front_z(bed.geom());
    let face = front - 2.0;
    // Fixture: the full-width wall is 10 m wide, wider than the tank.
    let (lo, hi) = span.unwrap_or((-5.0, 5.0));
    // Sunk 1 m below the ground, so its foot has no edge for the belt to catch.
    let wall = bed.spawn_static(
        Vec3::new((lo + hi) / 2.0, (WALL_HEIGHT - 1.0) / 2.0, face - 1.0),
        Vec3::new(hi - lo, WALL_HEIGHT + 1.0, 2.0),
    );
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    let begin = bed.hull().position;
    let (mut max_pitch, mut min_pitch, mut max_rise) = (0.0_f32, 0.0_f32, 0.0_f32);
    let (mut late_normal_up, mut late_friction_up, mut late_belt_up, mut late_push) =
        (vec![], vec![], vec![], vec![]);
    let (mut peak_normal_up, mut peak_friction_up) = (0.0_f32, 0.0_f32);
    let mut touching = std::collections::BTreeSet::new();
    let mut belt_touch = false;
    let mut first_touch = None;
    for tick in 0..8 * TICKS {
        bed.tick(1.0, 0.0);
        if first_touch.is_none() {
            first_touch = bed.first_touch(wall);
        }
        let hull = bed.hull();
        max_pitch = max_pitch.max(hull.pitch());
        min_pitch = min_pitch.min(hull.pitch());
        max_rise = max_rise.max(hull.position.y - begin.y);
        let (mut normal, mut friction) = (Vec3::ZERO, Vec3::ZERO);
        for (force, n) in bed.tank_solver_forces() {
            if force.body == Some(wall) {
                let along = n * force.force.dot(n);
                normal += along;
                friction += force.force - along;
            }
        }
        let belt: Vec3 = arm::belt_forces(&bed.app, bed.tank)
            .iter()
            .filter(|f| f.body == Some(wall))
            .map(|f| f.force)
            .sum();
        belt_touch |= belt != Vec3::ZERO;
        touching.extend(bed.touching_colliders(wall));
        peak_normal_up = peak_normal_up.max(normal.y);
        peak_friction_up = peak_friction_up.max(friction.y);
        if tick >= 6 * TICKS {
            late_normal_up.push(normal.y);
            late_friction_up.push(friction.y);
            late_belt_up.push(belt.y);
            late_push.push(normal.length());
        }
    }
    let mean = |v: &[f32]| mean_rms(v).0;
    vec![
        ("case", json!(case)),
        ("first_touch", json!(first_touch)),
        ("solver_colliders_over_push", json!(format!("{touching:?}"))),
        ("belt_channel_touched", json!(belt_touch)),
        ("normal_force_last_2s_n", f(mean(&late_push))),
        ("normal_vertical_last_2s_n", f(mean(&late_normal_up))),
        ("normal_vertical_peak_up_n", f(peak_normal_up)),
        ("friction_vertical_last_2s_n", f(mean(&late_friction_up))),
        ("friction_vertical_peak_up_n", f(peak_friction_up)),
        ("belt_channel_vertical_last_2s_n", f(mean(&late_belt_up))),
        ("max_rise_m", f(max_rise)),
        (
            "pitch_range_deg",
            json!(format!(
                "{:.2} .. {:.2}",
                min_pitch.to_degrees(),
                max_pitch.to_degrees()
            )),
        ),
        ("end_z_past_face_m", f(face - bed.hull().position.z)),
    ]
}

/// Line 4b. Walls taller than the run-up (the owner's front-wheel case): one full width, and one
/// a single belt lane wide under each belt, where the drawn geometry decides what meets it.
fn walls(card: &mut Card) {
    let belt_lanes = {
        let bed = Bed::on_ground(Some(base_ground()), flat());
        lanes(bed.geom())
    }; // The bed drops here, releasing the boot lease each push takes.
    let mut rows = vec![wall_push("full width, 10 m", None)];
    for (side, lo, hi) in belt_lanes {
        let case = format!("{side:?} belt lane only, x {lo:.3} .. {hi:.3} m");
        rows.push(wall_push(&case, Some((lo, hi))));
    }
    card.push(Line {
        id: "walls",
        title: "4b. Walls: full width, and one belt lane wide",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "A wall {WALL_HEIGHT} m tall and 2 m thick, its face 2 m ahead of the belt's front point, pushed \
             at full throttle from rest for 8 s. The one-lane walls span one shoe's width \
             (its grip columns) plus {LANE_MARGIN} m each side, centred on that belt. The \
             wall's force on the hull is split per solver contact into the part along the \
             contact normal and the friction across it; vertical parts are upward positive, \
             means over the last 2 s, peaks the largest upward value. The belt channel's \
             force is the arm's own, unsplit. first_touch names what of the tank met the wall \
             on the first tick any did; pitch is nose-up positive."
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
        let (mut beyond_at, mut beyond_rise) = (f32::NAN, f32::NAN);
        for tick in 0..15 * TICKS {
            bed.tick(1.0, 0.0);
            let hull = bed.hull();
            sink = sink.max(begin.y - hull.position.y);
            max_pitch = max_pitch.max(hull.pitch());
            min_pitch = min_pitch.min(hull.pitch());
            if hull.position.z < far - 6.0 {
                beyond_at = tick as f32 * DT;
                beyond_rise = hull.position.y - begin.y;
                break;
            }
        }
        let crossed = beyond_rise > -0.5 * top;
        rows.push(vec![
            ("width_m", f(width)),
            ("width_over_span", f(share)),
            ("crossed", json!(crossed)),
            ("time_s", f(beyond_at)),
            ("rise_6m_beyond_m", f(beyond_rise)),
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
             plateaus; full throttle from rest, the near edge 6 m ahead. Crossed iff, when the \
             hull centre first passes 6 m beyond the far edge (within 15 s), it is within half \
             the depth of its start level. Sink is the hull's deepest drop below its start."
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

/// Fixture: 0.8 × 0.5 × 0.6 m pieces (cube root of volume 0.62 m, above the 0.5 m size floor).
const PIECE: Vec3 = Vec3::new(0.8, 0.5, 0.6);
const PIECE_GAP: f32 = 0.05;

/// A two-layer pile ahead of the tank: a 5 × 6 bottom layer and a 4 × 5 top layer, its near edge
/// 6 m ahead of the belt's front point. Returns the near edge and the pile's centre (z).
fn pile_ahead(bed: &mut Bed) -> (f32, f32) {
    let near = front_z(bed.geom()) - 6.0;
    for (layer, (nx, nz)) in [(5, 6), (4, 5)].into_iter().enumerate() {
        for i in 0..nx {
            for k in 0..nz {
                let x = (i as f32 - (nx - 1) as f32 / 2.0) * (PIECE.x + PIECE_GAP);
                let z =
                    near - (layer as f32 * 0.5 + k as f32) * (PIECE.z + PIECE_GAP) - PIECE.z / 2.0;
                let y = PIECE.y * (layer as f32 + 0.5) + 0.002;
                bed.spawn_piece(Vec3::new(x, y, z), PIECE);
            }
        }
    }
    (near, near - 3.0 * (PIECE.z + PIECE_GAP))
}

/// One layer of pieces under each track's ground run, the tank parked on top; returns the
/// support level (m). Each row is centred on its track's shoe and spans its road wheels.
fn pile_under(bed: &mut Bed) -> f32 {
    let geom = bed.geom();
    let rows: Vec<(f32, f32, f32)> = Side::ALL
        .iter()
        .map(|&side| {
            let wheels = geom.road_wheels.get(side);
            let (lo, hi) = wheels
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), w| {
                    (lo.min(w.z), hi.max(w.z))
                });
            (geom.grip_columns(side)[1].0, lo, hi)
        })
        .collect();
    for (x, lo, hi) in rows {
        let count = ((hi - lo) / (PIECE.z + PIECE_GAP)).ceil() as usize + 1;
        let start = (lo + hi) / 2.0 - (count - 1) as f32 * (PIECE.z + PIECE_GAP) / 2.0;
        for k in 0..count {
            let z = start + k as f32 * (PIECE.z + PIECE_GAP);
            bed.spawn_piece(Vec3::new(x, PIECE.y / 2.0 + 0.002, z), PIECE);
        }
    }
    PIECE.y + 0.002
}

/// Parked 4 s on pieces: the row's parked measures, and the work over those ticks.
fn park_on_pieces(
    bed: &mut Bed,
    begin_y: f32,
    work: &mut std::collections::BTreeMap<String, f64>,
) -> Row {
    let (mut ys, mut vys, mut sinks, mut ratios) = (vec![], vec![], vec![], vec![]);
    let (mut tank_support, mut belt_support) = (vec![], vec![]);
    let mut park_piece_speed = 0.0_f32;
    let mut asleep_under_belt = 0;
    bed.take_work();
    let park = 4 * TICKS;
    for _ in 0..park {
        bed.tick(0.0, 0.0);
        let hull = bed.hull();
        ys.push(hull.position.y - begin_y);
        vys.push(hull.velocity.y);
        sinks.push(bed.sink_into_pieces());
        park_piece_speed = park_piece_speed.max(bed.max_piece_speed());
        let on_pieces = |f: &&WorldForce| f.body.is_some_and(|b| bed.pieces.contains(&b));
        let from_pieces: f32 = bed
            .world_forces()
            .iter()
            .filter(on_pieces)
            .map(|f| f.force.y)
            .sum();
        let belt = arm::belt_forces(&bed.app, bed.tank);
        belt_support.push(
            belt.iter()
                .filter(on_pieces)
                .map(|f| f.force.y)
                .sum::<f32>(),
        );
        let asleep: std::collections::BTreeSet<Entity> = belt
            .iter()
            .filter_map(|f| f.body)
            .filter(|b| bed.pieces.contains(b) && bed.app.world().get::<Sleeping>(*b).is_some())
            .collect();
        asleep_under_belt = asleep_under_belt.max(asleep.len());
        tank_support.push(from_pieces);
        let pressed = bed.world_support_on_pieces() - bed.awake_piece_weight();
        if from_pieces > 0.0 {
            ratios.push(pressed / from_pieces);
        }
        accumulate_work(bed, work);
    }
    let (rise, jitter) = mean_rms(&ys);
    let (_, vy_rms) = mean_rms(&vys);
    let (sink, _) = mean_rms(&sinks);
    let (support, _) = mean_rms(&tank_support);
    let (belt, _) = mean_rms(&belt_support);
    let (ratio, ratio_rms) = mean_rms(&ratios);
    // Resolution: the reaction law holds to 5 % of the load, a margin for solver iteration error.
    let symmetric = !ratios.is_empty() && (ratio - 1.0).abs() < 0.05;
    vec![
        ("hull_rise_m", f(rise)),
        ("vertical_jitter_rms_m", f(jitter)),
        ("vertical_speed_rms_m_s", f(vy_rms)),
        ("sink_mean_m", f(sink)),
        ("sink_max_m", f(sinks.iter().copied().fold(0.0, f32::max))),
        ("piece_speed_max_parked_m_s", f(park_piece_speed)),
        (
            "support_from_pieces_share_of_weight",
            f(support / bed.weight()),
        ),
        ("belt_channel_share_of_that", f(belt / support)),
        ("belt_loaded_pieces_asleep_max", json!(asleep_under_belt)),
        ("reaction_ratio", f(ratio)),
        ("reaction_ratio_rms", f(ratio_rms)),
        ("reaction_symmetric", json!(symmetric)),
    ]
}

/// Line 8. Rubble: a pile driven onto at two throttles, and a layer under the parked tank.
fn rubble(card: &mut Card, work_rows: &mut Vec<Row>) {
    let ground = base_ground();
    let mut rows = Vec::new();
    let mut symmetric = true;
    let mut control = Vec::new();
    for throttle in [0.6, 1.0] {
        let mut bed = Bed::on_ground(Some(ground), flat());
        let (near, centre) = pile_ahead(&mut bed);
        bed.place(Vec3::ZERO, Quat::IDENTITY);
        bed.start();
        // Let the pile settle with the tank parked clear of it. The reader's control: unloaded,
        // the world's support on the awake pieces is their weight.
        for tick in 0..2 * TICKS {
            bed.tick(0.0, 0.0);
            let weight = bed.awake_piece_weight();
            if tick >= TICKS / 4 && weight > 0.0 {
                control.push(bed.world_support_on_pieces() / weight);
            }
        }
        let begin = bed.hull().position;
        let mut drive_piece_speed = 0.0_f32;
        let mut reached = f32::NAN;
        for tick in 0..15 * TICKS {
            bed.tick(throttle, 0.0);
            drive_piece_speed = drive_piece_speed.max(bed.max_piece_speed());
            if bed.hull().position.z < centre {
                reached = tick as f32 * DT;
                break;
            }
        }
        let advanced = near - bed.hull().position.z;
        // Brake to a stop, then park.
        for _ in 0..2 * TICKS {
            let moving = bed.hull().speed() > crate::track::transmission::PARK_ENGAGE_SPEED;
            bed.tick(if moving { -1.0 } else { 0.0 }, 0.0);
        }
        let mut work = Default::default();
        let mut row: Row = vec![
            ("case", json!(format!("pile ahead, {throttle} throttle"))),
            ("reached_pile_centre_s", f(reached)),
            ("hull_centre_past_near_edge_m", f(advanced)),
            ("piece_speed_max_drive_m_s", f(drive_piece_speed)),
        ];
        row.extend(park_on_pieces(&mut bed, begin.y, &mut work));
        symmetric &= row
            .iter()
            .any(|(k, v)| *k == "reaction_symmetric" && *v == json!(true));
        work_rows.push(work_row(
            &format!("parked on the pile ahead, {throttle} throttle"),
            &work,
            4 * TICKS,
        ));
        rows.push(row);
    }

    let mut bed = Bed::on_ground(Some(ground), flat());
    let level = pile_under(&mut bed);
    bed.place(Vec3::new(0.0, level, 0.0), Quat::IDENTITY);
    bed.start();
    let begin_y = bed.hull().position.y - level;
    let mut work = Default::default();
    let mut row: Row = vec![
        ("case", json!("a layer under the parked tank's tracks")),
        ("reached_pile_centre_s", Value::Null),
        ("hull_centre_past_near_edge_m", Value::Null),
        ("piece_speed_max_drive_m_s", Value::Null),
    ];
    row.extend(park_on_pieces(&mut bed, begin_y, &mut work));
    symmetric &= row
        .iter()
        .any(|(k, v)| *k == "reaction_symmetric" && *v == json!(true));
    work_rows.push(work_row(
        "parked on a layer under its tracks",
        &work,
        4 * TICKS,
    ));
    rows.push(row);
    let (control, control_rms) = mean_rms(&control);
    card.push(Line {
        id: "rubble",
        title: "8. Rubble",
        kind: Kind::Law,
        pass: arm::CLAIMS_PIECE_REACTION.then_some(symmetric),
        rows,
        note: format!(
            "Pieces of {FIXTURE_SUBSTANCE} (shipped density and friction), {PIECE} m. Pile ahead: \
             5 × 6 under 4 × 5, near edge 6 m ahead of the belt's front point; drive until the \
             hull centre is over the pile's centre (15 s at most), brake, park 4 s. Layer under: \
             one row under each track's road wheels, the tank placed on it at rest, parked 4 s. \
             Parked values are over those 4 s; rise is against the hull's rest level on the \
             ground. Sink is the rest-pose belt bottom's depth inside pieces. The reaction ratio \
             is (world support on the awake pieces − their weight) / (the pieces' support on the \
             hull): 1 when every piece under load takes the equal and opposite reaction, 0 when \
             the pieces hold the tank up unloaded. A sleeping piece leaves the solver: the \
             count of belt-loaded pieces asleep says how many hold the tank as if fixed. The \
             reader's control, the settling pile's world support over its awake weight: \
             {control:.4} (RMS {control_rms:.4}), 1 when the reader is right. The ratio is a law \
             check only where the arm claims it (this arm: {}).",
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
/// Fixture: the step heights the reach columns are read at (m above the parked tank's ground).
const REACH_HEIGHTS: [(&str, f32); 6] = [
    ("reach_below_0.2_m", 0.2),
    ("reach_below_0.4_m", 0.4),
    ("reach_below_0.6_m", 0.6),
    ("reach_below_0.8_m", 0.8),
    ("reach_below_1.0_m", 1.0),
    ("reach_below_1.2_m", 1.2),
];

/// The most forward hull-frame z (forward is −z) of a convex part's points at or below height
/// `h`: a vertex below it, or an edge between two vertices crossing it. Every pair is tried, so
/// the answer is the section of the part's convex hull.
fn reach_below(part: &[Vec3], h: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    let mut keep = |z: f32| best = Some(best.map_or(z, |b| b.min(z)));
    for (i, a) in part.iter().enumerate() {
        if a.y <= h {
            keep(a.z);
        }
        for b in &part[i + 1..] {
            if (a.y - h) * (b.y - h) < 0.0 {
                keep(a.z + (h - a.y) / (b.y - a.y) * (b.z - a.z));
            }
        }
    }
    best
}

/// One collider part of a role: its points (hull frame, heights above the ground) and, for a
/// spawned collider, its shape and pose on the body.
type Part = (Vec<Vec3>, Option<(Collider, ColliderTransform)>);

/// Line 0's lateral strips against the belt lanes, in [`forward_by_region`]'s order.
const REGIONS: [&str; 3] = [
    "forward_between_lanes",
    "forward_in_lanes",
    "forward_outboard",
];

/// Resolution: the spacing of line 0's ray grid (m), a small fraction of a shoe's width.
const RAY_GRID_M: f32 = 0.02;

/// The most forward point, as (z, height), of a collider part in each lateral strip (between
/// the lanes, in them, outboard of them), from a grid of rays cast rearward (+z) at heights up
/// to [`WALL_HEIGHT`] above the ground.
fn forward_by_region(
    collider: &Collider,
    at: &ColliderTransform,
    points: &[Vec3],
    ground: f32,
    lanes: &[(Side, f32, f32); 2],
) -> [Option<(f32, f32)>; 3] {
    let region = |x: f32| {
        if lanes.iter().any(|&(_, lo, hi)| (lo..=hi).contains(&x)) {
            1
        } else if x > lanes[0].2 && x < lanes[1].1 {
            0
        } else {
            2
        }
    };
    let lo = points.iter().fold(Vec3::INFINITY, |a, &p| a.min(p));
    let hi = points.iter().fold(Vec3::NEG_INFINITY, |a, &p| a.max(p));
    let bottom = lo.y.max(0.0);
    let columns = ((hi.x - lo.x) / RAY_GRID_M).ceil() as usize;
    let rows = ((hi.y.min(WALL_HEIGHT) - bottom).max(0.0) / RAY_GRID_M).ceil() as usize;
    let mut best = [None; 3];
    for i in 0..=columns {
        let x = (lo.x + i as f32 * RAY_GRID_M).min(hi.x);
        for j in 0..=rows {
            let h = bottom + j as f32 * RAY_GRID_M;
            let origin = Vec3::new(x, h + ground, lo.z - 1.0);
            let reach = hi.z - lo.z + 2.0;
            // A ray beside the part's outline misses it: nothing of the part at that (x, h).
            let Some((distance, _)) =
                collider.cast_ray(at.translation, at.rotation, origin, Vec3::Z, reach, true)
            else {
                continue;
            };
            let z = origin.z + distance;
            let slot: &mut Option<(f32, f32)> = &mut best[region(x)];
            if slot.is_none_or(|(best_z, _)| z < best_z) {
                *slot = Some((z, h));
            }
        }
    }
    best
}

/// Line 0. Where the tank's colliders sit against its belt's front wrap: construction facts in
/// the parked hull's frame (forward is −z), heights above the flat ground it rests on.
fn geometry(card: &mut Card) {
    let mut bed = Bed::on_ground(Some(base_ground()), flat());
    bed.place(Vec3::ZERO, Quat::IDENTITY);
    bed.start();
    for _ in 0..2 * TICKS {
        bed.tick(0.0, 0.0);
    }
    let hull = bed.hull();
    // The flat ground is world y = 0: the point under the hull origin, in the hull frame.
    let ground = (hull.rotation.inverse() * Vec3::new(0.0, -hull.position.y, 0.0)).y;
    let (pitch, _, _) = hull.rotation.to_euler(EulerRot::XYZ);
    let lanes = lanes(bed.geom());
    let mut parts: std::collections::BTreeMap<String, Vec<Part>> = Default::default();
    let colliders: Vec<(Entity, Part)> = {
        let world = bed.app.world_mut();
        world
            .query::<(
                Entity,
                &Collider,
                &ColliderOf,
                &ColliderTransform,
                &CollisionLayers,
            )>()
            .iter(world)
            // A collider with no filters (the ballistic volumes) meets nothing in the world.
            .filter(|(_, _, of, _, layers)| {
                of.body == bed.tank && layers.filters != LayerMask::NONE
            })
            .map(|(entity, collider, _, at, _)| {
                let shape = collider.shape();
                let local: Vec<Vec3> = if let Some(hull) = shape.as_convex_polyhedron() {
                    hull.points().to_vec()
                } else {
                    let half = shape
                        .as_cuboid()
                        .expect("a colliding tank collider is a convex hull or a cuboid")
                        .half_extents;
                    (0..8)
                        .map(|i| {
                            half * Vec3::new(
                                [-1.0, 1.0][i & 1],
                                [-1.0, 1.0][(i >> 1) & 1],
                                [-1.0, 1.0][i >> 2],
                            )
                        })
                        .collect()
                };
                let points = local
                    .into_iter()
                    .map(|p| {
                        let p = at.transform_point(p);
                        Vec3::new(p.x, p.y - ground, p.z)
                    })
                    .collect();
                (entity, (points, Some((collider.clone(), *at))))
            })
            .collect()
    };
    for (entity, part) in colliders {
        parts.entry(bed.role(entity)).or_default().push(part);
    }
    // The belt's rest outline, per side: every route circle at its outer face.
    let geom = bed.geom();
    let outer = geom.model.pin_to_outer;
    for side in Side::ALL {
        let outline = geom
            .rest
            .get(side)
            .iter()
            .flat_map(|&(centre, radius)| {
                (0..64).map(move |step| {
                    let angle = std::f32::consts::TAU * step as f32 / 64.0;
                    Vec3::new(
                        0.0,
                        centre.y + (radius + outer) * angle.sin(),
                        centre.x + (radius + outer) * angle.cos(),
                    )
                })
            })
            .map(|p| Vec3::new(p.x, p.y - ground, p.z))
            .collect();
        parts
            .entry("belt outline (rest pose)".into())
            .or_default()
            .push((outline, None));
    }
    let rows = parts
        .iter()
        .map(|(role, parts)| {
            let all = || parts.iter().flat_map(|(points, _)| points);
            let nose = all()
                .min_by(|a, b| a.z.total_cmp(&b.z))
                .expect("a part has points");
            let low = all()
                .min_by(|a, b| a.y.total_cmp(&b.y))
                .expect("a part has points");
            let mut row: Row = vec![
                ("collider", json!(role)),
                ("parts", json!(parts.len())),
                ("nose_z_m", f(nose.z)),
                ("nose_height_m", f(nose.y)),
                ("lowest_height_m", f(low.y)),
                ("lowest_at_z_m", f(low.z)),
            ];
            let mut forward = [None; 3];
            for (points, collider) in parts {
                let part = match collider {
                    Some((collider, at)) => forward_by_region(collider, at, points, ground, &lanes),
                    // The belt outline is the lanes' own content.
                    None => [None, Some((nose.z, nose.y)), None],
                };
                for (best, found) in forward.iter_mut().zip(part) {
                    if let Some(found) = found
                        && best.is_none_or(|(z, _): (f32, f32)| found.0 < z)
                    {
                        *best = Some(found);
                    }
                }
            }
            for (key, found) in REGIONS.into_iter().zip(forward) {
                row.push((
                    key,
                    found.map_or(Value::Null, |(z, h)| json!(format!("{z:.3} at {h:.2} m"))),
                ));
            }
            for (key, h) in REACH_HEIGHTS {
                let reach = parts
                    .iter()
                    .filter_map(|(points, _)| reach_below(points, h))
                    .reduce(f32::min);
                row.push((key, reach.map_or(Value::Null, f)));
            }
            row
        })
        .collect();
    card.push(Line {
        id: "geometry",
        title: "0. Where the tank's colliders sit",
        kind: Kind::Observation,
        pass: None,
        rows,
        note: format!(
            "Construction facts of the spawned tank, parked 2 s on flat ground (pitch \
             {:.2}°, the ground at hull-frame y {ground:.4} m): every collider on the hull body, by role, in the hull frame (forward is \
             −z), heights above the ground under the hull origin. Nose: the most forward \
             vertex. Reach below h: the most forward point of the role at or below height h, \
             the first thing of the tank a step of that height meets on a level approach \
             (null: nothing of it is that low). Forward between, in and outboard of the lanes: \
             the most forward point (z at height) of the role at heights up to {WALL_HEIGHT} m \
             within that lateral strip, the first thing of the tank a wall over that strip \
             meets; the lanes are each shoe's span plus {LANE_MARGIN} m a side ({}), as line \
             4b's one-lane walls. The belt outline is every route circle of the rest pose at the \
             belt's outer face.",
            pitch.to_degrees(),
            lanes
                .iter()
                .map(|(side, lo, hi)| format!("{side:?} x {lo:.3} .. {hi:.3} m"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    });
}

#[test]
#[ignore = "the track scorecard, run on demand"]
fn track_scorecard() {
    let mut card = Card::default();
    let mut work_rows = Vec::new();
    geometry(&mut card);
    slope_hold(&mut card);
    turning(&mut card);
    acceleration(&mut card);
    climbing(&mut card);
    walls(&mut card);
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
