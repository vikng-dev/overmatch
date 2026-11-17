# View clock: timing slice 2 review closure

Subject: Overmatch's multiplayer tank game timing and damage-model presentation; game state only.
Crew are HP pools at stations. The lane is `lane/view-clock`; the reviewed implementation is
`da5cf764`. `main` merged first, without conflicts, at `8bf2a6e7`.

## Current clock law

Frame start maps completed authority ticks to time. For each whole-tick completion,
`x = previous completion-frame Real - frame start`; the unknown process-epoch offset stays in
x and cancels when the cursor inverts the same stamped map. The target is
**`median(received frame-start interval) + max_W(x)`**. Server work and delivery lateness have
one treatment: **no outlier exclusion**, retention by local receipt time for required
`view_window_s`, with eviction only when age exceeds W (`src/net/view_clock.rs:203`, `:208`, `:248`). Applied delay slews at v seconds per
Real second. The cursor inverts the map, stays forward-only within a connection, clamps to
completed received endpoints, and normally advances at most `(1+v)` simulation seconds per
Real second. Backlog in simulation ticks above `max(view_cut_s, admission_s)` cuts forward,
warns and counts; a cut does not clear the lateness window (`src/net/view_clock.rs:336`). No completed stamp pair is the
waiting state: samplers and rig attachment wait, fire facts hold, and trace ticks are null.

The complete contract lives in [net-motion-contract.md](../docs/design/net-motion-contract.md).
Required `assets/net_timing.ron` owns W, v, continuity, pending-rig lifetime and the shared server
admission cap. No code default or alternative controller remains.

**Departure still travels.** The twelve-byte stamp retains f64 frame start and f32 departure
offset. `Sample::work_s` and `usual_work_s` read departure for `client_jitter.view_clock.usual_work_ms`,
a diagnostic only; `scripts/perf/online_cost.py` and the offline harness parse that record. It has
no contribution to the target. The codec's representability checks change compatibility, so
`PROTOCOL_REV` is **105**, with the codec/type hash re-pinned by the protocol test.

## Findings checked against source

Every listed finding was confirmed. None was rejected. Net-22/#187 remains the owner's explicit
slice-3 deferral: this review changes clock publication, not arrival-owned turret behavior.

