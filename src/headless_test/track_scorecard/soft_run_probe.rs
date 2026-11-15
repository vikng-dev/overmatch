//! The soft run's probe, run on demand (the game's tank driving physics on loose construction
//! pieces; game state only). It drives the shipped tank, as the game builds it, onto, over and
//! about `rubble_climb`'s settled house heap, where loose pieces meet one soft run per side and
//! the stop (`track::soft_run`), and reads how the hull rides and how the pieces move.
//!
//! Run: `scripts/serial-cargo test --profile perf --lib --features dev_ui,bitprobe
//! soft_run_probe -- --ignored --nocapture`, one `SOFTRUN` JSON line per start, [`SEEDS`] starts
//! per case.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::rubble_climb::{APPROACH_M, Case, Heap, Mode, live_pieces, settle};
use super::*;
use crate::debris::DebrisPiece;
use crate::track::soft_run::{Face, SoftRun};
use crate::track::stop_contacts::BeltFace;
use crate::track::terrain::FieldSurface;

/// Fixture: the pull and the pivot hold their command as `belt_friction`'s house cases do (s).
const PULL_S: usize = 4;
const PIVOT_S: usize = 8;
/// Fixture: the straight drive's bound, `rubble_climb`'s drive (s).
const STRAIGHT_S: usize = 25;
/// Fixture: the creep's throttle and its bound (s).
const CREEP_THROTTLE: f32 = 0.3;
const CREEP_S: usize = 40;
/// Fixture: how long a stopped tank may take to sleep before the start reads it as awake (s).
const SLEEP_BOUND_S: usize = 15;
/// Fixture: `belt_friction::house_bed`'s rest speed and approach bounds.
const REST_M_S: f32 = 0.05;
const APPROACH_S: usize = 10;
const REST_S: usize = 15;
/// Fixture: starts per case, each its own nudge of the placement ([`nudge`]).
const SEEDS: u32 = 12;
/// Resolution: worlds stepped at once, one per performance core the probe leaves the machine.
const THREADS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Drive {
    /// `belt_friction`'s house cases: onto the heap's middle, parked, rested, then the command.
    Pull,
    Pivot,
    /// Through the heap at full throttle until the belts' rear clears its far edge.
    Straight,
    /// The same at [`CREEP_THROTTLE`].
    Creep,
}

impl Drive {
    fn label(self) -> &'static str {
        match self {
            Self::Pull => "house-pull",
            Self::Pivot => "house-pivot",
            Self::Straight => "straight",
            Self::Creep => "creep",
        }
    }
}

/// Seed `seed`'s shift of the tank's placement after the heap settles: none for seed 0, else
/// sideways and along each within half a millimetre, spread by the golden-ratio sequences.
fn nudge(seed: u32) -> Vec3 {
    let spread = |step: f64| ((f64::from(seed) * step).fract() - 0.5) as f32 * 1e-3;
    match seed {
        0 => Vec3::ZERO,
        _ => Vec3::new(spread(0.618_034), 0.0, spread(0.754_878)),
    }
}

/// The runs' volumes in the hull frame, as the game built them on the tank: each side's outline in
/// `(z, y)`, counter-clockwise, and the shoe's lateral bounds.
struct Runs {
    sides: Vec<(Vec<Vec2>, f32, f32)>,
    /// Points on the runs' surface, hull frame: every outline edge at [`BELT_SAMPLE_M`] or finer,
    /// at the shoe's two edges and its middle.
    surface: Vec<Vec3>,
}

/// Resolution: the spacing of a run surface's sample points (m), a tenth of the 0.5 m size floor of
/// a loose piece, so a piece's overlap with the belt is read to within its curvature there.
const BELT_SAMPLE_M: f32 = 0.05;

/// A closed polygon's edges, the last vertex back to the first.
fn edges(outline: &[Vec2]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    outline
        .iter()
        .copied()
        .zip(outline.iter().copied().cycle().skip(1))
}

