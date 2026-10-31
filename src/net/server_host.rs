//! What the dedicated server's host gives it: the CPU it may use and the threads it starts, and the
//! levers that override them.
//!
//! - [`host_task_pools`] sizes Bevy's task pools for this host by the rule below;
//!   `server_plugin_floor` mounts it, so the server and the bitprobe fixture run on the same pools.
//! - [`log_startup`] prints one `server: host` line at startup: `available_parallelism`, each Bevy task
//!   pool's thread count, and (Linux) the container's cgroup CPU quota and cpuset. An unreadable
//!   value logs as `unknown`.
//! - [`read_throttle`] reads the cgroup's cumulative throttle counters for `server_stats`.
//! - [`StallLever`] reads `OVERMATCH_SERVER_STALL`: a dev instrument that stands in for a host too
//!   slow for the tick rate, so a client's ride through a slow authority reproduces on any machine.
//!
//! The cgroup parsers take the file's text, so they are tested from fixtures. Off Linux the files
//! do not exist and every reading is `None`.
//!
//! # Task-pool sizing
//!
//! [`task_pool_options`]: the compute pool gets every core the process may use, async compute and
//! IO one thread each. One is the least either can have: a pool with no thread never runs a spawned
//! task, and Avian's step blocks on the tree-optimization tasks it spawns on the async pool. The
//! main thread drives the schedule without running the compute pool's tasks (bevy_ecs 0.19's
//! multi-threaded executor scopes with `tick_task_pool_executor: false`), so compute = cores leaves
//! no core idle in a parallel phase and starts no compute thread a core cannot run.
//!
//! The core count is `std::thread::available_parallelism`. On Linux that is the smaller of the
//! affinity mask (the cpuset) and the cgroup v1/v2 CPU quota rounded down (Rust 1.98 std,
//! `sys/thread/unix.rs`), so a fractional quota gets no thread it cannot run. On Edgegap a 2.00-cpu
//! quota over cpuset 0-15 reads 2 and a 1.00-cpu quota reads 1 (MEASURED, the startup lines of
//! `.agents/scratch/perf-2026-10-31/`).
//!
//! Evidence, 30 tanks on Edgegap (that folder's README, TLDR 3; MEASURED): Bevy's stock split gives
//! 2 vCPU pools of 1/1/1 (compute/async/io), and the server then held ~102 % of one core and ran no
//! faster than on 1 vCPU (tick p50 17.5 against 18.5 ms). Pools 2/1/1 cut the tick p50 to 13.4 ms
//! on alpha.4 and 10.9 ms on alpha.5; 2/2/2 was worse (14.6–15.2 ms). On 1 vCPU, a compute pool of
//! 2 ran into the quota: tick p95 88 ms, throttled 441–472 ms of every second.

use core::num::NonZero;
use core::time::Duration;

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool};

/// The task-pool sizing (module doc) for a host of `cores` usable cores.
///
/// Each pool's policy pins `min_threads == max_threads`: Bevy clamps its proportional count into
/// that range (`TaskPoolThreadAssignmentPolicy::get_number_of_threads`, bevy_app 0.19), so the pin
/// is the count whatever Bevy reads the host as.
fn task_pool_options(cores: NonZero<usize>) -> TaskPoolOptions {
    let mut options = TaskPoolOptions::default();
    for (policy, threads) in [
        (&mut options.compute, cores.get()),
        (&mut options.async_compute, 1),
        (&mut options.io, 1),
    ] {
        policy.min_threads = threads;
        policy.max_threads = threads;
    }
    options
}

/// The task pools [`task_pool_options`] gives this host. An unreadable core count is 1, as it is
/// in Bevy's own sizing.
pub(crate) fn host_task_pools() -> TaskPoolPlugin {
    let cores = std::thread::available_parallelism().unwrap_or(NonZero::<usize>::MIN);
    TaskPoolPlugin {
        task_pool_options: task_pool_options(cores),
    }
}

/// The stall lever.
pub(super) const STALL_LEVER: &str = "OVERMATCH_SERVER_STALL";

