//! Rubble climb probe (the game's tank driving physics, game state only): what carries the
//! shipped tank on a heap of loose construction pieces, and which state flips while it climbs
//! or stalls there. A diagnosis instrument, run on demand:
//!
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe rubble_climb_probe -- --ignored --nocapture`
//!
//! `RUBBLE_CLIMB_ONLY=<text>` runs only the cases whose label contains the text;
//! `RUBBLE_CLIMB_TAG=<text>` names the run in the per-tick CSV files under `target/rubble-climb/`.
//!
//! Two heaps: `yard` is line 8c's, the track sandbox's rubble yard pile; `house` is the
//! construction fixture map's brick house, dropped by breaking its ground supports and every
//! standing chunk whose centroid lies under [`COLLAPSE_BAND_M`], as the destruction workload
//! (`net::loadtest`) breaks supports. Either is left to settle asleep. Each case prints
//! one `RUBBLECLIMB {json}` line. Every value is MEASURED per tick from Avian body state,
//! `ContactTransfers`, `TrackContacts`, `TrackDrive` and `TankTransmission`; shares are of the
//! tank's weight, taken along world up. Every line is an observation.
//!
//! Fixture: every placement, duration, threshold and bin below is fixture data.

use super::*;
use crate::track::oracle::DepthOracle;

/// Fixture: the longest wait for the heap to sleep before the drive (s).
const SETTLE_S: usize = 30;
/// Fixture: the drive's length (s); a paused case's coast in the middle, and its second leg.
const DRIVE_S: usize = 25;
const PAUSE_S: usize = 5;
/// Fixture: hull speed under which a driven tank counts as stalled (m/s).
const STALL_M_S: f32 = 0.05;
/// Fixture: belt speed over which a belt counts as running (m/s).
const BELT_RUNNING_M_S: f32 = 0.1;
/// Fixture: the share of the tank's weight over which a road-wheel station counts as loaded.
const STATION_LOADED: f32 = 0.005;
/// Fixture: how near the hull origin a piece counts as under the tank (m, horizontal).
const UNDER_TANK_M: f32 = 4.0;
/// Fixture: the house's chunks with a centroid under this height are broken with its supports (m).
const COLLAPSE_BAND_M: f32 = 1.2;
/// Fixture: the belt's front stands this far short of the heap's near edge at the start (m).
const APPROACH_M: f32 = 2.0;
/// Fixture: the flat world's samples per side, a cell near the shipped grid's.
const HOUSE_SAMPLES: u32 = 1025;

#[derive(Clone, Copy, PartialEq)]
enum Heap {
    Yard,
    House,
    /// The house left standing, for the tank to ram.
    Standing,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Drive at the case's throttle for `DRIVE_S`.
    Through,
    /// As `Through`; after the first whole second stalled on the heap, coast `PAUSE_S` with no
    /// demand, then drive `DRIVE_S` again.
    StallPause,
    /// Full throttle until the hull origin is over the heap's middle, no demand until the hull
    /// has rested a second, then the case's throttle for `DRIVE_S`: the readings start there.
    StopGo,
}

struct Case {
    label: String,
    heap: Heap,
    throttle: f32,
    /// The hull's sideways offset from the heap's centre line (m).
    offset: f32,
    mode: Mode,
}

/// One tick's readings.
struct Tick {
    throttle: f32,
    z: f32,
    rise: f32,
    speed: f32,
    pitch: f32,
    belt: [f32; 2],
    gear: u8,
    reverse: bool,
    park: bool,
    shift: u8,
    clutch: String,
    rpm: f32,
    /// Sampled-support contacts per side, how many of them sit at their friction limit, and
    /// their summed upward force (N).
    spring_n: [usize; 2],
    spring_saturated: usize,
    spring_up: f32,
    /// The part of `spring_up` from solids (standing construction), not the terrain field.
    spring_solid_up: f32,
    /// The part of `solver_up` from bodies that are no loose piece (standing construction).
    structure_up: [f32; 4],
    /// Solver contacts with another body, by the tank collider's role: upward force (N) and
    /// points, as [belt segment, bump stop, side plate, hull proxy].
    solver_up: [f32; 4],
    solver_points: [usize; 4],
    /// The hull proxies' summed normal force (N) and the distinct pieces they touch.
    hull_normal: f32,
    hull_pieces: usize,
    /// Upward force near each road-wheel station (N), left side then right, front to rear:
    /// sampled support, belt segments and bump stops.
    stations: Vec<f32>,
    /// The terrain field's clearance under each road-wheel station's rest contact point (m),
    /// the only ground the drawn wheels and belt read (`track::view::drive_track_views`).
    view_clearance: Vec<f32>,
    awake: usize,
    asleep_under_tank: usize,
    piece_sleep_changes: usize,
    tank_asleep: bool,
    /// The Driver's reading this tick (`track::sense`).
    sensed: Option<crate::track::sense::Sensed>,
}

const ROLES: [&str; 4] = ["belt segment", "bump stop", "side plate", "hull proxy"];

fn read(
    bed: &Bed,
    throttle: f32,
    begin_y: f32,
    asleep: &mut std::collections::HashMap<Entity, bool>,
) -> Tick {
    let hull = bed.hull();
    let world = bed.app.world();
    let geom = bed.geom();
    let inverse = hull.rotation.inverse();
    let wheels: Vec<Vec3> = Side::ALL
        .iter()
        .flat_map(|&side| geom.road_wheels.get(side).iter().copied())
        .collect();
    // The nearest road-wheel station on a world point's side of the hull.
    let station = |point: Vec3| {
        let local = inverse * (point - hull.position);
        wheels
            .iter()
            .enumerate()
            .filter(|(_, w)| (w.x < 0.0) == (local.x < 0.0))
            .min_by(|a, b| (a.1.z - local.z).abs().total_cmp(&(b.1.z - local.z).abs()))
            .expect("each side has road wheels")
            .0
    };
    let mut stations = vec![0.0; wheels.len()];

    let contacts = &world
        .get::<TrackContacts>(bed.tank)
        .expect("a driving tank reports its belt contacts")
        .0;
    let (mut spring_n, mut spring_saturated, mut spring_up) = ([0; 2], 0, 0.0);
    let mut spring_solid_up = 0.0;
    for (si, side) in contacts.iter().enumerate() {
        for c in side {
            spring_n[si] += 1;
            let up = (c.normal * c.load).y;
            spring_up += up;
            if c.collider.is_some() {
                spring_solid_up += up;
            }
            stations[station(c.point)] += up;
            if c.load > 0.0 && c.traction.length() >= 0.95 * c.mu * c.load {
                spring_saturated += 1;
            }
        }
    }

    let (mut solver_up, mut solver_points) = ([0.0; 4], [0; 4]);
    let (mut hull_normal, mut hull_pieces) = (0.0, Vec::new());
    let mut structure_up = [0.0; 4];
    for (point, force) in bed.solver_points() {
        let (collider, piece, force) = if point.other == bed.tank {
            let collider = point
                .other_collider
                .expect("a solver contact names both of its colliders");
            (collider, point.reference, force)
        } else if point.reference == bed.tank {
            (point.reference_collider, point.other, -force)
        } else {
            continue;
        };
        let role = bed.role(collider);
        let Some(ri) = ROLES.iter().position(|r| role.starts_with(r)) else {
            panic!("an unclassified tank collider met a body: {role}");
        };
        solver_up[ri] += force.y;
        solver_points[ri] += 1;
        let loose = world.get::<crate::debris::DebrisPiece>(piece).is_some();
        if !loose {
            structure_up[ri] += force.y;
        }
        if ri == 3 {
            hull_normal += force.dot(point.normal).abs();
            if loose {
                hull_pieces.push(piece);
            }
        } else if ri < 2 {
            stations[station(point.point)] += force.y;
        }
    }
    hull_pieces.sort_unstable();
    hull_pieces.dedup();

    let field = world
        .resource::<TrackField>()
        .field
        .as_ref()
        .expect("the booted world built its track field");
    let down = hull.rotation * Vec3::NEG_Y;
    let view_clearance = wheels
        .iter()
        .map(|w| {
            let rest_contact = hull.position + hull.rotation * Vec3::new(w.x, -bed.rest_y, w.z);
            -field.depth_along(rest_contact, down, 0.5)
        })
        .collect();

    let (mut awake, mut asleep_under_tank, mut piece_sleep_changes) = (0, 0, 0);
    for &piece in &bed.pieces {
        let at = world
            .get::<Position>(piece)
            .expect("a live piece has a position");
        let now = world.get::<Sleeping>(piece).is_some();
        // A piece born this tick starts awake.
        if asleep.insert(piece, now).unwrap_or(false) != now {
            piece_sleep_changes += 1;
        }
        if !now {
            awake += 1;
        } else if (at.0 - hull.position).with_y(0.0).length() < UNDER_TANK_M {
            asleep_under_tank += 1;
        }
    }

    let drive = world
        .get::<crate::track::sim::TrackDrive>(bed.tank)
        .expect("a tank carries its belts");
    let state = world
        .get::<crate::track::sim::TankTransmission>(bed.tank)
        .expect("a tank carries its transmission")
        .0;
    Tick {
        throttle,
        z: hull.position.z,
        rise: hull.position.y - begin_y,
        speed: hull.speed(),
        pitch: hull.pitch(),
        belt: [drive.sides[0].speed, drive.sides[1].speed],
        gear: state.gear,
        reverse: state.reverse,
        park: state.park,
        shift: state.shift_ticks,
        clutch: format!("{:?}", state.clutch),
        rpm: state.omega_e / crate::track::transmission::RPM_TO_RAD,
        spring_n,
        spring_saturated,
        spring_up,
        spring_solid_up,
        structure_up,
        solver_up,
        solver_points,
        hull_normal,
        hull_pieces: hull_pieces.len(),
        stations,
        view_clearance,
        awake,
        asleep_under_tank,
        piece_sleep_changes,
        tank_asleep: world.get::<Sleeping>(bed.tank).is_some(),
        sensed: world
            .get::<crate::track::sense::DriverSense>(bed.tank)
            .expect("a driving tank carries its Driver's ledger")
            .reading,
    }
}

/// Mean, standard deviation, minimum and maximum of the sensed resistance over `ticks` that
/// carry a reading and pay no cut (N).
fn resistance(ticks: &[&Tick]) -> Value {
    let x: Vec<f64> = ticks
        .iter()
        .filter(|t| t.shift == 0)
        .filter_map(|t| t.sensed.map(|s| f64::from(s.resistance)))
        .collect();
    if x.is_empty() {
        return Value::Null;
    }
    let n = x.len() as f64;
    let mean = x.iter().sum::<f64>() / n;
    let sd = (x.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
    json!({
        "ticks": x.len(),
        "mean_n": mean,
        "sd_n": sd,
        "min_n": x.iter().copied().fold(f64::INFINITY, f64::min),
        "max_n": x.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    })
}

/// How many times `key` changes from one tick to the next.
fn changes<T: PartialEq>(ticks: &[Tick], key: impl Fn(&Tick) -> T) -> usize {
    ticks
        .windows(2)
        .filter(|w| key(&w[0]) != key(&w[1]))
        .count()
}

fn mean(ticks: &[&Tick], key: impl Fn(&Tick) -> f32) -> Value {
    if ticks.is_empty() {
        return Value::Null;
    }
    f(ticks.iter().map(|t| key(t)).sum::<f32>() / ticks.len() as f32)
}

/// The means of one set of ticks: who carries the tank, and what its drivetrain does.
fn carried(ticks: &[&Tick], weight: f32, wheels_per_side: usize) -> Value {
    let share = |key: &dyn Fn(&Tick) -> f32| mean(ticks, |t| key(t) / weight);
    let stations: Vec<Value> = (0..2 * wheels_per_side)
        .map(|i| share(&|t| t.stations[i]))
        .collect();
    let clearance: Vec<Value> = (0..2 * wheels_per_side)
        .map(|i| mean(ticks, |t| t.view_clearance[i]))
        .collect();
    let mut clutch = std::collections::BTreeMap::<&str, usize>::new();
    for t in ticks {
        *clutch.entry(t.clutch.as_str()).or_default() += 1;
    }
    json!({
        "ticks": ticks.len(),
        "share_sampled_support": share(&|t| t.spring_up),
        "share_sampled_on_solids": share(&|t| t.spring_solid_up),
        "share_on_structure_by_role": (0..4).map(|ri| share(&|t| t.structure_up[ri])).collect::<Vec<_>>(),
        "share_belt_segments": share(&|t| t.solver_up[0]),
        "share_bump_stops": share(&|t| t.solver_up[1]),
        "share_side_plates": share(&|t| t.solver_up[2]),
        "share_hull_proxies": share(&|t| t.solver_up[3]),
        "hull_proxy_normal_share": share(&|t| t.hull_normal),
        "hull_proxy_points": mean(ticks, |t| t.solver_points[3] as f32),
        "hull_proxy_pieces": mean(ticks, |t| t.hull_pieces as f32),
        "belt_segment_points": mean(ticks, |t| t.solver_points[0] as f32),
        "bump_stop_points": mean(ticks, |t| t.solver_points[1] as f32),
        "station_share_left": stations[..wheels_per_side],
        "station_share_right": stations[wheels_per_side..],
        "view_clearance_left_m": clearance[..wheels_per_side],
        "view_clearance_right_m": clearance[wheels_per_side..],
        "sampled_contacts": mean(ticks, |t| (t.spring_n[0] + t.spring_n[1]) as f32),
        "sampled_saturated_share": mean(ticks, |t| {
            let n = t.spring_n[0] + t.spring_n[1];
            if n == 0 { 0.0 } else { t.spring_saturated as f32 / n as f32 }
        }),
        "belt_left_m_s": mean(ticks, |t| t.belt[0]),
        "belt_right_m_s": mean(ticks, |t| t.belt[1]),
        "rpm": mean(ticks, |t| t.rpm),
        "gear": mean(ticks, |t| f32::from(t.gear)),
        "clutch_ticks": clutch,
        "pieces_awake": mean(ticks, |t| t.awake as f32),
        "pieces_asleep_under_tank": mean(ticks, |t| t.asleep_under_tank as f32),
        "rise_m": mean(ticks, |t| t.rise),
        "pitch_deg": mean(ticks, |t| t.pitch.to_degrees()),
        "sense_resistance": resistance(ticks),
    })
}

/// The live loose pieces, ascending by entity.
fn live_pieces(bed: &mut Bed) -> Vec<Entity> {
    let world = bed.app.world_mut();
    let mut pieces: Vec<Entity> = world
        .query_filtered::<Entity, (With<crate::debris::DebrisPiece>, With<Position>)>()
        .iter(world)
        .collect();
    pieces.sort_unstable();
    pieces
}

/// The shipped tank alone on the construction fixture map over level ground, parked clear of
/// its brick house.
fn house_bed() -> Bed {
    let manifest = crate::map::tests::construction_fixture_manifest();
    let samples = vec![0.0; (HOUSE_SAMPLES * HOUSE_SAMPLES) as usize];
    let grid = HeightGrid::new(samples.into(), HOUSE_SAMPLES, manifest.extent);
    let mut app = booted_sim_on_map(Some(grid), Some(manifest));
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
    let mut track = world.resource_mut::<TrackField>();
    let field = track
        .field
        .take()
        .expect("the booted world built its track field");
    track.field = Some(field.with_ground(Some(GroundField::uniform(base_ground()))));
    let mut bed = Bed {
        app,
        tank,
        rest_y,
        pieces: Vec::new(),
    };
    // Fixture: well clear of the house and of what falls from it.
    bed.place(Vec3::new(40.0, 0.0, -20.0), Quat::IDENTITY);
    bed
}

/// Break the fixture house's ground supports and its chunks under the band, with the lines
/// the destruction workload delivers (`net::loadtest`): each chunk's own work pool twice over,
/// deposited at its centroid.
fn collapse_house(bed: &mut Bed) {
    use crate::ballistics::{
        BallisticVolume, DeliveredLine, PenetrationMarks, ShellMarched, VolumeCrossing,
    };
    use crate::construction::{Body, Element, breakage::Breakage};
    let world = bed.app.world_mut();
    let volumes: Vec<(Entity, Entity, u16)> = world
        .query_filtered::<(Entity, &ChildOf, &Element), With<BallisticVolume>>()
        .iter(world)
        .map(|(volume, parent, element)| (volume, parent.parent(), element.0))
        .collect();
    let (root, pose) = world
        .query_filtered::<(Entity, &crate::scatter::MapInstance, &Transform), (With<Body>, With<Breakage>)>()
        .iter(world)
        .find(|(_, identity, _)| identity.id == "brick_0001")
        .map(|(root, _, pose)| (root, *pose))
        .expect("the fixture map places its brick house");
    let body = world
        .get::<Breakage>(root)
        .expect("the query matched a breakable construction");
    let anchored: Vec<u32> = body.kind.asset.anchors.iter().map(|a| a.chunk).collect();
    let lines: Vec<DeliveredLine> = body
        .live
        .structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(c, chunk)| {
            chunk.buried
                || anchored.contains(&body.kind.layout.chunks[*c])
                || chunk.centroid.y < COLLAPSE_BAND_M
        })
        .map(|(c, chunk)| {
            let element = body.kind.layout.element_of[c];
            let volume = volumes
                .iter()
                .find(|&&(_, parent, e)| parent == root && e == element)
                .expect("an untouched chunk's element keeps its volume")
                .0;
            let at = pose.translation + pose.rotation * chunk.centroid;
            DeliveredLine {
                crossing: VolumeCrossing {
                    volume,
                    entry: at,
                    end: at,
                    chord: 0.0,
                    cost: 0.0,
                    energy: 2.0 * chunk.volume * chunk.material.crush,
                    perforated: true,
                },
                contacts: vec![],
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
                entrance_energy: 0.0,
            }
        })
        .collect();
    let shell = world.spawn(PenetrationMarks::default()).id();
    world.trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        burst: None,
        lines,
    });
    world.despawn(shell);
}

/// The heap a case drives at, settled, with the tank parked short of it.
struct Site {
    asleep_after_s: f32,
    awake_at_drive: usize,
    standing_chunks: Option<usize>,
    top: f32,
    near: f32,
    far: f32,
    left: f32,
    right: f32,
    mid: f32,
}

fn settle(case: &Case) -> (Bed, Site) {
    use crate::track_sandbox::rubble;
    let mut bed = match case.heap {
        Heap::Yard => {
            let kind = rubble::yard_source();
            let centre = Vec3::NEG_Z * rubble::spot_ahead_m();
            let mut bed = Bed::on_ground(Some(base_ground()), flat());
            for piece in rubble::build_yard(&kind, centre).pile {
                bed.app.world_mut().spawn(piece.body);
            }
            bed.place(Vec3::new(40.0, 0.0, -20.0), Quat::IDENTITY);
            bed.start();
            bed
        }
        Heap::House | Heap::Standing => {
            let mut bed = house_bed();
            bed.start();
            bed.tick(0.0, 0.0);
            if case.heap == Heap::House {
                collapse_house(&mut bed);
            }
            bed
        }
    };
    let mut asleep_after = f32::NAN;
    for tick in 1..=SETTLE_S * TICKS {
        bed.tick(0.0, 0.0);
        bed.pieces = live_pieces(&mut bed);
        // The first second lets the house's pieces leave it.
        if tick > TICKS && bed.awake_pieces().is_empty() {
            asleep_after = tick as f32 * DT;
            break;
        }
    }
    // The largest construction's standing chunks: how many, and their centroids' bounds as
    // (nearest z, farthest z, mean x).
    let standing = {
        let world = bed.app.world_mut();
        world
            .query_filtered::<(&Transform, &crate::construction::breakage::Breakage), With<crate::construction::Body>>()
            .iter(world)
            .map(|(pose, body)| {
                let at: Vec<Vec3> = (0..body.live.structure.chunks.len())
                    .filter(|&c| body.live.state.standing(c))
                    .map(|c| pose.transform_point(body.live.structure.chunks[c].centroid))
                    .collect();
                let n = at.len().max(1) as f32;
                (
                    at.len(),
                    at.iter().map(|p| p.z).fold(f32::NEG_INFINITY, f32::max),
                    at.iter().map(|p| p.z).fold(f32::INFINITY, f32::min),
                    at.iter().map(|p| p.x).sum::<f32>() / n,
                )
            })
            .max_by_key(|&(count, ..)| count)
    };
    let awake_at_drive = bed.awake_pieces().len();
    let world = bed.app.world();
    // The heap's bounds, from its pieces' bounds; its centre line is their mean x.
    let (mut top, mut near, mut far) = (0.0_f32, f32::NEG_INFINITY, f32::INFINITY);
    let (mut left, mut right, mut mid) = (f32::INFINITY, f32::NEG_INFINITY, 0.0);
    for &piece in &bed.pieces {
        let colliders = world
            .get::<RigidBodyColliders>(piece)
            .expect("a piece's body lists its colliders");
        let mut centre = 0.0;
        for collider in colliders.iter() {
            let aabb = world
                .get::<ColliderAabb>(collider)
                .expect("a settled piece's collider has bounds");
            top = top.max(aabb.max.y);
            near = near.max(aabb.max.z);
            far = far.min(aabb.min.z);
            left = left.min(aabb.min.x);
            right = right.max(aabb.max.x);
            centre += aabb.center().x / colliders.len() as f32;
        }
        mid += centre / bed.pieces.len() as f32;
    }
    if let (Heap::Standing, Some((_, z_near, z_far, x))) = (case.heap, standing) {
        (near, far, mid) = (z_near, z_far, x);
    }
    // The belt's front point, ahead of the hull origin along its forward (−z).
    let front = Side::ALL
        .iter()
        .flat_map(|&side| bed.geom().rest.get(side).iter())
        .map(|&(c, r)| -(c.x - r))
        .fold(f32::NEG_INFINITY, f32::max);
    bed.place(
        Vec3::new(mid + case.offset, 0.0, near + APPROACH_M + front),
        Quat::IDENTITY,
    );
    for _ in 0..2 * TICKS {
        bed.tick(0.0, 0.0);
    }
    let site = Site {
        asleep_after_s: asleep_after,
        awake_at_drive,
        standing_chunks: standing.map(|(count, ..)| count),
        top,
        near,
        far,
        left,
        right,
        mid,
    };
    (bed, site)
}

/// The case's drive: every tick's readings, the tick a stall pause began, and the tick the
/// readings start from (the stop-and-go's second drive, else the first).
fn drive(
    case: &Case,
    bed: &mut Bed,
    site: &Site,
    begin_y: f32,
) -> (Vec<Tick>, Option<usize>, usize) {
    let (near, far) = (site.near, site.far);
    let world = bed.app.world();
    let mut asleep: std::collections::HashMap<Entity, bool> = bed
        .pieces
        .iter()
        .map(|&p| (p, world.get::<Sleeping>(p).is_some()))
        .collect();
    let mut ticks: Vec<Tick> = Vec::new();
    let mut paused_at = None;
    let mut stalled = 0;
    let mut leg_start = 0;
    if case.mode == Mode::StopGo {
        let middle = (near + far) / 2.0;
        let mut phase_ticks = 0;
        while bed.hull().position.z > middle && phase_ticks < 10 * TICKS {
            bed.tick(1.0, 0.0);
            bed.pieces = live_pieces(bed);
            ticks.push(read(bed, 1.0, begin_y, &mut asleep));
            phase_ticks += 1;
        }
        let (mut rested, mut phase_ticks) = (0, 0);
        while rested < TICKS && phase_ticks < 15 * TICKS {
            bed.tick(0.0, 0.0);
            bed.pieces = live_pieces(bed);
            let now = read(bed, 0.0, begin_y, &mut asleep);
            rested = if now.speed.abs() < STALL_M_S {
                rested + 1
            } else {
                0
            };
            ticks.push(now);
            phase_ticks += 1;
        }
        leg_start = ticks.len();
    }
    let mut total = ticks.len() + DRIVE_S * TICKS;
    while ticks.len() < total {
        let tick = ticks.len();
        let coasting = paused_at.is_some_and(|at| tick < at + PAUSE_S * TICKS);
        let throttle = if coasting { 0.0 } else { case.throttle };
        bed.tick(throttle, 0.0);
        bed.pieces = live_pieces(bed);
        let now = read(bed, throttle, begin_y, &mut asleep);
        let on_heap = now.solver_points.iter().sum::<usize>() > 0;
        stalled = if on_heap && throttle > 0.0 && now.speed.abs() < STALL_M_S {
            stalled + 1
        } else {
            0
        };
        if case.mode == Mode::StallPause && paused_at.is_none() && stalled >= TICKS {
            paused_at = Some(tick + 1);
            total = tick + 1 + (PAUSE_S + DRIVE_S) * TICKS;
        }
        ticks.push(now);
    }
    (ticks, paused_at, leg_start)
}

fn write_series(tag: &str, label: &str, ticks: &[Tick], weight: f32) {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/rubble-climb");
    std::fs::create_dir_all(&dir).expect("the target directory exists or can be made");
    let mut csv = String::from(
        "tick,throttle,z,rise,speed,pitch_deg,belt_l,belt_r,gear,reverse,park,shift,clutch,rpm,\
         spring_l,spring_r,spring_sat,spring_share,seg_share,stop_share,plate_share,hull_share,\
         seg_pts,stop_pts,plate_pts,hull_pts,hull_pieces,awake,asleep_under,sleep_changes,\
         tank_asleep,sense_speed,sense_r,sense_dr\n",
    );
    for (i, t) in ticks.iter().enumerate() {
        csv += &format!(
            "{i},{},{:.3},{:.3},{:.3},{:.2},{:.3},{:.3},{},{},{},{},{},{:.0},{},{},{},{:.3},\
             {:.3},{:.3},{:.3},{:.3},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            t.throttle,
            t.z,
            t.rise,
            t.speed,
            t.pitch.to_degrees(),
            t.belt[0],
            t.belt[1],
            t.gear,
            u8::from(t.reverse),
            u8::from(t.park),
            t.shift,
            t.clutch,
            t.rpm,
            t.spring_n[0],
            t.spring_n[1],
            t.spring_saturated,
            t.spring_up / weight,
            t.solver_up[0] / weight,
            t.solver_up[1] / weight,
            t.solver_up[2] / weight,
            t.solver_up[3] / weight,
            t.solver_points[0],
            t.solver_points[1],
            t.solver_points[2],
            t.solver_points[3],
            t.hull_pieces,
            t.awake,
            t.asleep_under_tank,
            t.piece_sleep_changes,
            u8::from(t.tank_asleep),
            t.sensed
                .map_or(String::new(), |s| format!("{:.3}", s.speed)),
            t.sensed
                .map_or(String::new(), |s| format!("{:.0}", s.resistance)),
            t.sensed
                .map_or(String::new(), |s| format!("{:.0}", s.push_difference)),
        );
    }
    std::fs::write(dir.join(format!("{tag}-{}.csv", label)), csv)
        .expect("the per-tick series writes");
}

/// How many times each candidate state changes over `after`.
fn flips(after: &[Tick], weight: f32, wheels_per_side: usize) -> Value {
    let station_flips: usize = (0..2 * wheels_per_side)
        .map(|i| changes(after, |t| t.stations[i] > STATION_LOADED * weight))
        .sum();
    let side_flips: usize = (0..2)
        .map(|si| {
            changes(after, |t| {
                let side = si * wheels_per_side..(si + 1) * wheels_per_side;
                t.stations[side]
                    .iter()
                    .any(|&s| s > STATION_LOADED * weight)
            })
        })
        .sum();
    json!({
    "station_loaded": station_flips,
    "side_loaded": side_flips,
    "sampled_contact_count_left": changes(after, |t| t.spring_n[0]),
    "sampled_contact_count_right": changes(after, |t| t.spring_n[1]),
    "hull_proxy_touching": changes(after, |t| t.solver_points[3] > 0),
    "clutch": changes(after, |t| t.clutch.clone()),
    "gear": changes(after, |t| t.gear),
    "reverse": changes(after, |t| t.reverse),
    "park": changes(after, |t| t.park),
    "shifting": changes(after, |t| t.shift > 0),
    "belt_left_running": changes(after, |t| t.belt[0].abs() > BELT_RUNNING_M_S),
    "belt_right_running": changes(after, |t| t.belt[1].abs() > BELT_RUNNING_M_S),
    "belt_left_sign": changes(after, |t| t.belt[0] > 0.0),
    "belt_right_sign": changes(after, |t| t.belt[1] > 0.0),
    "sampled_majority_saturated": changes(after, |t| {
        2 * t.spring_saturated > t.spring_n[0] + t.spring_n[1]
    }),
    "piece_sleep_changes": after.iter().map(|t| t.piece_sleep_changes).sum::<usize>(),
    "tank_asleep": changes(after, |t| t.tank_asleep),
    })
}

fn run(case: &Case, tag: &str) {
    let (mut bed, site) = settle(case);
    let wheels_per_side = bed.geom().road_wheels.get(Side::Left).len();
    let max_droop = bed
        .app
        .world()
        .resource::<crate::track::sim::TrackGear>()
        .max_droop(Side::Left);
    let weight = bed.weight();
    let begin_y = bed.hull().position.y;
    let pieces_at_drive = bed.pieces.len();
    let (ticks, paused_at, leg_start) = drive(case, &mut bed, &site, begin_y);
    write_series(tag, &case.label, &ticks, weight);

    // From the first tick anything of the tank meets another body, or the readings' start.
    let first_touch = ticks
        .iter()
        .position(|t| t.solver_points.iter().sum::<usize>() > 0);
    let after = &ticks[first_touch.unwrap_or(ticks.len()).max(leg_start)..];
    let at_rest: Vec<&Tick> = ticks[leg_start.saturating_sub(TICKS)..leg_start]
        .iter()
        .collect();
    let driven: Vec<&Tick> = after.iter().filter(|t| t.throttle > 0.0).collect();
    let (stalled, moving): (Vec<&Tick>, Vec<&Tick>) = driven
        .iter()
        .copied()
        .partition(|t| t.speed.abs() < STALL_M_S);
    let longest_stall = after
        .iter()
        .fold((0, 0), |(run, best), t| {
            if t.throttle > 0.0 && t.speed.abs() < STALL_M_S {
                (run + 1, best.max(run + 1))
            } else {
                (0, best)
            }
        })
        .1;
    let end = ticks.last().expect("the drive ran");
    // Gear changes after the first touch, as (tick from the readings' start, gear), the first
    // stalled driven tick, and the first downshift at or after it: the stall's detection delay.
    let base = ticks.len() - after.len();
    let gear_steps: Vec<(usize, u8)> = after
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[0].gear != w[1].gear)
        .map(|(i, w)| (base + i + 1, w[1].gear))
        .collect();
    let first_stall = after
        .iter()
        .position(|t| t.throttle > 0.0 && t.speed.abs() < STALL_M_S)
        .map(|i| base + i);
    let downshift_after_stall = first_stall.and_then(|stall| {
        after
            .windows(2)
            .enumerate()
            .map(|(i, w)| (base + i + 1, w))
            .find(|(tick, w)| *tick > stall && w[1].gear < w[0].gear)
            .map(|(tick, _)| tick)
    });
    let clutch_out_ticks = after.iter().filter(|t| t.clutch == "Out").count();
    let line = json!({
        "tag": tag,
        "label": case.label,
        "throttle": case.throttle,
        "offset_m": case.offset,
        "tick_hz": TICKS,
        "heap": {
            "pieces": pieces_at_drive,
            "pieces_at_end": bed.pieces.len(),
            "pieces_awake_at_drive": site.awake_at_drive,
            "standing_chunks": site.standing_chunks,
            "centre_x": site.mid,
            "asleep_after_s": f(site.asleep_after_s),
            "top_m": site.top,
            "near_z": site.near,
            "far_z": site.far,
            "left_x": site.left,
            "right_x": site.right,
        },
        "rig": {
            "weight_n": weight,
            "wheels_per_side": wheels_per_side,
            "hull_rest_y_m": bed.rest_y,
            "view_max_droop_m": max_droop,
        },
        "outcome": {
            "first_touch_tick": first_touch,
            "paused_at_tick": paused_at,
            "leg_start_tick": leg_start,
            "leg_start_z": ticks.get(leg_start).map(|t| t.z),
            "ticks": ticks.len(),
            "end_z": end.z,
            "end_speed_m_s": end.speed,
            "hull_past_far_edge_m": site.far - end.z,
            "rise_max_m": ticks.iter().map(|t| t.rise).fold(0.0, f32::max),
            "driven_ticks": driven.len(),
            "stalled_ticks": stalled.len(),
            "longest_stall_ticks": longest_stall,
            "clutch_out_ticks_after_first_touch": clutch_out_ticks,
            "gear_steps": gear_steps,
            "first_stall_tick": first_stall,
            "first_downshift_after_stall_tick": downshift_after_stall,
        },
        "flips_after_first_touch": flips(after, weight, wheels_per_side),
        "at_rest_before_leg": carried(&at_rest, weight, wheels_per_side),
        "stalled": carried(&stalled, weight, wheels_per_side),
        "moving": carried(&moving, weight, wheels_per_side),
    });
    println!("RUBBLECLIMB {line}");
}

#[test]
#[ignore = "the rubble climb probe, run on demand"]
fn rubble_climb_probe() {
    let only = std::env::var("RUBBLE_CLIMB_ONLY").ok();
    let tag = std::env::var("RUBBLE_CLIMB_TAG").unwrap_or_else(|_| "run".to_owned());
    let mut cases = Vec::new();
    for (heap, name) in [
        (Heap::House, "house"),
        (Heap::Yard, "yard"),
        (Heap::Standing, "standing"),
    ] {
        for throttle in [0.3, 1.0] {
            for offset in [0.0, 0.7, 1.4] {
                cases.push(Case {
                    heap,
                    label: format!("{name}-t{throttle}-x{offset}"),
                    throttle,
                    offset,
                    mode: Mode::Through,
                });
                cases.push(Case {
                    heap,
                    label: format!("{name}-t{throttle}-x{offset}-stopgo"),
                    throttle,
                    offset,
                    mode: Mode::StopGo,
                });
            }
        }
        for offset in [0.0, 1.4] {
            cases.push(Case {
                heap,
                label: format!("{name}-t0.3-x{offset}-pause"),
                throttle: 0.3,
                offset,
                mode: Mode::StallPause,
            });
        }
    }
    // The ram across the house's width: how often the tank ends stalled.
    for step in -4..=4 {
        let offset = step as f32 * 0.35;
        cases.push(Case {
            heap: Heap::Standing,
            label: format!("ram-x{offset:.2}"),
            throttle: 1.0,
            offset,
            mode: Mode::Through,
        });
    }
    for case in &cases {
        if only.as_ref().is_some_and(|only| !case.label.contains(only)) {
            continue;
        }
        run(case, &tag);
    }
}