impl Runs {
    fn new(bed: &Bed) -> Self {
        let world = bed.app.world();
        let sides: Vec<_> = world
            .get::<RigidBodyColliders>(bed.tank)
            .expect("a tank carries its colliders")
            .iter()
            .filter_map(|collider| Some((collider, world.get::<SoftRun>(collider)?)))
            .map(|(collider, run)| {
                let side = world
                    .get::<BeltFace>(collider)
                    .expect("a run carries its side's belt face")
                    .side;
                let mut outline = run.outline();
                if edges(&outline).map(|(a, b)| a.perp_dot(b)).sum::<f32>() < 0.0 {
                    outline.reverse();
                }
                let cols = bed.geom().grip_columns(side);
                (outline, cols[0].0.min(cols[2].0), cols[0].0.max(cols[2].0))
            })
            .collect();
        assert_eq!(sides.len(), 2, "a driving tank hangs a run on each side");
        let mut surface = Vec::new();
        for (outline, x0, x1) in &sides {
            for (a, b) in edges(outline) {
                let steps = (a.distance(b) / BELT_SAMPLE_M).ceil().max(1.0) as usize;
                for k in 0..steps {
                    let q = a.lerp(b, k as f32 / steps as f32);
                    for x in [*x0, (x0 + x1) / 2.0, *x1] {
                        surface.push(Vec3::new(x, q.y, q.x));
                    }
                }
            }
        }
        Self { sides, surface }
    }

    /// Whether the hull-frame point `local` lies inside either run's volume.
    fn inside(&self, local: Vec3) -> bool {
        let p = Vec2::new(local.z, local.y);
        self.sides.iter().any(|(outline, x0, x1)| {
            (*x0..=*x1).contains(&local.x)
                && edges(outline).all(|(a, b)| (b - a).perp_dot(p - a) >= 0.0)
        })
    }
}

/// A tank collider's part in meeting loose pieces: the stop (hard, at full bump travel), a run
/// (soft, its depth read from the rest face, which is its datum), or anything else of the hull.
const ROLES: [&str; 3] = ["stop", "run", "hull"];

/// Fixture: a loaded hard point this deep is a contact the narrow phase got wrong, not an overlap
/// the solver let through: half the 0.5 m size floor of a loose piece (m).
const DEEP_M: f32 = 0.25;

fn role(world: &World, collider: Entity) -> usize {
    if world.get::<BumpStop>(collider).is_some() {
        0
    } else if world.get::<SoftRun>(collider).is_some() {
        1
    } else {
        2
    }
}

/// What the tank's colliders and the ground did to the loose pieces over the last step.
#[derive(Clone, Default)]
struct Contact {
    /// Per role, the deepest loaded point in a piece (m), the normal force on loose pieces summed
    /// (N), and the pieces borne on with load.
    pen: [f32; 3],
    force: [f32; 3],
    pieces: [usize; 3],
    /// Per role, the upward part of the normal force loose pieces put on the hull (N). The
    /// friction impulses' part is not read: a manifold keeps no tangent basis after the solve.
    lift: [f32; 3],
    /// Each loaded run contact's depth past the rest face (m).
    run_depths: Vec<f32>,
    /// Loaded hard points (every role but the run) deeper than [`DEEP_M`].
    deep: usize,
    /// The run's touching manifolds by the face `SoftRun::facing` reads them on, and of those the
    /// ones that bore load (a point's normal impulse above zero): rim, rim bearing, cap, cap
    /// bearing. Read at the step's end pose, against the narrow phase's normal from its start.
    faces: [usize; 4],
    /// Each run manifold under a law that bears: the stiffness the secant law it replaced would
    /// hand it (its force at the step's start over its points' mean overlap) over its tangent
    /// stiffness, and that force (N).
    secant: Vec<(f32, f32)>,
    /// The largest normal force the tank put on one piece, mean over the step (N).
    tank_force: f32,
    /// Pieces the tank and the ground both bore on: how many, the deepest of their ground points
    /// (m) and the largest ground normal force on one of them (N).
    squeezed: usize,
    squeeze_pen: f32,
    squeeze_force: f32,
}

