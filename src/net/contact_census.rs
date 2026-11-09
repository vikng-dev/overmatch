//! Per-tick census of the contact graph and a per-piece rest record, for the game's environment
//! destruction (game state only). An active trace snapshots the completed solver step before
//! sleeping removes its pairs and structural changes retire its bodies. Lifecycle observers
//! reconcile births and retirements independently. `contact_census_us` is a subtractable tick
//! leaf; post-tick reconciliation is inside `collector_us`. `scripts/perf/collapse.py` merges
//! piece records and immutable completed sleep episodes.
//!
//! Only awake touching pairs count (the active set); a sleeping pair is outside the solver.
//! Sensors are excluded: only pairs that generate constraints count.

use avian3d::dynamics::solver::islands::BodyIslandNode;
use avian3d::prelude::{
    AngularVelocity, ContactGraph, ContactPair, LinearVelocity, PhysicsLengthUnit, RigidBody,
    SleepThreshold, Sleeping, TimeToSleep,
};
use bevy::ecs::entity::EntityHashMap;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::tasks::{ComputeTaskPool, ParallelSliceMut};
use serde::Serialize;
use serde_json::{Value, json};

use crate::contact_transfer::ContactTransfers;
use crate::cost::stages::{Capture, Stage, Stages};

use crate::construction::{
    Body,
    breakage::{Piece, SweepAccount},
};
use crate::debris::DebrisPiece;
use crate::tank::Tank;
use crate::track::belt_segment::BeltSegment;

/// Resolution: a pair whose normal impulse over the step is at most this does no work. DERIVED:
/// the weight of 6.5 g over a 64 Hz tick, three orders under a piece at the size floor.
const SEAM_IMPULSE_NS: f32 = 1.0e-3;
/// Resolution: the separation bands the study asked for, metres.
const SEAM_NEAR_M: f32 = 2.0e-3;
const SEAM_FAR_M: f32 = 5.0e-3;

/// One kind's touching pairs: their points, distinct body pairs, and the pairs doing no work.
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct KindCounts {
    pairs: u32,
    points: u32,
    body_pairs: u32,
    /// Pairs whose summed normal impulse over the step is at most [`SEAM_IMPULSE_NS`], and their
    /// points.
    seam_pairs: u32,
    seam_points: u32,
    /// Seam pairs by least separation over their points (overlap reads as separated by at most
    /// the first band): at most 2 mm, at most 5 mm, more.
    seam_within_2mm: u32,
    seam_within_5mm: u32,
    seam_beyond_5mm: u32,
}

impl std::ops::AddAssign for KindCounts {
    fn add_assign(&mut self, other: Self) {
        self.pairs += other.pairs;
        self.points += other.points;
        self.body_pairs += other.body_pairs;
        self.seam_pairs += other.seam_pairs;
        self.seam_points += other.seam_points;
        self.seam_within_2mm += other.seam_within_2mm;
        self.seam_within_5mm += other.seam_within_5mm;
        self.seam_beyond_5mm += other.seam_beyond_5mm;
    }
}

/// A body's side of a touching pair.
#[derive(Clone, Copy)]
enum Side {
    Piece(Entity),
    /// A standing construction.
    Standing,
    /// Terrain or any other static body, a standalone collider included.
    Ground,
    /// A tank collider that is no belt segment (hull and side plates).
    Hull,
    Belt,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    PiecePiece,
    PieceStanding,
    PieceGround,
    PieceHull,
    PieceBelt,
    PieceOther,
    /// No piece on either side.
    Other,
}

impl Kind {
    fn of(a: Side, b: Side) -> Self {
        use Side::*;
        match (a, b) {
            (Piece(_), Piece(_)) => Self::PiecePiece,
            (Piece(_), Standing) | (Standing, Piece(_)) => Self::PieceStanding,
            (Piece(_), Ground) | (Ground, Piece(_)) => Self::PieceGround,
            (Piece(_), Hull) | (Hull, Piece(_)) => Self::PieceHull,
            (Piece(_), Belt) | (Belt, Piece(_)) => Self::PieceBelt,
            (Piece(_), Other) | (Other, Piece(_)) => Self::PieceOther,
            _ => Self::Other,
        }
    }
}

/// The per-kind table of one tick.
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Kinds {
    piece_piece: KindCounts,
    piece_standing: KindCounts,
    piece_ground: KindCounts,
    piece_hull: KindCounts,
    piece_belt: KindCounts,
    piece_other: KindCounts,
    other: KindCounts,
}

impl Kinds {
    fn get_mut(&mut self, kind: Kind) -> &mut KindCounts {
        match kind {
            Kind::PiecePiece => &mut self.piece_piece,
            Kind::PieceStanding => &mut self.piece_standing,
            Kind::PieceGround => &mut self.piece_ground,
            Kind::PieceHull => &mut self.piece_hull,
            Kind::PieceBelt => &mut self.piece_belt,
            Kind::PieceOther => &mut self.piece_other,
            Kind::Other => &mut self.other,
        }
    }

    fn all_mut(&mut self) -> [&mut KindCounts; 7] {
        [
            &mut self.piece_piece,
            &mut self.piece_standing,
            &mut self.piece_ground,
            &mut self.piece_hull,
            &mut self.piece_belt,
            &mut self.piece_other,
            &mut self.other,
        ]
    }

    fn add(&mut self, mut other: Self) {
        for (mine, theirs) in self.all_mut().into_iter().zip(other.all_mut()) {
            *mine += *theirs;
        }
    }
}

