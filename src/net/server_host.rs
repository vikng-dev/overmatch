//! What the dedicated server's host gives it: the CPU it may use and the threads it starts, and the
//! lever that sets those threads.
//!
//! - [`log_startup`] prints one `server: host` line at startup: `available_parallelism`, each Bevy
//!   task pool's thread count, and (Linux) the container's cgroup CPU quota and cpuset. An
//!   unreadable value logs as `unknown`.
//! - [`read_throttle`] reads the cgroup's cumulative throttle counters for `server_stats`.
//! - [`ThreadsLever`] reads `OVERMATCH_SERVER_THREADS` for the task-pool size.
//!
//! The cgroup parsers take the file's text, so they are tested from fixtures. Off Linux the files
//! do not exist and every reading is `None`.

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, ComputeTaskPool, IoTaskPool};

/// The lever: the total thread count handed to Bevy's task pool options.
pub(super) const THREADS_LEVER: &str = "OVERMATCH_SERVER_THREADS";

/// The task-pool thread lever.
///
/// Bevy splits a total over three pools (IO, async compute, compute) and gives each at least one
/// thread (`TaskPoolThreadAssignmentPolicy::min_threads`), so a total of 1 still starts three
/// worker threads; the startup line shows the real counts.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct ThreadsLever(pub usize);

impl ThreadsLever {
    /// Unset, empty or `default` is `Ok(None)`: Bevy sizes the pools from `available_parallelism`.
    /// Anything else must be a positive integer, or the server refuses to start.
    pub(super) fn parse(value: Option<&str>) -> Result<Option<Self>, String> {
        let Some(value) = value.map(str::trim) else {
            return Ok(None);
        };
        if value.is_empty() || value.eq_ignore_ascii_case("default") {
            return Ok(None);
        }
        match value.parse::<usize>() {
            Ok(n) if n > 0 => Ok(Some(Self(n))),
            _ => Err(format!(
                "{THREADS_LEVER}='{value}': want a positive integer or `default`"
            )),
        }
    }

    /// The lever from the environment.
    pub(super) fn from_env() -> Result<Option<Self>, String> {
        Self::parse(std::env::var(THREADS_LEVER).ok().as_deref())
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
pub(super) fn log_startup(lever: Option<&ThreadsLever>) {
    let pool =
        |threads: Option<usize>| threads.map_or_else(|| "unknown".to_owned(), |n| n.to_string());
    let parallelism = std::thread::available_parallelism()
        .map_or_else(|err| format!("unknown ({err})"), |n| n.to_string());
    info!(
        "server: host available_parallelism={parallelism} \
         pools(compute/async_compute/io)={}/{}/{} threads_lever={} cpu_quota={} cpuset={}",
        pool(ComputeTaskPool::try_get().map(|p| p.thread_num())),
        pool(AsyncComputeTaskPool::try_get().map(|p| p.thread_num())),
        pool(IoTaskPool::try_get().map(|p| p.thread_num())),
        lever.map_or_else(|| "default".to_owned(), |l| l.0.to_string()),
        cpu_max().map_or_else(|| "unknown".to_owned(), |q| q.describe()),
        cpuset().as_deref().unwrap_or("unknown"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unset and `default` keep Bevy's sizing; a bad value must stop the server rather than
    /// silently run the wrong arm of an A/B.
    #[test]
    fn the_threads_lever_is_a_positive_integer_or_default() {
        assert_eq!(ThreadsLever::parse(None), Ok(None));
        assert_eq!(ThreadsLever::parse(Some("")), Ok(None));
        assert_eq!(ThreadsLever::parse(Some("default")), Ok(None));
        assert_eq!(ThreadsLever::parse(Some(" 1 ")), Ok(Some(ThreadsLever(1))));
        assert_eq!(ThreadsLever::parse(Some("8")), Ok(Some(ThreadsLever(8))));
        for bad in ["0", "-1", "two", "1.5", "1 2"] {
            assert!(ThreadsLever::parse(Some(bad)).is_err(), "{bad}");
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