/// The step's contacts, and the pieces a track collider bore on.
fn contacts(bed: &Bed) -> (Contact, BTreeSet<Entity>) {
    let world = bed.app.world();
    let piece = |e: Entity| world.get::<DebrisPiece>(e).is_some();
    let mut out = Contact::default();
    let mut touched: [BTreeSet<Entity>; 3] = Default::default();
    let mut on_tank: BTreeMap<Entity, f32> = BTreeMap::new();
    let mut on_ground: BTreeMap<Entity, (f32, f32)> = BTreeMap::new();
    // A point's force is the impulse it bore over the step's last substep, over that substep.
    // `normal_impulse` adds the accumulated impulse at every solve and relax pass, so it reads
    // near twice the impulse applied: the vertical forces on a resting hull summed to up to 1.8
    // times its weight with it (MEASURED); it only marks the points that bore load in the step.
    let substep = DT / world.resource::<SubstepCount>().0 as f32;
    let to_hull = bed.hull().rotation.inverse();
    for pair in world.resource::<ContactGraph>().iter_active_touching() {
        for (collider, sign) in [(pair.collider1, 1.0), (pair.collider2, -1.0)] {
            let Some(run) = world.get::<SoftRun>(collider) else {
                continue; // Only a run reads a manifold on a face.
            };
            for manifold in &pair.manifolds {
                let face = match run.facing(to_hull * (manifold.normal * sign)) {
                    Face::Rim(_) => 0,
                    Face::Cap => 2,
                };
                out.faces[face] += 1;
                if manifold.points.iter().any(|p| p.normal_impulse > 0.0) {
                    out.faces[face + 1] += 1;
                }
                let law = manifold
                    .compliance
                    .expect("a run's manifold carries its law");
                let bearing = || {
                    (manifold.points.iter())
                        .map(|p| p.penetration)
                        .filter(|&o| law.bears(o))
                };
                let force: f32 = bearing().map(|o| law.stiffness * (o + law.rest)).sum();
                let tangent = law.stiffness * bearing().count() as f32;
                let (sum, n) = (manifold.points.iter())
                    .filter(|p| p.penetration > 0.0)
                    .fold((0.0, 0.0), |(s, n), p| (s + p.penetration, n + 1.0));
                if tangent > 0.0 && n > 0.0 {
                    out.secant.push((force / (sum / n) / tangent, force));
                }
            }
        }
        let loaded = || {
            pair.manifolds
                .iter()
                .flat_map(|m| m.points.iter())
                .filter(|p| p.normal_impulse > 0.0)
        };
        let force = loaded().map(|p| p.warm_start_normal_impulse).sum::<f32>() / substep;
        if loaded().next().is_none() {
            continue; // Speculative: it carried nothing.
        }
        let depth = loaded()
            .map(|p| p.penetration)
            .fold(f32::NEG_INFINITY, f32::max);
        // The normal points from collider 1 to collider 2: the solver pushes body 2 along it and
        // body 1 against it.
        let lift = pair
            .manifolds
            .iter()
            .map(|m| {
                let impulse: f32 = m
                    .points
                    .iter()
                    .filter(|p| p.normal_impulse > 0.0)
                    .map(|p| p.warm_start_normal_impulse)
                    .sum();
                m.normal.y * impulse
            })
            .sum::<f32>()
            / substep;
        let one = (pair.collider1, pair.body1.unwrap_or(pair.collider1));
        let two = (pair.collider2, pair.body2.unwrap_or(pair.collider2));
        for (((collider, body), (_, other)), up) in
            [(one, two), (two, one)].into_iter().zip([-lift, lift])
        {
            if !piece(other) {
                continue;
            }
            if body == bed.tank {
                *on_tank.entry(other).or_default() += force;
                let r = role(world, collider);
                out.force[r] += force;
                out.lift[r] += up;
                out.pen[r] = out.pen[r].max(depth);
                touched[r].insert(other);
                if r == 1 {
                    out.run_depths.push(depth);
                } else {
                    out.deep += loaded().filter(|p| p.penetration > DEEP_M).count();
                }
            } else if world.get::<FieldSurface>(collider).is_some() {
                let ground = on_ground.entry(other).or_insert((f32::NEG_INFINITY, 0.0));
                *ground = (ground.0.max(depth), ground.1 + force);
            }
        }
    }
    out.pieces = touched.each_ref().map(BTreeSet::len);
    out.tank_force = on_tank.values().copied().fold(0.0, f32::max);
    for (body, &(depth, force)) in &on_ground {
        if on_tank.contains_key(body) {
            out.squeezed += 1;
            out.squeeze_pen = out.squeeze_pen.max(depth);
            out.squeeze_force = out.squeeze_force.max(force);
        }
    }
    let [stop, run, _] = touched;
    (out, stop.into_iter().chain(run).collect())
}

/// One tick of the hull, the belts and the pieces.
struct Sample {
    position: Vec3,
    /// Hull-frame angular velocity (rad/s): `x` pitch, `y` yaw, `z` roll.
    spin: Vec3,
    /// World vertical acceleration over the tick (m/s²).
    heave: f32,
    belt: [f32; 2],
    speed: f32,
    /// Heading about world up, unwrapped (rad).
    heading: f32,
    awake: usize,
    asleep: bool,
    contact: Contact,
    /// Tracked pieces whose centre lies inside a run's volume ([`Runs::inside`]).
    inside: usize,
    /// The deepest run surface point inside a tracked piece (m).
    clip: f32,
    /// The belts' sampled support this tick (`TrackContacts`, ground and standing solids): normal
    /// load (N), tangent force along the hull's forward axis (N), grip budget `Σ μ·N_elastic` (N)
    /// and load-weighted mean slip speed (m/s).
    support: [f32; 4],
    /// The upward part of the sampled support's force on the hull, normal and tangent (N).
    support_up: f32,
    /// The hull's nose-up pitch (deg).
    pitch: f32,
    /// The drivetrain: gear, engine speed (rpm), and 1 with the main clutch in.
    drivetrain: [f32; 3],
}

