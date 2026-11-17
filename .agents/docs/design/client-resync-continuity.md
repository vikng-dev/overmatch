# Client stream continuity

Subject: the game's netcode, presentation and damage model; game state only.

`net::view_clock` owns a forward-only server-domain cursor. Both Lightyear 0.31.0 sync
controllers are disabled through `SyncSystems::Sync`; no handshake or steady-state controller
can rewind it. `InterpolationTimeline` samples this cursor. Before a completed stamp pair exists,
the timeline is unsynced and `InterpolationSystems::All` waits; there is no invented tick zero.

An already-visible continuous component holds its newest sample during silence. New components
wait for their first visibility tick; explicit authority removal and despawn still win. Missing
stamps bridge received endpoints without extrapolation. A link backlog beyond the continuity
budget cuts forward, logs and counts the discontinuity. The map retains the cursor's predecessor
and future points through a hold. The timing law lives in [the motion contract](net-motion-contract.md).

Rig attachment runs after presence preparation. Clients mount `install_replica_consumers` and
the production view-clock installer; `tests/stream_presence.rs` drives completed stamps and
independently probes fixed gameplay. It checks complete root poses and collider ownership during
holds and recovery, while weapon countdowns hold on view time. Public-path retention ages before
new receipts enter, so a fresh path cannot inherit a cut's elapsed duration.

The retired backward-resync restoration (`net::resync`, its `Presented` state and removal
observers) has no runtime reader or implementation. Its original dependency failure was real:
Lightyear pruned a bracketing pair, rewound before it, then removed live poses as unknown history.
Forward-only presentation removes that cause. Continuous-stream presence tests retain the
separate first-visibility, explicit-removal and despawn responsibilities.

Historical evidence: the supplied `caps-1101b` capture reported a delay target changing from
35.1 to 147.6 ms and a subsequent resync (MEASURED in its logs). The capture did not establish
why the target changed. Those are observations of the retired controller, never current timing
requirements. Current validation and playback observations live in
[the lane report](../../scratch/view-clock-slice-2.md).
