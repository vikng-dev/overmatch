//! Hold the end of each main-world frame until the cap deadline, in `Last`.
//! [`super::Settings::frame_limit_period`] owns arming: effective vsync OFF plus a non-off cap;
//! otherwise this costs one resource read.
//!
//! Coarse sleep saves a core; spinning the tail absorbs OS timer overshoot (hundreds of µs on
//! macOS/Linux, up to 15.6 ms default resolution on Windows). The std-only hybrid follows
//! spin_sleep/bevy_framepace.
//!
//! The spin window is MEASURED from sleep overshoot by [`Limiter::observe`]: instant attack, slow
//! release, clamped to [`MIN_SPIN`]..=[`MAX_SPIN`]. DERIVED: a fixed 1.5 ms at 240 FPS spends ~36%
//! of a 4.17 ms frame spinning; ~100 µs spends ~2.4%. Coarse timers reach the old fixed ceiling.
//!
//! `NonSendMarker` keeps the wait on the main thread, off `ComputeTaskPool` workers needed by
//! pipelined render/visibility/extract work, especially on low-core machines. Blocking the main
//! schedule's tail delays the next extract and paces uncapped presentation. Winit's frame events
//! are already consumed; the wait appears upstream as a long frame.
//!
//! Deadlines advance on a fixed grid, not wake + period: adding workload to each cycle would
//! undershoot the cap. A whole-slot overrun resets to now + period instead of racing to catch up.
//!
//! MEASURED (`the_limiter_holds_the_cap`, loose release-mode timing bound for CI): 60 simulated
//! frames at 250 FPS take >= 0.95x the ideal 240 ms; slower workloads pass through within tolerance.

use std::time::{Duration, Instant};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;

use super::{PresentCaps, Settings};

/// The floor of the adaptive spin window. A `thread::sleep` that returns exactly on its deadline
/// does not exist on any of the three target platforms, so a zero floor would just re-enter the
/// sleep loop for a few microseconds at a time; 100 µs is under 2.5% of a 240 FPS frame.
const MIN_SPIN: Duration = Duration::from_micros(100);

/// Spin-window ceiling: absorbs a scheduler quantum of coarse-sleep overshoot; larger misses
/// are repaid by the grid rather than more busy-waiting.
const MAX_SPIN: Duration = Duration::from_micros(1500);

/// Release rate of the spin window: each in-budget sleep pulls the estimate `1/DECAY` of the way
/// down toward what was actually observed. A power of two, so the arithmetic is a shift.
const DECAY: u32 = 8;

/// Fixed-grid deadline and measured spin window.
pub(super) struct Limiter {
    /// The next deadline on the fixed grid, or `None` while disarmed.
    next: Option<Instant>,
    /// Asymmetric EWMA of sleep overshoot. Starts at [`MAX_SPIN`] for first-frame precision
    /// and decays toward observed error.
    spin: Duration,
}

impl Default for Limiter {
    fn default() -> Self {
        Self {
            next: None,
            spin: MAX_SPIN,
        }
    }
}

impl Limiter {
    /// Instant attack avoids averaging away a visible deadline miss. Release by `1/DECAY` keeps
    /// the estimate from collapsing onto the best sleep and missing deadlines until it recovers.
    fn observe(&mut self, overshoot: Duration) {
        self.spin = if overshoot > self.spin {
            overshoot
        } else {
            self.spin - (self.spin - overshoot) / DECAY
        }
        .clamp(MIN_SPIN, MAX_SPIN);
    }
}

/// `Last` system; [`NonSendMarker`] keeps the wait off the shared compute pool (see module doc).
pub(super) fn limit_frame_rate(
    _non_send_marker: NonSendMarker,
    settings: Res<Settings>,
    caps: Res<PresentCaps>,
    mut limiter: Local<Limiter>,
) {
    match settings.frame_limit_period(*caps) {
        Some(period) => wait_for_deadline(&mut limiter, period, Instant::now()),
        // Disarmed: forget the grid, so re-arming starts fresh instead of "catching up" to a
        // deadline scheduled while the cap was off.
        None => limiter.next = None,
    }
}

/// Hold until the grid deadline, then schedule the next one. Split from the system (and handed
/// `now`) so the grid arithmetic and the pass-through path are testable without an `App`.
fn wait_for_deadline(limiter: &mut Limiter, period: Duration, now: Instant) {
    let target = match limiter.next {
        // Still inside the schedule: hold to the grid.
        Some(target) if target + period >= now => target,
        // First armed frame, or the frame overran its whole slot: reset the grid rather than
        // sleeping a stale deadline or racing to repay one.
        _ => now,
    };
    sleep_until(limiter, target);
    // `target`, not `Instant::now()`: the grid advances by exact periods, so oversleep on one
    // frame is repaid on the next instead of compounding into rate drift.
    limiter.next = Some(target + period);
}