/// A piece a track collider bore on, sampled every tick from its first touch.
struct PieceTrack {
    /// The speed under which Avian may put it to rest (m/s).
    rest: f32,
    /// (tick, position, velocity along the hull's forward axis, speed, centre inside a run).
    samples: Vec<(usize, Vec3, f32, f32, bool)>,
}

struct Recorder {
    ticks: Vec<Sample>,
    tracks: BTreeMap<Entity, PieceTrack>,
    origin: Sample,
    runs: Runs,
    /// The tank's weight under the world's gravity (N).
    weight: f32,
}

impl Recorder {
    fn new(bed: &Bed) -> Self {
        let hull = bed.hull();
        let forward = hull.forward();
        let world = bed.app.world();
        Self {
            weight: world
                .get::<ComputedMass>(bed.tank)
                .expect("a dynamic tank carries its mass")
                .value()
                * world.resource::<Gravity>().0.length(),
            runs: Runs::new(bed),
            ticks: Vec::new(),
            tracks: BTreeMap::new(),
            origin: Sample {
                position: hull.position,
                spin: hull.rotation.inverse() * hull.angular,
                heave: 0.0,
                belt: [0.0; 2],
                speed: hull.speed(),
                heading: (-forward.x).atan2(-forward.z),
                awake: bed.awake_pieces().len(),
                asleep: false,
                contact: Contact::default(),
                inside: 0,
                clip: 0.0,
                support: [0.0; 4],
                support_up: 0.0,
                pitch: 0.0,
                drivetrain: [0.0; 3],
            },
        }
    }

    /// The deepest run surface point inside any tracked piece now (m).
    fn clip(&self, world: &World, hull: &Hull) -> f32 {
        let points: Vec<Vec3> = self
            .runs
            .surface
            .iter()
            .map(|&p| hull.position + hull.rotation * p)
            .collect();
        let mut deepest = 0.0_f32;
        for &piece in self.tracks.keys() {
            let Some(colliders) = world.get::<RigidBodyColliders>(piece) else {
                continue; // The piece left the world (despawned or merged).
            };
            for collider in colliders.iter() {
                let (Some(shape), Some(at), Some(rot), Some(aabb)) = (
                    world.get::<Collider>(collider),
                    world.get::<Position>(collider),
                    world.get::<Rotation>(collider),
                    world.get::<ColliderAabb>(collider),
                ) else {
                    continue; // A collider the destruction model removed has no depth.
                };
                for &point in &points {
                    if point.cmplt(aabb.min).any() || point.cmpgt(aabb.max).any() {
                        continue;
                    }
                    let (surface, inside) = shape.project_point(*at, *rot, point, false);
                    if inside {
                        deepest = deepest.max(surface.distance(point));
                    }
                }
            }
        }
        deepest
    }

    fn last(&self) -> &Sample {
        self.ticks.last().unwrap_or(&self.origin)
    }