/// Awake pieces and pairs still in first fall, under one reading of having reached ground.
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct FirstFall {
    /// Awake pieces that have not reached ground or a standing structure.
    awake: u32,
    /// Awake pieces with contacts, all of them with pieces that have not reached it.
    only_first_fall_contacts: u32,
    /// Touching piece-piece pairs where neither piece has reached it.
    pairs: u32,
}

/// Airborne pieces this tick.
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Airborne {
    awake_pieces: u32,
    /// Awake pieces with no touching pair at all.
    no_contact: u32,
    /// "Reached" means any touching pair with ground or a standing structure.
    touch: FirstFall,
    /// "Reached" means such a pair that carried normal impulse, the stricter reading: a chunk born
    /// beside a still-standing neighbour touches it without landing on it.
    loaded: FirstFall,
}

/// Islands holding an awake piece.
#[derive(Clone, Copy, Default, Serialize)]
pub(crate) struct Islands {
    with_awake_piece: u32,
    /// The awake pieces of the island holding the most of them.
    largest_awake_pieces: u32,
    /// Of `with_awake_piece`, those that also hold a tank.
    with_tank: u32,
}

/// Sweep sources by whether their swept bounds met a standing owner.
#[derive(Clone, Default, Serialize)]
pub(crate) struct SweepSplit {
    sources_no_owner: u32,
    sources_owner_met: u32,
    /// Shape casts; a source that met no owner casts none.
    casts: u32,
    /// Casts of the source that cast the most.
    max_casts_per_source: u32,
    /// One entry per source, zero-cast sources included, summed across all its owners.
    casts_per_source: Vec<u32>,
}

/// One tick's contact census: a nested row field, so `collapse.py`'s `_us` summation skips it.
#[derive(Clone, Default, Serialize)]
pub(crate) struct ContactTick {
    census_us: f64,
    kinds: Kinds,
    airborne: Airborne,
    islands: Islands,
    sweep: SweepSplit,
}

/// One piece's rest history, on the census's tick numbers (the trace's row numbers).
#[derive(Clone, Copy, Serialize)]
struct PieceRecord {
    /// Entity bits, the merge key across stats lines.
    piece: u64,
    /// `None` for a debris-stress fragment, which belongs to no construction.
    construction: Option<u16>,
    chunks: usize,
    /// First tick in the capture: birth, or the first observation of a piece already present.
    born: u32,
    /// First tick touching ground or a standing structure, and first with normal impulse.
    first_ground: Option<u32>,
    first_loaded: Option<u32>,
    /// Latest transition from no support contact to touching/loaded support in this awake episode.
    last_ground: Option<u32>,
    last_loaded: Option<u32>,
    episode_born: u32,
    #[serde(skip)]
    support: u8,
    #[serde(skip)]
    motion: (f32, f32),
    /// Last tick its speed was not under the sleep thresholds (Avian's own comparison).
    last_fast: Option<u32>,
    /// The latest tick it went to sleep, the island it slept in and its speeds then (m/s, rad/s).
    sleep: Option<u32>,
    island: Option<u32>,
    linear_at_sleep: Option<f32>,
    angular_at_sleep: Option<f32>,
    sleeps: u32,
    awake_ticks: u32,
    /// The tick it left the world (split, retired or despawned).
    gone: Option<u32>,
    #[serde(skip)]
    awake: bool,
    #[serde(skip)]
    changed: bool,
}

/// The sleep thresholds pieces were compared with: linear in m/s, angular in rad/s.
#[derive(Clone, Copy, Serialize)]
struct Thresholds {
    linear: f32,
    angular: f32,
    time_to_sleep_s: f32,
}

/// Every piece's record so far. Stats lines drain what changed; the reader keeps each piece's
/// latest, so the last line's snapshot is at most one period stale for a piece still awake.
#[derive(Resource, Default)]
pub(crate) struct PieceRecords {
    tick: u32,
    thresholds: Option<Thresholds>,
    live: EntityHashMap<PieceRecord>,
    finished: Vec<PieceRecord>,
    episodes: Vec<PieceRecord>,
}

impl PieceRecords {
    /// The records changed since the last call.
    pub(super) fn drain(&mut self) -> Value {
        let mut records = std::mem::take(&mut self.finished);
        records.extend(
            self.live
                .values_mut()
                .filter_map(|record| std::mem::take(&mut record.changed).then_some(*record)),
        );
        json!({"thresholds": self.thresholds, "records": records, "episodes": std::mem::take(&mut self.episodes)})
    }
}

/// The existing ram/loadtest recorders own activity; sweep counters read only this tick's flag.
#[derive(Resource, Default)]
pub(super) struct StepSnapshot {
    row: Option<ContactTick>,
}

impl PieceRecord {
    fn new(entity: Entity, piece: Option<&Piece>, tick: u32) -> Self {
        Self {
            piece: entity.to_bits(),
            construction: piece.map(|p| p.construction),
            chunks: piece.map_or(1, |p| p.chunks.len()),
            born: tick,
            first_ground: None,
            first_loaded: None,
            last_ground: None,
            last_loaded: None,
            episode_born: tick,
            support: 0,
            motion: (0.0, 0.0),
            last_fast: None,
            sleep: None,
            island: None,
            linear_at_sleep: None,
            angular_at_sleep: None,
            sleeps: 0,
            awake_ticks: 0,
            gone: None,
            awake: true,
            changed: true,
        }
    }
}

