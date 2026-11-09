//! Linux host observations for the game's server cost recorder. Each /proc reading preserves
//! its own typed absence; deltas require two successful samples of the same counter owner.
//! Kernel field order and units: https://www.kernel.org/doc/html/latest/filesystems/proc.html
//! THP mode: https://www.kernel.org/doc/html/latest/admin-guide/mm/transhuge.html

use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug, Serialize)]
#[serde(tag = "absence", rename_all = "snake_case")]
enum Unavailable {
    Io {
        path: String,
        cause: String,
    },
    Missing {
        path: String,
        field: String,
    },
    Invalid {
        path: String,
        field: String,
        value: String,
    },
    Baseline {
        field: String,
    },
    CounterRegressed {
        field: String,
    },
    ClockTicks {
        cause: String,
    },
    ThreadCensus {
        expected_compute: usize,
        found_compute: usize,
        main_found: bool,
    },
}

type Reading<T> = Result<T, Unavailable>;

fn read(path: &str) -> Reading<String> {
    std::fs::read_to_string(path).map_err(|error| Unavailable::Io {
        path: path.to_owned(),
        cause: error.to_string(),
    })
}

fn number(fields: &[&str], index: usize, path: &str, field: &str) -> Reading<u64> {
    let value = fields.get(index).ok_or_else(|| Unavailable::Missing {
        path: path.to_owned(),
        field: field.to_owned(),
    })?;
    value.parse().map_err(|_| Unavailable::Invalid {
        path: path.to_owned(),
        field: field.to_owned(),
        value: (*value).to_owned(),
    })
}

#[derive(Debug, Serialize)]
struct Task {
    name: String,
    minor_faults: u64,
    major_faults: u64,
    cpu_ticks: u64,
    started: u64,
}

fn task(text: &str, path: &str) -> Reading<Task> {
    // comm may contain spaces and parentheses; fields after its final ')' have fixed positions.
    let (prefix, rest) = text.rsplit_once(") ").ok_or_else(|| Unavailable::Missing {
        path: path.to_owned(),
        field: "comm terminator".to_owned(),
    })?;
    let (_, name) = prefix.split_once('(').ok_or_else(|| Unavailable::Missing {
        path: path.to_owned(),
        field: "comm".to_owned(),
    })?;
    let fields: Vec<_> = rest.split_whitespace().collect();
    let user = number(&fields, 11, path, "utime")?;
    let system = number(&fields, 12, path, "stime")?;
    let cpu_ticks = user
        .checked_add(system)
        .ok_or_else(|| Unavailable::Invalid {
            path: path.to_owned(),
            field: "utime + stime".to_owned(),
            value: format!("{user} + {system} overflows u64"),
        })?;
    Ok(Task {
        name: name.to_owned(),
        minor_faults: number(&fields, 7, path, "minflt")?,
        major_faults: number(&fields, 9, path, "majflt")?,
        cpu_ticks,
        started: number(&fields, 19, path, "starttime")?,
    })
}

fn read_task(path: &str) -> Reading<Task> {
    task(&read(path)?, path)
}

fn steal(text: &str) -> Reading<u64> {
    let fields: Vec<_> = text
        .lines()
        .find(|line| line.starts_with("cpu "))
        .ok_or_else(|| Unavailable::Missing {
            path: "/proc/stat".to_owned(),
            field: "cpu".to_owned(),
        })?
        .split_whitespace()
        .collect();
    number(&fields, 8, "/proc/stat", "steal")
}

fn delta(now: u64, before: Option<u64>, field: &str) -> Reading<u64> {
    let before = before.ok_or_else(|| Unavailable::Baseline {
        field: field.to_owned(),
    })?;
    now.checked_sub(before)
        .ok_or_else(|| Unavailable::CounterRegressed {
            field: field.to_owned(),
        })
}

/// Only Linux owns this state; other platforms have neither it nor its JSON fields.
pub(super) struct HostProbe {
    steal: Option<u64>,
    faults: Option<(u64, u64)>,
    threads: BTreeMap<u32, Task>,
    ticks_per_second: Reading<u64>,
    tids: Option<Vec<u32>>,
}

