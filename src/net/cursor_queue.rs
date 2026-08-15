//! Tick-stamped server announcements held until the interpolation cursor crosses their tick.
//!
//! Every interpolated hull renders at the interpolation cursor (`net::interp_delay` sizes its lag),
//! but a server-announced EVENT arrives a derived delay earlier than the motion it belongs to: the
//! wire hands it over `RTT/2` after the authority tick, while the stream draws that tick `RTT/2 +
//! delay` later. Presenting at arrival therefore desynchronizes announcement from motion by the
//! whole interpolation delay — an opponent's shot bangs before its hull visibly rocks. This queue
//! is the scheduling rule that closes the seam: hold the announcement, release it when the cursor
//! crosses the announcement's tick, with no free constant anywhere (release time IS the crossing).
//!
//! Mechanics, each pinned by a test below:
//! - an announcement whose tick the cursor has already crossed releases on the same drain (a late
//!   join or a long round trip presents immediately, exactly as before this queue existed);
//! - a stalled cursor HOLDS — nothing here expires by time, only [`CursorQueue::CAP`] bounds memory;
//! - a drain releases in tick order, arrival order within a tick;
//! - entries are plain wire data, never entity-keyed, so a despawned subject cannot strand an
//!   entry: every entry leaves on its crossing and the consumer re-resolves it there
//!   (`net::client`'s resolve path, which already owns the missing-replica case).
//!
//! Member classes: the fire announcement (`net::client::HeldFireEvents`, covering opponent fire
//! and the fused own echo), and under `OVERMATCH_CURSOR_HIT_FEEL` the
//! own being-hit cue (`net::hit_feel::HeldHitCues`; default stays at arrival by design — reaction
//! time). Ricochet/impact facts stay OUT: they arm the sanctioned-outcome buffer for the cosmetic
//! shell march, whose presentation is already slaved to the shell's own flight and
//! `crate::PredictedPresent` re-aging (ADR-0021).

use std::collections::VecDeque;

use lightyear::core::tick::Tick;

/// Whether the fractional cursor `cursor_tick + overstep` has crossed `event` — lightyear's own
/// wrapping tick difference, so the rule survives the `u32::MAX` boundary.
fn crossed(event: Tick, cursor_tick: Tick, overstep: f64) -> bool {
    f64::from(event - cursor_tick) <= overstep
}

/// Bounded hold of tick-stamped announcements awaiting the interpolation cursor.
#[derive(Debug)]
pub(super) struct CursorQueue<T> {
    held: VecDeque<(Tick, T)>,
}

impl<T> Default for CursorQueue<T> {
    fn default() -> Self {
        Self {
            held: VecDeque::new(),
        }
    }
}

impl<T> CursorQueue<T> {
    /// DERIVED like `PendingFireEvents::CAP`: 256 exceeds the 60-fire synchronized 30-tank volley
    /// and stays under the 2,048-entry `ShotId` dedup horizon, so a held id cannot outlive its
    /// duplicate guard. A memory bound only — eviction is by capacity, never by time.
    const CAP: usize = 256;

    /// Hold one announcement. Returns the oldest entry evicted when the bound is hit.
    pub(super) fn hold(&mut self, tick: Tick, payload: T) -> Option<(Tick, T)> {
        self.held.push_back((tick, payload));
        (self.held.len() > Self::CAP)
            .then(|| self.held.pop_front())
            .flatten()
    }

    /// Release every announcement whose tick the cursor has crossed, in tick order (arrival order
    /// within a tick — the sort is stable). Everything else stays held, however long the cursor
    /// stalls.
    pub(super) fn release(&mut self, cursor_tick: Tick, overstep: f64) -> Vec<(Tick, T)> {
        if self.held.is_empty() {
            return Vec::new();
        }
        let mut due = Vec::new();
        let mut kept = VecDeque::with_capacity(self.held.len());
        for (tick, payload) in self.held.drain(..) {
            if crossed(tick, cursor_tick, overstep) {
                due.push((tick, payload));
            } else {
                kept.push_back((tick, payload));
            }
        }
        self.held = kept;
        due.sort_by_key(|(tick, _)| *tick - cursor_tick);
        due
    }