/// `OVERMATCH_SERVER_STALL=<ms>[@<from_s>-<to_s>]`: every fixed tick sleeps `ms` of wall time, for
/// the whole run or only while the server's real elapsed time lies in `[from_s, to_s)`.
///
/// A tick that costs more than its 15.625 ms budget runs the server below 64 Hz the way a slow host
/// does: Bevy's fixed loop catches up several ticks per frame until `Time<Virtual>`'s max delta
/// clamps it, and lightyear sends one replication message per frame that ran the fixed loop.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub(super) struct StallLever {
    per_tick: Duration,
    /// Real seconds since startup, `[from, to)`; `None` is the whole run.
    window: Option<(f64, f64)>,
}

impl StallLever {
    /// Unset, empty, `off` or `0` is `Ok(None)`. Anything else must parse, or the server refuses to
    /// start: a typo must not run a stall condition with no stall in it.
    pub(super) fn parse(value: Option<&str>) -> Result<Option<Self>, String> {
        let Some(value) = value.map(str::trim) else {
            return Ok(None);
        };
        if value.is_empty() || value.eq_ignore_ascii_case("off") || value == "0" {
            return Ok(None);
        }
        let bad = || format!("{STALL_LEVER}='{value}': want <ms>[@<from_s>-<to_s>] or `off`");
        let (ms, window) = match value.split_once('@') {
            None => (value, None),
            Some((ms, window)) => {
                let (from, to) = window.split_once('-').ok_or_else(bad)?;
                let from = from.trim().parse::<f64>().map_err(|_| bad())?;
                let to = to.trim().parse::<f64>().map_err(|_| bad())?;
                if !(from.is_finite() && to.is_finite() && 0.0 <= from && from < to) {
                    return Err(bad());
                }
                (ms, Some((from, to)))
            }
        };
        let ms = ms.trim().parse::<u64>().map_err(|_| bad())?;
        if ms == 0 {
            return Err(bad());
        }
        Ok(Some(Self {
            per_tick: Duration::from_millis(ms),
            window,
        }))
    }

    /// The lever from the environment.
    pub(super) fn from_env() -> Result<Option<Self>, String> {
        Self::parse(std::env::var(STALL_LEVER).ok().as_deref())
    }

    fn active_at(&self, elapsed_s: f64) -> bool {
        self.window
            .is_none_or(|(from, to)| from <= elapsed_s && elapsed_s < to)
    }

    fn describe(&self) -> String {
        let window = self.window.map_or_else(
            || "the whole run".to_owned(),
            |(from, to)| format!("from {from} s to {to} s of real time"),
        );
        format!("{} ms per tick, {window}", self.per_tick.as_millis())
    }
}

/// Mount the stall lever: one sleep at the head of every fixed tick it covers.
pub(super) fn install_stall(app: &mut App, lever: StallLever) {
    info!("server: stall lever — {} ({STALL_LEVER})", lever.describe());
    app.insert_resource(lever)
        .add_systems(FixedFirst, stall_tick);
}

fn stall_tick(lever: Res<StallLever>, time: Res<Time<Real>>) {
    if lever.active_at(time.elapsed_secs_f64()) {
        std::thread::sleep(lever.per_tick);
    }
}

/// A cgroup CPU quota: `quota_us` per `period_us`, `None` quota meaning unlimited.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CpuMax {
    pub quota_us: Option<u64>,
    pub period_us: u64,
}

impl CpuMax {
    /// cgroup v2 `cpu.max`: `max 100000` or `100000 100000`.
    pub(super) fn parse_v2(text: &str) -> Option<Self> {
        let mut fields = text.split_whitespace();
        let quota = fields.next()?;
        let period_us = fields.next()?.parse().ok()?;
        let quota_us = if quota == "max" {
            None
        } else {
            Some(quota.parse().ok()?)
        };
        Some(Self {
            quota_us,
            period_us,
        })
    }

    /// cgroup v1 `cpu.cfs_quota_us` (`-1` is unlimited) and `cpu.cfs_period_us`.
    pub(super) fn parse_v1(quota: &str, period: &str) -> Option<Self> {
        let quota: i64 = quota.trim().parse().ok()?;
        let period_us = period.trim().parse().ok()?;
        Some(Self {
            quota_us: u64::try_from(quota).ok(),
            period_us,
        })
    }