impl HostProbe {
    pub(super) fn new() -> Self {
        // SAFETY: sysconf has no pointer arguments. A non-positive reading is a named absence.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let ticks_per_second = if ticks > 0 {
            Ok(ticks as u64)
        } else {
            Err(Unavailable::ClockTicks {
                cause: std::io::Error::last_os_error().to_string(),
            })
        };
        let mut probe = Self {
            steal: None,
            faults: None,
            threads: BTreeMap::new(),
            ticks_per_second,
            tids: None,
        };
        // Log admission failures too: the first periodic sample must never disguise them as zero.
        println!("server_host_baseline {}", probe.sample(Duration::ZERO));
        probe
    }

    pub(super) fn sample(&mut self, elapsed: Duration) -> Value {
        let started = Instant::now();
        let steal = read("/proc/stat").and_then(|text| steal(&text));
        let steal_delta = steal
            .as_ref()
            .map_err(|error| json!(error))
            .and_then(|&now| {
                delta(now, self.steal, "host steal ticks").map_err(|error| json!(error))
            });
        self.steal = steal.ok(); // Failure clears the baseline; the next reading names its absence.
        let faults = read_task("/proc/self/stat");
        let fault_delta = faults.as_ref().map_err(|error| json!(error)).and_then(|now| {
            Ok(json!({
                "minor": delta(now.minor_faults, self.faults.map(|x| x.0), "process minflt").map_err(|e| json!(e))?,
                "major": delta(now.major_faults, self.faults.map(|x| x.1), "process majflt").map_err(|e| json!(e))?,
            }))
        });
        self.faults = faults
            .ok()
            .map(|task| (task.minor_faults, task.major_faults));
        let threads = self.sample_threads(elapsed);
        json!({ "clock_ticks_per_second": self.ticks_per_second, "steal_ticks": steal_delta, "page_faults": fault_delta, "thread_cpu": threads, "host_probe_us": started.elapsed().as_secs_f64() * 1e6 })
    }

    fn sample_threads(&mut self, elapsed: Duration) -> Reading<Vec<Value>> {
        let previous = std::mem::take(&mut self.threads);
        let path = "/proc/self/task";
        // ComputeTaskPool has a fixed worker cohort. Retry the census until it is complete;
        // later periods open only those task files, and an incomplete census invalidates the cache.
        let ids = if let Some(ids) = self.tids.take() {
            ids
        } else {
            std::fs::read_dir(path)
                .map_err(|error| Unavailable::Io {
                    path: path.to_owned(),
                    cause: error.to_string(),
                })?
                .map(|entry| {
                    let entry = entry.map_err(|error| Unavailable::Io {
                        path: path.to_owned(),
                        cause: error.to_string(),
                    })?;
                    entry
                        .file_name()
                        .to_string_lossy()
                        .parse::<u32>()
                        .map_err(|_| Unavailable::Invalid {
                            path: entry.path().display().to_string(),
                            field: "tid".to_owned(),
                            value: entry.file_name().to_string_lossy().into_owned(),
                        })
                })
                .collect::<Reading<Vec<_>>>()?
        };
        let mut next = BTreeMap::new();
        let mut rows = Vec::new();
        for id in ids {
            let stat_path = format!("{path}/{id}/stat");
            match read_task(&stat_path) {
                Ok(task) if id == std::process::id() || task.name.starts_with("Compute Task") => {
                    // Bevy names compute workers "Compute Task Pool (N)"; Linux comm can truncate
                    // that name. Async and IO pools have distinct prefixes in bevy_app's builder.
                    let before = previous
                        .get(&id)
                        .filter(|before| before.started == task.started);
                    let ticks = delta(
                        task.cpu_ticks,
                        before.map(|task| task.cpu_ticks),
                        "thread CPU ticks",
                    );
                    let cpu = ticks
                        .as_ref()
                        .map_err(|error| json!(error))
                        .and_then(|ticks| {
                            let hz = self
                                .ticks_per_second
                                .as_ref()
                                .map_err(|error| json!(error))?;
                            if elapsed.is_zero() {
                                return Err(json!(Unavailable::Baseline {
                                    field: "thread period".to_owned()
                                }));
                            }
                            Ok(*ticks as f64 / *hz as f64 / elapsed.as_secs_f64() * 100.0)
                        });
                    rows.push(json!({ "tid": id, "name": task.name, "role": if id == std::process::id() { "main" } else { "compute" }, "ticks": ticks, "cpu_pct": cpu }));
                    next.insert(id, task);
                }
                Ok(_) => {} // Async compute, IO and transport threads are outside this census.
                Err(error) => rows.push(json!({ "tid": id, "reading": Err::<(), _>(error) })),
            }
        }
        let main_found = next.contains_key(&std::process::id());
        let found_compute = next.keys().filter(|&&id| id != std::process::id()).count();
        let expected_compute =
            bevy::tasks::ComputeTaskPool::try_get().map(|pool| pool.thread_num());
        if let Some(expected_compute) = expected_compute
            && (expected_compute != found_compute || !main_found)
        {
            rows.push(json!({ "census": Err::<(), _>(Unavailable::ThreadCensus { expected_compute, found_compute, main_found }) }));
        }
        if main_found && expected_compute == Some(found_compute) {
            self.tids = Some(next.keys().copied().collect());
        }
        self.threads = next;
        Ok(rows)
    }
}

