//! Wall/process CPU brackets and work counts for the game's authority simulation (game state only).
//! Only the authority stats consumer installs this resource. No lock is held during work,
//! including owner-parallel evaluation; overlapping belt brackets publish one wall interval.

use super::clock::Edge;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use bevy::prelude::{Add, App, On, Remove, Res, Resource};
use serde::{Serialize, Serializer, ser::SerializeMap};

/// An index enum and its wire labels, declared as one list: a variant's discriminant indexes
/// its own label, so a label cannot drift from its variant.
macro_rules! labelled {
    ($(#[$doc:meta])* $name:ident, $labels:ident, $count:ident { $($variant:ident = $label:literal,)+ }) => {
        $(#[$doc])*
        #[derive(Clone, Copy)]
        pub(crate) enum $name {
            $($variant,)+
        }
        pub(crate) const $labels: [&str; [$($label),+].len()] = [$($label),+];
        pub(crate) const $count: usize = $labels.len();
    };
}

labelled!(Stage, NAMES, COUNT {
    Intake = "intake_us",
    Structure = "structure_us",
    Refit = "refit_us",
    Publish = "publish_us",
    Contacts = "contacts_us",
    Sweep = "sweep_us",
    SweepCensus = "sweep_census_us",
    ContactCensus = "contact_census_us",
    Crumble = "crumble_us",
    ChildColliderWriteback = "child_collider_writeback_us",
    BeltContacts = "belt_contacts_us",
    // The game's damage model, per field throw: reading its reach, then walking its directions.
    FieldReach = "field_reach_us",
    FieldWalk = "field_walk_us",
    // Per planned crossing: the struck body's completion read, and the crew-box line pass.
    Completion = "completion_us",
    LinePass = "line_pass_us",
    // A burst's blast rays cast through avian, apart from the intake around them.
    Blast = "blast_us",
});

labelled!(Detail, DETAIL_NAMES, DETAIL_COUNT {
    CrumbleSolve = "solve_us",
    CrumbleSplit = "split_us",
    BeltPartition = "belt_partition_us",
});

labelled!(
    /// Counts accompany wall brackets; they do not add time or change law decisions.
    Work, WORK_NAMES, WORK_COUNT {
        ProcessCpuReads = "process_cpu_reads",
        AssessedOwners = "assessed_owners",
        StructureSolveColumns = "structure_solve_columns",
        PreparedBodies = "prepared_bodies",
        PreparedColliders = "prepared_colliders",
        Landings = "landings",
        LandingPasses = "landing_passes",
        LandingOps = "landing_ops",
        FactorAttempts = "factor_attempts",
        FactorNewCalls = "factor_new_calls",
        FactorFailedOps = "factor_failed_ops",
        FactorFailures = "factor_failures",
        FactorOrderingBuilds = "factor_ordering_builds",
        KernelOfCalls = "kernel_of_calls",
        KernelOfChunks = "kernel_of_chunks",
        RehomedColliders = "rehomed_colliders",
        ContactPoints = "contact_points",
        RestingStaticPoints = "resting_static_points",
        RestingMovingPoints = "resting_moving_points",
        ContactRecords = "contact_records",
        ContactReadRoots = "contact_read_roots",
        CollidersAdded = "colliders_added",
        CollidersRemoved = "colliders_removed",
        // Belt footprint work, independent of the construction contact reader.
        BeltNearColliders = "belt_near_colliders",
        BeltCandidates = "belt_candidates",
        BeltSlabChecks = "belt_slab_checks",
        BeltSlabAabbPasses = "belt_slab_aabb_passes",
        BeltOverlaps = "belt_overlaps",
        BeltDynamicContacts = "belt_dynamic_contacts",
        BeltDispatches = "belt_dispatches",
        BeltBoundaryBuilds = "belt_boundary_builds",
        BeltPartitionFaces = "belt_partition_faces",
        BeltPartitionCells = "belt_partition_cells",
        BeltPartitionPatches = "belt_partition_patches",
        BeltDiscoveryTests = "belt_discovery_tests",
        // Envelope partition internals: boundary edges, sweep intervals, edge pairs tested for a
        // crossing, edges visited to build intervals, owner region tests, and buffers allocated
        // per admitted face and per band.
        BeltPartitionEdges = "belt_partition_edges",
        BeltSweepIntervals = "belt_sweep_intervals",
        BeltSweepPairs = "belt_sweep_pairs",
        BeltSweepEdgeVisits = "belt_sweep_edge_visits",
        BeltOwnerTests = "belt_owner_tests",
        BeltPartitionAllocs = "belt_partition_allocs",
        BeltTerrainSamples = "belt_terrain_samples",
        // The game's damage model: one planned crossing, one field throw, and per throw the
        // armour its reach holds; per direction the candidates whose geometry was read
        // and their face crossings.
        MarchCrossings = "march_crossings",
        FieldThrows = "field_throws",
        FieldReached = "field_reached",
        FieldNear = "field_near",
        FieldHits = "field_hits",
        FieldDirections = "field_directions",
        // Triangles handed to the crossing test: by a field direction's hard walk, and by every
        // other ballistic corridor (contact sweeps, the crossing chain, witness walks).
        FieldTriTests = "field_tri_tests",
        MarchTriTests = "march_tri_tests",
        // A burst's blast: rays cast through avian, and (owner, ray) pairs walked over an
        // owner's chunks, of which a moving group's.
        BlastCasts = "blast_casts",
        BlastOwnerRays = "blast_owner_rays",
        BlastPieceRays = "blast_piece_rays",
        // The game's damage model's pose history: per-collider entries written, of colliders on
        // the armour layer (a construction's; a tank's armour is no collider) and of every other.
        PoseInsertsArmour = "pose_inserts_armour",
        PoseInsertsOther = "pose_inserts_other",
        // Armour-set frame poses written, one per frame whose node's local pose was written.
        FramePoseWrites = "frame_pose_writes",
        // A round's first-contact casts, and per cast the armour-set boxes it tested by level
        // (tank, frame, part) and the tank armour parts whose shape it swept.
        MarchCasts = "march_casts",
        MarchTankBoxes = "march_tank_boxes",
        MarchFrameBoxes = "march_frame_boxes",
        MarchPartBoxes = "march_part_boxes",
        MarchArmourShapes = "march_armour_shapes",
    }
);

#[derive(Resource, Clone)]
pub(crate) struct Stages(
    Arc<[AtomicU64; COUNT]>,
    Arc<[AtomicU64; WORK_COUNT]>,
    Arc<Mutex<BeltReaders>>,
    Arc<[AtomicU64; DETAIL_COUNT]>,
    Arc<[Mutex<DetailReaders>; DETAIL_COUNT]>,
    Arc<[AtomicU64; COUNT]>,
);

// WORK_COUNT exceeds the standard library's array Default implementations (lengths 0..=32).
impl Default for Stages {
    fn default() -> Self {
        Self(
            Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            Arc::default(),
            Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            Arc::new(std::array::from_fn(|_| Mutex::default())),
            Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
        )
    }
}

/// An existing ram or scheduled capture keeps this tick. Sim instruments need no net dependency.
#[derive(Resource, Default)]
pub(crate) struct Capture {
    pub(crate) active: bool,
}

/// Belt reads from different owners can overlap; both clocks need one union interval.
#[derive(Default)]
struct BeltReaders {
    readers: usize,
    since: Option<Edge>,
}
#[derive(Default)]
struct DetailReaders {
    readers: usize,
    since: Option<std::time::Instant>,
}
pub(crate) type NamedCounts = NamedValues<WORK_COUNT, u64>;
pub(crate) type NamedTimes<const N: usize> = NamedValues<N, f64>;

/// Actual collider lifecycle, including standing refits; rehoming a collider adds no event.
pub(crate) fn install_lifecycle_counts(app: &mut App) {
    app.add_observer(
        |_: On<Add, avian3d::prelude::Collider>, stages: Res<Stages>| {
            stages.add(Work::CollidersAdded, 1);
        },
    );
    app.add_observer(
        |_: On<Remove, avian3d::prelude::Collider>, stages: Res<Stages>| {
            stages.add(Work::CollidersRemoved, 1);
        },
    );
}

impl Stages {
    /// Nested wall unions across owner tasks. Solve and split can overlap on different owners;
    /// they are detail inside Crumble, as the partition is inside BeltContacts, never additive
    /// leaves debited by the phase clock.
    pub(crate) fn detail(stages: Option<&Self>, detail: Detail) -> Option<DetailBracket> {
        stages.map(|stages| {
            let readers = stages.4.clone();
            let mut active = readers[detail as usize]
                .lock()
                .expect("detail timing lock is not poisoned");
            if active.readers == 0 {
                active.since = Some(std::time::Instant::now());
            }
            active.readers += 1;
            drop(active);
            DetailBracket {
                totals: stages.3.clone(),
                readers,
                detail,
            }
        })
    }

    pub(crate) fn take_details(&self) -> NamedTimes<DETAIL_COUNT> {
        NamedTimes::new(
            &DETAIL_NAMES,
            std::array::from_fn(|i| self.3[i].swap(0, Ordering::Relaxed) as f64 / 1000.0),
        )
    }

    pub(crate) fn add(&self, work: Work, count: u64) {
        self.1[work as usize].fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn take_counts(&self) -> NamedCounts {
        NamedCounts::new(
            &WORK_NAMES,
            std::array::from_fn(|i| self.1[i].swap(0, Ordering::Relaxed)),
        )
    }

    pub(super) fn totals_ns(&self) -> [u64; 2] {
        [&self.0, &self.5].map(|totals| totals.iter().map(|ns| ns.load(Ordering::Relaxed)).sum())
    }

    pub(crate) fn take_cpu_us(&self) -> NamedTimes<COUNT> {
        NamedTimes::new(
            &NAMES,
            std::array::from_fn(|i| self.5[i].swap(0, Ordering::Relaxed) as f64 / 1000.0),
        )
    }
    /// An absent resource means instrumentation is off, without reading the clock.
    pub(crate) fn start(stages: Option<&Self>, stage: Stage) -> Option<Bracket> {
        stages.map(|stages| {
            // Only the union edges need atomic visibility: a new opening must follow the
            // previous union's close even if its worker was waiting for this lock.
            let mut belt_active = matches!(stage, Stage::BeltContacts)
                .then(|| stages.2.lock().expect("belt timing lock is not poisoned"));
            let start = Edge::now();
            stages.add(Work::ProcessCpuReads, 1);
            let debit = matches!(stage, Stage::Sweep).then(|| {
                (
                    Stage::SweepCensus,
                    [
                        stages.0[Stage::SweepCensus as usize].load(Ordering::Relaxed),
                        stages.5[Stage::SweepCensus as usize].load(Ordering::Relaxed),
                    ],
                )
            });
            let belt = belt_active.as_mut().map(|active| {
                if active.readers == 0 {
                    active.since = Some(start);
                }
                active.readers += 1;
                stages.2.clone()
            });
            drop(belt_active);
            Bracket {
                totals: stages.0.clone(),
                cpu_totals: stages.5.clone(),
                counts: stages.1.clone(),
                stage,
                start,
                belt,
                debit,
            }
        })
    }

    pub(crate) fn take_us(&self) -> [f64; COUNT] {
        std::array::from_fn(|i| self.0[i].swap(0, Ordering::Relaxed) as f64 / 1000.0)
    }
}

/// Serialize stage names from their owner, so consumers can include newly added stages.
#[derive(Clone, Copy)]
pub(crate) struct NamedValues<const N: usize, T> {
    names: &'static [&'static str; N],
    pub(crate) values: [T; N],
}

impl<const N: usize, T> NamedValues<N, T> {
    pub(crate) fn new(names: &'static [&'static str; N], values: [T; N]) -> Self {
        Self { names, values }
    }
}

impl<const N: usize, T: Serialize> Serialize for NamedValues<N, T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(N))?;
        for (name, us) in self.names.iter().zip(&self.values) {
            map.serialize_entry(name, us)?;
        }
        map.end()
    }
}