fn begin(
    ram: Option<Res<super::ram::Ram>>,
    load: Option<Res<super::loadtest::Loadtest>>,
    mut capture: ResMut<Capture>,
    mut snapshot: ResMut<StepSnapshot>,
    mut records: ResMut<PieceRecords>,
) {
    capture.active = ram.as_deref().is_some_and(super::ram::Ram::recording)
        || load
            .as_deref()
            .is_some_and(super::loadtest::Loadtest::recording);
    snapshot.row = None;
    if capture.active {
        records.tick += 1;
    }
}

fn activate(
    ram: Option<Res<super::ram::Ram>>,
    load: Option<Res<super::loadtest::Loadtest>>,
    mut capture: ResMut<Capture>,
    mut records: ResMut<PieceRecords>,
) {
    // Loadtest admission opens in FixedUpdate; its first row still needs this step's snapshot.
    if !capture.active
        && (ram.as_deref().is_some_and(super::ram::Ram::recording)
            || load
                .as_deref()
                .is_some_and(super::loadtest::Loadtest::recording))
    {
        capture.active = true;
        records.tick += 1;
    }
}

fn snapshot(
    mut census: ContactCensus,
    capture: Res<Capture>,
    mut snapshot: ResMut<StepSnapshot>,
    stages: Res<Stages>,
) {
    if capture.active {
        let _leaf = Stages::start(Some(&stages), Stage::ContactCensus);
        snapshot.row = Some(census.tick());
    }
}

/// Installed only with the stats recorder. Observers keep even a piece born and retired between
/// snapshots; the pre-sleep read owns motion and contacts, not structural publication order.
pub(super) fn install(app: &mut App) {
    install_reading(app);
    app.add_systems(
        bevy::app::FixedMain,
        begin.before(bevy::app::FixedMain::run_fixed_main),
    );
    app.add_systems(
        FixedPostUpdate,
        activate.before(crate::construction::breakage::SweepSet),
    );
}

fn install_reading(app: &mut App) {
    use avian3d::prelude::{PhysicsSchedule, PhysicsStepSystems};
    app.init_resource::<PieceRecords>()
        .init_resource::<Capture>()
        .init_resource::<StepSnapshot>()
        .add_systems(
            PhysicsSchedule,
            snapshot
                .after(PhysicsStepSystems::Solver)
                .before(PhysicsStepSystems::Sleeping),
        )
        .add_observer(
            |event: On<Add, DebrisPiece>,
             capture: Res<Capture>,
             mut records: ResMut<PieceRecords>,
             pieces: Query<Option<&Piece>>,
             stages: Res<Stages>| {
                if capture.active {
                    let _leaf = Stages::start(Some(&stages), Stage::ContactCensus);
                    let tick = records.tick;
                    let piece = pieces
                        .get(event.entity)
                        .expect("an added piece exists during its observer");
                    records
                        .live
                        .insert(event.entity, PieceRecord::new(event.entity, piece, tick));
                }
            },
        )
        .add_observer(
            |event: On<Remove, DebrisPiece>,
             capture: Res<Capture>,
             mut records: ResMut<PieceRecords>,
             stages: Res<Stages>,
             pieces: Query<Option<&Piece>>| {
                if capture.active {
                    let _leaf = Stages::start(Some(&stages), Stage::ContactCensus);
                    let mut record = records.live.remove(&event.entity).unwrap_or_else(|| {
                        let piece = pieces
                            .get(event.entity)
                            .expect("the retiring piece exists in its removal observer");
                        PieceRecord::new(event.entity, piece, records.tick)
                    }); // A piece already present at admission may retire before its first solve.
                    record.gone = Some(records.tick);
                    records.finished.push(record);
                }
            },
        )
        .add_observer(
            |event: On<Add, Sleeping>,
             capture: Res<Capture>,
             mut records: ResMut<PieceRecords>,
             pieces: Query<&BodyIslandNode, With<DebrisPiece>>,
             stages: Res<Stages>| {
                if capture.active
                    && let Ok(island) = pieces.get(event.entity)
                {
                    let _leaf = Stages::start(Some(&stages), Stage::ContactCensus);
                    let tick = records.tick;
                    let record = records
                        .live
                        .get_mut(&event.entity)
                        .expect("a piece is recorded at birth or before sleeping");
                    record.sleep = Some(tick);
                    record.island = Some(island.island_id().0);
                    record.linear_at_sleep = Some(record.motion.0);
                    record.angular_at_sleep = Some(record.motion.1);
                    record.sleeps += 1;
                    record.awake = false;
                    record.changed = true;
                    let episode = *record;
                    records.episodes.push(episode);
                }
            },
        );
}

/// Attach the pre-physics sweep account to the completed step, without re-reading its graph.
pub(super) fn finish(snapshot: &mut StepSnapshot, sweep: &SweepAccount) -> ContactTick {
    let mut row = snapshot
        .row
        .take()
        .expect("an active capture snapshots every completed physics step");
    row.sweep = SweepSplit {
        sources_no_owner: sweep.sources_no_owner as u32,
        sources_owner_met: (sweep.sources - sweep.sources_no_owner) as u32,
        casts: sweep.work.casts as u32,
        max_casts_per_source: sweep.max_casts_per_source as u32,
        casts_per_source: sweep.casts_per_source.iter().map(|&n| n as u32).collect(),
    };
    row
}

enum Reach {
    Piece(Entity),
    /// Ground or a standing structure; whether the pair carried load.
    Ground(bool),
    /// A tank or another body: not a piece, not support.
    Else,
}

/// One piece's end of a touching pair.
struct Touch {
    piece: Entity,
    reach: Reach,
}