/// The hybrid wait: coarse `sleep` down to the current spin window, then spin. Returns at or (by at
/// most the OS's last-tick jitter) after `target`, and leaves the window sized by what the sleeps it
/// just performed actually cost.
fn sleep_until(limiter: &mut Limiter, target: Instant) {
    loop {
        let now = Instant::now();
        let Some(remaining) = target.checked_duration_since(now) else {
            return;
        };
        if remaining > limiter.spin {
            let requested = remaining - limiter.spin;
            std::thread::sleep(requested);
            // The measurement that sizes the next window. `saturating_sub`: an undersleep (Windows
            // has been seen returning a tick early) is an overshoot of zero, not a negative one.
            limiter.observe(now.elapsed().saturating_sub(requested));
        } else {
            break;
        }
    }
    while Instant::now() < target {
        std::hint::spin_loop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cap actually caps: N waits at period P take at least N*P wall time (minus one period —
    /// the first armed frame establishes the grid rather than waiting). The lower bound is the
    /// capping property; no upper bound is asserted tightly because CI machines oversleep. It holds
    /// from the narrowest spin window the clamp allows, the most sleep-precision this code ever asks
    /// the OS for: whatever the estimate, the spin tail covers an under-estimate.
    #[test]
    fn the_limiter_holds_the_cap() {
        let period = Duration::from_millis(4); // a 250 FPS cap
        let frames = 60;
        let mut limiter = Limiter::default();
        for _ in 0..200 {
            limiter.observe(Duration::ZERO);
        }
        assert_eq!(limiter.spin, MIN_SPIN);
        let start = Instant::now();
        for _ in 0..frames {
            wait_for_deadline(&mut limiter, period, Instant::now());
        }
        let elapsed = start.elapsed();
        let floor = period * (frames - 1);
        assert!(
            elapsed >= floor,
            "{frames} frames at {period:?} took {elapsed:?} — the cap is not capping (floor {floor:?})",
        );
    }

    /// Over-budget work must pass through without stacked wait debt. MEASURED work time excludes
    /// OS sleep overshoot from the limiter's cost; assuming requested work time makes the test flaky.
    #[test]
    fn a_slow_frame_is_not_punished() {
        let period = Duration::from_millis(2);
        let work = Duration::from_millis(5);
        let mut limiter = Limiter::default();
        let start = Instant::now();
        let mut worked = Duration::ZERO;
        for _ in 0..10 {
            let frame = Instant::now();
            std::thread::sleep(work); // the simulated over-budget frame
            worked += frame.elapsed();
            wait_for_deadline(&mut limiter, period, Instant::now());
        }
        let elapsed = start.elapsed();
        // Whatever the work actually took, the limiter may not add stacked debt on top — a few
        // periods of slack covers loop overhead.
        assert!(
            elapsed < worked + period * 4,
            "an over-budget workload was further delayed by the limiter: \
             {elapsed:?} total for {worked:?} of work",
        );
    }

    /// The grid is exact: consecutive deadlines are exactly one period apart while frames stay in
    /// budget, which is what repays oversleep instead of compounding it.
    #[test]
    fn deadlines_advance_on_a_fixed_grid() {
        let period = Duration::from_millis(1);
        let mut limiter = Limiter::default();
        wait_for_deadline(&mut limiter, period, Instant::now());
        let first = limiter.next.expect("armed");
        wait_for_deadline(&mut limiter, period, Instant::now());
        let second = limiter.next.expect("still armed");
        assert_eq!(second.duration_since(first), period);
    }

    /// Instant attack, slow release and both clamps let precise timers spin less; see module doc.
    #[test]
    fn the_spin_window_tracks_measured_oversleep() {
        let mut limiter = Limiter::default();
        assert_eq!(
            limiter.spin, MAX_SPIN,
            "the first capped frame must be as precise as the old fixed window"
        );

        // Release: a machine whose sleeps land clean walks the window down to the floor, and stops
        // there — never below the cost of one `Instant::now()` round trip.
        for _ in 0..200 {
            limiter.observe(Duration::ZERO);
        }
        assert_eq!(
            limiter.spin, MIN_SPIN,
            "the window bottoms out at the floor"
        );

        // Attack: ONE bad sleep widens it immediately, all the way to the observed overshoot.
        limiter.observe(Duration::from_micros(700));
        assert_eq!(limiter.spin, Duration::from_micros(700));
        // And a single good sleep after it only releases 1/DECAY of the way back down (700 µs less
        // an eighth of itself, in whole nanoseconds).
        limiter.observe(Duration::ZERO);
        assert_eq!(limiter.spin, Duration::from_nanos(612_500));

        // Ceiling: a Windows-grade 15.6 ms miss is capped rather than spun for.
        limiter.observe(Duration::from_millis(16));
        assert_eq!(
            limiter.spin, MAX_SPIN,
            "the window never exceeds the ceiling"
        );
    }
}