    fn describe(&self) -> String {
        match self.quota_us {
            None => "unlimited".to_owned(),
            Some(quota) => format!(
                "{:.2} cpus ({quota}/{} us)",
                quota as f64 / self.period_us.max(1) as f64,
                self.period_us
            ),
        }
    }
}

/// The cgroup's cumulative throttle counters.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(super) struct Throttle {
    /// Enforcement periods in which the group was throttled.
    pub nr_throttled: u64,
    /// Total time the group was throttled, microseconds.
    pub throttled_usec: u64,
}

impl Throttle {
    /// `cpu.stat`: v2 carries `throttled_usec`, v1 carries `throttled_time` in nanoseconds.
    pub(super) fn parse(text: &str) -> Option<Self> {
        let field = |name: &str| {
            text.lines().find_map(|line| {
                let mut parts = line.split_whitespace();
                if parts.next()? != name {
                    return None;
                }
                parts.next()?.parse::<u64>().ok()
            })
        };
        let throttled_usec =
            field("throttled_usec").or_else(|| field("throttled_time").map(|ns| ns / 1000))?;
        Some(Self {
            nr_throttled: field("nr_throttled")?,
            throttled_usec,
        })
    }

    /// This reading minus an earlier one; a counter that ran backwards saturates to 0.
    pub(super) fn since(self, earlier: Self) -> Self {
        Self {
            nr_throttled: self.nr_throttled.saturating_sub(earlier.nr_throttled),
            throttled_usec: self.throttled_usec.saturating_sub(earlier.throttled_usec),
        }
    }
}

/// A cgroup file; the paths exist on Linux only, so off Linux every read is `None`.
fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The container's CPU quota, v2 first and v1 as the fallback.
fn cpu_max() -> Option<CpuMax> {
    read("/sys/fs/cgroup/cpu.max")
        .and_then(|text| CpuMax::parse_v2(&text))
        .or_else(|| {
            CpuMax::parse_v1(
                &read("/sys/fs/cgroup/cpu/cpu.cfs_quota_us")?,
                &read("/sys/fs/cgroup/cpu/cpu.cfs_period_us")?,
            )
        })
}

