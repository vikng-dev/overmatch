//! Wall brackets for the game's environment destruction model (game state only).
//! Only the authority stats consumer installs this resource. No lock is held during work,
//! including owner-parallel evaluation; each bracket commits once on its calling thread.

use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Instant;

use bevy::prelude::Resource;

#[derive(Clone, Copy)]
pub(crate) enum Stage {
    Intake,
    Structure,
    Refit,
    Publish,
    Contacts,
    Sweep,
    Crumble,
}

pub(crate) const NAMES: [&str; 7] = [
    "intake_us",
    "structure_us",
    "refit_us",
    "publish_us",
    "contacts_us",
    "sweep_us",
    "crumble_us",
];
pub(crate) const COUNT: usize = NAMES.len();

#[derive(Resource, Default)]
pub(crate) struct Stages(Arc<[AtomicU64; COUNT]>);

impl Stages {
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
