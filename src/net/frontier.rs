//! Read-only detection of interpolation timeline snaps after connection setup.

use bevy::prelude::*;
use lightyear::core::time::TickInstant;
use lightyear::interpolation::timeline::InterpolationTimeline;
use lightyear::prelude::{Connected, NetworkTimeline, SyncSystems};

pub(super) fn install(app: &mut App) {
    app.init_resource::<FrontierDiag>();
    app.init_resource::<PreSyncCursor>();
    app.add_observer(allow_handshake_resync);
    app.add_systems(
        PostUpdate,
        (
            note_pre_sync_cursor.before(SyncSystems::Sync),
            count_interp_resyncs.after(SyncSystems::Sync),
        ),
    );
}

#[derive(Resource, Default)]
struct FrontierDiag {
    sync_events: u64,
    // Each connection owes one handshake snap; later snaps are faults.
    handshake_spent: bool,
}

/// A (re)connection resets the interpolation timeline, so its first resync is the handshake.
fn allow_handshake_resync(_connected: On<Add, Connected>, mut diag: ResMut<FrontierDiag>) {
    diag.handshake_spent = false;
}

/// The interpolation cursor as it stood entering this frame's timeline sync.
#[derive(Resource, Default)]
struct PreSyncCursor(Option<TickInstant>);

fn note_pre_sync_cursor(
    timeline: Option<Res<InterpolationTimeline>>,
    mut pre: ResMut<PreSyncCursor>,
) {
    pre.0 = timeline.map(|timeline| timeline.now());
}

/// Report interpolation-timeline resyncs after the handshake: every remote hull jumps.
///
/// Lightyear emits no event for this timeline's resync, so it is detected: the cursor advances
/// only in `PreUpdate` (`TimelineSystems::Advance`), and inside `SyncSystems::Sync` the speed
/// controller only rescales it — the one write to `now` there is `TimelineSync::resync`'s snap. So
/// `now` differing across the sync set IS a resync. (A snap onto the exact current instant is
/// invisible, and moves nothing to observe.)
fn count_interp_resyncs(
    timeline: Option<Res<InterpolationTimeline>>,
    pre: Res<PreSyncCursor>,
    mut diag: ResMut<FrontierDiag>,
) {
    let (Some(timeline), Some(before)) = (timeline, pre.0) else {
        return;
    };
    if timeline.now() == before {
        return;
    }
    diag.sync_events += 1;
    if !diag.handshake_spent {
        diag.handshake_spent = true;
        return;
    }
    warn!(
        "net: FRONTIER interpolation resync #{} — a steady-state resync snapped every remote hull",
        diag.sync_events
    );
}