fn cpuset() -> Option<String> {
    read("/sys/fs/cgroup/cpuset.cpus.effective")
        .or_else(|| read("/sys/fs/cgroup/cpuset/cpuset.effective_cpus"))
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// The cgroup's cumulative throttle counters now.
pub(super) fn read_throttle() -> Option<Throttle> {
    read("/sys/fs/cgroup/cpu.stat")
        .and_then(|text| Throttle::parse(&text))
        .or_else(|| Throttle::parse(&read("/sys/fs/cgroup/cpu/cpu.stat")?))
}

/// The one startup line. Call after the plugins are built, so the task pools exist.
pub(super) fn log_startup() {
    let pool =
        |threads: Option<usize>| threads.map_or_else(|| "unknown".to_owned(), |n| n.to_string());
    let parallelism = std::thread::available_parallelism()
        .map_or_else(|err| format!("unknown ({err})"), |n| n.to_string());
    info!(
        "server: host available_parallelism={parallelism} \
         pools(compute/async_compute/io)={}/{}/{} cpu_quota={} cpuset={}",
        pool(ComputeTaskPool::try_get().map(|p| p.thread_num())),
        pool(AsyncComputeTaskPool::try_get().map(|p| p.thread_num())),
        pool(IoTaskPool::try_get().map(|p| p.thread_num())),
        cpu_max().map_or_else(|| "unknown".to_owned(), |q| q.describe()),
        cpuset().as_deref().unwrap_or("unknown"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compute pool is exactly the cores the process may use: one fewer leaves a core idle
    /// (pools 1/1/1 held a 2-vCPU host to ~102 % of one core), one more starts a thread no core can
    /// run (2 compute threads on 1 vCPU ran into the quota). Async compute and IO keep one thread
    /// each, the least that runs a spawned task. Every deployed server's pools rest on it.
    #[test]
    fn the_compute_pool_gets_every_core_and_no_more() {
        for cores in [1, 2, 4, 10] {
            let options = task_pool_options(NonZero::new(cores).unwrap());
            for (pool, policy, threads) in [
                ("compute", &options.compute, cores),
                ("async_compute", &options.async_compute, 1),
                ("io", &options.io, 1),
            ] {
                assert_eq!(
                    (policy.min_threads, policy.max_threads),
                    (threads, threads),
                    "{pool} on {cores} cores"
                );
            }
        }
    }

    /// Unset and `off` must leave a production server unstalled; a malformed value must stop the
    /// server rather than run a slow-server condition at full rate; the window bounds are
    /// half-open, so a run can name back-to-back windows.
    #[test]
    fn the_stall_lever_is_off_unless_it_names_milliseconds() {
        for off in [None, Some(""), Some("off"), Some("OFF"), Some("0")] {
            assert_eq!(StallLever::parse(off), Ok(None), "{off:?}");
        }
        let whole = StallLever::parse(Some("30")).unwrap().unwrap();
        assert_eq!(whole.per_tick, Duration::from_millis(30));
        assert!(whole.active_at(0.0) && whole.active_at(1.0e6));
        let window = StallLever::parse(Some(" 30@40-100 ")).unwrap().unwrap();
        assert_eq!(window.window, Some((40.0, 100.0)));
        assert!(!window.active_at(39.9));
        assert!(window.active_at(40.0));
        assert!(window.active_at(99.9));
        assert!(!window.active_at(100.0));
        for bad in [
            "-1",
            "1.5",
            "x",
            "30@",
            "30@40",
            "30@100-40",
            "30@40-40",
            "30@-1-5",
            "0@1-2",
            "30@a-b",
        ] {
            assert!(StallLever::parse(Some(bad)).is_err(), "{bad}");
        }
    }

    /// The quota decides whether the container is capped; a parser that reads `max` as a number or
    /// `-1` as a quota reports an uncapped host as capped, or the reverse.
    #[test]
    fn cpu_max_parses_v2_and_v1() {
        let unlimited = CpuMax {
            quota_us: None,
            period_us: 100_000,
        };
        let one_cpu = CpuMax {
            quota_us: Some(100_000),
            period_us: 100_000,
        };
        assert_eq!(CpuMax::parse_v2("max 100000\n"), Some(unlimited));
        assert_eq!(CpuMax::parse_v2("100000 100000\n"), Some(one_cpu));
        assert_eq!(CpuMax::parse_v2(""), None);
        assert_eq!(CpuMax::parse_v2("garbage"), None);
        assert_eq!(
            CpuMax::parse_v1("-1\n", "100000\n"),
            Some(CpuMax {
                quota_us: None,
                period_us: 100_000
            })
        );
        assert_eq!(
            CpuMax::parse_v1("200000\n", "100000\n"),
            Some(CpuMax {
                quota_us: Some(200_000),
                period_us: 100_000
            })
        );
        assert_eq!(CpuMax::parse_v1("x", "100000"), None);
        assert_eq!(
            CpuMax::parse_v2("100000 100000").unwrap().describe(),
            "1.00 cpus (100000/100000 us)"
        );
    }

    /// v2 reports microseconds, v1 nanoseconds; mixing them up is a 1000x error in the throttled
    /// time. A file missing a counter reads as unknown, never as zero.
    #[test]
    fn throttle_parses_v2_and_v1_and_deltas_saturate() {
        let v2 = "usage_usec 5\nnr_periods 100\nnr_throttled 7\nthrottled_usec 2500\nnr_bursts 0\n";
        let want = Throttle {
            nr_throttled: 7,
            throttled_usec: 2500,
        };
        assert_eq!(Throttle::parse(v2), Some(want));
        let v1 = "nr_periods 100\nnr_throttled 7\nthrottled_time 2500000\n";
        assert_eq!(Throttle::parse(v1), Some(want));
        assert_eq!(Throttle::parse("usage_usec 5\n"), None);
        assert_eq!(Throttle::parse("nr_throttled 7\n"), None);
        let later = Throttle {
            nr_throttled: 9,
            throttled_usec: 4000,
        };
        assert_eq!(
            later.since(want),
            Throttle {
                nr_throttled: 2,
                throttled_usec: 1500
            }
        );
        assert_eq!(
            want.since(later),
            Throttle {
                nr_throttled: 0,
                throttled_usec: 0
            }
        );
    }
}