/// A compute worker's reusable share of the census.
#[derive(Default)]
struct PairWork {
    kinds: Kinds,
    body_pairs: Vec<(Kind, Entity, Entity)>,
    touches: Vec<Touch>,
}

/// The piece has a touching pair.
const CONTACT: u8 = 1;
/// It touches something other than a piece.
const NON_PIECE: u8 = 2;
/// It touches a piece that has reached ground, in each reading of [`Airborne`].
const BLOCKED_TOUCH: u8 = 4;
const BLOCKED_LOADED: u8 = 8;
const GROUND: u8 = 16;
const LOADED: u8 = 32;

type PieceFields = (
    Entity,
    &'static LinearVelocity,
    &'static AngularVelocity,
    &'static SleepThreshold,
    &'static BodyIslandNode,
    Has<Sleeping>,
    Option<&'static Piece>,
);

#[derive(bevy::ecs::system::SystemParam)]
pub(super) struct ContactCensus<'w, 's> {
    graph: Res<'w, ContactGraph>,
    transfers: Res<'w, ContactTransfers>,
    normal_transfers: Local<'s, HashMap<(Entity, Entity), f32>>,
    bodies: Query<'w, 's, &'static RigidBody>,
    standing: Query<'w, 's, (), With<Body>>,
    tanks: Query<'w, 's, &'static BodyIslandNode, With<Tank>>,
    tank_marks: Query<'w, 's, (), With<Tank>>,
    belts: Query<'w, 's, (), With<BeltSegment>>,
    pieces: Query<'w, 's, PieceFields, With<DebrisPiece>>,
    piece_marks: Query<'w, 's, (), With<DebrisPiece>>,
    length_unit: Res<'w, PhysicsLengthUnit>,
    time_to_sleep: Res<'w, TimeToSleep>,
    records: ResMut<'w, PieceRecords>,
    workers: Local<'s, Vec<PairWork>>,
    body_pairs: Local<'s, Vec<(Kind, Entity, Entity)>>,
    flags: Local<'s, EntityHashMap<u8>>,
    awake_islands: Local<'s, HashMap<u32, u32>>,
    tank_islands: Local<'s, HashSet<u32>>,
}

impl ContactCensus<'_, '_> {
    fn side(&self, collider: Entity, body: Option<Entity>) -> Side {
        let Some(body) = body else {
            return Side::Ground; // A collider with no body is static.
        };
        if self.piece_marks.contains(body) {
            Side::Piece(body)
        } else if self.standing.contains(body) {
            Side::Standing
        } else if self.tank_marks.contains(body) {
            if self.belts.contains(collider) {
                Side::Belt
            } else {
                Side::Hull
            }
        } else if self.bodies.get(body).is_ok_and(RigidBody::is_static) {
            Side::Ground
        } else {
            Side::Other // Includes a body that left the world after the solve.
        }
    }

    fn count_pair(&self, pair: &ContactPair, work: &mut PairWork) {
        let a = self.side(pair.collider1, pair.body1);
        let b = self.side(pair.collider2, pair.body2);
        let kind = Kind::of(a, b);
        let counts = work.kinds.get_mut(kind);
        let points = pair
            .manifolds
            .iter()
            .map(|m| m.points.len() as u32)
            .sum::<u32>();
        let key = (
            pair.collider1.min(pair.collider2),
            pair.collider1.max(pair.collider2),
        );
        // A speculative pair may generate no solver constraint and transfers no momentum.
        let impulse = self.normal_transfers.get(&key).copied().unwrap_or(0.0);
        let loaded = impulse > SEAM_IMPULSE_NS;
        counts.pairs += 1;
        counts.points += points;
        if !loaded {
            counts.seam_pairs += 1;
            counts.seam_points += points;
            let separation = pair
                .manifolds
                .iter()
                .flat_map(|m| m.points.iter())
                .map(|p| -p.penetration)
                .fold(f32::INFINITY, f32::min);
            if separation <= SEAM_NEAR_M {
                counts.seam_within_2mm += 1;
            } else if separation <= SEAM_FAR_M {
                counts.seam_within_5mm += 1;
            } else {
                counts.seam_beyond_5mm += 1;
            }
        }
        let first = pair.body1.unwrap_or(pair.collider1);
        let second = pair.body2.unwrap_or(pair.collider2);
        work.body_pairs
            .push((kind, first.min(second), first.max(second)));
        for (mine, theirs) in [(a, b), (b, a)] {
            let Side::Piece(piece) = mine else { continue };
            let reach = match theirs {
                Side::Piece(other) => Reach::Piece(other),
                Side::Standing | Side::Ground => Reach::Ground(loaded),
                Side::Hull | Side::Belt | Side::Other => Reach::Else,
            };
            work.touches.push(Touch { piece, reach });
        }
    }

    /// Every piece's motion this tick, before any of its contacts are read.
    fn record_motion(&mut self, tick: u32) {
        let unit = self.length_unit.0;
        let time_to_sleep = self.time_to_sleep.0;
        let records = &mut *self.records;
        for (entity, velocity, spin, rest, _, sleeping, piece) in &self.pieces {
            records.thresholds = Some(Thresholds {
                linear: rest.linear * unit,
                angular: rest.angular,
                time_to_sleep_s: time_to_sleep,
            });
            let record = records
                .live
                .entry(entity)
                .or_insert_with(|| PieceRecord::new(entity, piece, tick));
            if !sleeping && !record.awake {
                record.episode_born = tick;
                record.last_ground = None;
                record.last_loaded = None;
                record.support = 0;
                record.last_fast = None;
                record.sleep = None;
                record.island = None;
                record.linear_at_sleep = None;
                record.angular_at_sleep = None;
                record.awake_ticks = 0;
            }
            let (linear, angular) = (velocity.length(), spin.length());
            record.motion = (linear, angular);
            if !sleeping {
                record.awake_ticks += 1;
                record.changed = true;
                let linear_limit = unit * unit * rest.linear * rest.linear.abs();
                let angular_limit = rest.angular * rest.angular.abs();
                if linear * linear >= linear_limit || angular * angular >= angular_limit {
                    record.last_fast = Some(tick);
                }
            }
            record.awake = !sleeping;
        }
    }