/// Raw host facts retain their kernel labels; no topology is inferred from a core count.
pub(super) fn log_startup() {
    let thp = read("/sys/kernel/mm/transparent_hugepage/enabled");
    let topology = read("/proc/cpuinfo")
        .map(|text| {
            text.split("\n\n")
                .filter_map(|block| {
                    let fields: BTreeMap<_, _> = block
                        .lines()
                        .filter_map(|line| line.split_once(':'))
                        .filter(|(key, _)| {
                            matches!(
                                key.trim(),
                                "processor"
                                    | "physical id"
                                    | "core id"
                                    | "siblings"
                                    | "cpu cores"
                                    | "model name"
                            )
                        })
                        .map(|(key, value)| (key.trim().to_owned(), value.trim().to_owned()))
                        .collect();
                    (!fields.is_empty()).then_some(fields)
                })
                .collect::<Vec<_>>()
        })
        .and_then(|rows| {
            if rows.is_empty() {
                Err(Unavailable::Missing {
                    path: "/proc/cpuinfo".to_owned(),
                    field: "CPU topology fields".to_owned(),
                })
            } else {
                Ok(rows)
            }
        });
    println!(
        "server_host_facts {}",
        json!({ "transparent_hugepage_mode": thp, "cpu_topology": topology, "cpu_allowed_list": read("/proc/self/status").and_then(|text| text.lines().find_map(|line| line.strip_prefix("Cpus_allowed_list:")).map(|v| v.trim().to_owned()).ok_or_else(|| Unavailable::Missing { path: "/proc/self/status".to_owned(), field: "Cpus_allowed_list".to_owned() })) })
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Operators need the process's own faults and CPU, not the adjacent child counters; comm's
    /// spaces and parentheses must not shift stat's fields.
    #[test]
    fn stat_fields_survive_parentheses_and_spaces_in_comm() {
        let fields = (3..=22)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        let observed = task(&format!("7 (Compute (pool)) {fields}"), "fixture").unwrap();
        assert_eq!(observed.name, "Compute (pool)");
        assert_eq!(
            (
                observed.minor_faults,
                observed.major_faults,
                observed.cpu_ticks,
                observed.started
            ),
            (10, 12, 29, 22)
        );
        assert!(task("malformed", "fixture").is_err());
    }

    /// Host observations must remain usable when procfs/sysfs is absent (including this parser
    /// fixture on macOS); failed reads retain the named file and never become numeric zero.
    #[test]
    fn absent_host_files_keep_a_typed_reason() {
        let value = json!(read("/overmatch-nonexistent-proc-fixture"));
        assert_eq!(value["Err"]["absence"], "io");
        log_startup();
        let mut probe = HostProbe::new();
        let sample = probe.sample(Duration::from_secs(1));
        assert!(sample.get("page_faults").is_some());
        assert!(sample.get("thread_cpu").is_some());
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(sample["steal_ticks"]["Err"]["absence"], "io");
            assert_eq!(sample["page_faults"]["Err"]["absence"], "io");
            assert_eq!(sample["thread_cpu"]["Err"]["absence"], "io");
        }
        assert!(sample["host_probe_us"].as_f64().unwrap().is_finite());
    }

    /// A reset or missing baseline must remain distinguishable from an idle CPU.
    #[test]
    fn missing_and_regressing_counters_have_named_absences() {
        assert!(matches!(
            delta(3, None, "fixture"),
            Err(Unavailable::Baseline { .. })
        ));
        assert!(matches!(
            delta(3, Some(4), "fixture"),
            Err(Unavailable::CounterRegressed { .. })
        ));
        assert_eq!(delta(3, Some(3), "fixture").unwrap(), 0);
        assert_eq!(steal("cpu  1 2 3 4 5 6 7 8 9 10\n").unwrap(), 8);
        assert!(steal("cpu  1 2\n").is_err());
    }
}