    fn step(&mut self, bed: &mut Bed, throttle: f32, steer: f32) {
        let before = bed.hull().velocity.y;
        bed.tick(throttle, steer);
        bed.pieces = live_pieces(bed);
        let tick = self.ticks.len();
        let (contact, touched) = contacts(bed);
        let world = bed.app.world();
        for piece in touched {
            self.tracks.entry(piece).or_insert_with(|| PieceTrack {
                rest: world
                    .get::<SleepThreshold>(piece)
                    .expect("a dynamic body carries its sleep threshold")
                    .linear
                    * world.resource::<PhysicsLengthUnit>().0,
                samples: Vec::new(),
            });
        }
        let hull = bed.hull();
        let forward = hull.forward();
        let mut inside = 0;
        for (&piece, track) in &mut self.tracks {
            let (Some(at), Some(v)) = (
                world.get::<Position>(piece),
                world.get::<LinearVelocity>(piece),
            ) else {
                continue; // The piece left the world (despawned or merged): its track ends.
            };
            let within = self
                .runs
                .inside(hull.rotation.inverse() * (at.0 - hull.position));
            inside += usize::from(within);
            track
                .samples
                .push((tick, at.0, v.0.dot(forward), v.0.length(), within));
        }
        let drive = world
            .get::<crate::track::sim::TrackDrive>(bed.tank)
            .expect("a driving tank carries its belts");
        let heading = {
            let raw = (-forward.x).atan2(-forward.z);
            let last = self.last().heading;
            last + (raw - last + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI
        };
        let sample = Sample {
            position: hull.position,
            spin: hull.rotation.inverse() * hull.angular,
            heave: (hull.velocity.y - before) / DT,
            belt: [drive.sides[0].speed, drive.sides[1].speed],
            speed: hull.speed(),
            heading,
            awake: bed.awake_pieces().len(),
            asleep: world.get::<Sleeping>(bed.tank).is_some(),
            contact,
            inside,
            clip: self.clip(world, &hull),
            support: {
                let belts = world
                    .get::<TrackContacts>(bed.tank)
                    .expect("a driving tank carries its belt contacts");
                let all = || belts.0.iter().flatten();
                let load = all().map(|c| c.load).sum::<f32>();
                [
                    load,
                    all().map(|c| c.tangential_force().dot(forward)).sum(),
                    all().map(|c| c.mu * c.load_elastic).sum(),
                    all().map(|c| c.slip.abs() * c.load).sum::<f32>() / load.max(1.0),
                ]
            },
            support_up: world
                .get::<TrackContacts>(bed.tank)
                .expect("a driving tank carries its belt contacts")
                .0
                .iter()
                .flatten()
                .map(|c| (c.normal * c.load + c.tangential_force()).y)
                .sum(),
            pitch: forward.y.asin().to_degrees(),
            drivetrain: {
                let state = world
                    .get::<crate::track::sim::TankTransmission>(bed.tank)
                    .expect("a tank carries its transmission")
                    .0;
                [
                    f32::from(state.gear),
                    state.omega_e * 60.0 / std::f32::consts::TAU,
                    f32::from(u8::from(matches!(
                        state.clutch,
                        crate::track::transmission::ClutchState::In
                    ))),
                ]
            },
        };
        self.ticks.push(sample);
    }
}

fn peak_rms(ticks: &[Sample], key: impl Fn(&Sample) -> f32) -> Value {
    let peak = ticks.iter().map(|t| key(t).abs()).fold(0.0, f32::max);
    let rms = (ticks.iter().map(|t| key(t).powi(2)).sum::<f32>() / ticks.len() as f32).sqrt();
    json!([peak, rms])
}

fn max_of(ticks: &[Sample], key: impl Fn(&Sample) -> f32) -> f32 {
    ticks.iter().map(key).fold(0.0, f32::max)
}

/// The 95th percentile of a series, 0 when empty.
fn p95(mut values: Vec<f32>) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(f32::total_cmp);
    values[(values.len() - 1) * 95 / 100]
}

/// The median, 95th percentile and largest of a series, `null` when empty.
fn spread(mut values: Vec<f32>) -> Value {
    if values.is_empty() {
        return Value::Null;
    }
    values.sort_by(f32::total_cmp);
    let at = |q: usize| values[(values.len() - 1) * q / 100];
    json!([at(50), at(95), at(100)])
}

