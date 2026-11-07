//! Wall brackets for the game's environment destruction model (game state only).
//! Only the authority stats consumer installs this resource. No lock is held during work,
//! including owner-parallel evaluation; each bracket commits once on its calling thread.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

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
    Crumble = "crumble_us",
});

labelled!(
    /// Counts accompany wall brackets; they do not add time or change law decisions.
    Work, WORK_NAMES, WORK_COUNT {
        AssessedOwners = "assessed_owners",
        StructureSolveColumns = "structure_solve_columns",
        PreparedBodies = "prepared_bodies",
        PreparedColliders = "prepared_colliders",
        Landings = "landings",
        LandingPasses = "landing_passes",
        LandingOps = "landing_ops",
        ContactPoints = "contact_points",
        CollidersAdded = "colliders_added",
        CollidersRemoved = "colliders_removed",
    }
);

#[derive(Resource, Default)]
pub(crate) struct Stages(Arc<[AtomicU64; COUNT]>, Arc<[AtomicU64; WORK_COUNT]>);
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
    pub(crate) fn add(&self, work: Work, count: u64) {
        self.1[work as usize].fetch_add(count, Ordering::Relaxed);
    }

    pub(crate) fn take_counts(&self) -> NamedCounts {
        NamedCounts::new(
            &WORK_NAMES,
            std::array::from_fn(|i| self.1[i].swap(0, Ordering::Relaxed)),
        )
    }

    pub(super) fn total_ns(&self) -> u64 {
        self.0.iter().map(|ns| ns.load(Ordering::Relaxed)).sum()
    }
    /// An absent resource means instrumentation is off, without reading the clock.
    pub(crate) fn start(stages: Option<&Self>, stage: Stage) -> Option<Bracket> {
        stages.map(|stages| Bracket {
            totals: stages.0.clone(),
            stage,
            start: Instant::now(),
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

pub(crate) struct Bracket {
    totals: Arc<[AtomicU64; COUNT]>,
    stage: Stage,
    start: Instant,
}

impl Drop for Bracket {
    fn drop(&mut self) {
        let ns = u64::try_from(self.start.elapsed().as_nanos())
            .expect("one destruction stage cannot run for the u64 nanosecond span");
        self.totals[self.stage as usize].fetch_add(ns, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(stages.total_ns(), 0);
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
        assert_eq!(stages.take_us(), [2.0, 0.0, 5.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(stages.take_us(), [0.0; COUNT]);
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