    /// This tick's census, updating every piece's record. `sweep` is this tick's account.
    fn tick(&mut self) -> ContactTick {
        let started = std::time::Instant::now();
        let tick = self.records.tick;
        self.record_motion(tick);
        self.normal_transfers.clear();
        for point in self.transfers.solver_groups().flatten() {
            let other = point
                .other_collider
                .expect("a solver transfer has two colliders");
            let key = (
                point.reference_collider.min(other),
                point.reference_collider.max(other),
            );
            *self.normal_transfers.entry(key).or_default() += point.impulse.dot(point.normal);
        }

        // Each batch owns a disjoint pair range and its reusable buffers.
        let pairs = self.graph.active_pairs();
        let pool = ComputeTaskPool::get();
        let batches = pool.thread_num();
        let mut workers = std::mem::take(&mut *self.workers);
        workers.resize_with(batches, PairWork::default);
        let census = &*self;
        workers.par_chunk_map_mut(pool, 1, |worker, work| {
            let work = &mut work[0];
            work.kinds = Kinds::default();
            work.body_pairs.clear();
            work.touches.clear();
            let range = pairs.len() * worker / batches..pairs.len() * (worker + 1) / batches;
            for pair in &pairs[range] {
                if pair.is_touching() && pair.generates_constraints() {
                    census.count_pair(pair, work);
                }
            }
        });

        let mut row = ContactTick::default();
        let body_pairs = &mut *self.body_pairs;
        body_pairs.clear();
        for work in &mut workers {
            row.kinds.add(work.kinds);
            body_pairs.append(&mut work.body_pairs);
        }
        // Incidence spans batches; only the keyed reduction is serial.
        body_pairs.sort_unstable();
        body_pairs.dedup();
        for &(kind, ..) in body_pairs.iter() {
            row.kinds.get_mut(kind).body_pairs += 1;
        }

        // First every piece's ground state, so a pair reads both ends as of this tick.
        let records = &mut *self.records;
        let flags = &mut *self.flags;
        flags.clear();
        for touch in workers.iter().flat_map(|work| &work.touches) {
            let flag = flags.entry(touch.piece).or_default();
            *flag |= CONTACT;
            if !matches!(touch.reach, Reach::Piece(_)) {
                *flag |= NON_PIECE;
            }
            if let Reach::Ground(loaded) = touch.reach {
                *flag |= GROUND | if loaded { LOADED } else { 0 };
                let record = records
                    .live
                    .get_mut(&touch.piece)
                    .expect("every piece was recorded before its pairs");
                if record.support & 1 == 0 {
                    record.last_ground = Some(tick);
                }
                record.first_ground.get_or_insert(tick);
                if loaded {
                    if record.support & 2 == 0 {
                        record.last_loaded = Some(tick);
                    }
                    record.first_loaded.get_or_insert(tick);
                }
                record.changed = true;
            }
        }
        let (mut touch_pairs, mut loaded_pairs) = (0, 0);
        for touch in workers.iter().flat_map(|work| &work.touches) {
            let Reach::Piece(other) = touch.reach else {
                continue;
            };
            let reading = |piece: &Entity| {
                let record = &records.live[piece];
                (record.first_ground.is_none(), record.first_loaded.is_none())
            };
            let (touch_fall, loaded_fall) = reading(&other);
            let flag = flags
                .get_mut(&touch.piece)
                .expect("set by this touch's first pass");
            if !touch_fall {
                *flag |= BLOCKED_TOUCH;
            }
            if !loaded_fall {
                *flag |= BLOCKED_LOADED;
            }
            // Each piece-piece pair has two touches: count it at its lesser end.
            if touch.piece < other {
                let (own_touch, own_loaded) = reading(&touch.piece);
                touch_pairs += u32::from(touch_fall && own_touch);
                loaded_pairs += u32::from(loaded_fall && own_loaded);
            }
        }
        row.airborne.touch.pairs = touch_pairs;
        row.airborne.loaded.pairs = loaded_pairs;

        let awake_islands = &mut *self.awake_islands;
        awake_islands.clear();
        for (entity, _, _, _, island, sleeping, _) in &self.pieces {
            if sleeping {
                continue;
            }
            row.airborne.awake_pieces += 1;
            *awake_islands.entry(island.island_id().0).or_default() += 1;
            let record = &records.live[&entity];
            row.airborne.touch.awake += u32::from(record.first_ground.is_none());
            row.airborne.loaded.awake += u32::from(record.first_loaded.is_none());
            match flags.get(&entity) {
                None => row.airborne.no_contact += 1,
                Some(flag) if flag & NON_PIECE == 0 => {
                    row.airborne.touch.only_first_fall_contacts +=
                        u32::from(flag & BLOCKED_TOUCH == 0);
                    row.airborne.loaded.only_first_fall_contacts +=
                        u32::from(flag & BLOCKED_LOADED == 0);
                }
                Some(_) => {}
            }
        }
        row.islands.with_awake_piece = awake_islands.len() as u32;
        // No awake piece means no island to measure.
        row.islands.largest_awake_pieces = awake_islands.values().copied().max().unwrap_or(0);
        let tank_islands = &mut *self.tank_islands;
        tank_islands.clear();
        row.islands.with_tank = self
            .tanks
            .iter()
            .filter(|island| {
                let id = island.island_id().0;
                awake_islands.contains_key(&id) && tank_islands.insert(id)
            })
            .count() as u32;

        for (entity, record) in &mut records.live {
            // Sleeping contacts are outside the solver; their last support persists until wake.
            if record.awake {
                let flags = flags.get(entity).copied().unwrap_or(0);
                record.support =
                    u8::from(flags & GROUND != 0) | (u8::from(flags & LOADED != 0) << 1);
            }
        }
        *self.workers = workers;
        row.census_us = started.elapsed().as_secs_f64() * 1.0e6;
        row
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avian3d::prelude::{Collider, Gravity};

    const CUBE: f32 = 1.0; // Fixture: a unit cube, the only shape every scene needs.

    fn app(gravity: Vec3) -> App {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            TransformPlugin,
            AssetPlugin::default(),
            crate::offline_physics(),
        ))
        .init_asset::<Mesh>()
        .insert_resource(Gravity(gravity))
        .init_resource::<PieceRecords>()
        .init_resource::<SweepAccount>();
        app.init_resource::<Stages>();
        install_reading(&mut app);
        app.world_mut().resource_mut::<Capture>().active = true;
        app.add_systems(FixedFirst, |mut records: ResMut<PieceRecords>| {
            records.tick += 1;
        });
        crate::headless_test::start_fixed_clock(&mut app);
        app.add_plugins(crate::contact_transfer::sim_plugin);
        crate::headless_test::finish_plugins(&mut app);
        app.update(); // Initialize the real clock before the fixture's first measured step.
        app
    }

    fn cube_at(x: f32, y: f32) -> impl Bundle {
        (
            Transform::from_xyz(x, y, 0.0),
            Collider::cuboid(CUBE, CUBE, CUBE),
        )
    }

    fn piece(app: &mut App, x: f32, y: f32) -> Entity {
        app.world_mut()
            .spawn((RigidBody::Dynamic, DebrisPiece, cube_at(x, y)))
            .id()
    }

    /// A slab whose top face is y = 0.
    fn ground(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                RigidBody::Static,
                Transform::from_xyz(0.0, -0.5, 0.0),
                Collider::cuboid(400.0, CUBE, 400.0),
            ))
            .id()
    }

    fn read_pairs(app: &mut App) -> ContactTick {
        app.world_mut()
            .run_system_cached(|mut census: ContactCensus| census.tick())
            .unwrap()
    }

    fn step(app: &mut App, ticks: usize) -> ContactTick {
        for _ in 0..ticks {
            app.update();
        }
        app.world_mut()
            .run_system_cached(
                |mut snapshot: ResMut<StepSnapshot>, sweep: Res<SweepAccount>| {
                    finish(&mut snapshot, &sweep)
                },
            )
            .unwrap()
    }

    /// The table must split pairs by what touches what, and tell a body touched through two
    /// colliders from two bodies touched. Zero gravity: nothing is pressed, so every pair is a
    /// seam; a miscount by kind or a confusion of body pairs with collider pairs breaks it.
    #[test]
    fn pairs_are_kinded_and_collider_pairs_are_told_from_body_pairs() {
        let mut app = app(Vec3::ZERO);
        ground(&mut app);
        // Fixture: a piece on two side-by-side colliders on the slab: 2 collider pairs, 1 body pair.
        let wide = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                DebrisPiece,
                Transform::from_xyz(0.0, 0.5, 0.0),
            ))
            .id();
        for x in [-1.0, 1.0] {
            let local = Transform::from_xyz(x, 0.0, 0.0);
            app.world_mut()
                .spawn((ChildOf(wide), local, Collider::cuboid(CUBE, CUBE, CUBE)));
        }
        // Fixture: a piece on a piece on the slab.
        piece(&mut app, 10.0, 0.5);
        piece(&mut app, 10.0, 1.5);
        // Fixture: a piece on a standing construction, high off the slab.
        app.world_mut()
            .spawn((RigidBody::Static, Body, cube_at(20.0, 5.5)));
        piece(&mut app, 20.0, 6.5);
        // Fixture: pieces on a tank's hull and on its belt segment.
        let tank = app
            .world_mut()
            .spawn((
                RigidBody::Dynamic,
                Tank,
                Transform::from_xyz(30.0, 5.5, 0.0),
            ))
            .id();
        app.world_mut().spawn((
            ChildOf(tank),
            Transform::IDENTITY,
            Collider::cuboid(CUBE, CUBE, CUBE),
        ));
        app.world_mut().spawn((
            ChildOf(tank),
            Transform::from_xyz(3.0, 0.0, 0.0),
            Collider::cuboid(CUBE, CUBE, CUBE),
            BeltSegment::FIXTURE,
        ));
        piece(&mut app, 30.0, 6.5);
        piece(&mut app, 33.0, 6.5);
        // Fixture: a piece on an unmarked dynamic body, itself on the slab (a pair with no piece).
        app.world_mut()
            .spawn((RigidBody::Dynamic, cube_at(40.0, 0.5)));
        piece(&mut app, 40.0, 1.5);

        let row = step(&mut app, 4);
        let kinds = row.kinds;
        for (name, counts, pairs, bodies) in [
            ("piece_ground", kinds.piece_ground, 3, 2),
            ("piece_piece", kinds.piece_piece, 1, 1),
            ("piece_standing", kinds.piece_standing, 1, 1),
            ("piece_hull", kinds.piece_hull, 1, 1),
            ("piece_belt", kinds.piece_belt, 1, 1),
            ("piece_other", kinds.piece_other, 1, 1),
            ("other", kinds.other, 1, 1),
        ] {
            assert_eq!(counts.pairs, pairs, "{name} collider pairs");
            assert_eq!(counts.body_pairs, bodies, "{name} body pairs");
            assert_eq!(counts.seam_pairs, pairs, "{name}: nothing is pressed");
            assert!(counts.points >= pairs, "{name} carries points");
            assert_eq!(counts.seam_points, counts.points, "{name} seam points");
        }
    }

    /// Seam share's definition: a pressed pair is no seam, and an unpressed pair is banded by its
    /// least separation over its points. Fixture separations sit either side of each band edge.
    #[test]
    fn a_pressed_pair_is_no_seam_and_seams_band_by_least_separation() {
        let mut pressed = app(Vec3::NEG_Y * 9.81); // Fixture: gravity presses the cube on the slab.
        ground(&mut pressed);
        piece(&mut pressed, 0.0, 0.5);
        let row = step(&mut pressed, 8);
        assert_eq!(row.kinds.piece_ground.pairs, 1);
        assert_eq!(row.kinds.piece_ground.seam_pairs, 0);

        let mut free = app(Vec3::ZERO);
        ground(&mut free);
        piece(&mut free, 0.0, 0.5);
        step(&mut free, 4);
        for (separation, near, middle, far) in [
            (-0.001, 1, 0, 0), // Overlap reads as the nearest band.
            (0.002, 1, 0, 0),
            (0.003, 0, 1, 0),
            (0.005, 0, 1, 0),
            (0.006, 0, 0, 1),
        ] {
            let mut graph = free.world_mut().resource_mut::<ContactGraph>();
            for pair in graph.active_pairs_mut() {
                for point in pair.manifolds.iter_mut().flat_map(|m| m.points.iter_mut()) {
                    point.penetration = -separation;
                }
            }
            let row = read_pairs(&mut free);
            let counts = row.kinds.piece_ground;
            assert_eq!(
                (
                    counts.seam_within_2mm,
                    counts.seam_within_5mm,
                    counts.seam_beyond_5mm
                ),
                (near, middle, far),
                "separation {separation}"
            );
        }
    }

    /// Who is in first fall, and the islands the awake pieces share. A chain a-b-c hangs off the
    /// slab through c alone, so a and b have never touched ground; d shares an island with a tank;
    /// e touches nothing. Dropping the grounded-neighbour check, counting a pair at both ends or
    /// counting a static slab as linking an island breaks this.
    #[test]
    fn first_fall_and_islands_read_the_touch_graph() {
        let mut app = app(Vec3::ZERO);
        ground(&mut app);
        piece(&mut app, 0.0, 2.5); // a, on b.
        piece(&mut app, 0.0, 1.5); // b, on c.
        piece(&mut app, 0.0, 0.5); // c, on the slab.
        let tank = app
            .world_mut()
            .spawn((RigidBody::Dynamic, Tank, cube_at(10.0, 5.5)))
            .id();
        piece(&mut app, 10.0, 6.5); // d, on the tank.
        piece(&mut app, 20.0, 5.5); // e, free.
        let row = step(&mut app, 4);
        assert!(app.world().get::<RigidBody>(tank).is_some());
        let air = row.airborne;
        assert_eq!(air.awake_pieces, 5);
        assert_eq!(air.no_contact, 1, "e");
        assert_eq!(air.touch.awake, 4, "a, b, d, e");
        assert_eq!(
            air.touch.only_first_fall_contacts, 1,
            "a; b touches grounded c and d touches a tank"
        );
        assert_eq!(air.touch.pairs, 1, "a-b; b-c has a grounded end");
        assert_eq!(row.islands.with_awake_piece, 3, "a-b-c, d with the tank, e");
        assert_eq!(row.islands.largest_awake_pieces, 3);
        assert_eq!(row.islands.with_tank, 1);
    }

    /// The piece record must carry the rest history the rest summary relies on: landing before
    /// sleep, the last fast tick a full `TimeToSleep` before sleep, and a despawn once.
    #[test]
    fn a_piece_record_follows_fall_rest_and_departure() {
        let mut app = app(Vec3::NEG_Y * 9.81); // Fixture: falls about 0.1 m onto the slab.
        ground(&mut app);
        let cube = piece(&mut app, 0.0, 0.6);
        let mut sleep = None;
        for _ in 0..400 {
            let row = step(&mut app, 1);
            if app.world().get::<Sleeping>(cube).is_some() {
                assert_eq!(
                    row.kinds.piece_ground.pairs, 1,
                    "the sleep step still has its final contact"
                );
                sleep = Some(());
                break;
            }
        }
        sleep.expect("a settled cube sleeps inside the fixture window");
        let rest_ticks = {
            let time = app
                .world()
                .resource::<Time<Fixed>>()
                .timestep()
                .as_secs_f32();
            (app.world().resource::<TimeToSleep>().0 / time).floor() as u32
        };
        let records = app.world_mut().resource_mut::<PieceRecords>();
        let record = records.live[&cube];
        let (ground_tick, loaded) = (record.first_ground.unwrap(), record.first_loaded.unwrap());
        let (last_fast, slept) = (record.last_fast.unwrap(), record.sleep.unwrap());
        assert_eq!(record.born, 0);
        assert!(ground_tick <= loaded);
        assert!(last_fast + rest_ticks <= slept, "{last_fast} {slept}");
        assert_eq!(record.sleeps, 1);
        assert!(record.island.is_some());
        assert_eq!(record.awake_ticks, slept);

        // Sleep ticks are stable: a further tick neither changes the record nor reports it again.
        step(&mut app, 1);
        let mut records = app.world_mut().resource_mut::<PieceRecords>();
        assert_eq!(records.drain()["records"].as_array().unwrap().len(), 1);
        assert_eq!(records.drain()["records"].as_array().unwrap().len(), 0);

        app.world_mut().entity_mut(cube).despawn();
        step(&mut app, 1);
        let mut records = app.world_mut().resource_mut::<PieceRecords>();
        let drained = records.drain();
        assert_eq!(drained["records"][0]["gone"], slept + 1);
        assert!(records.live.is_empty());
        assert_eq!(records.drain()["records"].as_array().unwrap().len(), 0);
    }
    /// The count reader needs the final contact of a body retired in the same tick and every
    /// birth, including a departure that never survives to the next physics step.
    #[test]
    fn pre_sleep_snapshot_survives_retirement_and_lifecycle_keeps_short_lived_pieces() {
        let mut app = app(Vec3::ZERO);
        ground(&mut app);
        let cube = piece(&mut app, 0.0, 0.5);
        app.add_systems(
            FixedPostUpdate,
            (move |mut commands: Commands| {
                commands.entity(cube).despawn();
                let brief = commands
                    .spawn((RigidBody::Dynamic, DebrisPiece, cube_at(10.0, 10.0)))
                    .id();
                commands.entity(brief).despawn();
            })
            .after(avian3d::prelude::PhysicsSystems::StepSimulation)
            .run_if(run_once),
        );
        let row = step(&mut app, 1);
        assert_eq!(
            row.kinds.piece_ground.pairs, 1,
            "the retired body's completed contact survives"
        );
        let mut records = app.world_mut().resource_mut::<PieceRecords>();
        let drained = records.drain();
        let rows = drained["records"].as_array().unwrap();
        assert_eq!(
            rows.len(),
            2,
            "birth and retirement do not require a surviving body"
        );
        assert!(
            rows.iter()
                .all(|r| r["born"] == 0 && r["gone"] == 1 || r["born"] == 1 && r["gone"] == 1)
        );
        assert!(
            rows.iter()
                .any(|r| r["piece"] == cube.to_bits() && r["first_ground"] == 1)
        );
    }

    /// Seam and first-fall readers use transferred normal momentum, independent of the solver's
    /// accumulated display impulse. Both sides of the seam cutoff, including equality, matter.
    #[test]
    fn normal_transfer_cutoff_drives_seams_and_loaded_first_fall() {
        use crate::contact_transfer::ContactTransfer;
        for (impulse, seam) in [
            (SEAM_IMPULSE_NS * 0.6, true),
            (SEAM_IMPULSE_NS, true),
            (SEAM_IMPULSE_NS * 1.1, false),
        ] {
            let mut app = app(Vec3::ZERO);
            let slab = ground(&mut app);
            let cube = piece(&mut app, 0.0, 0.5);
            step(&mut app, 1);
            app.world_mut().insert_resource(ContactTransfers::fixture(
                vec![vec![ContactTransfer {
                    reference: slab,
                    reference_collider: slab,
                    other: cube,
                    other_collider: Some(cube),
                    point: Vec3::ZERO,
                    normal: Vec3::Y,
                    impulse: Vec3::Y * impulse,
                    effective_mass: 1.0,
                    closing_speed: 0.0,
                }]],
                1.0 / 64.0,
            ));
            for pair in app
                .world_mut()
                .resource_mut::<ContactGraph>()
                .active_pairs_mut()
            {
                for point in pair.manifolds.iter_mut().flat_map(|m| &mut m.points) {
                    point.normal_impulse = 100.0;
                }
            }
            let row = read_pairs(&mut app);
            assert_eq!(row.kinds.piece_ground.seam_pairs, u32::from(seam));
            assert_eq!(row.airborne.loaded.awake, u32::from(seam));
        }
    }

    /// A wake cannot mutate a completed sleep cohort. Last landing belongs to the new awake
    /// episode while the first contact remains the piece's first-fall evidence.
    #[test]
    fn waking_and_resleeping_preserves_completed_episodes() {
        let mut app = app(Vec3::ZERO);
        ground(&mut app);
        let cube = piece(&mut app, 0.0, 0.5);
        step(&mut app, 2);
        app.world_mut().entity_mut(cube).insert(Sleeping);
        let first = app.world().resource::<PieceRecords>().episodes[0];
        app.world_mut()
            .entity_mut(cube)
            .remove::<Sleeping>()
            .insert(LinearVelocity(Vec3::X));
        step(&mut app, 1);
        let current = app.world().resource::<PieceRecords>().live[&cube];
        assert!(current.sleep.is_none());
        assert_eq!(current.last_fast, Some(3));
        app.world_mut().entity_mut(cube).insert(Sleeping);
        let records = app.world().resource::<PieceRecords>();
        assert_eq!(records.episodes.len(), 2);
        assert_eq!(records.episodes[0].sleep, first.sleep);
        assert_eq!(records.episodes[0].last_fast, first.last_fast);
        assert_eq!(records.episodes[1].sleep, Some(3));
        assert_eq!(records.episodes[1].first_ground, first.first_ground);
    }
}