/// One phase's readings over ticks `range`, against the hull's ride `static_y` before the drive.
fn phase(rec: &Recorder, range: Range<usize>, static_y: f32) -> Value {
    let ticks = &rec.ticks[range.clone()];
    if ticks.is_empty() {
        return json!({ "ticks": 0 });
    }
    let start = if range.start == 0 {
        &rec.origin
    } else {
        &rec.ticks[range.start - 1]
    };
    let end = &ticks[ticks.len() - 1];
    let (mut reversals, mut reversing, mut swing, mut piece_speed, mut touched) =
        (0, 0, 0.0_f32, 0.0_f32, 0);
    // How far the touched pieces moved over the ground within the phase (m): the largest, and
    // the sum, with its vertical part apart.
    let (mut pushed, mut pushed_sum, mut lowered_sum) = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut inside_pieces = 0;
    for track in rec.tracks.values() {
        let within: Vec<_> = track
            .samples
            .iter()
            .filter(|s| range.contains(&s.0))
            .collect();
        if within.is_empty() {
            continue;
        }
        touched += 1;
        inside_pieces += usize::from(within.iter().any(|s| s.4));
        let flips = within
            .windows(2)
            .filter(|w| w[1].0 == w[0].0 + 1)
            .filter(|w| {
                w[0].2 * w[1].2 < 0.0 && w[0].2.abs() > track.rest && w[1].2.abs() > track.rest
            })
            .count();
        reversals += flips;
        reversing += usize::from(flips > 0);
        for w in within.windows(3).filter(|w| w[2].0 == w[0].0 + 2) {
            swing = swing.max((w[1].1 - (w[0].1 + w[2].1) / 2.0).length());
        }
        piece_speed = within.iter().map(|s| s.3).fold(piece_speed, f32::max);
        let moved = within[within.len() - 1].1 - within[0].1;
        let across = Vec2::new(moved.x, moved.z).length();
        pushed = pushed.max(across);
        pushed_sum += across;
        lowered_sum -= moved.y;
    }
    // Per role: the deepest loaded point, its 95th percentile over the ticks the role bore on a
    // piece, those ticks, and the role's share of the upward force loose pieces put on the hull.
    let lift_all: f32 = ticks.iter().flat_map(|t| t.contact.lift).sum();
    let roles: serde_json::Map<String, Value> = ROLES
        .iter()
        .enumerate()
        .map(|(r, name)| {
            let on: Vec<f32> = ticks
                .iter()
                .filter(|t| t.contact.pieces[r] > 0)
                .map(|t| t.contact.pen[r])
                .collect();
            let lift: f32 = ticks.iter().map(|t| t.contact.lift[r]).sum();
            let row = json!({
                "pen_max_m": on.iter().copied().fold(0.0, f32::max),
                "ticks": on.len(),
                "pen_p95_m": p95(on),
                "lift_share": if lift_all > 0.0 { lift / lift_all } else { 0.0 },
            });
            ((*name).to_owned(), row)
        })
        .collect();
    // The last two seconds' means: where a stalled drive's tractive force goes.
    let tail = {
        let tail = &ticks[ticks.len().saturating_sub(2 * TICKS)..];
        let mean =
            |key: &dyn Fn(&Sample) -> f32| tail.iter().map(key).sum::<f32>() / tail.len() as f32;
        let pieces: serde_json::Map<String, Value> = ROLES
            .iter()
            .enumerate()
            .map(|(r, name)| (name.to_string(), json!(mean(&|t| t.contact.force[r]))))
            .collect();
        json!({
            "belt_m_s": mean(&|t| (t.belt[0].abs() + t.belt[1].abs()) / 2.0),
            "hull_m_s": mean(&|t| t.speed.abs()),
            "support_n": mean(&|t| t.support[0]),
            "support_forward_n": mean(&|t| t.support[1]),
            "grip_budget_n": mean(&|t| t.support[2]),
            "slip_m_s": mean(&|t| t.support[3]),
            "pitch_deg": mean(&|t| t.pitch),
            "gear": mean(&|t| t.drivetrain[0]),
            "engine_rpm": mean(&|t| t.drivetrain[1]),
            "clutch_in": mean(&|t| t.drivetrain[2]),
            "piece_force_n": pieces,
        })
    };
    json!({
        "ticks": ticks.len(),
        "piece_ticks": ticks.iter().filter(|t| t.contact.pieces.iter().any(|&n| n > 0)).count(),
        "travel_m": start.position.z - end.position.z,
        "rise_max_m": max_of(ticks, |t| t.position.y) - static_y,
        "rise_mean_m": ticks.iter().map(|t| t.position.y - static_y).sum::<f32>() / ticks.len() as f32,
        "yaw_deg": (end.heading - start.heading).to_degrees(),
        "pitch_rate_deg_s": peak_rms(ticks, |t| t.spin.x.to_degrees()),
        "roll_rate_deg_s": peak_rms(ticks, |t| t.spin.z.to_degrees()),
        "heave_m_s2": peak_rms(ticks, |t| t.heave),
        "belt_peak_m_s": max_of(ticks, |t| t.belt[0].abs().max(t.belt[1].abs())),
        "hull_speed_peak_m_s": max_of(ticks, |t| t.speed.abs()),
        "roles": roles,
        "run_depth_m": spread(ticks.iter().flat_map(|t| t.contact.run_depths.iter().copied()).collect()),
        "deep_points": ticks.iter().map(|t| t.contact.deep).sum::<usize>(),
        "deep_ticks": ticks.iter().filter(|t| t.contact.deep > 0).count(),
        "tank_force_peak_n": max_of(ticks, |t| t.contact.tank_force),
        "squeezed_peak": ticks.iter().map(|t| t.contact.squeezed).max(),
        "squeeze_pen_max_m": max_of(ticks, |t| t.contact.squeeze_pen),
        "squeeze_force_peak_n": max_of(ticks, |t| t.contact.squeeze_force),
        "touched_pieces": touched,
        "reversals": reversals,
        "reversing_pieces": reversing,
        "swing_max_m": swing,
        "piece_speed_peak_m_s": piece_speed,
        "pushed_max_m": pushed,
        "pushed_sum_m": pushed_sum,
        "lowered_sum_m": lowered_sum,
        "inside_pieces": inside_pieces,
        "inside_ticks": ticks.iter().filter(|t| t.inside > 0).count(),
        "clip_max_m": max_of(ticks, |t| t.clip),
        "clip_p95_m": p95(ticks.iter().filter(|t| t.clip > 0.0).map(|t| t.clip).collect()),
        "tail": tail,
        "awake_peak": ticks.iter().map(|t| t.awake).max(),
        "awake_end": end.awake,
    })
}

