//! The game's damage model, how a round moves across armour, game state only: what the contact walk
//! does per geometric class, on real shots through the real march ([`resolve::decide`] behind
//! [`resolve::at_first_touch`]). A measurement, run on demand; nothing here passes or fails on a
//! number, and the shipped tank is its fixture, never its spec:
//!
//! `scripts/serial-cargo test --lib --features dev_ui,bitprobe contact_classes_on_the_shipped_hull -- --ignored --nocapture`
//!
//! Its tables replace the block between [`TABLES_BEGIN`] and [`TABLES_END`] of [`RECORD`]; a rerun
//! reproduces the aims from [`SEED`].
//!
//! An aim is a point on a surface and a heading at a stated obliquity from that surface's normal,
//! drawn area-weighted ([`Source::Uniform`]), beside a triangle edge ([`Source::NearEdge`]) or on a
//! face looking into enclosed air ([`Source::Inside`]). Its class is read from the armour alone
//! ([`read_class`]), the disc laid at the round's calibre even where the round flies one line. A
//! shot's birth is set so its post-step line is the aimed one and it reaches first touch at the
//! aim's [`Aim::phase`] of its first tick ([`before_step`]). Every exchange the march decides is
//! noted through [`note`], which also times one contact read.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use avian3d::parry::shape::TypedShape;
use avian3d::prelude::{
    AngularVelocity, Collider, CollisionLayers, LayerMask, LinearVelocity, PhysicsPlugins,
    Position, RigidBody, Rotation, SpatialQueryFilter,
};
use bevy::ecs::system::RunSystemOnce;
use bevy::time::TimeUpdateStrategy;

use super::crew_sweep::{next, unit};
use super::fuzz::PROBE_TANK_AT;
use super::mechanism::SolidShot;
use super::walk::{Begin, DiscFrame, Shot};
use super::*;
use crate::tank::{TankSimSource, spawn_headless_tank};

/// The record the sweep's tables land in, from the manifest directory.
const RECORD: &str = ".agents/scratch/research/contact-classes-2026-11-03.md";
const TABLES_BEGIN: &str = "<!-- contact_classes tables begin -->";
const TABLES_END: &str = "<!-- contact_classes tables end -->";

/// The authority's fixed tick; its seconds are [`DT`].
const TICK: Duration = Duration::from_micros(15_625);
const DT: f32 = 0.015_625;

/// Fixture data: the draw's seed.
const SEED: u64 = 0x5eed_c0de_0260;
/// Fixture data: degrees from a face's normal.
const OBLIQUITIES: [f32; 4] = [45.0, 60.0, 75.0, 85.0];
/// Fixture data: a spent round's speed as a share of its muzzle speed.
const SPENT_SHARE: f32 = 0.25;
/// Fixture data: the sideways step between the two aims of a pair (m).
const PAIR_STEP: f32 = 5.0e-4;
/// Fixture data: the shares of a tick flown before first touch, for the phase series. The two
/// smallest are deliberately small: they bound the back-up [`resolve::at_first_touch`] may open.
const PHASES: [f32; 7] = [0.001, 0.01, 0.05, 0.25, 0.5, 0.75, 0.95];
/// Fixture data: ticks a shot is followed; a round still flying then is counted as flying on.
const MAX_TICKS: usize = 8;
/// Fixture data: a third aim gets a phase series; the rest would cost its seven shots each.
const PHASE_EVERY: usize = 3;
/// Fixture data: the unrelated moving body's speed (m/s) and how far off the tank it stands (m).
const UNRELATED_SPEED: f32 = 20.0;
const UNRELATED_AWAY: f32 = 40.0;
/// Fixture data: a moving target's speed (m/s), across the line of fire and closing on it.
const TARGET_SPEED: f32 = 15.0;
/// Fixture data: two faces within this angle's cosine read as one direction (about 15 degrees).
const PARALLEL_COS: f32 = 0.966;

/// How far one sweep looks, per class, obliquity and calibre.
#[derive(Clone, Copy)]
struct Effort {
    /// Aims kept per cell.
    per_cell: usize,
    /// Candidates tried per obliquity and calibre before the cell stands short.
    candidates: usize,
    /// Aims per class flown for the moving-target and unrelated-motion arms.
    moving: usize,
}

// --- The march's note ----------------------------------------------------------------------------

/// What the march decided at one contact, read in the frozen scene it was decided in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Miss,
    Skid,
    Held,
    ZeroTurn,
    Buried,
    Through,
}

impl Kind {
    fn bite(self) -> bool {
        matches!(self, Kind::Buried | Kind::Through)
    }

    fn rests(self) -> bool {
        matches!(self, Kind::Held | Kind::ZeroTurn)
    }
}

/// One sample's first entered face at a read: the identity a hit carries.
#[derive(Clone, Copy, Debug)]
struct Face {
    sample: usize,
    primitive: Entity,
    shell: u32,
    /// 0 a triangle's interior, 1 a welded edge, 2 a welded vertex.
    feature: u8,
    t: f64,
    point: Vec3,
    normal: Vec3,
    /// Primitives the welded run opening on this face spans.
    run_primitives: usize,
}

/// One decided exchange, in the frame it was decided in (a moving body's own, for its contact).
#[derive(Clone, Debug)]
pub(super) struct Note {
    kind: Kind,
    /// The flight held material when this was read.
    holds_material: bool,
    /// The distance discovery handed the decision.
    handed: f32,
    axis: Vec3,
    speed_in: f32,
    speed_out: f32,
    /// Heading out; `None` when the round stopped here.
    exit: Option<Vec3>,
    /// The mark: where the read says the round met the armour.
    strike: Vec3,
    /// The axis at the exchange.
    turn: Vec3,
    normal: Vec3,
    coverage: f32,
    /// Metres the axis flew to the exchange; 0 is an exchange at zero clock.
    clock: f32,
    /// The exchange deposited something: a bounce's jar, or a bite's chords.
    dose: bool,
    /// A perforation's exit face.
    hole: Option<Vec3>,
    /// The tracked body's position at the contact's moment.
    body_at: Option<Vec3>,
    faces: Vec<Face>,
    /// One contact read: the closing walk and the law's verdict on it.
    micros: f64,
}

/// The notes of the shot in flight. Its presence is what arms [`note`]; a world without it pays one
/// `Option` check, and parallel tests never share one.
#[derive(Resource)]
pub(super) struct ContactLog {
    notes: Mutex<Vec<Note>>,
    /// The body whose pose a note carries, for a moving target's own frame.
    tracked: Entity,
}

/// Note `decision` as [`resolve::decide`] made it. Reads only.
pub(super) fn note(
    context: &resolve::ResolveContext<'_, '_, '_>,
    admit: &dyn Fn(Entity) -> bool,
    round: &SolidShot,
    preset: crate::rounds::ContactPreset,
    flight: &resolve::Flight,
    distance: f32,
    decision: &resolve::Decision,
) {
    let Some(log) = context.world.contact_log.as_ref() else {
        return;
    };
    let interior = flight
        .continuation
        .as_ref()
        .expect("decide read this flight's starts");
    let axis = Vec3::from(flight.direction);
    let body = round.body();
    let radius = resolve::disc_radius(body.caliber);
    let began = Instant::now();
    let walked = resolve::closing_walk(
        context,
        admit,
        interior.anchor,
        interior.origin,
        axis,
        flight.frame,
        radius,
        &interior.seeds,
        distance + resolve::ENTRANCE_SPAN,
    )
    .expect("decide closed this walk in this scene");
    let begin = walk::begin(
        &walked,
        &Shot {
            caliber: body.caliber,
            speed: flight.speed,
            contact: preset,
        },
        &context.laws,
    )
    .expect("decide began this walk in this scene");
    let micros = began.elapsed().as_secs_f64() * 1.0e6;
    let corridor = resolve::build_corridor(
        context,
        admit,
        interior.anchor,
        interior.origin,
        axis,
        walked.length,
        flight.frame,
        radius,
        &interior.seeds,
    )
    .expect("the closing walk built this corridor");
    let start = interior.anchor + interior.origin;
    let mut faces = Vec::new();
    if let Some(hand_off) = walked.hand_off().expect("begin read this hand-off") {
        for &(sample, run) in &hand_off.read.runs {
            let run = &walked.walks[sample].runs[run];
            let lane = &corridor.samples[sample];
            if let Some(hit) = lane
                .hits
                .iter()
                .filter(|hit| hit.entering)
                .min_by(|a, b| (a.t - run.start).abs().total_cmp(&(b.t - run.start).abs()))
            {
                faces.push(face(
                    sample,
                    hit,
                    start + lane.offset,
                    axis,
                    run.primitives.len(),
                ));
            }
        }
    }
    let body_at = context.world.motion.as_ref().and_then(|motion| {
        let moment = context.world.moment.get()?;
        Some(motion.bodies.get(&log.tracked)?.at(moment).position)
    });
    let base = Note {
        kind: Kind::Miss,
        holds_material: flight.holds_material(),
        handed: distance,
        axis,
        speed_in: flight.speed,
        speed_out: flight.speed,
        exit: Some(axis),
        strike: start,
        turn: start,
        normal: Vec3::ZERO,
        coverage: 0.0,
        clock: 0.0,
        dose: false,
        hole: None,
        body_at,
        faces,
        micros,
    };
    let noted = match decision {
        resolve::Decision::Clear { resume, travel, .. } => Note {
            turn: *resume,
            strike: *resume,
            clock: *travel,
            ..base
        },
        resolve::Decision::Bounce(bounce) => {
            let kind = match (&begin, &bounce.continuation) {
                (Begin::Held { .. }, _) => Kind::Held,
                (_, Some(_)) => Kind::Skid,
                (_, None) => Kind::ZeroTurn,
            };
            Note {
                kind,
                speed_out: bounce.speed,
                exit: (bounce.speed > 0.0).then_some(Vec3::from(bounce.direction)),
                strike: bounce.position,
                turn: bounce.resume,
                normal: bounce.normal,
                coverage: bounce.coverage,
                clock: bounce.travel,
                dose: bounce.shock > 0.0,
                ..base
            }
        }
        resolve::Decision::Bite(bite) => {
            let (kind, speed_out, exit, hole) = match &bite.end {
                resolve::BiteEnd::Embedded { .. } => (Kind::Buried, 0.0, None, None),
                resolve::BiteEnd::Perforated {
                    exit,
                    direction,
                    speed,
                    ..
                } => (
                    Kind::Through,
                    *speed,
                    Some(Vec3::from(*direction)),
                    Some(*exit),
                ),
            };
            Note {
                kind,
                speed_out,
                exit,
                strike: bite.entrance,
                turn: start + axis * bite.approach,
                normal: bite.normal,
                coverage: bite.coverage,
                clock: bite.approach,
                dose: !bite.deposits.is_empty(),
                hole,
                ..base
            }
        }
    };
    log.notes
        .lock()
        .expect("the march notes from one system")
        .push(noted);
}

/// The [`Face`] of `hit`, on the ray from `from` along `axis`.
fn face(sample: usize, hit: &walk::FaceHit, from: Vec3, axis: Vec3, run_primitives: usize) -> Face {
    Face {
        sample,
        primitive: hit.primitive,
        shell: hit.shell,
        feature: match hit.contact {
            walk::Contact::Face(_) => 0,
            walk::Contact::Edge(..) => 1,
            walk::Contact::Vertex(_) => 2,
        },
        t: hit.t,
        point: from + axis * hit.t as f32,
        normal: hit.true_normal,
        run_primitives,
    }
}

// --- The range -----------------------------------------------------------------------------------

#[derive(Resource)]
struct Mount(RigidBody);

#[derive(Resource)]
struct Mounted(Entity);

fn mount_tank(
    mut commands: Commands,
    source: TankSimSource,
    mount: Res<Mount>,
    mounted: Option<Res<Mounted>>,
) {
    if mounted.is_some() {
        return;
    }
    let content = source.get();
    let tank = spawn_headless_tank(
        &mut commands,
        content,
        (
            Transform::from_translation(PROBE_TANK_AT),
            Name::new(content.spec().display_name.clone()),
            mount.0,
        ),
    );
    commands.insert_resource(Mounted(tank));
}

/// The sim as it ships (march, collider motion, fixed tick) around the shipped tank. No damage
/// plugin: an emptied pool has no consequence here, and every shot starts from full pools.
struct Range {
    app: App,
    mount: RigidBody,
    root: Entity,
    /// The unrelated moving body of a kinematic range.
    decoy: Option<Entity>,
}

impl Range {
    fn new(mount: RigidBody) -> Self {
        Self::on(mount, None)
    }