pub(crate) struct DetailBracket {
    totals: Arc<[AtomicU64; DETAIL_COUNT]>,
    readers: Arc<[Mutex<DetailReaders>; DETAIL_COUNT]>,
    detail: Detail,
}

impl Drop for DetailBracket {
    fn drop(&mut self) {
        let index = self.detail as usize;
        let mut active = self.readers[index]
            .lock()
            .expect("detail timing lock is not poisoned");
        active.readers -= 1;
        if active.readers == 0 {
            let ns = active
                .since
                .take()
                .expect("the first detail reader opens its interval")
                .elapsed()
                .as_nanos();
            self.totals[index].fetch_add(
                u64::try_from(ns).expect("a detail interval fits in u64 nanoseconds"),
                Ordering::Relaxed,
            );
        }
    }
}

pub(crate) struct Bracket {
    totals: Arc<[AtomicU64; COUNT]>,
    stage: Stage,
    start: Edge,
    belt: Option<Arc<Mutex<BeltReaders>>>,
    debit: Option<(Stage, [u64; 2])>,
    cpu_totals: Arc<[AtomicU64; COUNT]>,
    counts: Arc<[AtomicU64; WORK_COUNT]>,
}

impl Drop for Bracket {
    fn drop(&mut self) {
        let mut belt_active = self
            .belt
            .as_ref()
            .map(|belt| belt.lock().expect("belt timing lock is not poisoned"));
        let now = Edge::now();
        self.counts[Work::ProcessCpuReads as usize].fetch_add(1, Ordering::Relaxed);
        let start = if let Some(active) = belt_active.as_mut() {
            active.readers -= 1;
            if active.readers != 0 {
                return;
            }
            active
                .since
                .take()
                .expect("the first belt reader opens its interval")
        } else {
            self.start
        };
        self.record_ns(now.since(start));
    }
}

