//! The frame-rate limiter: hold the END of each main-world frame until the cap's deadline.
//!
//! Armed by exactly one conjunction — [`super::Settings::frame_limit_period`] returns `Some` only
//! under an EFFECTIVE vsync OFF with a non-off [`super::FrameCap`] — so with any present-mode wait
//! in play this module costs one resource read per frame and nothing else.
//!
//! # Shape: a hybrid coarse-sleep + adaptive spin, in `Last`
//!
//! `std::thread::sleep` overshoots (macOS/Linux timer slack is hundreds of microseconds; Windows'
//! default timer resolution is up to 15.6 ms), and a pure spin burns a core. So the wait sleeps in
//! coarse chunks while the remainder is large, then SPINS the final stretch against `Instant::now()`
//! — the standard shape (spin_sleep, bevy_framepace) hand-rolled on std alone.
//!
//! **The spin window is MEASURED, not assumed.** It used to be a fixed 1.5 ms, which at a 240 FPS
//! cap is ~36% of every 4.17 ms frame spent in a busy loop — a whole core burned to buy timing
//! precision that most machines do not need. Instead every `thread::sleep` call reports how far past
//! its request it actually returned, and the window tracks that overshoot ([`Limiter::observe`]):
//! instant attack (one bad sleep widens the window immediately), slow release, clamped to
//! [`MIN_SPIN`]..=[`MAX_SPIN`]. A machine whose sleeps land within ~100 µs therefore spins ~100 µs
//! (~2.4% of a core at that same cap) and one with a coarse timer degrades to the old fixed window.
//!
//! It runs as a MAIN-THREAD system at the end of `Last`, i.e. inside bevy's frame, not against the
//! runner: blocking the main schedule's tail delays the NEXT frame's start, which paces the whole
//! pipeline (with pipelined rendering the render app consumes one extract per main-world frame, so
//! main-loop cadence IS present cadence under an uncapped present mode). Nothing here fights the
//! winit runner — by the time this system runs the frame's events are long consumed, and the sleep
//! is indistinguishable from a long frame to everything upstream.
//!
//! **Why the `NonSendMarker`** (the same pin `probe` and `settings::observe_window_mode` use): an
//! ordinary `Send` system may be handed to any `ComputeTaskPool` worker by the multithreaded
//! executor, so this one would park a POOL thread — the very threads pipelined rendering and the
//! parallel visibility/extract work run on — for the whole tail of every frame. On a low-core
//! machine (where a frame cap is most likely to be set) that starves the render app the limiter is
//! supposed to be pacing. The marker forces the wait onto the main thread, which is the one thread
//! that has nothing else to do while the frame is being held.
//!
//! # The schedule, not the gap
//!
//! Deadlines advance on a fixed grid (`deadline += period`), not `now + period`: sleeping "period
//! after the previous wake" would add the frame's own work time to every cycle and undershoot the
//! target rate by exactly the workload. When a frame overruns its slot entirely, the grid resets to
//! `now + period` instead of racing to catch up — a spiral of death is a limiter bug, not a cap.
//!
//! MEASURED (test `the_limiter_holds_the_cap`, release-mode timing asserted loosely enough for CI):
//! 60 simulated frames at a 250 FPS cap take >= 0.95x the ideal 240 ms wall time, and a workload
//! slower than the cap is passed through un-delayed within tolerance.

use std::time::{Duration, Instant};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;

use super::{PresentCaps, Settings};

/// The floor of the adaptive spin window. A `thread::sleep` that returns exactly on its deadline
/// does not exist on any of the three target platforms, so a zero floor would just re-enter the
/// sleep loop for a few microseconds at a time; 100 µs is under 2.5% of a 240 FPS frame.
const MIN_SPIN: Duration = Duration::from_micros(100);

/// The ceiling of the adaptive spin window — the value this whole knob used to be fixed at. Wide
/// enough to absorb the OS oversleeping a coarse chunk by a scheduler quantum; anything beyond it
/// is left to the grid to repay rather than bought with more busy-waiting.
const MAX_SPIN: Duration = Duration::from_micros(1500);

