# Client presentation through an interpolation resync

Subject: the game's netcode, client presentation and damage model; game state only.

## Cause verified in dependency source

The pinned lightyear_interpolation 0.30.1 `TimelinePlugin::sync_timeline` synchronizes the
presentation cursor in `PostUpdate`, on received-packet samples. `TimelineSync::sync` in
lightyear_sync 0.30.1 snaps on initial synchronization or when the cursor/objective error exceeds
`SyncConfig::max_error_margin` (DERIVED installed bound: ten ticks, inherited by
`sync_margin::derived_interp_sync`). Smaller errors steer clock speed. A resync sets the cursor
to the remote estimate minus the configured delay and jitter margin; gameplay state transitions
are not themselves a resync trigger.

In the next `Update`, `update_interpolation_history` prunes histories and calls
`queue_history_presence`. That function removes a live component both for `HistoryState::Removed`
and for **no known state at or before the cursor**. `drain_old_history` retains a bracketing pair
as the cursor advances, but a backward resync can land before that retained pair. The resulting
absence lasts until the cursor reaches retained history or an applicable older sample arrives;
it has no one-frame bound. This is separate from ordinary starvation beyond the newest sample,
where the existing interpolation sampler holds the newest value.

The rule applies to every interpolated registration: `Position`, `Rotation`, `TrackDrive`,
`DriveSignal` and `DebrisPose`. It does not remove non-interpolated velocity, weapon state,
identity, hierarchy or transforms. Because `RunFixedMainLoop` precedes `Update`, a removal left
at the end of that update reaches the next frame's fixed gameplay and physics schedules.

## Decision and invariant

Retain an already-presented stream component through unknown history, centrally in `net::resync`.
Its local `Presented` state distinguishes a sampled value from a held value. A removal observer
restores the last live sample within the same deferred-command flush that removes it; scheduled
consumers retain their required-component contracts. This uses Bevy 0.19.1's verified `Remove`
observer semantics: the old value is still queryable, and observer commands flush before
Lightyear proceeds to apply interpolation. The dependency versions are those resolved in
`Cargo.lock`.

The boundary distinguishes unknown history from an explicit server removal. A new component
without a previously presented sample still waits for its first visibility tick. Explicit
removal clears presentation state. Restoration expires on despawn instead of resurrecting the
entity. Known samples resume the ordinary interpolator and return the state to sampled. No
position is projected, no historical sample is invented, and no registry or vendor patch is
needed. Wire registrations and serialized types are unchanged.

Rig attachment owns its ordering after interpolation. An init snapshot can contain a component
whose first visibility tick is still ahead of the cursor; the root must not acquire a rig and
its required-pose consumers until presence preparation has made that decision. Both production
clients and the replay mount the same ordered attachment configuration.

Per-consumer optional-pose skips were rejected: every consumer would have to remember the same
transport detail. Gating only remote firing would leave every other pose reader exposed.
`shooting::fire` again requires a posed tank; its resync-specific skip and skip test are gone.
That shared system enumerates remote muzzles but skips roots without owner-private weapon state;
it still serves authority/offline firing and the client's own fire bookkeeping. Remote rounds
present from server facts, not remote client firing. Collider history remains authority-only
because clients never run the simulation walk, independently of resync handling.

## Supplied capture evidence and separate findings

MEASURED in `caps-1101b/logs/client.log`: the logged interpolation-delay **target** rose from
35.1 ms to 147.6 ms, with the latter log at 07:16:00.803856Z; the steady-state resync was logged
at 07:16:00.804524Z. The delay logger prints the target, not the follower's written value.
MEASURED in the server log: twenty bot knockouts in the game's damage model at 07:16:00.708Z.
The ordering is correlated, not proof that the knockout burst caused the clock error. The
capture has no controller error trace that establishes that causal chain. Receipt scheduling
and estimator/objective movement can cause this class of backward correction; the fix does not
depend on which caused this particular resync.

MEASURED: all twenty-six `server_late_input_mismatch` diagnostics have idle gameplay fields in
their printed buffers. DERIVED from those buffers and the input implementation: newly arriving
tick-stamped commands replace inherited idle commands carrying an older `for_tick`.
Lightyear's native input buffer compares the whole action state,
including that stamp, for already-simulated ticks. This explains these errors without implying
twenty-six changed gameplay actions. The input margin and diagnostics are unchanged.

MEASURED by the supplied capture: bots drove straight in one file, approximately 9.1 m apart,
with guns on neighbours, despite `drive_bot` claiming a circle. Source confirms `bot_home_xz`
uses the one-dimensional X lane fan-out with DERIVED spacing twice the content contact radius;
all bots start with identity rotation and the same fixed throttle, steer and unaimed trigger.
The observed failure to turn is not diagnosed by this lane. Bot placement and control are
unchanged and need a separate owner decision before the next Battle capture.

## Regression evidence

MEASURED: the initial dependency-backed replay failed with "a resync removed a live replica's
required pose". The repaired tests exercise the actual Lightyear presence pipeline, repeated
unknown-history frames, exact last-sample retention, sampling recovery, first visibility,
explicit removal and despawn, including every continuous component registration.

The composed replay runs assembled replica tanks through the headless client gameplay frame,
using the production `install_replica_consumers` mount, including fire receipt, recoil,
destruction, debris, wall-ledger and public-path presentation beside the full simulation schedule.
It asserts that weapons
and child colliders exist, that their body poses remain present, and that the fixed weapon clock
advances. The fixture uses the shipped vehicle as data and asserts only class invariants.
The full Battle capture is deliberately left to the orchestrator's capture session.

MEASURED final lane gates:

| Gate | Result |
|---|---|
| Formatting and CI workspace/all-targets clippy (`dev_ui,bitprobe`, warnings denied) | Passed |
| Resync, shooting and authority-only collider-history targets | 25 passed |
| `net::` | 287 passed; live hosted probe ignored |
| `headless_test` | 17 passed; two on-demand measurement beds ignored |
| `doc_citations`, `ui_ascii`, `net_boundary`, `bake::` | 43 passed |

No live hosted probe, pricing run or Battle capture was executed. The UI ownership fixture's
camera and spec-store composition correction is a separate lane commit, titled
"net: supply the client contracts in UI ownership fixtures (#219)".