/// The hull's rise over `static_y` (m), the run's contact depths, whether the stop bore on a
/// piece, and what holds the hull up against its weight (N, means), over the half second of
/// `rec` before tick `end` and from tick `floor` on.
fn at_rest(rec: &Recorder, floor: usize, end: usize, static_y: f32) -> Value {
    let ticks = &rec.ticks[end.saturating_sub(TICKS / 2).max(floor)..end];
    if ticks.is_empty() {
        return Value::Null;
    }
    let mean =
        |key: &dyn Fn(&Sample) -> f32| ticks.iter().map(key).sum::<f32>() / ticks.len() as f32;
    json!({
        "lift_n": ([0, 1, 2].map(|r| mean(&|t| t.contact.lift[r]))),
        "support_up_n": mean(&|t| t.support_up),
        "weight_n": rec.weight,
        "rise_m": ticks.iter().map(|t| t.position.y - static_y).sum::<f32>() / ticks.len() as f32,
        "run_depth_m": spread(ticks.iter().flat_map(|t| t.contact.run_depths.iter().copied()).collect()),
        "stop_ticks": ticks.iter().filter(|t| t.contact.pieces[0] > 0).count(),
        "stop_pen_max_m": ticks.iter().filter(|t| t.contact.pieces[0] > 0).map(|t| t.contact.pen[0]).fold(0.0, f32::max),
        "clip_max_m": max_of(ticks, |t| t.clip),
        "ticks": ticks.len(),
    })
}

fn parked(bed: &Bed) -> bool {
    bed.app
        .world()
        .get::<crate::track::sim::TankTransmission>(bed.tank)
        .expect("a tank carries its transmission")
        .0
        .park
}