/// Release rate of the spin window: each in-budget sleep pulls the estimate `1/DECAY` of the way
/// down toward what was actually observed. A power of two, so the arithmetic is a shift.
const DECAY: u32 = 8;

/// The limiter's state: the next deadline on the fixed grid (`None` while disarmed) plus the
/// measured spin window.
pub(super) struct Limiter {
    /// The next deadline on the fixed grid, or `None` while disarmed.
    next: Option<Instant>,
    /// How much of the tail of each wait to spin — an asymmetric EWMA of the observed
    /// `thread::sleep` overshoot. Starts at [`MAX_SPIN`] so the very first capped frame is as
    /// precise as the old fixed window, and walks down from there.
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
    /// Fold one measured sleep overshoot into the spin window.
    ///
    /// Asymmetric on purpose: a single bad sleep widens the window IMMEDIATELY (missing a deadline
    /// is visible as a dropped frame, so the estimate must not average a spike away), while a good
    /// one only narrows it by `1/DECAY` — otherwise the window would collapse onto the best sleep
    /// the machine ever managed and then miss every deadline until it grew back.
    fn observe(&mut self, overshoot: Duration) {
        self.spin = if overshoot > self.spin {
            overshoot
        } else {
            self.spin - (self.spin - overshoot) / DECAY
        }
        .clamp(MIN_SPIN, MAX_SPIN);
    }
}

/// The `Last` system. Reads the settings every frame (one `Option` construction); does nothing at
/// all unless the cap is armed.
///
/// The `NonSendMarker` is load-bearing — see the module doc: it keeps the wait off the shared
/// `ComputeTaskPool`, which pipelined rendering needs while this thread is parked.
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
    /// capping property; no upper bound is asserted tightly because CI machines oversleep.
    #[test]
    fn the_limiter_holds_the_cap() {
        let period = Duration::from_millis(4); // a 250 FPS cap
        let frames = 60;
        let mut limiter = Limiter::default();
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

    /// A workload SLOWER than the cap passes through: when every frame overruns its slot, the
    /// limiter resets the grid instead of stacking debt, and adds (nearly) no extra wait. The
    /// workload's own time is MEASURED, not assumed — `thread::sleep` overshoots by a scheduler
    /// quantum per call, and blaming that on the limiter made the first cut of this test flaky.
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

    /// Disarming forgets the grid: the next armed wait starts fresh from `now` instead of holding
    /// to a deadline scheduled before the cap was switched off.
    #[test]
    fn rearming_starts_a_fresh_grid() {
        let period = Duration::from_millis(50);
        let mut limiter = Limiter::default();
        wait_for_deadline(&mut limiter, period, Instant::now());
        // Simulate the disarm the system performs when the cap switches off.
        limiter.next = None;
        let start = Instant::now();
        wait_for_deadline(&mut limiter, period, Instant::now());
        assert!(
            start.elapsed() < period / 2,
            "the first wait after re-arming must establish the grid, not sleep a stale deadline",
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

    /// The adaptive window: instant attack, slow release, clamped at both ends. This is the
    /// property that lets a well-behaved machine spin ~100 µs instead of the 1.5 ms this used to
    /// burn unconditionally, without ever spinning LESS than the machine's own sleep error.
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

    /// The adaptive window may not break the wall-clock floor: whatever the estimate, the wait
    /// returns at or after the deadline — the spin tail is what covers an under-estimate.
    #[test]
    fn a_narrow_window_still_holds_the_deadline() {
        let period = Duration::from_millis(2);
        let mut limiter = Limiter::default();
        // Force the narrowest window the clamp allows, i.e. the most sleep-precision this code
        // ever asks the OS for.
        for _ in 0..200 {
            limiter.observe(Duration::ZERO);
        }
        assert_eq!(limiter.spin, MIN_SPIN);
        let start = Instant::now();
        for _ in 0..20 {
            wait_for_deadline(&mut limiter, period, Instant::now());
        }
        assert!(
            start.elapsed() >= period * 19,
            "the narrow window undershot the grid: {:?} for 20 frames at {period:?}",
            start.elapsed(),
        );
    }
}
