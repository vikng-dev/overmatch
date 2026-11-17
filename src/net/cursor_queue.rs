//! The game's damage-model presentation: public fire announcements and own echoes release at
//! max(arrival, crossing on `net::view_clock`). Arrival alone cannot fuse a shot with its hull's
//! visible pose. Before a view epoch, announcements hold; nothing expires by time.
//!
//! Releases are in tick order, arrival order within a tick. Entries are wire facts rather than
//! entity mappings, so a despawn cannot strand them: the consumer resolves the subject at release.
//! `CursorQueue::CAP` bounds retained facts. Public paths use their own buffer on the same cursor;
//! owner-private hit cues have a separate receiver.

use std::collections::VecDeque;

use lightyear::core::tick::Tick;

/// Whether the fractional cursor `cursor_tick + overstep` has crossed `event` — lightyear's signed
/// tick difference (monotonic ticks, saturating to the `i32` range).
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
    /// Resolution: 256 exceeds a 30-combatant x 2-weapon volley (60 fires, DERIVED),
    /// below the 2,048-id dedup allocation. Eviction visibly omits the oldest cosmetic.
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

        // Already behind the cursor, an announcement releases on the first drain — the late-join /
        // long-RTT case presents immediately.
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
}