    /// The range over `ground`, which the march reads as it reads any live map's, or over none.
    /// Only a kinematic range without ground carries the decoy: on a map it would stand in the
    /// shots' way.
    fn on(mount: RigidBody, ground: Option<HeightGrid>) -> Self {
        let decoyed = mount == RigidBody::Kinematic && ground.is_none();
        let mut app = App::new();
        if let Some(ground) = ground {
            app.insert_resource(ground);
        }
        app.add_plugins((
            MinimalPlugins,
            bevy::transform::TransformPlugin,
            AssetPlugin::default(),
            PhysicsPlugins::default(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(TimeUpdateStrategy::ManualDuration(TICK))
        .insert_resource(RetainSpentShells(true))
        .insert_resource(Mount(mount))
        .add_plugins((crate::bake::plugin, sim_plugin))
        .add_systems(Update, mount_tank);
        while app.plugins_state() == bevy::app::PluginsState::Adding {
            std::thread::sleep(Duration::from_millis(1));
        }
        app.finish();
        app.cleanup();
        for _ in 0..16 {
            app.update();
            if app.world().get_resource::<Mounted>().is_some() {
                break;
            }
        }
        for _ in 0..8 {
            app.update();
        }
        let root = app.world().resource::<Mounted>().0;
        app.insert_resource(ContactLog {
            notes: Mutex::new(Vec::new()),
            tracked: root,
        });
        let decoy = decoyed.then(|| {
            let decoy = app
                .world_mut()
                .spawn((
                    Transform::from_translation(decoy_at()),
                    RigidBody::Kinematic,
                    LinearVelocity::ZERO,
                    Collider::cuboid(2.0, 2.0, 2.0),
                    CollisionLayers::new([crate::Layer::Armor], LayerMask::ALL),
                ))
                .id();
            for _ in 0..4 {
                app.update();
            }
            decoy
        });
        Self {
            app,
            mount,
            root,
            decoy,
        }
    }

    /// Stand the tank at its spawn pose moving at `velocity` and the decoy at its own moving at
    /// `decoy`, one whole tick into that motion: the next tick's interval starts at the returned
    /// tank position.
    fn place(&mut self, velocity: Vec3, decoy: Vec3) -> Vec3 {
        let (root, other) = (self.root, self.decoy);
        let world = self.app.world_mut();
        world.entity_mut(root).insert((
            Transform::from_translation(PROBE_TANK_AT),
            Position(PROBE_TANK_AT),
            Rotation::IDENTITY,
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
        ));
        if let Some(other) = other {
            world.entity_mut(other).insert((
                Transform::from_translation(decoy_at()),
                Position(decoy_at()),
                Rotation::IDENTITY,
                LinearVelocity::ZERO,
                AngularVelocity::ZERO,
            ));
        }
        self.app.update();
        let world = self.app.world_mut();
        world.entity_mut(root).insert(LinearVelocity(velocity));
        if let Some(other) = other {
            world.entity_mut(other).insert(LinearVelocity(decoy));
        }
        self.app.update();
        self.app
            .world()
            .get::<Position>(root)
            .expect("the tank root is a body")
            .0
    }

    /// Fly `round` from `birth` so its post-step velocity is `post`, and read what the march noted.
    /// A panic in the march is a defect in the game and a poisoned world: it is reported, and the
    /// range rebuilt.
    fn fly(&mut self, round: &crate::rounds::Round, birth: Vec3, post: Vec3) -> Flown {
        let launch = before_step(post, round.drag_k);
        let direction = Dir3::new(launch).expect("an aimed velocity has a heading");
        let mut fire = FireShell::of_round(
            round,
            birth,
            birth,
            direction,
            crate::spec::FireMechanism::Single,
            false,
            None,
            None,
        );
        fire.speed = launch.length();
        let app = &mut self.app;
        let flew = catch_unwind(AssertUnwindSafe(|| {
            app.world_mut().trigger(fire);
            for _ in 0..MAX_TICKS {
                app.update();
                let world = app.world_mut();
                if world
                    .query_filtered::<(), With<Projectile>>()
                    .iter(world)
                    .next()
                    .is_none()
                {
                    break;
                }
            }
        }));
        if let Err(payload) = flew {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic with no message".to_owned());
            *self = Range::new(self.mount);
            return Flown {
                notes: Vec::new(),
                burst: false,
                panicked: Some(message),
            };
        }
        let world = self.app.world_mut();
        let shells: Vec<Entity> = world
            .query_filtered::<Entity, With<ShellPath>>()
            .iter(world)
            .collect();
        let burst = shells.iter().any(|&shell| {
            world.get::<PenetrationMarks>(shell).is_some_and(|marks| {
                marks
                    .log
                    .iter()
                    .any(|mark| matches!(mark, ArmourMark::Burst { .. }))
            })
        });
        for shell in shells {
            world.despawn(shell);
        }
        for mut health in world.query::<&mut ComponentHealth>().iter_mut(world) {
            health.refill();
        }
        let notes = std::mem::take(
            &mut *world
                .resource::<ContactLog>()
                .notes
                .lock()
                .expect("no march is running between shots"),
        );
        Flown {
            notes,
            burst,
            panicked: None,
        }
    }
}

fn decoy_at() -> Vec3 {
    PROBE_TANK_AT + Vec3::X * UNRELATED_AWAY
}

/// The launch velocity whose one [`freeflight_step`] gives `post`: the inverse of the step's drag
/// and gravity, so the march's first discovery runs on the aimed line at the aimed speed.
fn before_step(post: Vec3, drag_k: f32) -> Vec3 {
    let speed = post.length();
    post / speed * (speed / (1.0 - drag_k * speed * DT)) - gravity() * DT
}

/// A shot's exchanges, as the march noted them.
#[derive(Clone, Debug)]
struct Flown {
    notes: Vec<Note>,
    burst: bool,
    panicked: Option<String>,
}

impl Flown {
    fn contacts(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter().filter(|note| note.kind != Kind::Miss)
    }

    fn first(&self) -> Option<&Note> {
        self.contacts().next()
    }
}

// --- The armour and the classes ------------------------------------------------------------------

/// One armour triangle in the world.
#[derive(Clone, Copy)]
struct WorldTriangle {
    corners: [Vec3; 3],
    normal: Vec3,
    area: f32,
    primitive: Entity,
    shell: u32,
}

fn world_triangles(
    colliders: Query<(Entity, &Position, &Rotation, &Collider, &BallisticSurfaces)>,
) -> Vec<WorldTriangle> {
    let mut out = Vec::new();
    for (entity, position, rotation, collider, surfaces) in &colliders {
        let TypedShape::TriMesh(mesh) = collider.shape_scaled().as_typed_shape() else {
            continue;
        };
        for index in 0..surfaces.shells.len() {
            let triangle = mesh.triangle(index as u32);
            let corners =
                [triangle.a, triangle.b, triangle.c].map(|corner| position.0 + rotation.0 * corner);
            let cross = (corners[1] - corners[0]).cross(corners[2] - corners[0]);
            let Some(normal) = cross.try_normalize() else {
                continue;
            };
            out.push(WorldTriangle {
                corners,
                normal,
                area: cross.length() * 0.5,
                primitive: entity,
                shell: surfaces.shells[index],
            });
        }
    }
    out
}

/// The geometric classes the sweep reports. `Curved` is a class the brief did not list: a disc
/// wholly on convex facets within the walk's own coplanar cosine, split out so it does not count as
/// an edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Class {
    Full,
    Thin,
    Edge,
    Strip,
    Inside,
    Gap,
    Raised,
    Seam,
    Wedge,
    Curved,
    Moving,
}

impl Class {
    const ALL: [Class; 11] = [
        Class::Full,
        Class::Thin,
        Class::Edge,
        Class::Strip,
        Class::Inside,
        Class::Gap,
        Class::Raised,
        Class::Seam,
        Class::Wedge,
        Class::Curved,
        Class::Moving,
    ];

    fn name(self) -> &'static str {
        match self {
            Class::Full => "full plate",
            Class::Thin => "thin plate",
            Class::Edge => "edge",
            Class::Strip => "strip",
            Class::Inside => "inside face",
            Class::Gap => "narrow gap",
            Class::Raised => "raised detail",
            Class::Seam => "seam",
            Class::Wedge => "wedge",
            Class::Curved => "curved facets",
            Class::Moving => "moving target",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Uniform,
    NearEdge,
    Inside,
}

#[derive(Clone, Copy)]
struct Candidate {
    source: Source,
    point: Vec3,
    along: Vec3,
    normal: Vec3,
    primitive: Entity,
    theta: f32,
    calibre: f32,
    flies_a_disc: bool,
    lateral: Vec3,
    phase: f32,
    /// How far the corridor's start is drawn back to stand clear of the tank.
    far: f32,
}

/// An aim that reached its surface, with what its armour read at classification.
#[derive(Clone, Debug)]
struct Aim {
    class: Class,
    source: Source,
    group: usize,
    theta: usize,
    start: Vec3,
    along: Vec3,
    /// Metres from `start` to first touch of the disc the round really flies.
    first: f32,
    lateral: Vec3,
    /// The share of the first tick flown before first touch.
    phase: f32,
    /// The welded coplanar patch under the disc spans this many primitives and shells.
    primitives: usize,
    shells: usize,
    /// Most primitives in one sample's first welded run.
    run_primitives: usize,
    /// Faces whose hit is a welded edge or vertex rather than a triangle's interior.
    features: [usize; 3],
}

fn coplanar(a: &Face, b: &Face, laws: &walk::WalkLaws) -> bool {
    a.normal.dot(b.normal) >= laws.event_plane_cos
        && (b.point - a.point).dot(a.normal).abs() <= laws.event_plane_tolerance
}

/// Each sample's first entered face on `corridor`, in sample order.
fn first_faces(
    corridor: &walk::DiscCorridor,
    walked: &walk::DiscWalk,
    start: Vec3,
    along: Vec3,
) -> Vec<Option<Face>> {
    corridor
        .samples
        .iter()
        .enumerate()
        .map(|(sample, lane)| {
            lane.hits
                .iter()
                .filter(|hit| hit.entering)
                .min_by(|a, b| a.t.total_cmp(&b.t))
                .map(|hit| {
                    let run_primitives = walked.walks[sample]
                        .runs
                        .first()
                        .map_or(0, |run| run.primitives.len());
                    face(sample, hit, start + lane.offset, along, run_primitives)
                })
        })
        .collect()
}

/// The class `faces` read, with the welded patch under the disc. Mutually exclusive, in this order:
/// a wedge (one other face, not parallel, rising off the base within a calibre), raised detail (a
/// proud face), a strip (the base covers the disc across its middle and leaves it on two sides), an
/// edge, curved facets, a seam (one plane, more than one primitive or shell), a narrow gap (the
/// plate's far side within two calibres of the next plate), a thin plate, a full plate.
fn read_class(
    faces: &[Option<Face>],
    walked: &walk::DiscWalk,
    along: Vec3,
    calibre: f32,
    laws: &walk::WalkLaws,
) -> Option<(Class, Vec<Face>)> {
    let present: Vec<Face> = faces.iter().flatten().copied().collect();
    let nearest = *present.iter().min_by(|a, b| a.t.total_cmp(&b.t))?;
    let near = |face: &&Face| (face.point - nearest.point).dot(nearest.normal).abs() <= calibre;
    let mut clusters: Vec<Vec<Face>> = Vec::new();
    for face in present.iter().filter(near) {
        match clusters
            .iter_mut()
            .find(|cluster| coplanar(&cluster[0], face, laws))
        {
            Some(cluster) => cluster.push(*face),
            None => clusters.push(vec![*face]),
        }
    }
    let base_at = (0..clusters.len()).max_by_key(|&at| clusters[at].len())?;
    let base = clusters[base_at].clone();
    let normal = base[0].normal;
    let rise_of = |cluster: &[Face]| {
        cluster
            .iter()
            .map(|face| (face.point - base[0].point).dot(normal))
            .fold(f32::NEG_INFINITY, f32::max)
    };
    let tolerance = laws.event_plane_tolerance;
    let others: Vec<&Vec<Face>> = clusters
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != base_at)
        .map(|(_, cluster)| cluster)
        .collect();
    let rising: Vec<&&Vec<Face>> = others
        .iter()
        .filter(|cluster| rise_of(cluster) > tolerance)
        .collect();
    let covered: usize = clusters.iter().map(Vec::len).sum();
    // A plate narrower than the disc: the base holds the ring in two or more arcs, or the axis alone.
    let narrow = base.len() < faces.len() && {
        let on = |sample: usize| base.iter().any(|face| face.sample == sample);
        let ring = faces.len() - 1;
        let arcs = (0..ring)
            .filter(|&at| on(1 + at) && !on(1 + (at + 1) % ring))
            .count();
        arcs >= 2 || (on(0) && arcs == 0)
    };
    let class = if rising.len() == 1 && rising[0][0].normal.dot(normal) < PARALLEL_COS {
        Class::Wedge
    } else if !rising.is_empty() {
        Class::Raised
    } else if narrow {
        Class::Strip
    } else if !others.is_empty() {
        let facets = covered == faces.len()
            && others
                .iter()
                .all(|cluster| cluster[0].normal.dot(normal) >= laws.event_plane_cos);
        if facets { Class::Curved } else { Class::Edge }
    } else if base.len() < faces.len() {
        Class::Edge
    } else {
        let identities: BTreeSet<(Entity, u32)> = base
            .iter()
            .map(|face| (face.primitive, face.shell))
            .collect();
        if identities.len() > 1 {
            Class::Seam
        } else {
            let square = along.dot(normal).abs();
            let runs = &walked.walks[0].runs;
            let through = runs
                .first()
                .map_or(f32::INFINITY, |run| (run.end - run.start) as f32 * square);
            let gap = runs
                .get(1)
                .zip(runs.first())
                .map(|(after, run)| (after.start - run.end) as f32 * square);
            if gap.is_some_and(|gap| gap <= 2.0 * calibre) {
                Class::Gap
            } else if through < calibre {
                Class::Thin
            } else {
                Class::Full
            }
        }
    };
    Some((class, base))
}

/// Whether air surrounds `point`: on each world axis the first face met is entered, not left.
fn enclosed(context: &resolve::ResolveContext<'_, '_, '_>, point: Vec3) -> bool {
    [
        Vec3::X,
        Vec3::NEG_X,
        Vec3::Y,
        Vec3::NEG_Y,
        Vec3::Z,
        Vec3::NEG_Z,
    ]
    .into_iter()
    .all(|axis| {
        let frame = DiscFrame::anchored(axis).expect("a world axis");
        let Ok(corridor) = resolve::build_corridor(
            context,
            &|_| true,
            point,
            Vec3::ZERO,
            axis,
            12.0,
            frame,
            0.0,
            &[],
        ) else {
            return false;
        };
        corridor.samples[0]
            .hits
            .iter()
            .min_by(|a, b| a.t.total_cmp(&b.t))
            .is_some_and(|hit| hit.entering)
    })
}

/// Read each candidate against the armour; `None` where it never reaches its own surface.
fn classify_batch(
    In((batch, group, theta_at)): In<(Vec<Candidate>, usize, usize)>,
    world: ProjectileMarchWorld,
    colliders: Query<(
        &'static Position,
        &'static Rotation,
        &'static Collider,
        Option<&'static BallisticSurfaces>,
    )>,
) -> Vec<Option<Aim>> {
    world.moment.set(Some(0.0));
    let context = resolve::ResolveContext {
        world: &world,
        colliders: &colliders,
        deposit: true,
        laws: walk::WalkLaws::default(),
    };
    batch
        .into_iter()
        .map(|candidate| classify(&context, &candidate, group, theta_at))
        .collect()
}

fn classify(
    context: &resolve::ResolveContext<'_, '_, '_>,
    candidate: &Candidate,
    group: usize,
    theta_at: usize,
) -> Option<Aim> {
    let &Candidate {
        source,
        point,
        along,
        normal,
        primitive,
        theta,
        calibre,
        flies_a_disc,
        lateral,
        phase,
        far,
    } = candidate;
    let radians = theta.to_radians();
    let radius = calibre * 0.5;
    let frame = DiscFrame::anchored(along)?;
    let filter = SpatialQueryFilter::from_mask(crate::Layer::Armor);
    let heading = Dir3::new(along).ok()?;
    let lead = 0.1 + 1.5 * radius * radians.tan();
    let (start, back) = if source == Source::Inside {
        let lifted = point + normal * calibre.max(0.03);
        if !enclosed(context, lifted) {
            return None;
        }
        let room = context
            .world
            .spatial
            .cast_ray(
                point - along * 1.0e-3,
                Dir3::new(-along).ok()?,
                far,
                true,
                &filter,
            )
            .map_or(far, |hit| hit.distance);
        let back = (room * 0.5).min(1.0);
        if back < lead {
            return None;
        }
        (point - along * back, back)
    } else {
        let start = point - along * lead;
        for offset in resolve::layout(&frame, radius) {
            if context
                .world
                .spatial
                .cast_ray(start + offset - along * far, heading, far, true, &filter)
                .is_some()
            {
                return None;
            }
        }
        (start, lead)
    };
    let reach = back + 1.0 + 2.0 * calibre / radians.cos();
    let walked = resolve::closing_walk(
        context,
        &|_| true,
        start,
        Vec3::ZERO,
        along,
        frame,
        radius,
        &[],
        reach,
    )
    .ok()?;
    let corridor = resolve::build_corridor(
        context,
        &|_| true,
        start,
        Vec3::ZERO,
        along,
        walked.length,
        frame,
        radius,
        &[],
    )
    .ok()?;
    let faces = first_faces(&corridor, &walked, start, along);
    let (class, base) = read_class(&faces, &walked, along, calibre, &context.laws)?;
    // The aim's own surface is among what the disc first meets, and the base it is labelled by
    // stands at the labelled obliquity.
    let on_aim = faces.iter().flatten().any(|face| {
        face.primitive == primitive && (face.point - point).dot(normal).abs() <= 2.0e-3
    });
    let read_theta = (-along.dot(base[0].normal))
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees();
    if !on_aim || (read_theta - theta).abs() > 10.0 {
        return None;
    }
    let first = if flies_a_disc {
        faces.iter().flatten().map(|face| face.t).reduce(f64::min)?
    } else {
        faces[0]
            .as_ref()
            .or_else(|| faces.iter().flatten().next())
            .map(|face| face.t)?
    } as f32;
    let mut features = [0; 3];
    for face in faces.iter().flatten() {
        features[usize::from(face.feature)] += 1;
    }
    Some(Aim {
        class: if source == Source::Inside {
            Class::Inside
        } else {
            class
        },
        source,
        group,
        theta: theta_at,
        start,
        along,
        first,
        lateral,
        phase,
        primitives: base
            .iter()
            .map(|face| face.primitive)
            .collect::<BTreeSet<_>>()
            .len(),
        shells: base
            .iter()
            .map(|face| (face.primitive, face.shell))
            .collect::<BTreeSet<_>>()
            .len(),
        run_primitives: faces
            .iter()
            .flatten()
            .map(|face| face.run_primitives)
            .max()
            .unwrap_or(0),
        features,
    })
}

/// One candidate from `source` at `theta` degrees.
fn draw(
    state: &mut u64,
    triangles: &[WorldTriangle],
    areas: &[f32],
    source: Source,
    theta: f32,
    calibre: f32,
    far: f32,
) -> Candidate {
    let total = *areas.last().expect("the tank has armour triangles");
    let picked = if source == Source::NearEdge {
        (next(state) % triangles.len() as u64) as usize
    } else {
        let wanted = unit(state) * total;
        areas
            .partition_point(|&area| area < wanted)
            .min(triangles.len() - 1)
    };
    let triangle = &triangles[picked];
    let [a, b, c] = triangle.corners;
    let point = if source == Source::NearEdge {
        let edge = (next(state) % 3) as usize;
        let (from, to, away) = (
            triangle.corners[edge],
            triangle.corners[(edge + 1) % 3],
            triangle.corners[(edge + 2) % 3],
        );
        let run = to - from;
        let inward = (away - from).reject_from(run).normalize_or_zero();
        from + run * unit(state) + inward * (unit(state) * 2.0 - 1.0) * calibre * 0.75
    } else {
        let (u, v) = (unit(state), unit(state));
        let (u, v) = if u + v > 1.0 {
            (1.0 - u, 1.0 - v)
        } else {
            (u, v)
        };
        a + (b - a) * u + (c - a) * v
    };
    let normal = triangle.normal;
    let tangent = normal.any_orthonormal_vector();
    let bitangent = normal.cross(tangent);
    let azimuth = unit(state) * std::f32::consts::TAU;
    let (sin, cos) = theta.to_radians().sin_cos();
    let along =
        (-normal * cos + (tangent * azimuth.cos() + bitangent * azimuth.sin()) * sin).normalize();
    let (across, up) = along.any_orthonormal_pair();
    let roll = unit(state) * std::f32::consts::TAU;
    Candidate {
        source,
        point,
        along,
        normal,
        primitive: triangle.primitive,
        theta,
        calibre,
        flies_a_disc: resolve::disc_radius(calibre) > 0.0,
        lateral: across * roll.cos() + up * roll.sin(),
        phase: unit(state),
        far,
    }
}

// --- The sweep -----------------------------------------------------------------------------------

/// One round the sweep fires, in its calibre's group.
struct Arm {
    key: String,
    round: crate::rounds::Round,
    group: usize,
}

/// A shot flown, with its pair and its phase series.
struct Row {
    class: Class,
    arm: usize,
    group: usize,
    theta: usize,
    /// 0 muzzle, 1 spent, 2 a gap's just-through speed.
    speed: usize,
    birth: Vec3,
    post: Vec3,
    flown: Flown,
    pair: Option<Flown>,
    phases: Vec<Flown>,
}

const SPEED_LABELS: [&str; 3] = ["muzzle", "spent", "just through"];

/// The world birth of `aim` flown at `speed`, at `phase` of its first tick.
fn birth_of(aim: &Aim, speed: f32, phase: f32) -> Vec3 {
    if aim.class == Class::Inside {
        aim.start
    } else {
        aim.start + aim.along * (aim.first - phase * speed * DT)
    }
}

fn median_p95_worst(values: &mut [f32]) -> (f32, f32, f32) {
    if values.is_empty() {
        return (f32::NAN, f32::NAN, f32::NAN);
    }
    values.sort_by(f32::total_cmp);
    let at = |share: f32| values[((values.len() - 1) as f32 * share).round() as usize];
    (at(0.5), at(0.95), values[values.len() - 1])
}

fn percent(part: usize, whole: usize) -> String {
    if whole == 0 {
        "-".to_owned()
    } else {
        format!("{:.1} %", 100.0 * part as f64 / whole as f64)
    }
}

fn triple(values: &mut [f32], digits: usize) -> String {
    let (median, p95, worst) = median_p95_worst(values);
    format!("{median:.digits$} | {p95:.digits$} | {worst:.digits$}")
}

/// The first-exchange shares of `rows`, one table row.
fn outcome_row(label: &str, rows: &[&Row]) -> String {
    let n = rows.len();
    let mut counts = BTreeMap::<Kind, usize>::new();
    let mut none = 0;
    for row in rows {
        match row.flown.first() {
            Some(note) => *counts.entry(note.kind).or_default() += 1,
            None => none += 1,
        }
    }
    let share = |kind: Kind| percent(counts.get(&kind).copied().unwrap_or(0), n);
    let anywhere = rows
        .iter()
        .filter(|row| row.flown.contacts().any(|note| note.kind == Kind::ZeroTurn))
        .count();
    format!(
        "| {label} | {n} | {} | {} | {} | {} | {} | {} | {} |\n",
        percent(none, n),
        share(Kind::Skid),
        share(Kind::Held),
        share(Kind::ZeroTurn),
        share(Kind::Buried),
        share(Kind::Through),
        percent(anywhere, n),
    )
}

const OUTCOME_HEAD: &str = "| | n | no contact | skid | Held | zero-turn rest | buried | through | zero-turn anywhere in the shot |\n|---|---|---|---|---|---|---|---|---|\n";

/// What the sweep gathered, for its tables.
struct Sweep<'a> {
    rows: &'a [Row],
    arms: &'a [Arm],
    groups: &'a [f32],
    /// The aims flown.
    aims: &'a [Aim],
    /// Every uniform draw that read a class, kept or over quota: a hull-wide rate.
    uniform: &'a [Aim],
    effort: Effort,
    mesh: &'a str,
    motion: &'a str,
    defects: &'a [String],
}

/// The sweep's tables as Markdown.
#[expect(
    clippy::too_many_lines,
    reason = "one report, table after table, in the order the record reads"
)]
fn tables(sweep: &Sweep<'_>) -> String {
    let &Sweep {
        rows,
        arms,
        groups,
        aims,
        uniform,
        effort,
        mesh,
        motion,
        defects,
    } = sweep;
    let mut out = String::new();
    let w = &mut out;
    let flown = |row: &&Row| row.flown.panicked.is_none();
    let rows: Vec<&Row> = rows.iter().filter(flown).collect();
    let owned: Vec<&Row> = rows.clone();
    let class_rows = |class: Class| -> Vec<&Row> {
        owned
            .iter()
            .copied()
            .filter(|row| row.class == class)
            .collect()
    };

    writeln!(
        w,
        "All tables MEASURED on the shipped hull (the fixture), edit-profile test binary, seed \
         `{SEED:#x}`, up to {} aims per class, obliquity and calibre. Calibres fired: {}. Rounds: \
         {}. Speeds: muzzle and {SPENT_SHARE} of it (fixture). Phase: uniform per aim.\n",
        effort.per_cell,
        groups
            .iter()
            .map(|calibre| format!("{:.1} mm", calibre * 1000.0))
            .collect::<Vec<_>>()
            .join(", "),
        arms.iter()
            .map(|arm| arm.key.as_str())
            .collect::<Vec<_>>()
            .join(", "),
    )
    .ok();

    writeln!(
        w,
        "### T0. What the armour offers: aims kept, and the class mix of uniform aims\n"
    )
    .ok();
    writeln!(w, "Uniform-aim share is a rate over the hull's surface area; other columns are quotas, not rates.\n").ok();
    write!(w, "| class |").ok();
    for calibre in groups {
        write!(
            w,
            " {:.1} mm kept | {:.1} mm uniform mix |",
            calibre * 1000.0,
            calibre * 1000.0
        )
        .ok();
    }
    writeln!(w).ok();
    write!(w, "|---|").ok();
    for _ in groups {
        write!(w, "---|---|").ok();
    }
    writeln!(w).ok();
    for class in Class::ALL {
        write!(w, "| {} |", class.name()).ok();
        for (group, _) in groups.iter().enumerate() {
            let kept = aims
                .iter()
                .filter(|aim| aim.group == group && aim.class == class)
                .count();
            let uniform_all = uniform.iter().filter(|aim| aim.group == group).count();
            let uniform_class = uniform
                .iter()
                .filter(|aim| aim.group == group && aim.class == class)
                .count();
            write!(w, " {kept} | {} |", percent(uniform_class, uniform_all)).ok();
        }
        writeln!(w).ok();
    }
    writeln!(w).ok();

    writeln!(w, "### T1. Outcome of the first exchange, per class\n").ok();
    write!(w, "{OUTCOME_HEAD}").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        if !of.is_empty() {
            write!(w, "{}", outcome_row(class.name(), &of)).ok();
        }
    }
    writeln!(w).ok();

    writeln!(
        w,
        "### T1a. Zero-turn rests anywhere in the shot, by reason\n"
    )
    .ok();
    writeln!(w, "A zero-turn rest is a skid the march ended where it read it: the round stops and is charged its whole speed. `held material`: a sample's start was inside armour. `no approach`: nothing held and no distance to turn in. Position: first contact, after a skid, after a bite.\n").ok();
    writeln!(w, "| class | shots | with one | share | held material | no approach | other | first contact | after skid | after bite | on the surface just skidded | median mm since the last exchange | mean speed lost (m/s) | coverage below 1 |").ok();
    writeln!(
        w,
        "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
    )
    .ok();
    let walk_laws = walk::WalkLaws::default();
    for class in Class::ALL {
        let of = class_rows(class);
        if of.is_empty() {
            continue;
        }
        let mut with = 0;
        let (mut held, mut none, mut other) = (0, 0, 0);
        let (mut first, mut after_skid, mut after_bite, mut same_surface) = (0, 0, 0, 0);
        let (mut lost, mut count, mut partial) = (0.0_f64, 0, 0);
        let mut since: Vec<f32> = Vec::new();
        for row in &of {
            let contacts: Vec<&Note> = row.flown.contacts().collect();
            let mut any = false;
            for (at, note) in contacts.iter().enumerate() {
                if note.kind != Kind::ZeroTurn {
                    continue;
                }
                any = true;
                count += 1;
                lost += f64::from(note.speed_in);
                partial += usize::from(note.coverage < 1.0);
                if note.holds_material {
                    held += 1;
                } else if note.handed == 0.0 {
                    none += 1;
                } else {
                    other += 1;
                }
                match at.checked_sub(1).map(|before| contacts[before]) {
                    None => first += 1,
                    Some(before) => {
                        if before.kind.bite() {
                            after_bite += 1;
                        } else {
                            after_skid += 1;
                            same_surface += usize::from(one_contact(before, note, &walk_laws));
                        }
                        since.push((note.turn - before.turn).length() * 1000.0);
                    }
                }
            }
            with += usize::from(any);
        }
        let (since_median, _, _) = median_p95_worst(&mut since);
        writeln!(
            w,
            "| {} | {} | {with} | {} | {held} | {none} | {other} | {first} | {after_skid} | {after_bite} | {same_surface} | {since_median:.1} | {:.0} | {partial} |",
            class.name(),
            of.len(),
            percent(with, of.len()),
            if count == 0 { 0.0 } else { lost / f64::from(count) },
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(w, "### T1b. First exchange per class and obliquity\n").ok();
    write!(w, "{OUTCOME_HEAD}").ok();
    for class in Class::ALL {
        for (at, theta) in OBLIQUITIES.iter().enumerate() {
            let of: Vec<&Row> = class_rows(class)
                .into_iter()
                .filter(|row| row.theta == at)
                .collect();
            if !of.is_empty() {
                write!(
                    w,
                    "{}",
                    outcome_row(&format!("{} {theta:.0}°", class.name()), &of)
                )
                .ok();
            }
        }
    }
    writeln!(w).ok();

    writeln!(w, "### T1c. First exchange per class and round\n").ok();
    write!(w, "{OUTCOME_HEAD}").ok();
    for class in Class::ALL {
        for (at, arm) in arms.iter().enumerate() {
            let of: Vec<&Row> = class_rows(class)
                .into_iter()
                .filter(|row| row.arm == at)
                .collect();
            if !of.is_empty() {
                write!(
                    w,
                    "{}",
                    outcome_row(&format!("{} {}", class.name(), arm.key), &of)
                )
                .ok();
            }
        }
    }
    writeln!(w).ok();

    writeln!(w, "### T1d. First exchange per class and speed\n").ok();
    write!(w, "{OUTCOME_HEAD}").ok();
    for class in Class::ALL {
        for (at, label) in SPEED_LABELS.iter().enumerate() {
            let of: Vec<&Row> = class_rows(class)
                .into_iter()
                .filter(|row| row.speed == at)
                .collect();
            if !of.is_empty() {
                write!(
                    w,
                    "{}",
                    outcome_row(&format!("{} {label}", class.name()), &of)
                )
                .ok();
            }
        }
    }
    writeln!(w).ok();

    writeln!(
        w,
        "### T1f. Exit speed share of a first-exchange skid, per obliquity, against sin θ\n"
    )
    .ok();
    writeln!(
        w,
        "| class | θ label | sin θ | skids | median | 95th | worst |\n|---|---|---|---|---|---|---|"
    )
    .ok();
    for class in Class::ALL {
        for (at, theta) in OBLIQUITIES.iter().enumerate() {
            let mut shares: Vec<f32> = class_rows(class)
                .into_iter()
                .filter(|row| row.theta == at)
                .filter_map(|row| row.flown.first())
                .filter(|note| note.kind == Kind::Skid)
                .map(|note| note.speed_out / note.speed_in.max(1.0))
                .collect();
            if !shares.is_empty() {
                writeln!(
                    w,
                    "| {} | {theta:.0}° | {:.3} | {} | {} |",
                    class.name(),
                    theta.to_radians().sin(),
                    shares.len(),
                    triple(&mut shares, 3),
                )
                .ok();
            }
        }
    }
    writeln!(w).ok();

    writeln!(w, "### T1e. How the shot ended\n").ok();
    writeln!(
        w,
        "| class | n | flying on | rest | buried | a burst mark |\n|---|---|---|---|---|---|"
    )
    .ok();
    for class in Class::ALL {
        let of = class_rows(class);
        if of.is_empty() {
            continue;
        }
        let (mut on, mut rest, mut buried, mut burst) = (0, 0, 0, 0);
        for row in &of {
            burst += usize::from(row.flown.burst);
            match row.flown.contacts().last().map(|note| note.kind) {
                Some(kind) if kind.rests() => rest += 1,
                Some(Kind::Buried) => buried += 1,
                _ => on += 1,
            }
        }
        let n = of.len();
        writeln!(
            w,
            "| {} | {n} | {} | {} | {} | {} |",
            class.name(),
            percent(on, n),
            percent(rest, n),
            percent(buried, n),
            percent(burst, n),
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(
        w,
        "### T2. Strike offset: the mark from the axis at the exchange, in the round's radii\n"
    )
    .ok();
    writeln!(w, "First exchange. `off axis` is the mark's distance from the axis point at the exchange; `off line` its distance from the axis line. Radii of calibre / 2, a 7.9 mm round included.\n").ok();
    writeln!(w, "| class | n | off axis median | 95th | worst | within 1 r | off line median | 95th | worst |\n|---|---|---|---|---|---|---|---|---|").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        let (mut along, mut across) = (Vec::new(), Vec::new());
        for row in &of {
            let radius = groups[row.group] * 0.5;
            if let Some(note) = row.flown.first() {
                let off = note.strike - note.turn;
                along.push(off.length() / radius);
                across.push(off.reject_from(note.axis).length() / radius);
            }
        }
        if along.is_empty() {
            continue;
        }
        let within = along.iter().filter(|value| **value <= 1.0).count();
        let n = along.len();
        let share = percent(within, n);
        writeln!(
            w,
            "| {} | {n} | {} | {share} | {} |",
            class.name(),
            triple(&mut along, 2),
            triple(&mut across, 2),
        )
        .ok();
    }
    writeln!(w).ok();
    writeln!(
        w,
        "### T2a. The same in millimetres per obliquity, skids only, at the largest calibre\n"
    )
    .ok();
    writeln!(w, "| class | θ | n | off axis median mm | 95th | worst | DERIVED r/cos θ mm |\n|---|---|---|---|---|---|---|").ok();
    for class in Class::ALL {
        for (at, theta) in OBLIQUITIES.iter().enumerate() {
            let mut off: Vec<f32> = class_rows(class)
                .into_iter()
                .filter(|row| row.theta == at && row.group == 0)
                .filter_map(|row| row.flown.first())
                .filter(|note| note.kind == Kind::Skid)
                .map(|note| (note.strike - note.turn).length() * 1000.0)
                .collect();
            if off.is_empty() {
                continue;
            }
            let n = off.len();
            writeln!(
                w,
                "| {} | {theta:.0}° | {n} | {} | {:.0} |",
                class.name(),
                triple(&mut off, 0),
                groups[0] * 500.0 / theta.to_radians().cos(),
            )
            .ok();
        }
    }
    writeln!(w).ok();

    writeln!(w, "### T3. Exchanges at one contact\n").ok();
    writeln!(w, "One contact is a run of consecutive exchanges each sharing a shell with, or coplanar with, the one before. `zero clock`: exchanges with no axis travel; the 5+ column is the design's over-4 gate.\n").ok();
    writeln!(w, "| class | contacts | 1 | 2 | 3 | 4 | 5+ | worst | exchanges | at zero clock | shots with one |\n|---|---|---|---|---|---|---|---|---|---|---|").ok();
    let laws = walk::WalkLaws::default();
    for class in Class::ALL {
        let of = class_rows(class);
        if of.is_empty() {
            continue;
        }
        let mut hist = [0usize; 5];
        let (mut worst, mut exchanges, mut zero, mut shots_zero, mut contacts_n) = (0, 0, 0, 0, 0);
        for row in &of {
            let notes: Vec<&Note> = row.flown.contacts().collect();
            let mut any = false;
            let mut chain = 0;
            for (at, note) in notes.iter().enumerate() {
                exchanges += 1;
                if note.clock <= 0.0 {
                    zero += 1;
                    any = true;
                }
                let same = at > 0 && one_contact(notes[at - 1], note, &laws);
                if same {
                    chain += 1;
                } else {
                    if chain > 0 {
                        hist[(chain as usize - 1).min(4)] += 1;
                        contacts_n += 1;
                        worst = worst.max(chain);
                    }
                    chain = 1;
                }
            }
            if chain > 0 {
                hist[(chain as usize - 1).min(4)] += 1;
                contacts_n += 1;
                worst = worst.max(chain);
            }
            shots_zero += usize::from(any);
        }
        writeln!(
            w,
            "| {} | {contacts_n} | {} | {} | {} | {} | {} | {worst} | {exchanges} | {zero} | {shots_zero} |",
            class.name(),
            hist[0],
            hist[1],
            hist[2],
            hist[3],
            hist[4],
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(
        w,
        "### T4. Aim continuity: two aims {:.1} mm apart\n",
        PAIR_STEP * 1000.0
    )
    .ok();
    writeln!(w, "First exchange of each. Exit angle over pairs that both leave; speed step (of speed in) over all pairs. `flips`: the two read different outcomes. `past §7.2`: angle step beyond the design's accepted step at that obliquity (9.2°, 18.5°, 27.7° at 75°, 60°, 45°).\n").ok();
    writeln!(w, "| class | pairs | flips | angle n | median ° | 95th | worst | past §7.2 | speed median % | 95th | worst |\n|---|---|---|---|---|---|---|---|---|---|---|").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        let (mut pairs, mut flips, mut past) = (0, 0, 0);
        let (mut angles, mut speeds) = (Vec::new(), Vec::new());
        for row in &of {
            let Some(pair) = &row.pair else { continue };
            if pair.panicked.is_some() {
                continue;
            }
            let (Some(a), Some(b)) = (row.flown.first(), pair.first()) else {
                continue;
            };
            pairs += 1;
            flips += usize::from(a.kind != b.kind);
            speeds.push((a.speed_out - b.speed_out).abs() / a.speed_in.max(1.0) * 100.0);
            if let (Some(x), Some(y)) = (a.exit, b.exit) {
                let angle = x.angle_between(y).to_degrees();
                angles.push(angle);
                let accepted = match row.theta {
                    0 => 27.7,
                    1 => 18.5,
                    2 => 9.2,
                    _ => f32::INFINITY,
                };
                past += usize::from(angle > accepted);
            }
        }
        if pairs == 0 {
            continue;
        }
        let angle_n = angles.len();
        writeln!(
            w,
            "| {} | {pairs} | {flips} | {angle_n} | {} | {past} | {} |",
            class.name(),
            triple(&mut angles, 2),
            triple(&mut speeds, 2),
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(
        w,
        "### T5. Tick phase: one aim at {} phases\n",
        PHASES.len()
    )
    .ok();
    writeln!(w, "Phases {PHASES:?} of the first tick flown before first touch. `flips` is aims whose first outcome changed with phase; `flips ≥ 0.05` drops the two smallest phases, which are deliberately small. Spread is the widest distance between the axis points of the first exchange, and between exit speeds as a share of speed in.\n").ok();
    writeln!(w, "| class | aims | flips | flips ≥ 0.05 | turn spread median mm | 95th | worst | speed spread median % | 95th | worst |\n|---|---|---|---|---|---|---|---|---|---|").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        let (mut aims_n, mut flips, mut flips_wide) = (0, 0, 0);
        let (mut turns, mut speeds) = (Vec::new(), Vec::new());
        for row in &of {
            if row.phases.is_empty() {
                continue;
            }
            let firsts: Vec<Option<&Note>> = row.phases.iter().map(Flown::first).collect();
            let kinds = |skip: usize| -> BTreeSet<Option<Kind>> {
                firsts
                    .iter()
                    .skip(skip)
                    .map(|note| note.map(|note| note.kind))
                    .collect()
            };
            aims_n += 1;
            flips += usize::from(kinds(0).len() > 1);
            flips_wide += usize::from(kinds(2).len() > 1);
            let notes: Vec<&Note> = firsts.iter().flatten().copied().collect();
            let mut turn = 0.0_f32;
            let mut speed = 0.0_f32;
            for a in &notes {
                for b in &notes {
                    turn = turn.max((a.turn - b.turn).length());
                    speed = speed.max((a.speed_out - b.speed_out).abs() / a.speed_in.max(1.0));
                }
            }
            turns.push(turn * 1000.0);
            speeds.push(speed * 100.0);
        }
        if aims_n == 0 {
            continue;
        }
        writeln!(
            w,
            "| {} | {aims_n} | {flips} | {flips_wide} | {} | {} |",
            class.name(),
            triple(&mut turns, 1),
            triple(&mut speeds, 2),
        )
        .ok();
    }
    writeln!(w).ok();

    write!(w, "{motion}").ok();

    writeln!(w, "### T7. Crossed-plate re-reads\n").ok();
    writeln!(w, "After a through, a later read in the same shot whose faces include a primitive and shell the crossing read. The same closed shell has a far wall, so `near` keeps reads within two calibres of the hole: the plate actually just crossed. `gap, just through` flies each gap aim at the speed that only just gets through its first plate.\n").ok();
    writeln!(w, "| class | shots through | shots re-reading | re-reads | near | near re-bites | near zero-turn | all re-bites | farthest from the hole mm |\n|---|---|---|---|---|---|---|---|---|").ok();
    let mut reread_row = |label: &str, of: &[&Row]| {
        let (mut through, mut shots, mut reads, mut near, mut near_bites, mut near_zero, mut bites) =
            (0, 0, 0, 0, 0, 0, 0);
        let mut farthest = 0.0_f32;
        for row in of {
            let calibre = groups[row.group];
            let notes: Vec<&Note> = row.flown.contacts().collect();
            let mut crossed: BTreeSet<(Entity, u32)> = BTreeSet::new();
            let mut hole: Option<Vec3> = None;
            let mut any = false;
            let mut went = false;
            for note in notes {
                let identities: BTreeSet<(Entity, u32)> = note
                    .faces
                    .iter()
                    .map(|face| (face.primitive, face.shell))
                    .collect();
                if !crossed.is_disjoint(&identities) {
                    any = true;
                    reads += 1;
                    bites += usize::from(note.kind.bite());
                    if let Some(hole) = hole {
                        let away = (note.strike - hole).length();
                        farthest = farthest.max(away * 1000.0);
                        if away <= 2.0 * calibre {
                            near += 1;
                            near_bites += usize::from(note.kind.bite());
                            near_zero += usize::from(note.kind == Kind::ZeroTurn);
                        }
                    }
                }
                if note.kind == Kind::Through {
                    went = true;
                    crossed.extend(identities);
                    hole = note.hole;
                }
            }
            through += usize::from(went);
            shots += usize::from(any);
        }
        if through > 0 {
            writeln!(
                w,
                "| {label} | {through} | {shots} | {reads} | {near} | {near_bites} | {near_zero} | {bites} | {farthest:.0} |",
            )
            .ok();
        }
    };
    for class in Class::ALL {
        let of: Vec<&Row> = rows
            .iter()
            .copied()
            .filter(|row| row.class == class && row.speed < 2)
            .collect();
        reread_row(class.name(), &of);
    }
    let just_through: Vec<&Row> = rows.iter().copied().filter(|row| row.speed == 2).collect();
    reread_row("gap, just through", &just_through);
    writeln!(w).ok();

    writeln!(w, "### T8. Ownership on real meshes\n").ok();
    writeln!(w, "{mesh}").ok();
    writeln!(w, "At classification, per aim's disc (uniform aims only are a hull-wide rate): the welded coplanar patch under the disc spanning more than one primitive or shell, a sample's first run spanning more than one primitive, and the features the hits name.\n").ok();
    writeln!(w, "| calibre | uniform aims | patch over >1 primitive | patch over >1 shell | first run over >1 primitive | hits on triangle interior | on welded edge | on welded vertex |\n|---|---|---|---|---|---|---|---|").ok();
    for (group, calibre) in groups.iter().enumerate() {
        let of: Vec<&Aim> = uniform.iter().filter(|aim| aim.group == group).collect();
        let features = of.iter().fold([0; 3], |sum, aim| {
            [
                sum[0] + aim.features[0],
                sum[1] + aim.features[1],
                sum[2] + aim.features[2],
            ]
        });
        let total: usize = features.iter().sum();
        writeln!(
            w,
            "| {:.1} mm | {} | {} | {} | {} | {} | {} | {} |",
            calibre * 1000.0,
            of.len(),
            percent(of.iter().filter(|aim| aim.primitives > 1).count(), of.len()),
            percent(of.iter().filter(|aim| aim.shells > 1).count(), of.len()),
            percent(
                of.iter().filter(|aim| aim.run_primitives > 1).count(),
                of.len()
            ),
            percent(features[0], total),
            percent(features[1], total),
            percent(features[2], total),
        )
        .ok();
    }
    writeln!(w).ok();
    writeln!(w, "\"Own the whole run\": after a first contact, the later reads in the same shot an owner of that contact's scope would skip, landing beyond the first contact's footprint (`r / cos θ + r`), so on material the round had not reached. Scopes: the primitive, the shell, and the coplanar plane. `of which bite` and `of which dose` are what skipping would remove. `welded A+B` counts first exchanges whose run spans more than one primitive and that ended in a bite: owning the run takes its unreached primitives with it.\n").ok();
    writeln!(w, "| class | later reads | beyond footprint | in primitive | of which bite | of which dose | in shell | of which bite | of which dose | in plane | of which bite | of which dose | welded A+B bites / bites |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        if of.is_empty() {
            continue;
        }
        let (mut later, mut beyond) = (0, 0);
        let mut scoped = [[0usize; 3]; 3];
        let (mut welded, mut bites) = (0, 0);
        for row in &of {
            let notes: Vec<&Note> = row.flown.contacts().collect();
            let Some(first) = notes.first() else { continue };
            if first.kind.bite() {
                bites += 1;
                welded += usize::from(first.faces.iter().any(|face| face.run_primitives > 1));
            }
            let radius = groups[row.group] * 0.5;
            let square = first.axis.dot(first.normal).abs().max(1.0e-3);
            let reach = radius / square + radius;
            for note in notes.iter().skip(1) {
                later += 1;
                if (note.strike - first.strike).length() <= reach {
                    continue;
                }
                beyond += 1;
                let in_scope = [
                    note.faces.iter().any(|face| {
                        first
                            .faces
                            .iter()
                            .any(|own| own.primitive == face.primitive)
                    }),
                    note.faces.iter().any(|face| {
                        first
                            .faces
                            .iter()
                            .any(|own| own.primitive == face.primitive && own.shell == face.shell)
                    }),
                    note.faces
                        .iter()
                        .any(|face| first.faces.iter().any(|own| coplanar(own, face, &laws))),
                ];
                for (scope, hit) in in_scope.into_iter().enumerate() {
                    if hit {
                        scoped[scope][0] += 1;
                        scoped[scope][1] += usize::from(note.kind.bite());
                        scoped[scope][2] += usize::from(note.dose);
                    }
                }
            }
        }
        writeln!(
            w,
            "| {} | {later} | {beyond} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {welded} / {bites} |",
            class.name(),
            scoped[0][0],
            scoped[0][1],
            scoped[0][2],
            scoped[1][0],
            scoped[1][1],
            scoped[1][2],
            scoped[2][0],
            scoped[2][1],
            scoped[2][2],
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(w, "### T9. One contact read\n").ok();
    writeln!(w, "The closing walk plus the law's verdict on it, per exchange, µs, in the edit profile: a dev build, not a shipping one, on a machine other agents share (no compile runs during it; other work may). Not frame or tick time.\n").ok();
    writeln!(
        w,
        "| | n | median µs | 95th | worst |\n|---|---|---|---|---|"
    )
    .ok();
    let mut cost_row = |label: &str, mut values: Vec<f32>| {
        if !values.is_empty() {
            let n = values.len();
            writeln!(w, "| {label} | {n} | {} |", triple(&mut values, 0)).ok();
        }
    };
    for (group, calibre) in groups.iter().enumerate() {
        cost_row(
            &format!("{:.1} mm", calibre * 1000.0),
            owned
                .iter()
                .filter(|row| row.group == group)
                .flat_map(|row| row.flown.contacts())
                .map(|note| note.micros as f32)
                .collect(),
        );
    }
    writeln!(w).ok();

    writeln!(w, "### Sanity counts\n").ok();
    writeln!(w, "Exchanges that should not exist if the march holds its own laws. `gained speed`: a leaving exchange left faster than it arrived. `into the face`: a full-coverage skid whose exit heads into the face it read. `backward`: a leaving exchange whose exit opposes the axis it arrived on.\n").ok();
    writeln!(
        w,
        "| class | exchanges | gained speed | into the face | backward |\n|---|---|---|---|---|"
    )
    .ok();
    for class in Class::ALL {
        let of = class_rows(class);
        if of.is_empty() {
            continue;
        }
        let (mut exchanges, mut gained, mut into, mut backward) = (0, 0, 0, 0);
        for row in &of {
            for note in row.flown.contacts() {
                exchanges += 1;
                gained += usize::from(note.speed_out > note.speed_in * 1.0001);
                let Some(exit) = note.exit else { continue };
                if note.kind == Kind::Skid && note.coverage >= 1.0 && exit.dot(note.normal) < -1e-3
                {
                    into += 1;
                }
                backward += usize::from(exit.dot(note.axis) < 0.0);
            }
        }
        writeln!(
            w,
            "| {} | {exchanges} | {gained} | {into} | {backward} |",
            class.name(),
        )
        .ok();
    }
    writeln!(w).ok();

    writeln!(w, "### Reproductions\n").ok();
    writeln!(w, "Per class, its longest chain and its first zero-turn rest, as the seeded instrument flew them. World metres; the tank stands at its spawn pose unless a line says it moves.\n").ok();
    for class in Class::ALL {
        let of = class_rows(class);
        let longest = of
            .iter()
            .max_by_key(|row| row.flown.contacts().count())
            .filter(|row| row.flown.contacts().count() > 4);
        let zero_turn = of
            .iter()
            .find(|row| row.flown.contacts().any(|note| note.kind == Kind::ZeroTurn));
        for (label, row) in [("longest chain", longest), ("first zero-turn", zero_turn)] {
            let Some(row) = row else { continue };
            writeln!(
                w,
                "- {} {label}: {} at {} ({} exchanges), birth {:?}, post-step velocity {:?}",
                class.name(),
                arms[row.arm].key,
                SPEED_LABELS[row.speed],
                row.flown.contacts().count(),
                row.birth,
                row.post,
            )
            .ok();
        }
    }
    writeln!(w).ok();

    writeln!(w, "### Panics met in the march\n").ok();
    if defects.is_empty() {
        writeln!(w, "None.\n").ok();
    }
    for defect in defects {
        writeln!(w, "- {defect}").ok();
    }
    writeln!(w).ok();
    out
}

/// `b` is an exchange at the contact `a` was: it shares a shell with it or lies on its plane.
fn one_contact(a: &Note, b: &Note, laws: &walk::WalkLaws) -> bool {
    a.faces.iter().any(|own| {
        b.faces.iter().any(|face| {
            (own.primitive == face.primitive && own.shell == face.shell)
                || coplanar(own, face, laws)
        })
    })
}

/// The flat patches of `triangles`: triangles with normals within 2.6° and 2 mm of one plane,
/// connected across gaps of at most `calibre`.
fn flat_patches(triangles: &[WorldTriangle], calibre: f32, laws: &walk::WalkLaws) -> String {
    const TIGHT_COS: f32 = 0.999;
    let mut parent: Vec<usize> = (0..triangles.len()).collect();
    fn root(parent: &mut [usize], mut at: usize) -> usize {
        while parent[at] != at {
            parent[at] = parent[parent[at]];
            at = parent[at];
        }
        at
    }
    let bounds: Vec<(Vec3, Vec3)> = triangles
        .iter()
        .map(|triangle| {
            let [a, b, c] = triangle.corners;
            (a.min(b).min(c), a.max(b).max(c))
        })
        .collect();
    let mut buckets = BTreeMap::<(i32, i32, i32, i32), Vec<usize>>::new();
    for (at, triangle) in triangles.iter().enumerate() {
        let n = triangle.normal;
        let offset = triangle.corners[0].dot(n);
        buckets
            .entry((
                (n.x * 20.0).round() as i32,
                (n.y * 20.0).round() as i32,
                (n.z * 20.0).round() as i32,
                (offset / (2.0 * laws.event_plane_tolerance)).round() as i32,
            ))
            .or_default()
            .push(at);
    }
    for members in buckets.values() {
        for (i, &a) in members.iter().enumerate() {
            for &b in &members[i + 1..] {
                let (ta, tb) = (&triangles[a], &triangles[b]);
                if ta.normal.dot(tb.normal) < TIGHT_COS
                    || (tb.corners[0] - ta.corners[0]).dot(ta.normal).abs()
                        > laws.event_plane_tolerance
                {
                    continue;
                }
                let gap = (bounds[a].0 - bounds[b].1)
                    .max(bounds[b].0 - bounds[a].1)
                    .max(Vec3::ZERO);
                if gap.length() <= calibre {
                    let (x, y) = (root(&mut parent, a), root(&mut parent, b));
                    parent[x] = y;
                }
            }
        }
    }
    let mut patches = BTreeMap::<usize, Vec<usize>>::new();
    for at in 0..triangles.len() {
        let top = root(&mut parent, at);
        patches.entry(top).or_default().push(at);
    }
    let area_of = |patch: &[usize]| patch.iter().map(|&at| triangles[at].area).sum::<f32>();
    let spans = |patch: &[usize]| {
        (
            patch
                .iter()
                .map(|&at| triangles[at].primitive)
                .collect::<BTreeSet<_>>()
                .len(),
            patch
                .iter()
                .map(|&at| (triangles[at].primitive, triangles[at].shell))
                .collect::<BTreeSet<_>>()
                .len(),
        )
    };
    let total: f32 = triangles.iter().map(|triangle| triangle.area).sum();
    let many: Vec<&Vec<usize>> = patches.values().filter(|patch| patch.len() > 1).collect();
    let over_primitive: Vec<&&Vec<usize>> =
        many.iter().filter(|patch| spans(patch).0 > 1).collect();
    let over_shell: Vec<&&Vec<usize>> = many.iter().filter(|patch| spans(patch).1 > 1).collect();
    let primitives: BTreeSet<Entity> = triangles
        .iter()
        .map(|triangle| triangle.primitive)
        .collect();
    let shells: BTreeSet<(Entity, u32)> = triangles
        .iter()
        .map(|triangle| (triangle.primitive, triangle.shell))
        .collect();
    format!(
        "The mesh: {} armour triangles, {} primitives, {} closed shells, {:.1} m² of faces. Flat \
         patches (normals within 2.6°, 2 mm of one plane, gaps at most {:.0} mm): {} of more than \
         one triangle; {} span more than one primitive ({:.1} % of the face area), {} span more \
         than one shell ({:.1} % of the face area). A hit names its volume, primitive, closed \
         shell, triangle and welded feature (triangle interior, welded edge or vertex); no finite \
         patch identity exists in the certificate.\n",
        triangles.len(),
        primitives.len(),
        shells.len(),
        total,
        calibre * 1000.0,
        many.len(),
        over_primitive.len(),
        100.0
            * over_primitive
                .iter()
                .map(|patch| area_of(patch))
                .sum::<f32>()
            / total,
        over_shell.len(),
        100.0 * over_shell.iter().map(|patch| area_of(patch)).sum::<f32>() / total,
    )
}

/// The moving-target and unrelated-motion arms, as a Markdown section, with the rows they flew.
fn motion_arms(
    arms: &[Arm],
    groups: &[f32],
    aims: &[Aim],
    effort: Effort,
    defects: &mut Vec<String>,
) -> (String, Vec<Row>) {
    let mut range = Range::new(RigidBody::Kinematic);
    let mut out = String::new();
    let mut rows = Vec::new();
    let w = &mut out;

    // The first arm of each calibre group.
    let mut seen = BTreeSet::new();
    let chosen: Vec<usize> = (0..arms.len())
        .filter(|&at| seen.insert(arms[at].group))
        .collect();

    // Unrelated motion: the same shot twice with the decoy still, then with it moving.
    let (mut shots, mut repeats, mut differs, mut kinds) = (0, 0, 0, 0);
    let mut moved_turn = 0.0_f32;
    for &at in &chosen {
        let arm = &arms[at];
        for class in Class::ALL {
            for theta in 0..OBLIQUITIES.len() {
                let of = aims
                    .iter()
                    .filter(|aim| {
                        aim.group == arm.group && aim.class == class && aim.theta == theta
                    })
                    .take(effort.moving);
                for aim in of {
                    let speed = arm.round.muzzle_speed;
                    let birth = birth_of(aim, speed, aim.phase);
                    let post = aim.along * speed;
                    range.place(Vec3::ZERO, Vec3::ZERO);
                    let a = range.fly(&arm.round, birth, post);
                    range.place(Vec3::ZERO, Vec3::ZERO);
                    let b = range.fly(&arm.round, birth, post);
                    range.place(Vec3::ZERO, Vec3::X * UNRELATED_SPEED);
                    let c = range.fly(&arm.round, birth, post);
                    if let Some(message) = a.panicked.as_ref().or(c.panicked.as_ref()) {
                        defects.push(repro(arm, class, birth, post, message));
                        continue;
                    }
                    shots += 1;
                    repeats += usize::from(!same_notes(&a, &b));
                    if !same_notes(&a, &c) {
                        differs += 1;
                        kinds += usize::from(
                            a.notes.iter().map(|n| n.kind).collect::<Vec<_>>()
                                != c.notes.iter().map(|n| n.kind).collect::<Vec<_>>(),
                        );
                        if let (Some(x), Some(y)) = (a.first(), c.first()) {
                            moved_turn = moved_turn.max((x.turn - y.turn).length());
                        }
                    }
                }
            }
        }
    }
    writeln!(w, "### T6. Unrelated motion, and a moving target\n").ok();
    writeln!(
        w,
        "Same shot with a kinematic body {UNRELATED_AWAY} m off the tank standing still, then moving \
         {UNRELATED_SPEED} m/s. Compared bit for bit on every noted exchange's kind, speeds, strike \
         and axis point. `repeat` is the same still shot flown twice, the noise floor.\n"
    )
    .ok();
    writeln!(w, "| shots | repeat differs | motion differs | of which outcome sequence differs | widest first-exchange axis move mm |\n|---|---|---|---|---|").ok();
    writeln!(
        w,
        "| {shots} | {repeats} | {differs} | {kinds} | {:.3} |\n",
        moved_turn * 1000.0
    )
    .ok();

    // A moving target against its static equivalent, in its own frame.
    let mut stats = BTreeMap::<Class, MovingStats>::new();
    for &at in &chosen {
        let arm = &arms[at];
        for class in Class::ALL {
            for theta in 0..OBLIQUITIES.len() {
                let of = aims
                    .iter()
                    .filter(|aim| {
                        aim.group == arm.group && aim.class == class && aim.theta == theta
                    })
                    .take(effort.moving);
                for aim in of {
                    let speed = arm.round.muzzle_speed;
                    let relative = aim.along * speed;
                    let birth = birth_of(aim, speed, aim.phase);
                    let at_rest = range.place(Vec3::ZERO, Vec3::ZERO);
                    let still = range.fly(&arm.round, birth, relative);
                    let across = aim.along.any_orthonormal_vector() * TARGET_SPEED;
                    let closing = -aim.along * TARGET_SPEED;
                    for target in [across, closing] {
                        let there = range.place(target, Vec3::ZERO);
                        let shifted = birth + (there - at_rest);
                        let moving = range.fly(&arm.round, shifted, relative + target);
                        if let Some(message) = still.panicked.as_ref().or(moving.panicked.as_ref())
                        {
                            defects.push(repro(
                                arm,
                                Class::Moving,
                                shifted,
                                relative + target,
                                message,
                            ));
                            continue;
                        }
                        let entry = stats.entry(class).or_default();
                        entry.shots += 1;
                        let (Some(x), Some(y)) = (still.first(), moving.first()) else {
                            entry.contact_flips +=
                                usize::from(still.first().is_some() != moving.first().is_some());
                            continue;
                        };
                        entry.flips += usize::from(x.kind != y.kind);
                        let own = |note: &Note, point: Vec3| {
                            point - note.body_at.expect("a kinematic range tracks its tank")
                        };
                        entry
                            .strike
                            .push((own(x, x.strike) - own(y, y.strike)).length() * 1000.0);
                        entry
                            .turn
                            .push((own(x, x.turn) - own(y, y.turn)).length() * 1000.0);
                        entry
                            .speed
                            .push((x.speed_out - y.speed_out).abs() / x.speed_in.max(1.0) * 100.0);
                        if let (Some(a), Some(b)) = (x.exit, y.exit) {
                            entry.angle.push(a.angle_between(b).to_degrees());
                        }
                        rows.push(Row {
                            class: Class::Moving,
                            arm: at,
                            group: arm.group,
                            theta,
                            speed: 0,
                            birth: shifted,
                            post: relative + target,
                            flown: moving,
                            pair: None,
                            phases: Vec::new(),
                        });
                    }
                }
            }
        }
    }
    writeln!(
        w,
        "A target moving {TARGET_SPEED} m/s (across the line, and closing on it) against the same \
         relative shot at a still target, first exchange in the body's frame, one round per calibre \
         of {} calibres.\n",
        groups.len()
    )
    .ok();
    writeln!(w, "| aim class | shots | contact flips | outcome flips | strike move median mm | 95th | worst | axis move median mm | 95th | worst | exit angle median ° | 95th | worst | speed median % | 95th | worst |\n|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|").ok();
    for (class, mut stat) in stats {
        writeln!(
            w,
            "| {} | {} | {} | {} | {} | {} | {} | {} |",
            class.name(),
            stat.shots,
            stat.contact_flips,
            stat.flips,
            triple(&mut stat.strike, 2),
            triple(&mut stat.turn, 2),
            triple(&mut stat.angle, 3),
            triple(&mut stat.speed, 3),
        )
        .ok();
    }
    writeln!(w).ok();
    (out, rows)
}

#[derive(Default)]
struct MovingStats {
    shots: usize,
    contact_flips: usize,
    flips: usize,
    strike: Vec<f32>,
    turn: Vec<f32>,
    angle: Vec<f32>,
    speed: Vec<f32>,
}

fn same_notes(a: &Flown, b: &Flown) -> bool {
    a.notes.len() == b.notes.len()
        && a.notes.iter().zip(&b.notes).all(|(x, y)| {
            x.kind == y.kind
                && x.speed_in.to_bits() == y.speed_in.to_bits()
                && x.speed_out.to_bits() == y.speed_out.to_bits()
                && x.strike.to_array().map(f32::to_bits) == y.strike.to_array().map(f32::to_bits)
                && x.turn.to_array().map(f32::to_bits) == y.turn.to_array().map(f32::to_bits)
        })
}

fn repro(arm: &Arm, class: Class, birth: Vec3, post: Vec3, message: &str) -> String {
    let line: String = message
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(240)
        .collect();
    format!(
        "{} aim, `{}`: born at {birth:?}, post-step velocity {post:?}, tank at {PROBE_TANK_AT:?}: {line}",
        class.name(),
        arm.key,
    )
}

/// Fly the sweep at `effort` and return its tables.
#[expect(
    clippy::too_many_lines,
    reason = "one seeded order: the aims, the sweep, the gap's speeds and the motion arms share it"
)]
fn measure(effort: Effort) -> String {
    let began = Instant::now();
    let mut range = Range::new(RigidBody::Static);
    let triangles = range
        .app
        .world_mut()
        .run_system_once(world_triangles)
        .expect("the armour reads");
    let mut areas = Vec::with_capacity(triangles.len());
    let mut sum = 0.0;
    for triangle in &triangles {
        sum += triangle.area;
        areas.push(sum);
    }
    let lo = triangles
        .iter()
        .flat_map(|triangle| triangle.corners)
        .fold(Vec3::INFINITY, Vec3::min);
    let hi = triangles
        .iter()
        .flat_map(|triangle| triangle.corners)
        .fold(Vec3::NEG_INFINITY, Vec3::max);
    let far = (hi - lo).length() + 2.0;

    let registry = crate::guns::GunRegistry::shipped();
    let mut rounds: Vec<(String, crate::rounds::Round)> = registry
        .fired_rounds()
        .map(|(gun, key, round)| (format!("{gun} {key}"), round))
        .collect();
    rounds.sort_by(|a, b| {
        b.1.penetrator
            .caliber
            .total_cmp(&a.1.penetrator.caliber)
            .then(a.0.cmp(&b.0))
    });
    let mut groups: Vec<f32> = Vec::new();
    let arms: Vec<Arm> = rounds
        .into_iter()
        .map(|(key, round)| {
            let calibre = round.penetrator.caliber;
            let group = groups
                .iter()
                .position(|known| *known == calibre)
                .unwrap_or_else(|| {
                    groups.push(calibre);
                    groups.len() - 1
                });
            Arm { key, round, group }
        })
        .collect();

    // The aims: per calibre and obliquity, up to `per_cell` per class, uniform draws kept apart so
    // their mix stands as a rate.
    let mut aims: Vec<Aim> = Vec::new();
    let mut uniform: Vec<Aim> = Vec::new();
    for (group, &calibre) in groups.iter().enumerate() {
        for (theta_at, &theta) in OBLIQUITIES.iter().enumerate() {
            let mut state = SEED ^ ((group as u64) << 8) ^ (theta_at as u64);
            let mut kept = BTreeMap::<Class, usize>::new();
            let mut tried = 0;
            while tried < effort.candidates {
                let batch: Vec<Candidate> = (0..256)
                    .map(|at| {
                        let source = match at % 4 {
                            0 => Source::Uniform,
                            1 | 2 => Source::NearEdge,
                            _ => Source::Inside,
                        };
                        draw(&mut state, &triangles, &areas, source, theta, calibre, far)
                    })
                    .collect();
                tried += batch.len();
                let read = range
                    .app
                    .world_mut()
                    .run_system_once_with(classify_batch, (batch, group, theta_at))
                    .expect("the classifier runs");
                for aim in read.into_iter().flatten() {
                    if aim.source == Source::Uniform {
                        uniform.push(aim.clone());
                    }
                    let count = kept.entry(aim.class).or_default();
                    if *count < effort.per_cell {
                        *count += 1;
                        aims.push(aim);
                    }
                }
                let flown_classes = Class::ALL.len() - 1;
                if kept.len() == flown_classes
                    && kept.values().all(|count| *count >= effort.per_cell)
                {
                    break;
                }
            }
            println!(
                "aims {:.1} mm {theta:.0}°: tried {tried}, kept {kept:?}, {:.0} s",
                calibre * 1000.0,
                began.elapsed().as_secs_f32()
            );
        }
    }

    // The main sweep.
    let mut rows: Vec<Row> = Vec::new();
    let mut defects: Vec<String> = Vec::new();
    for (at, arm) in arms.iter().enumerate() {
        let muzzle = arm.round.muzzle_speed;
        for (speed_at, speed) in [muzzle, muzzle * SPENT_SHARE].into_iter().enumerate() {
            for (index, aim) in aims.iter().filter(|aim| aim.group == arm.group).enumerate() {
                let birth = birth_of(aim, speed, aim.phase);
                let post = aim.along * speed;
                let flown = range.fly(&arm.round, birth, post);
                if let Some(message) = &flown.panicked {
                    defects.push(repro(arm, aim.class, birth, post, message));
                }
                let pair = range.fly(&arm.round, birth + aim.lateral * PAIR_STEP, post);
                if let Some(message) = &pair.panicked {
                    let beside = birth + aim.lateral * PAIR_STEP;
                    defects.push(repro(arm, aim.class, beside, post, message));
                }
                let phases = if index % PHASE_EVERY == 0 && aim.class != Class::Inside {
                    PHASES
                        .iter()
                        .map(|&phase| {
                            let birth = birth_of(aim, speed, phase);
                            let flown = range.fly(&arm.round, birth, post);
                            if let Some(message) = &flown.panicked {
                                defects.push(repro(arm, aim.class, birth, post, message));
                            }
                            flown
                        })
                        .filter(|flown| flown.panicked.is_none())
                        .collect()
                } else {
                    Vec::new()
                };
                rows.push(Row {
                    class: aim.class,
                    arm: at,
                    group: arm.group,
                    theta: aim.theta,
                    speed: speed_at,
                    birth,
                    post,
                    flown,
                    pair: Some(pair),
                    phases,
                });
            }
            println!(
                "{} at {speed:.0} m/s: {} rows, {} defects, {:.0} s",
                arm.key,
                rows.len(),
                defects.len(),
                began.elapsed().as_secs_f32()
            );
        }
    }

    // A narrow gap at the speed that only just gets through its first plate, so a spent round
    // actually arrives in the gap.
    for (at, arm) in arms.iter().enumerate() {
        let of: Vec<&Aim> = aims
            .iter()
            .filter(|aim| aim.group == arm.group && aim.class == Class::Gap && aim.theta <= 1)
            .collect();
        for aim in of {
            let muzzle = arm.round.muzzle_speed;
            let through = |range: &mut Range, speed: f32| {
                let birth = birth_of(aim, speed, 0.5);
                range
                    .fly(&arm.round, birth, aim.along * speed)
                    .first()
                    .is_some_and(|note| note.kind == Kind::Through)
            };
            if !through(&mut range, muzzle) {
                continue;
            }
            let (mut low, mut high) = (30.0_f32, muzzle);
            for _ in 0..6 {
                let middle = (low + high) * 0.5;
                if through(&mut range, middle) {
                    high = middle;
                } else {
                    low = middle;
                }
            }
            let speed = high * 1.02;
            let birth = birth_of(aim, speed, 0.5);
            let flown = range.fly(&arm.round, birth, aim.along * speed);
            rows.push(Row {
                class: Class::Gap,
                arm: at,
                group: arm.group,
                theta: aim.theta,
                speed: 2,
                birth,
                post: aim.along * speed,
                flown,
                pair: None,
                phases: Vec::new(),
            });
        }
    }
    println!(
        "sweep done: {} rows, {:.0} s",
        rows.len(),
        began.elapsed().as_secs_f32()
    );

    let (motion, moving_rows) = motion_arms(&arms, &groups, &aims, effort, &mut defects);
    rows.extend(moving_rows);
    println!("motion arms done: {:.0} s", began.elapsed().as_secs_f32());

    let mesh = flat_patches(&triangles, groups[0], &walk::WalkLaws::default());
    let out = tables(&Sweep {
        rows: &rows,
        arms: &arms,
        groups: &groups,
        aims: &aims,
        uniform: &uniform,
        effort,
        mesh: &mesh,
        motion: &motion,
        defects: &defects,
    });
    println!("{out}");
    println!(
        "total: {} shots-rows, {:.0} s",
        rows.len(),
        began.elapsed().as_secs_f32()
    );
    out
}

/// A rerun replaces only the block between the markers, so the record's prose stands.
fn write_record(tables: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(RECORD);
    let held = std::fs::read_to_string(&path).unwrap_or_default();
    let block = format!("{TABLES_BEGIN}\n{tables}\n{TABLES_END}");
    let written = match (held.find(TABLES_BEGIN), held.find(TABLES_END)) {
        (Some(begin), Some(end)) => {
            format!(
                "{}{block}{}",
                &held[..begin],
                &held[end + TABLES_END.len()..]
            )
        }
        _ => format!("{held}{block}\n"),
    };
    std::fs::write(&path, written).expect("the record is writable");
}

/// The instrument still flies: a few aims per cell, no record written.
#[test]
#[ignore = "a measurement's smoke run, run on demand"]
fn contact_classes_smoke() {
    measure(Effort {
        per_cell: 2,
        candidates: 256,
        moving: 1,
    });
}

/// The measurement: real shots through the real march per geometric class, its tables in the record.
#[test]
#[ignore = "a measurement, run on demand"]
fn contact_classes_on_the_shipped_hull() {
    write_record(&measure(Effort {
        per_cell: 24,
        candidates: 4096,
        moving: 5,
    }));
}

// --- Reach on a real map -------------------------------------------------------------------------

/// Slant ranges a volley is fired from, metres. Fixture data: a chosen spread of engagement
/// distances, ASSUMED.
const REACH_RANGES: [f32; 4] = [60.0, 150.0, 300.0, 600.0];
/// Flights per round and hull state in one slice, and flights per volley: a line round flies in
/// long bursts, a disc round one shell at a time, so a slice flies many more of the first. Fixture
/// data.
const REACH_LINE: (usize, usize) = (6_000, 32);
const REACH_DISC: (usize, usize) = (600, 12);
/// Ticks a volley is followed: 3.1 s, past a 600 m flight and any skid off the hull. Fixture data.
const REACH_TICKS: usize = 200;
/// A moving hull's speed, m/s, and yaw rate, rad/s: the top of each drawn range. Fixture data.
const REACH_SPEED: f32 = 12.0;
const REACH_YAW: f32 = 0.5;
/// Muzzle height over the ground at the shooter's feet, m. Fixture data.
const REACH_MUZZLE: f32 = 2.2;
/// Half the span a hull's slope is read over, m, about half a hull's length. Fixture data.
const REACH_SLOPE_SPAN: f32 = 2.0;

#[derive(Default)]
struct Reach {
    flights: usize,
    touched: usize,
    max_fields: usize,
    panics: BTreeMap<String, usize>,
}

impl Reach {
    fn panic_kind(message: &str) -> String {
        [
            "UnexpectedExit",
            "UnexpectedEntry",
            "IncompleteCorridor",
            "DrainExhausted",
        ]
        .iter()
        .find(|kind| message.contains(**kind))
        .map_or_else(
            || message.chars().take(100).collect(),
            |kind| (*kind).to_owned(),
        )
    }
}

/// The bounds of every armour volume in the world.
fn armour_bounds(world: &mut World) -> (Vec3, Vec3) {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for (_, position, rotation, collider) in world
        .query::<(&VolumeOf, &Position, &Rotation, &Collider)>()
        .iter(world)
    {
        let aabb = collider.aabb(position.0, rotation.0);
        lo = lo.min(aabb.min);
        hi = hi.max(aabb.max);
    }
    (lo, hi)
}

/// Where a volley's hull stands and how it moves.
#[derive(Clone, Copy, Debug)]
struct Stance {
    at: Vec3,
    rotation: Quat,
    velocity: Vec3,
    spin: Vec3,
}

impl Range {
    fn stand(&mut self, stance: Stance) {
        let root = self.root;
        self.app.world_mut().entity_mut(root).insert((
            Transform::from_translation(stance.at).with_rotation(stance.rotation),
            Position(stance.at),
            Rotation(stance.rotation),
            LinearVelocity::ZERO,
            AngularVelocity::ZERO,
        ));
        self.app.update();
        self.app.world_mut().entity_mut(root).insert((
            LinearVelocity(stance.velocity),
            AngularVelocity(stance.spin),
        ));
        self.app.update();
    }

    /// Fire `headings` from `muzzle` together: each shell spawns when the world next flushes.
    fn fire(&mut self, round: &crate::rounds::Round, muzzle: Vec3, headings: &[Dir3]) {
        for &heading in headings {
            let fire = FireShell::of_round(
                round,
                muzzle,
                muzzle,
                heading,
                crate::spec::FireMechanism::Single,
                false,
                None,
                None,
            );
            self.app.world_mut().trigger(fire);
        }
    }

    /// Fly `headings` from `muzzle` together and read how many shells touched armour; a panic in the
    /// march gives its message and how many flights it leaves counted, the rest lost with the world.
    fn follow(
        &mut self,
        round: &crate::rounds::Round,
        muzzle: Vec3,
        headings: &[Dir3],
    ) -> Result<(usize, usize), (String, usize)> {
        self.fire(round, muzzle, headings);
        // A trigger queues its spawn: unflushed, the first flying count reads none and the volley
        // flies at wherever the next stance stands the hull.
        self.app.world_mut().flush();
        self.fly_out(headings.len())
    }

    /// Follow every flying shell out, `fired` of them the latest: see [`Self::follow`].
    fn fly_out(&mut self, fired: usize) -> Result<(usize, usize), (String, usize)> {
        for _ in 0..REACH_TICKS {
            let world = self.app.world_mut();
            let flying = world
                .query_filtered::<(), With<Projectile>>()
                .iter(world)
                .count();
            if flying == 0 {
                break;
            }
            let app = &mut self.app;
            let Err(payload) = catch_unwind(AssertUnwindSafe(|| app.update())) else {
                continue;
            };
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| {
                    payload
                        .downcast_ref::<&str>()
                        .map(|text| (*text).to_owned())
                })
                .unwrap_or_else(|| "a panic with no message".to_owned());
            return Err((message, fired.saturating_sub(flying) + 1));
        }
        let world = self.app.world_mut();
        let shells: Vec<Entity> = world
            .query_filtered::<Entity, Or<(With<ShellPath>, With<Projectile>)>>()
            .iter(world)
            .collect();
        let mut touched = 0;
        let mut max_fields = 0;
        for shell in shells {
            if let Some(fields) = world.get::<SpallMarks>(shell) {
                max_fields =
                    max_fields.max(fields.cones.len() + usize::from(fields.burst.is_some()));
            }
            if world
                .get::<PenetrationMarks>(shell)
                .is_some_and(|marks| !marks.log.is_empty())
            {
                touched += 1;
            }
            world.despawn(shell);
        }
        for mut health in world.query::<&mut ComponentHealth>().iter_mut(world) {
            health.refill();
        }
        Ok((touched, max_fields))
    }

    /// Stand the hull on `ground` at a drawn spot, yaw and the slope there, still or under way, and
    /// fly one volley at it from a muzzle at a drawn range with a clear line to the hull. Each shot
    /// aims at a drawn point in the hull's bounds, lofted and led by the round's own flight time. A
    /// panic prints the whole volley: its shots move the hull between them, so one alone need not
    /// reproduce it.
    fn volley(
        &mut self,
        round: &crate::rounds::Round,
        moving: bool,
        shots: usize,
        ground: &HeightGrid,
        seed: &mut u64,
        reach: &mut Reach,
    ) {
        let half = ground.half_extent() * 0.5;
        let spot = Vec2::new(
            (unit(seed) * 2.0 - 1.0) * half,
            (unit(seed) * 2.0 - 1.0) * half,
        );
        let slope = |dx: f32, dz: f32| {
            ground.height_at(spot.x + dx, spot.y + dz) - ground.height_at(spot.x - dx, spot.y - dz)
        };
        let up = Vec3::new(
            -slope(REACH_SLOPE_SPAN, 0.0),
            2.0 * REACH_SLOPE_SPAN,
            -slope(0.0, REACH_SLOPE_SPAN),
        )
        .normalize();
        let rotation = Quat::from_rotation_arc(Vec3::Y, up)
            * Quat::from_rotation_y(unit(seed) * std::f32::consts::TAU);
        let root = self.root;
        let world = self.app.world_mut();
        let (low, _) = armour_bounds(world);
        let foot = world
            .get::<Position>(root)
            .expect("the tank root is a body")
            .0
            .y
            - low.y;
        let (velocity, spin) = if moving {
            (
                rotation * Vec3::Z * (unit(seed) * REACH_SPEED),
                up * (unit(seed) * 2.0 - 1.0) * REACH_YAW,
            )
        } else {
            (Vec3::ZERO, Vec3::ZERO)
        };
        let stance = Stance {
            at: Vec3::new(spot.x, ground.height_at(spot.x, spot.y) + foot, spot.y),
            rotation,
            velocity,
            spin,
        };
        self.stand(stance);
        let (lo, hi) = armour_bounds(self.app.world_mut());
        let centre = (lo + hi) * 0.5;

        let range = REACH_RANGES[(unit(seed) * REACH_RANGES.len() as f32) as usize];
        let clear = (0..16).find_map(|_| {
            let bearing = unit(seed) * std::f32::consts::TAU;
            let xz = Vec2::new(
                centre.x + bearing.cos() * range,
                centre.z + bearing.sin() * range,
            );
            if !ground.contains_xz(xz.x, xz.y) {
                return None;
            }
            let muzzle = Vec3::new(xz.x, ground.height_at(xz.x, xz.y) + REACH_MUZZLE, xz.y);
            let line = centre - muzzle;
            ground
                .cast_ray(muzzle, line.normalize(), line.length())
                .is_none()
                .then_some(muzzle)
        });
        let Some(muzzle) = clear else {
            return;
        };
        let flight_time =
            ((round.drag_k * range).exp() - 1.0) / (round.drag_k * round.muzzle_speed);
        let headings: Vec<Dir3> = (0..shots)
            .map(|_| {
                let aim = lo
                    + (hi - lo) * Vec3::new(unit(seed), unit(seed), unit(seed))
                    + velocity * flight_time
                    - gravity() * 0.5 * flight_time * flight_time;
                Dir3::new(aim - muzzle).expect("a muzzle at range has a heading")
            })
            .collect();
        match self.follow(round, muzzle, &headings) {
            Ok((touched, max_fields)) => {
                reach.max_fields = reach.max_fields.max(max_fields);
                reach.flights += shots;
                reach.touched += touched;
            }
            Err((message, flown)) => {
                *reach.panics.entry(Reach::panic_kind(&message)).or_default() += 1;
                reach.flights += flown;
                println!("PANIC\t{stance:?}\t{muzzle:?}\t{headings:?}");
                *self = Range::on(self.mount, Some(ground.clone()));
            }
        }
    }
}

/// How often a player can meet a march panic: every fired round from a muzzle at range over the
/// shipped ground at a still and a moving hull, seeded by `REACH_SLICE`. A measurement, never a
/// gate; one line per round and hull state, summed across slices by the record:
///
/// `REACH_SLICE=0 scripts/serial-cargo test --lib --features dev_ui,bitprobe panic_reach_on_shipped_ground -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, run on demand"]
fn panic_reach_on_shipped_ground() {
    let slice: u64 = std::env::var("REACH_SLICE").map_or(0, |slice| {
        slice.parse().expect("REACH_SLICE is a slice number")
    });
    let ground = crate::terrain_grid::tests::shipped_grid();
    let registry = crate::guns::GunRegistry::shipped();
    let mut seed = SEED ^ slice.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let mut range = Range::on(RigidBody::Kinematic, Some(ground.clone()));
    for (gun, key, round) in registry.fired_rounds() {
        let (want, shots) = if round.flying.caliber < resolve::DISC_MIN_CALIBER {
            REACH_LINE
        } else {
            REACH_DISC
        };
        for moving in [false, true] {
            let mut reach = Reach::default();
            while reach.flights < want {
                range.volley(&round, moving, shots, &ground, &mut seed, &mut reach);
            }
            let hull = if moving { "moving" } else { "still" };
            println!(
                "MEASURED maximum fields: {} ({gun}/{key}, {hull}, {} flights)",
                reach.max_fields, reach.flights
            );
            println!(
                "REACH\t{slice}\t{gun}\t{key}\t{hull}\t{}\t{}\t{:?}",
                reach.flights, reach.touched, reach.panics
            );
        }
    }
}

/// The game's damage model: a cheap field-count sample, one real-muzzle volley per fired round
/// and motion arm. Counts are measurements, never pass conditions or per-shot limits.
#[test]
#[ignore = "a field-count measurement, run on demand"]
fn field_counts_from_real_muzzles() {
    let ground = crate::terrain_grid::tests::shipped_grid();
    let registry = crate::guns::GunRegistry::shipped();
    let mut seed = SEED;
    let mut range = Range::on(RigidBody::Kinematic, Some(ground.clone()));
    for (gun, key, round) in registry.fired_rounds() {
        let shots = if round.flying.caliber < resolve::DISC_MIN_CALIBER {
            REACH_LINE.1
        } else {
            REACH_DISC.1
        };
        for moving in [false, true] {
            let mut reach = Reach::default();
            range.volley(&round, moving, shots, &ground, &mut seed, &mut reach);
            println!(
                "MEASURED field sample: {gun}/{key} moving={moving} flights={} touched={} max_fields={} panics={:?}",
                reach.flights, reach.touched, reach.max_fields, reach.panics
            );
        }
    }
}

/// Shots that once ended the process, each flown from its birth on its post-step velocity with the
/// range's tank at its spawn pose. Fixture data.
const ONCE_FATAL: &[(&str, [f32; 3], [f32; 3])] = &[
    (
        "kwk36_l56 88mm_AP",
        [13.448209, 5.9313054, -3.65905],
        [-114.379295, -136.64839, -74.76734],
    ),
    (
        "kwk36_l56 88mm_AP",
        [13.447991, 5.931244, -3.6586041],
        [-114.379295, -136.64839, -74.76734],
    ),
    (
        "kwk36_l56 88mm_AP",
        [9.337844, 5.4328265, -8.553225],
        [92.55517, -50.016808, 162.10312],
    ),
    (
        "kwk36_l56 88mm_AP",
        [9.04861, 5.589129, -9.059797],
        [92.55517, -50.016808, 162.10312],
    ),
    (
        "kwk36_l56 88mm_AP",
        [10.570463, 5.4944863, -10.399087],
        [-5.4192257, -50.01674, 186.5865],
    ),
    (
        "kwk36_l56 88mm_APHE_ring",
        [13.448209, 5.9313054, -3.65905],
        [-114.379295, -136.64839, -74.76734],
    ),
    (
        "kwk36_l56 88mm_APHE_ring",
        [13.447991, 5.931244, -3.6586041],
        [-114.379295, -136.64839, -74.76734],
    ),
    (
        "kwk36_l56 88mm_APHE_ring",
        [9.337844, 5.4328265, -8.553225],
        [92.55517, -50.016808, 162.10312],
    ),
    (
        "kwk36_l56 88mm_APHE_ring",
        [9.04861, 5.589129, -9.059797],
        [92.55517, -50.016808, 162.10312],
    ),
    (
        "kwk36_l56 88mm_APHE_ring",
        [10.570463, 5.4944863, -10.399087],
        [-5.4192257, -50.01674, 186.5865],
    ),
    (
        "mg34 7.9mm_AP",
        [11.882101, -3.3190238, -4.647451],
        [153.88496, 666.9333, -318.66043],
    ),
    (
        "mg34 7.9mm_AP",
        [13.480293, 3.381734, -6.838281],
        [-300.5629, 434.80978, 539.0987],
    ),
    (
        "mg34 7.9mm_AP",
        [10.371233, 4.795458, -6.657592],
        [195.4084, -728.5137, -33.291897],
    ),
    (
        "mg34 7.9mm_AP",
        [13.626741, 4.3076735, -4.3707595],
        [-65.802635, -546.7091, 516.531],
    ),
    (
        "mg34 7.9mm_AP",
        [13.627799, 4.562245, -5.3784],
        [-133.4664, 113.61563, -70.034065],
    ),
    (
        "mg34 7.9mm_AP",
        [10.199447, 3.6056461, -7.847377],
        [94.37499, -163.12282, -10.529476],
    ),
    (
        "mg34 7.9mm_AP",
        [13.0494995, 1.7382418, -7.063931],
        [38.47124, 166.73332, -79.66511],
    ),
    (
        "mg34 7.9mm_AP",
        [13.627188, 4.792919, -6.4707155],
        [-94.37499, 163.26141, -8.101518],
    ),
    (
        "mg34 7.9mm_AP",
        [10.372812, 4.7832394, -7.2671714],
        [94.37499, -115.535774, -115.63481],
    ),
    (
        "mg34 7.9mm_AP",
        [13.476771, 3.3868294, -6.8319635],
        [-75.140724, 108.702446, 134.77467],
    ),
    (
        "mg34 7.9mm_AP",
        [10.373523, 4.7869205, -6.657982],
        [48.8521, -182.12842, -8.322974],
    ),
    (
        "mg34 7.9mm_AP",
        [13.625971, 4.3012667, -4.3647065],
        [-16.450659, -136.67728, 129.13275],
    ),
    (
        "mg34 7.9mm_AP",
        [13.628284, 4.320487, -4.3828654],
        [-16.450659, -136.67728, 129.13275],
    ),
    (
        "mg34 7.9mm_AP",
        [10.076007, 1.3322017, -5.4783688],
        [36.176228, 163.79178, -86.54535],
    ),
];

/// One volley's aim at a stance. Fixture data.
struct RecordedVolley {
    at: [f32; 3],
    rotation: [f32; 4],
    muzzle: [f32; 3],
    headings: [[f32; 3]; 12],
}

/// Two volleys flown together on the shipped ground, the first still in the air as the hull stands
/// at the second's stance. Their impulses leave the hull at a pose where a ring sample of the last
/// perforation runs 4.07 m inside one plate at 9e-5 rad to its face (MEASURED by trace). Fixture data.
const ALONG_A_LONG_FACE: [RecordedVolley; 2] = [
    RecordedVolley {
        at: [-1.5183538, 36.70495, -180.25223],
        rotation: [0.0034697661, 0.48154163, 0.029967407, -0.8759039],
        muzzle: [36.78557, 36.34161, -134.06073],
        headings: [
            [-0.64335775, 0.005794236, -0.76554376],
            [-0.66728973, 0.012387606, -0.74469525],
            [-0.6376797, 0.023199372, -0.7699522],
            [-0.6241195, 0.020292642, -0.7810653],
            [-0.6712373, 0.019244406, -0.74099267],
            [-0.67460257, 0.014795419, -0.7380328],
            [-0.66419727, 0.021979976, -0.74723417],
            [-0.65703243, 0.009313175, -0.7538048],
            [-0.62830156, 0.014898684, -0.77782726],
            [-0.64511967, 0.020812515, -0.76379806],
            [-0.64966846, 0.023592759, -0.75985146],
            [-0.67275816, 0.012579845, -0.7397556],
        ],
    },
    RecordedVolley {
        at: [48.706726, 40.30744, -334.5015],
        rotation: [-0.005675448, 0.92951214, 0.019321507, -0.3682411],
        muzzle: [76.51184, 40.35389, -281.1534],
        headings: [
            [-0.44125313, 0.013708162, -0.89727795],
            [-0.48371413, 0.0037529282, -0.87521803],
            [-0.45352304, 0.008243437, -0.89120644],
            [-0.46396488, 0.0014238217, -0.88585246],
            [-0.4147112, 0.003760295, -0.9099453],
            [-0.46606296, 0.012775306, -0.8846593],
            [-0.48613906, 0.0042797127, -0.873871],
            [-0.42873996, 0.009915315, -0.9033735],
            [-0.45848405, 0.004900111, -0.8886891],
            [-0.43908447, 0.0066684755, -0.89842105],
            [-0.49822277, 0.009318875, -0.866999],
            [-0.4645049, 0.013368636, -0.8854697],
        ],
    },
];

/// The game's damage model: a corridor's membership is the parity of crossings behind its start on
/// its own line, so a perforation's next corridor never meets an exit with nothing open, however
/// nearly its ray runs along a long face. A panic ends the server's whole match. Mutant: the next
/// starts' membership looked up on the rays the crossing walked.
#[test]
fn a_volley_along_a_long_face_flies_out() {
    let round = crate::guns::GunRegistry::shipped()
        .fired_rounds()
        .find(|(gun, key, _)| format!("{gun} {key}") == "kwk36_l56 88mm_AP")
        .expect("the arm ships")
        .2;
    let ground = crate::terrain_grid::tests::shipped_grid();
    let mut range = Range::on(RigidBody::Kinematic, Some(ground));
    for volley in &ALONG_A_LONG_FACE {
        range.stand(Stance {
            at: Vec3::from(volley.at),
            rotation: Quat::from_array(volley.rotation),
            velocity: Vec3::ZERO,
            spin: Vec3::ZERO,
        });
        let headings: Vec<Dir3> = volley
            .headings
            .iter()
            .map(|&heading| Dir3::new_unchecked(Vec3::from(heading)))
            .collect();
        range.fire(&round, Vec3::from(volley.muzzle), &headings);
    }
    let (touched, _) = range
        .fly_out(ALONG_A_LONG_FACE.len() * 12)
        .unwrap_or_else(|(message, _)| panic!("{message}"));
    assert!(touched > 0, "the volley no longer touches the hull");
}

/// The game's damage model: no shot ends the process through the march, on the aims that once did:
/// line rounds whose start stood under their resting height over a face, and discs whose rim stayed
/// on a face. A panic ends the server's whole match, and an aim that no longer touches tests nothing.
/// Mutant: a line round turning at first touch with an ungrown probe box, or a blend angle read with
/// `acos`.
#[test]
fn the_shots_that_once_ended_the_process_fly() {
    let registry = crate::guns::GunRegistry::shipped();
    let mut range = Range::new(RigidBody::Static);
    for (arm, birth, post) in ONCE_FATAL {
        let round = registry
            .fired_rounds()
            .find(|(gun, key, _)| format!("{gun} {key}") == *arm)
            .expect("the arm ships")
            .2;
        let flown = range.fly(&round, Vec3::from(*birth), Vec3::from(*post));
        assert_eq!(flown.panicked, None, "{arm} born at {birth:?}");
        assert!(
            flown.first().is_some(),
            "{arm} born at {birth:?} no longer touches the hull"
        );
    }
}