| Finding | Disposition and owner |
|---|---|
| Review P2-1, above-median server work | Fixed in `net/view_clock.rs`: frame-start lateness replaces departure lateness plus median work. `view_clock/udp.rs` exercises a Fixture compute distribution on a clean fixed-propagation link. The provisional choice is [F21](playtest-forks/README.md#f21--server-work-and-link-lateness-share-one-view-budget). |
| Review P2-2, permissive UDP allowance | Fixed in `view_clock/udp.rs`: derive the actual +9 ms jitter peak, round delivery up to render reads, subtract the preceding read, add the authority interval. Allow exactly one extra read phase, only upward. Mutant results are recorded below. |
| Review P2-3, mixed trace domains | Fixed in `shot_trace.rs`, `net/client.rs`, `trace.rs`, and `scripts/shot/analyze.py`. All client lifecycle rows use one server-domain view tick captured after clock drive. `trace.rs` carries any local counter separately as `local_tick`. Waiting is null. `dmg_lead = view tick at receipt - authority damage tick`: positive means the view passed the consequence, negative means confirmation arrived before its view crossing. This concerns game state only. |
| Review P2-4, stale prose | Fixed ADR-0037, the quality-standard table, glossary tick-index entry, `WeaponClock` docs, and both continuity/motion design documents. They describe the current owner and proofs rather than retaining contradictory present-tense rules below a status banner. |
| Error-handling P2, finite hostile epoch | Fixed in `tick_stamp/wire.rs` and `view_clock.rs`: admission reserves Duration arithmetic headroom; sample admission checks the lateness spread and carries a checked Duration to the debug HUD. Typed refusal follows `ServerGone` to the visible lobby cause. The wire test includes individually finite `1e30` fields; the match-close proof uses that hostile epoch. |
| Testing P2, substituted gameplay composition | Fixed `tests/stream_presence.rs`: mount the production view-clock installer and drive completed stamps. A separate fixed-gameplay counter proves gameplay runs while the view and weapon clock hold. |
| Testing P2, render-cadence coupling | Fixed `view_clock/udp.rs`: server deadlines read `server::TICK` instead of updating every other render frame. |
| Review P3, unread ViewCut message | Deleted the message registration/write. A private cut value has actual readers: the driver logs it and the tests inspect it; stats count cuts. |
| Review P3, hidden admission-cap coupling | Fixed `net/timing.rs`, `server_runner.rs`, `view_clock.rs`, `assets/net_timing.ron`: one required `admission_s`, read directly by server admission and client continuity. |
| Review P3, reader test accommodations | Fixed required `Res<ViewClock>` in fire receipt/path aging/confirmation; required `InterpolationTimeline` in drive; fixtures supply their contracts. No invented Tick(0) or timeline-default write before a stamp pair. Shared offline/network motion tracing keeps optional network resources, with an explicit client-role invariant. |
| Review P3, role and fixed-only publication | Fixed `protocol.rs` and the clock installer: running server state selects authority weapon publication; the client installer publishes view-owned weapon time each PreUpdate before fixed readers, including render frames without fixed steps. No role is inferred from ViewClock presence. |
| Review P3, aging silently accepts backward time | Fixed `client.rs`: connect/disconnect advances an epoch generation. Local aging resets for that generation, and backward motion inside an epoch asserts the named invariant. |
| Review P3, lost cap derivations | Restored Resolution sizing examples beside SeenShots, CursorQueue and PendingFireEvents caps, without treating an example fire rate as a vehicle law. |
| Review P3, fragile one-second stall proof | Replaced zero-cut output pin with a capped-stall observation; the runner's own admission test proves exact cap spending and discarded wall time. |
| Review P3, spike playback unproven | Added a headless real-UDP downlink-blackout scenario through hold, recovery and full window expiry, asserting only clock invariants and reporting off-rate playback. |
| Review P3, drift unproven | Added Fixture +/-50 ppm oscillators. Independent bound: window bias is at most abs(epsilon) W; neither process needs a shared local tick epoch. |
| Review P3, HUD clock scope | `shooting::tick_weapon_gate` names slice 3's remaining work: admit the arrival-owned gate snapshot on the same cursor that matures it. |
| Review P3, harness delay/null | Renamed the parsed field to `applied_above_latest_x_ms`, optional with a named absence (no epoch or retained sample); table aggregation omits absent observations rather than inventing zero. |
| Base-truths P3, transport timestep | Fixed `client.rs` and `client/hosted_e2e.rs`: both read `net::server::TICK`. |
| Brevity P3, respawn docs | Condensed to one authority-admission caveat; retained view units, rounding and absence cases. |
| Brevity P3, reconstruction temporary | Matched `present_released_shot` directly; retained the complete stale omission record. |
| Brevity P3, frame-trace duplication | Removed the repeated headroom formula and optional-resource explanation; retained the shared-composition contract and diagnostic-only RTT/jitter statement. |

## Playback observations

All durations here are MEASURED fixture observations, not output pins or tuning targets.
Fixtures use actual loopback UDP, the production protocol, view-clock installer and one-tick-per-send
server schedule. Time and opaque datagram delivery are controlled. Variable-work samples are
labelled **Fixture**, a reproducible synthetic load distribution, not a measured server profile.

Round 1 reference (median compute, Fixture 3 ms ordinary work): the 200 ms server silence held **176.5625 ms total**
(137.5 ms silence plus 39.0625 ms recovery), zero cuts, then bounded catch-up. Delay changed one
**7.8125 ms** read phase instead of retaining the stall. The one-second silence held **968.75 ms**,
zero cuts; the server admitted only its cap and discarded the excess. Those captures described
absence of server updates, not a proof that arbitrary above-median computation was covered.

Current MEASURED real-UDP observations (the corrected client-epoch blackout, Fixture 12 ms
ordinary work, 128 Hz reads):

| Scenario | Holds / cuts | Delay and pacing observation |
|---|---|---|
| Settled 80 ms RTT / integer jitter | 0 holds/minute, 0 cuts | Applied 70.3125 ms; DERIVED band 70.3125–78.125 ms. |
| Clean link, variable work distribution | 0 holds/minute, 0 cuts | Fixture work samples 1.5, 2, 3.6, 8, 14.3 ms; fixed propagation, no link jitter. |
| Single 200 ms server work stall | 179.6875 ms held; 0 cuts | Applied delay 70.3125 → 250 ms; above baseline for **10.8984375 s**, from 0.2578125 through 11.1484375 s after stall start. Nonzero slow playback occupied 0.4453125 s; fast playback 1.34375 s. |
| Single 1 s capped server work stall | 976.5625 ms held; 0 cuts observed | Applied delay peaked at 1046.875 ms and stayed above baseline for **14.8828125 s**, through 15.9296875 s after stall start. Nonzero slow playback 4.2734375 s, fast playback 5.484375 s. Zero cuts is an observation, not an assertion. |
| Single 500 ms downlink blackout | 484.375 ms held; 1 forward cut | Cut at 0.5 s advanced 31.3999939 ticks. Delay 70.3125 → 554.6875 ms; above baseline for **12.421875 s**, through 12.9140625 s after blackout start. Slow playback 2.4140625 s and fast playback 2.421875 s. |

Slow/fast means observed nonzero rate below 0.99 or above 1.01 respectively; those bands only
classify fixture output and do not govern playback. A normal step remains bounded by `1+v`.
The server-stall effect now lives in the same window as network lateness: the 200 ms stall
adds about 180 ms to applied delay and costs a retained-window interval plus paced recovery,
beside round 1's short hold-and-catch-up reading. This is F21's owner feel question.

The socket fixture lets loopback enqueue before advancing its independent manual time; real OS
scheduling is excluded from the controlled clean-link case. The first unpaced fixture run
observed one extra read slip after settling (one 7.8125 ms hold). The fixture was corrected at
that transport seam, without adding a production margin or relaxing its zero-hold requirement.

Mutant check, MEASURED: subtracting serialization departure (omitting compute from the target)
produced **58.3124999 ms**, rejected by the exact **70.3125–78.125 ms** band. The test reaches the
delay assertion before the hold assertion, so this directly proves the allowance rejects it.
Subtracting the fixture's actual 9 ms jitter contribution produced **61.3125 ms** and failed
the same delay assertion. Both mutants were removed, and the restored chosen law passed the final
net run. Logs: `/tmp/view-clock-mutant-no-compute.log`, `/tmp/view-clock-mutant-no-jitter.log`. The constant-work UDP fixture's
DERIVED target band is **70.3125–78.125 ms** at the current authority/render rates: peak jitter is
9 ms; ceil((12 + 40 + 9)/7.8125) = 8 reads; target = 15.625 + 8*7.8125 - 7.8125 = 70.3125 ms;
one possible socket read slip adds exactly 7.8125 ms. No symmetric or extra authority-tick
allowance is admitted. The Fixture work is now 12 ms so its omission exceeds one read quantum;
the original 3 ms fixture could hide a work omission inside the allowed read slip. This is
mutation-sensitive fixture design, not a changed production parameter.

## Additional findings

Lightyear 0.31.0's `update_interpolation_history`, `apply_interpolation`, and delayed-despawn
reader (registered in `InterpolationSystems::Prepare`) read `InterpolationTimeline` directly,
without testing `is_synced`. Therefore readiness needs a
run condition on `InterpolationSystems::All`, not merely `set_synced(false)`. Rig attachment
also waits for a view epoch so init-snapshot poses cannot assemble a visible tank before that
first presentation pair. This is the explicit startup behavior missing from the reviewed code.

Client trace producers beyond the two named receipt rows also used transport labels (cosmetic
spawn/trail, explanation and capture-end rows). The writer owns the client tick domain, so all
of them now get the same view tick without maintaining separate producer rules. Wall-microsecond
fields still own cross-process elapsed-time measurements.

Pinned API facts were checked in the local registry sources: Bevy 0.20.1 time/schedule APIs,
Lightyear 0.31.0 timeline setters, sync-set membership, sampler readiness and interpolation sets.
No web search, AWS service call, external write, agent spawn or push was used.

The jitter plot's tick-to-wall description now names the view domain, and the shot analyzer's
`tick_diff` matches the verified Lightyear 0.31.0 saturating i32 difference rather than the
retired wrapping rule. Missing trace-end domains remain absent, not an invented zero gap.

## Validation

All compiling commands use `scripts/serial-cargo`, `--features dev_ui,bitprobe`, and
`OVERMATCH_UNATTENDED=1`. The runner owns its window mode. The final Rust gates all passed after the last source edit. The three combat runs preceded only
two comment corrections; executable behavior was unchanged.

| Check | MEASURED result |
|---|---|
| `cargo fmt --all --check` | Passed |
| CI clippy, locked workspace/all targets, warnings denied | Passed |
| Audio-free server check, `--no-default-features --bin overmatch-server` | Passed; the two existing audio-only data dead-code warnings remain |
| Final `net::`, including protocol pins and the production trace installer proof | **423 passed**, 7 ignored, zero failures |
| `tests::doc_citations` | **3 passed**, zero failures |
| AWS harness / online-cost Python | **25 / 16 passed**, all AWS boundaries are offline fakes |
| Shot analysis / pen-cam latency / runner Python | **30 / 9 / 5 passed** |
| Shell / jitter analyzer | `shellcheck scripts/aws/host-setup` passed for the merged wrapper; no lane shell edit. Jitter-analyzer syntax passed. |
| Full library suite after the last source edit | **2,835 passed**, 50 ignored, zero failures, **387.36 s** |
| Shipped-binary combat-start runner | **3/3 passed**; each client completed 256 script ticks, exited zero, and produced exactly one authority shot after release |

Artifacts: `/tmp/view-clock-post-prose-checks.json` holds the final Rust gates;
`/tmp/view-clock-review-final-checks.json` holds the Python and combat results, and a preliminary
full suite that also passed before the two comment corrections (2,835 passed, 366.20 s).
The named per-check logs are alongside them,
and `/tmp/view-clock-review-combat-{1,2,3}/`. The previous lane's 2,831-pass suite preceded its
last source edit and is historical evidence only. The six lenses were self-checked over this
review's changes; the orchestrator's independent landing reviews remain its responsibility.

Combat observations (game state only; Fixture scripted run, not a latency requirement):

| Run | Authority release / first shot | Client local release / first input | Authority shots | Completion / exit |
|---|---|---|---|---|
| 1 | 100 / 119 | 22 / 22 | 1 | 256 / 0 |
| 2 | 98 / 117 | 22 / 22 | 1 | 256 / 0 |
| 3 | 99 / 118 | 22 / 22 | 1 | 256 / 0 |

The client-local columns are harness scheduling observations; they are never subtracted from
server-domain authority or view ticks. Lifecycle JSONL rows themselves carry the view tick.
