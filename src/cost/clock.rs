//! Paired wall/process CPU edges, only read by armed server phase/leaf instruments.

use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) struct Edge {
    pub(super) wall: Instant,
    pub(super) cpu: Duration,
}

impl Edge {
    pub(super) fn now() -> Self {
        Self::at(Instant::now())
    }

    pub(super) fn at(wall: Instant) -> Self {
        Self {
            wall,
            cpu: process_cpu().unwrap_or_else(crate::refuse_start),
        }
    }

    pub(super) fn since(self, start: Self) -> [u64; 2] {
        [self.wall.duration_since(start.wall), self.cpu - start.cpu].map(|duration| {
            u64::try_from(duration.as_nanos())
                .expect("an instrument interval fits in u64 nanoseconds")
        })
    }
}

/// libc 0.2.189: Linux CLOCK_PROCESS_CPUTIME_ID sums all process threads.
#[cfg(target_os = "linux")]
pub(crate) fn process_cpu() -> std::io::Result<Duration> {
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: a valid timespec output; the constant selects this process, not this thread.
    if unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, time.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: success initialized the output.
    let time = unsafe { time.assume_init() };
    Ok(Duration::new(time.tv_sec as u64, time.tv_nsec as u32))
}

/// Apple's Libc/gen/clock_gettime.c implements the process clock through getrusage;
/// call it directly, avoiding its timeval-to-timespec conversion. Resolution is microseconds.
#[cfg(target_os = "macos")]
pub(crate) fn process_cpu() -> std::io::Result<Duration> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: the pointer is valid for one rusage output.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: success initialized the output.
    let usage = unsafe { usage.assume_init() };
    let duration = |time: libc::timeval| {
        Duration::from_secs(time.tv_sec as u64) + Duration::from_micros(time.tv_usec as u64)
    };
    Ok(duration(usage.ru_utime) + duration(usage.ru_stime))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn process_cpu() -> std::io::Result<Duration> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "server process CPU clock requires Linux or macOS",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Instrument observation only: prints read cost, never pins host-dependent latency.
    #[test]
    fn observe_process_cpu_read_cost() {
        // Resolution: amortize timer and loop overhead across many reads.
        let samples = 100_000;
        let start = Instant::now();
        for _ in 0..samples {
            std::hint::black_box(process_cpu().unwrap());
        }
        eprintln!(
            "MEASURED process CPU clock mean: {:.3} us/read ({samples} samples, edit profile)",
            start.elapsed().as_secs_f64() * 1e6 / f64::from(samples)
        );
    }
}