fn run(drive: Drive, seed: u32) -> Value {
    let shift = nudge(seed);
    let (mut bed, site) = settle(&Case {
        label: format!("{}-{seed}", drive.label()),
        heap: Heap::House,
        throttle: 1.0,
        offset: shift.x,
        approach_m: APPROACH_M + shift.z,
        mode: Mode::Through,
    });
    let hull = bed.hull();
    let ground = bed
        .app
        .world()
        .resource::<crate::terrain_grid::HeightGrid>()
        .height_at(hull.position.x, hull.position.z);
    let sag = hull.position.y - ground - bed.rest_y;
    let mut rec = Recorder::new(&bed);
    let static_y = rec.origin.position.y;
    let mut phases = Vec::new();
    let mut outcome = serde_json::Map::new();
    match drive {
        Drive::Pull | Drive::Pivot => {
            let middle = (site.near + site.far) / 2.0;
            let begin = rec.ticks.len();
            while bed.hull().position.z > middle && rec.ticks.len() - begin < APPROACH_S * TICKS {
                rec.step(&mut bed, 1.0, 0.0);
            }
            outcome.insert(
                "reached_middle".into(),
                json!(bed.hull().position.z <= middle),
            );
            let braking = rec.ticks.len();
            while !parked(&bed) && rec.ticks.len() - braking < APPROACH_S * TICKS {
                rec.step(&mut bed, -1.0, 0.0);
            }
            outcome.insert("parked".into(), json!(parked(&bed)));
            let (resting, mut rested) = (rec.ticks.len(), 0);
            while rested < TICKS && rec.ticks.len() - resting < REST_S * TICKS {
                rec.step(&mut bed, 0.0, 0.0);
                rested = if rec.last().speed.abs() < REST_M_S {
                    rested + 1
                } else {
                    0
                };
            }
            outcome.insert("rested".into(), json!(rested >= TICKS));
            outcome.insert(
                "on_heap".into(),
                at_rest(&rec, resting, rec.ticks.len(), static_y),
            );
            phases.push(("approach", begin..rec.ticks.len()));
            let (throttle, steer, seconds) = match drive {
                Drive::Pull => (1.0, 0.0, PULL_S),
                _ => (0.0, 1.0, PIVOT_S),
            };
            let begin = rec.ticks.len();
            for _ in 0..seconds * TICKS {
                rec.step(&mut bed, throttle, steer);
            }
            phases.push(("drive", begin..rec.ticks.len()));
        }
        Drive::Straight | Drive::Creep => {
            let (throttle, seconds) = match drive {
                Drive::Creep => (CREEP_THROTTLE, CREEP_S),
                _ => (1.0, STRAIGHT_S),
            };
            // The belts' rear point, behind the hull origin along +z.
            let rear = Side::ALL
                .iter()
                .flat_map(|&side| bed.geom().rest.get(side).iter())
                .map(|&(c, r)| c.x + r)
                .fold(f32::NEG_INFINITY, f32::max);
            let begin = rec.ticks.len();
            while bed.hull().position.z + rear > site.far
                && rec.ticks.len() - begin < seconds * TICKS
            {
                rec.step(&mut bed, throttle, 0.0);
            }
            outcome.insert(
                "crossed".into(),
                json!(bed.hull().position.z + rear <= site.far),
            );
            outcome.insert("heap_len_m".into(), json!(site.near - site.far));
            phases.push(("drive", begin..rec.ticks.len()));
        }
    }
    // The stop: brake until the transmission parks, then no command until the tank sleeps.
    let begin = rec.ticks.len();
    while !parked(&bed) && rec.ticks.len() - begin < APPROACH_S * TICKS {
        rec.step(&mut bed, -1.0, 0.0);
    }
    let parked_after = parked(&bed).then(|| (rec.ticks.len() - begin) as f32 * DT);
    let mut slept = None;
    for _ in 0..SLEEP_BOUND_S * TICKS {
        rec.step(&mut bed, 0.0, 0.0);
        if rec.last().asleep {
            slept = Some((rec.ticks.len() - begin) as f32 * DT);
            break;
        }
    }
    phases.push(("stop", begin..rec.ticks.len()));
    let asleep_at = rec.ticks[begin..]
        .iter()
        .position(|t| t.asleep)
        .map_or(rec.ticks.len(), |k| begin + k);
    outcome.insert("asleep".into(), at_rest(&rec, begin, asleep_at, static_y));
    outcome.insert("parked_after_stop_s".into(), json!(parked_after));
    outcome.insert(
        "pieces_under_at_end".into(),
        json!(rec.last().contact.pieces.iter().sum::<usize>()),
    );
    let mut line = json!({
        "case": drive.label(),
        "seed": seed,
        "nudge_m": [shift.x, shift.z],
        "sag_m": sag,
        "sleep_after_stop_s": slept,
        "awake_at_end": rec.last().awake,
    });
    let object = line.as_object_mut().expect("the line is an object");
    object.extend(outcome);
    let faces = rec.ticks.iter().fold([0; 4], |sum, t| {
        [0, 1, 2, 3].map(|k| sum[k] + t.contact.faces[k])
    });
    object.insert("run_faces".into(), json!(faces));
    let mut secant: Vec<(f32, f32)> = (rec.ticks.iter())
        .flat_map(|t| t.contact.secant.iter().copied())
        .collect();
    if !secant.is_empty() {
        let weighted = secant.iter().map(|(r, f)| r * f).sum::<f32>()
            / secant.iter().map(|(_, f)| f).sum::<f32>();
        secant.sort_by(|a, b| a.0.total_cmp(&b.0));
        let at = |q: usize| secant[(secant.len() - 1) * q / 100].0;
        object.insert(
            "secant_ratio".into(),
            json!([at(25), at(50), at(75), at(95), weighted]),
        );
    }
    for (name, range) in phases {
        object.insert(name.into(), phase(&rec, range, static_y));
    }
    line
}

#[test]
#[ignore = "the soft run's probe, run on demand"]
fn soft_run_probe() {
    let jobs: Vec<(Drive, u32)> = (0..SEEDS)
        .flat_map(|seed| {
            [Drive::Pull, Drive::Pivot, Drive::Straight, Drive::Creep].map(|drive| (drive, seed))
        })
        .collect();
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            scope.spawn(|| {
                while let Some(&(drive, seed)) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    match std::panic::catch_unwind(|| run(drive, seed)) {
                        Ok(line) => println!("SOFTRUN {line}"),
                        Err(cause) => {
                            failed.fetch_add(1, Ordering::Relaxed);
                            let text = cause
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| cause.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                                .unwrap_or_default();
                            println!("SOFTRUN-FAILED {} {seed}: {text}", drive.label());
                        }
                    }
                }
            });
        }
    });
    assert_eq!(
        failed.into_inner(),
        0,
        "a start panicked: see SOFTRUN-FAILED"
    );
}