    /// Release everything regardless of tick — the no-cursor fallback (a client whose
    /// interpolation timeline has not synced draws no interpolated motion to fuse with, so its
    /// announcements present at arrival, exactly as before this queue existed).
    pub(super) fn release_all(&mut self) -> Vec<(Tick, T)> {
        self.held.drain(..).collect()
    }

    pub(super) fn clear(&mut self) {
        self.held.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.held.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AN ANNOUNCEMENT AHEAD OF THE CURSOR HOLDS, AND RELEASES EXACTLY AT THE CROSSING — the
    /// fractional comparison, not "within a tick of". Weakening `crossed` to `<`, adding any
    /// margin, or releasing unconditionally reds one of the three asserts.
    #[test]
    fn an_announcement_ahead_of_the_cursor_releases_exactly_at_the_crossing() {
        let mut queue = CursorQueue::default();
        queue.hold(Tick(100), "shot");
        assert!(
            queue.release(Tick(98), 0.5).is_empty(),
            "two ticks short: held",
        );
        assert!(
            queue.release(Tick(99), 0.999).is_empty(),
            "a fraction short: still held",
        );
        assert_eq!(
            queue.release(Tick(100), 0.0),
            vec![(Tick(100), "shot")],
            "the crossing itself releases",
        );
        assert_eq!(queue.len(), 0);
    }

    /// AN ANNOUNCEMENT ALREADY BEHIND THE CURSOR RELEASES ON THE FIRST DRAIN — the late-join /
    /// long-RTT case presents immediately.
    #[test]
    fn an_announcement_behind_the_cursor_releases_immediately() {
        let mut queue = CursorQueue::default();
        queue.hold(Tick(50), "late");
        assert_eq!(queue.release(Tick(60), 0.25), vec![(Tick(50), "late")]);
    }

    /// A STALLED CURSOR HOLDS, NEVER DROPS: a thousand drains at the same reading release nothing
    /// and lose nothing. Any time-based expiry in the queue reds this.
    #[test]
    fn a_stalled_cursor_holds_and_never_drops() {
        let mut queue = CursorQueue::default();
        queue.hold(Tick(500), "stalled");
        for _ in 0..1_000 {
            assert!(queue.release(Tick(490), 0.7).is_empty());
        }
        assert_eq!(queue.len(), 1, "the stall must not evict");
        assert_eq!(queue.release(Tick(500), 0.1), vec![(Tick(500), "stalled")]);
    }

    /// A drain releases in tick order, arrival order within a tick (the wire's unordered channels
    /// can deliver a later tick first). An unstable sort or plain arrival-order drain reds this.
    #[test]
    fn a_release_is_tick_ordered_and_arrival_ordered_within_a_tick() {
        let mut queue = CursorQueue::default();
        queue.hold(Tick(12), "a");
        queue.hold(Tick(10), "b");
        queue.hold(Tick(12), "c");
        queue.hold(Tick(11), "d");
        assert_eq!(
            queue.release(Tick(12), 0.0),
            vec![
                (Tick(10), "b"),
                (Tick(11), "d"),
                (Tick(12), "a"),
                (Tick(12), "c"),
            ],
        );
    }

    /// The capacity bound evicts the OLDEST held entry and nothing else — memory stays bounded
    /// under a stall without any entry expiring by time.
    #[test]
    fn capacity_evicts_the_oldest_entry_only() {
        let mut queue = CursorQueue::default();
        for index in 0..CursorQueue::<usize>::CAP {
            assert!(queue.hold(Tick(1_000 + index as u32), index).is_none());
        }
        let evicted = queue.hold(Tick(9_999), usize::MAX);
        assert_eq!(evicted, Some((Tick(1_000), 0)), "the oldest entry evicts");
        assert_eq!(queue.len(), CursorQueue::<usize>::CAP);
    }

    /// The crossing rule survives the `u32` tick boundary: an announcement just past the wrap is
    /// held by a cursor just before it, and releases once the cursor wraps after it.
    #[test]
    fn the_crossing_is_wrap_safe() {
        let mut queue = CursorQueue::default();
        queue.hold(Tick(2), "wrapped");
        assert!(
            queue.release(Tick(u32::MAX - 1), 0.5).is_empty(),
            "pre-wrap cursor: the announcement is 4 ticks ahead",
        );
        assert_eq!(queue.release(Tick(3), 0.0), vec![(Tick(2), "wrapped")]);
    }
}