impl Bracket {
    fn record_ns(&self, ns: [u64; 2]) {
        for (i, totals) in [&self.totals, &self.cpu_totals].into_iter().enumerate() {
            let nested = self.debit.map_or(0, |(stage, before)| {
                totals[stage as usize].load(Ordering::Relaxed) - before[i]
            });
            totals[self.stage as usize].fetch_add(
                ns[i]
                    .checked_sub(nested)
                    .expect("sweep census is contained by sweep"),
                Ordering::Relaxed,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// The census reader must be subtractable exactly once even though collection runs inside
    /// the gameplay sweep. A parent bracket that retains the nested leaf double-counts it.
    #[test]
    fn sweep_excludes_its_census_leaf() {
        let stages = Stages::default();
        // Fixture: an earlier census consumed 11 ns; this sweep's census consumes 30 of 100 ns.
        stages.0[Stage::SweepCensus as usize].store(11, Ordering::Relaxed);
        stages.5[Stage::SweepCensus as usize].store(13, Ordering::Relaxed);
        let parent = Stages::start(Some(&stages), Stage::Sweep).unwrap();
        stages.5[Stage::SweepCensus as usize].fetch_add(60, Ordering::Relaxed);
        stages.0[Stage::SweepCensus as usize].fetch_add(30, Ordering::Relaxed);
        parent.record_ns([100, 200]);
        std::mem::forget(parent); // The fixture supplied the elapsed time; do not sample it again.
        let values = stages.take_us();
        assert_eq!(values[Stage::Sweep as usize], 0.070);
        assert_eq!(values[Stage::SweepCensus as usize], 0.041);
        let cpu = stages.take_cpu_us().values;
        assert_eq!(cpu[Stage::Sweep as usize], 0.140);
        assert_eq!(cpu[Stage::SweepCensus as usize], 0.073);
        let wall = serde_json::to_value(NamedTimes::new(&NAMES, values)).unwrap();
        let cpu = serde_json::to_value(NamedTimes::new(&NAMES, cpu)).unwrap();
        assert_eq!(
            wall.as_object().unwrap().keys().collect::<Vec<_>>(),
            cpu.as_object().unwrap().keys().collect::<Vec<_>>()
        );
    }

    /// Parallel tank reads share one wall interval: publishing the first reader's span would
    /// let the phase consumer subtract overlapping work twice.
    #[test]
    fn overlapping_belt_readers_publish_only_after_the_last_reader() {
        let stages = Stages::default();
        let first = Stages::start(Some(&stages), Stage::BeltContacts);
        let second = Stages::start(Some(&stages), Stage::BeltContacts);
        drop(first);
        assert_eq!(
            stages.totals_ns(),
            [0, 0],
            "the interval still has a live reader"
        );
        drop(second);
        let completed = stages.take_us();
        assert!(completed[Stage::BeltContacts as usize] > 0.0);
        assert_eq!(stages.take_us(), [0.0; COUNT]);
        let cpu = stages.take_cpu_us();
        assert!(
            cpu.values
                .iter()
                .enumerate()
                .all(|(i, &us)| i == Stage::BeltContacts as usize || us == 0.0)
        );
        assert_eq!(stages.take_cpu_us().values, [0.0; COUNT]);
    }

    /// Nested crumble details can overlap between owners, but must never debit additive phases.
    /// The last reader publishes the union once; a drain starts the next tick empty.
    #[test]
    fn crumble_details_do_not_add_another_leaf() {
        let stages = Stages::default();
        let first = Stages::detail(Some(&stages), Detail::CrumbleSolve);
        let second = Stages::detail(Some(&stages), Detail::CrumbleSolve);
        let split = Stages::detail(Some(&stages), Detail::CrumbleSplit);
        drop(first);
        assert_eq!(stages.take_details().values, [0.0; DETAIL_COUNT]);
        drop(second);
        drop(split);
        let values = stages.take_details().values;
        assert!(values[Detail::CrumbleSolve as usize] > 0.0);
        assert!(values[Detail::CrumbleSplit as usize] > 0.0);
        assert_eq!(stages.totals_ns()[0], 0);
        assert_eq!(stages.take_details().values, [0.0; DETAIL_COUNT]);
        assert!(Stages::detail(None, Detail::CrumbleSolve).is_none());
    }

    /// Wave reports must distinguish allocation from collider reuse and count final removal.
    #[test]
    fn collider_lifecycle_counts_allocation_but_not_rehoming() {
        use avian3d::prelude::Collider;
        use bevy::prelude::ChildOf;

        let mut app = App::new();
        app.init_resource::<Stages>();
        install_lifecycle_counts(&mut app);
        let parent = app.world_mut().spawn_empty().id();
        let chunk = app.world_mut().spawn(Collider::default()).id();
        app.world_mut().entity_mut(chunk).insert(ChildOf(parent));
        let counts = serde_json::to_value(app.world().resource::<Stages>().take_counts()).unwrap();
        assert_eq!(counts["colliders_added"], 1);
        assert_eq!(counts["colliders_removed"], 0);
        app.world_mut().despawn(parent);
        let counts = serde_json::to_value(app.world().resource::<Stages>().take_counts()).unwrap();
        assert_eq!(counts["colliders_added"], 0);
        assert_eq!(counts["colliders_removed"], 1);
    }

    /// Match traces must drain every worker's work once without debiting counts from wall time.
    #[test]
    fn work_counts_are_named_and_drain_independently_of_time() {
        let stages = Stages::default();
        // Fixture: unequal worker contributions expose lost accumulation and wrong field names.
        std::thread::scope(|scope| {
            for count in [2, 3] {
                let stages = &stages;
                scope.spawn(move || stages.add(Work::Landings, count));
            }
        });
        assert_eq!(stages.totals_ns()[0], 0);
        let value = serde_json::to_value(stages.take_counts()).unwrap();
        assert_eq!(value["landings"], 5);
        assert_eq!(value["landing_passes"], 0);
        assert_eq!(stages.take_counts().values, [0; WORK_COUNT]);
    }

    /// The stats consumer must empty each stage independently at every fixed tick; a stale
    /// accumulator would count previous ticks again. Timings are observations, never thresholds.
    #[test]
    fn brackets_commit_to_their_stage_and_drain_once() {
        let stages = Stages::default();
        stages.0[Stage::Intake as usize].store(2000, Ordering::Relaxed); // Fixture: ns.
        stages.0[Stage::Refit as usize].store(5000, Ordering::Relaxed);
        let mut expected = [0.0; COUNT];
        expected[Stage::Intake as usize] = 2.0;
        expected[Stage::Refit as usize] = 5.0;
        assert_eq!(stages.take_us(), expected);
        assert_eq!(stages.take_us(), [0.0; COUNT]);
        stages.5[Stage::Intake as usize].store(7000, Ordering::Relaxed); // Fixture: CPU ns.
        let cpu = stages.take_cpu_us();
        assert_eq!(cpu.values[Stage::Intake as usize], 7.0);
        assert_eq!(stages.take_cpu_us().values, [0.0; COUNT]);
        assert!(Stages::start(None, Stage::Intake).is_none());
        let started = Instant::now();
        // Resolution: enough iterations to amortize the observation clock's own edges.
        let samples = 100_000;
        for _ in 0..samples {
            drop(Stages::start(Some(&stages), Stage::Structure));
        }
        let elapsed = started.elapsed();
        let measured = stages.take_us();
        assert_eq!((measured[0], measured[2]), (0.0, 0.0));
        assert!(measured[1] > 0.0);
        eprintln!(
            "MEASURED stage bracket overhead: {:.1} ns/call ({samples} calls; includes loop)",
            elapsed.as_nanos() as f64 / f64::from(samples)
        );
    }
}
