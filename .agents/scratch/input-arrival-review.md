# Timing slice 1 review disposition

Subject: the multiplayer tank game's fire-control input path and damage model; game state only.
Review base: `dc8d3476`. The two supplied review files were read without modification.

| Finding | Disposition |
|---|---|
| P1-1 archive smoke | Fixed: `scripts/ci/archive-smoke.sh` matches the live `harness: input slot` line. No log was added. |
| P1-2 AWS stats consumer | Server contract settled: use `$.inputs[*].arrival.stale_declarations` as an interval count, replacing `held%`; `$.inputs[*].arrival.stale_arrivals` counts accepted stale recoveries. Join rows by `$.inputs[*].link`. Both counters reset at each stats emission. Neither is a percentage or a substitute for tick matching. Both consumers now read this shape. The AWS window table sums `stale_decl` and `stale_arr`; its input-gap table retains each link. The perf table flattens arrival readings under `inputs.<link>.arrival`. Missing required fields refuse the record by name. |
| P2-1 malformed messages | Fixed: `receive_inputs` validates before resetting silence; refusal advances the same stale/lost laws as an empty receive. `last_refusal` has a production reader that suppresses repeated identical causes and clears after a valid message. The UDP scenario retains drive at stale, releases secondary fire, then brakes at lost while transport keepalives continue. |
| P2-2 deleted coverage | Fixed: restored both ownership/body arrival orders through the production claim installer and camera plugin; merged focus-loss/cruise and closed-window coverage; restored `ArrivalDelay` teardown; restored combat and workload-barrier preservation of script tick zero. Retired native-input and timeline-sync assertions stay deleted. |
| P2-3 recovery press | Fixed: `fresh_from` marks edges first sent in this message or the previous message, so one lost or superseded copy survives recovery. No wire field or client clock was added for freshness. The UDP test forces a second send with a selection edge before the server drains; it also exercises a dropped first copy. |
| P3 unused trigger route | Fixed: no producer or weapon-data user supplies primary held fire. Removed it from `TankCommand`, the wire edge payloads and answer, server state, client reconstruction, withdrawal reserve and shooting. Primary remains a click; secondary remains a held level. |
| P3 test-only fields | Fixed: removed `ClientInput.applied` and its latency ring; answer IDs/masks and observed effects prove the tests. Click-to-answer latency still has its existing production log. `ArrivalInput.refusal` became the production log-deduplication state above. |
| P3 client weapon gate | Kept with a named user: `crew_ui::update_status_panel` reads readiness, reload/swap countdowns and belt count. Diagnostics also read it through `trace::record_tick`/`trace::hash_tank_state`; replication, disclosure and spawn attachment consume its presence/value. Client maturation grants no firing authority. |
| P3 stale prose | Corrected the death-screen drain contract, complete-body claim comment, external wire-framing description, historical glossary input-delay claim and ADR status/context. ADR-0020's `fire_secondary` routing now agrees with the surviving field. |
| P3 duplicate click ownership | Fixed: feedback reads the verified, warmed `sfx::SfxClips.click` handle. Removed the path-loading helper. |
| P3 wire list | Fixed: both input channels enter `WIRE_TYPE_DEFS`; protocol revision is 104 and `WIRE_TYPES_HASH` is re-pinned to `0x58e2be8e2950233e`. |
| P3 net-15 | **Left and logged**, as expressly allowed by this task. The claim is real: `resync::hold_unknown_history` restores components after Lightyear queues removal for unknown history. Fixing it requires changing the interpolation presence decision, then deleting the observer/marker repair and re-proving first visibility, explicit removal, despawn and resync continuity. This review does not change `resync.rs`; the backlog row remains open. |
| Additional ratchet row net-22 | **Left and logged**: `protocol::apply_servo_angles` reads replicated `ServoAngles` directly in `GameplaySet`; the component is replicated without interpolation, and `tank::servo::drive_servos` assigns those published targets immediately. Remote turrets therefore lead delayed hulls. This is the existing presentation-clock decision tracked beside #187, not an input-arrival regression; the backlog row remains open for that decision. |
| Lens: replica firing tests | Fixed: removed direct client firing cases; retained authority composition failures, client gate-maturation absences and the scheduled proof that replicas never fire. |
| Lens: intermediate edge vector | Fixed: `send_inputs` issues each action directly in the same order. |
| Lens: repeated respawn rationale | Fixed: the detailed overlay race explanation stays beside the guard; the function doc retains its `PlayerInputSet` contract. |
| Lens: `clear_edges` restatement | Fixed: removed the name-as-sentence; retained the once-per-tick consumption invariant. |

## Client clock and presentation handoff

`net::client::run` installs `ClientPlugins` without replacing `InputTimelineConfig`: Lightyear
0.31.0 supplies `InputDelayConfig::no_input_delay()` and default `SyncConfig`. Its local objective
is the remote estimate plus RTT/2, four times jitter and three ticks (DERIVED from the pinned
`lightyear_sync::timeline::input` objective and `SyncConfig` defaults). At zero RTT/jitter this
is a DERIVED three-tick objective lead, 46.875 ms at the shipped 64 Hz; the controller deadband
means this is not a promise of exact realized phase. The arrival-owned device send/admission path
does not target or wait for that clock. `protocol::publish_shot_clock` publishes `LocalTimeline`
to the client's `WeaponClock`, so the status panel's gate maturation runs ahead of the delayed
server presentation. Hulls and server fire/recoil facts render from the server stream; the local
trigger click runs immediately. The own turret still runs ahead under the existing #187 review.

The orchestrator must correct this sentence in `AGENTS.md` (left untouched here):

> Only feedback to the player's own input runs ahead (fire flash and sound, recoil response);
> the own turret still does too, under review in #187.

The immediate fire-control feedback is the trigger click; fire flash/report and barrel recoil
follow server facts. The separately noted HUD gate also matures on the local clock. Interpolation
keeps the stream-derived delay law with zero Gaussian jitter multiplier and a derived half-tick
phase margin (`sync_margin::derived_interp_sync`), independent of the default local clock policy.

Additional cleanup found during verification: the resync design narrative also described client
firing; it now describes server-only firing and the live HUD gate. `InputGap::receive` no longer
returns a classification enum read only by tests; its production counters retain that evidence.

## Validation

MEASURED in this bay, with `OVERMATCH_UNATTENDED=1` and compilation exclusively through
`scripts/serial-cargo`, using `dev_ui,bitprobe`:

| Check | Result |
|---|---|
| `cargo fmt --all --check` | Passed |
| `bash -n scripts/ci/archive-smoke.sh` and installed shellcheck | Passed |
| `net::`, including the protocol pins and all new/restored scenarios | 434 passed, 7 ignored, none failed |
| `tests::doc_citations` | 3 passed, none failed |
| CI clippy: locked workspace, all targets, warnings denied | Passed |
| Full library suite, run once | 2,842 passed, 50 ignored, none failed; 315.26 seconds |

No archive runtime smoke, hosted capture or external service write was performed. The archive
script received the requested static shell checks. The two explicitly logged backlog residues
remain separate from these passing local checks.

## Landing review after the main merge

`main` merged without conflicts; both Cargo files retain release `0.7.0-alpha.19`.
Protocol revision 104 and its wire pin remain correct: no wire type changed.

| Supplied finding | Disposition |
|---|---|
| Harness: missing AWS executable | Fixed: the subprocess boundary converts `OSError` to `Refused`, with the command, OS cause and install/PATH remedy. Offline witnesses cover missing and non-executable commands. |
| Harness: missing bot log | Fixed: file errors reach the existing visible CLI handler. Only a successfully read empty log yields zero bots. |
| Harness: repeated record-refusal rationale | Fixed: the marker comment retains its source and the `server_stats:` counterexample; the module doc owns the rationale. DERIVED: one comment line removed. |
| Harness: fractional view window | Fixed: the fixture's whole-second journal cadence determines histogram spacing; a fractional-window witness checks containment and non-overlap. |
| Recovery copy budget | Fixed: required `recovery_fresh_copies` in `assets/net_timing.ron` is labelled `Taste` for input recovery tolerance, loaded before play, and positive. Classification and saturation read that value. A witness includes the `u8` limit; no code default exists. |
| Repeated trigger-routing prose | Fixed: removed the inline repeat; `TankCommand` and `fire` retain their owning docs. DERIVED: four comment lines removed. |
| Client teardown intermediates | Fixed: both assertions use `app.world()` directly. DERIVED: two bindings removed. |

All seven findings were verified and fixed; none was rejected. As requested, the second report's
pre-existing findings remain: capture phase durations in `command::auto_fire`, settling/aim-hold
timings in `tank::battle`, and padded overlay-guard prose in `death_screen::request_respawn`.
The previously recorded net-15 and net-22 decisions also remain untouched.

The producer audit verified every consumed server field against `server_stats::stats_line` and
its census producers, and every consumed client field against `frontier::record_jitter` and
`ViewClock::take_line`. The obsolete disconnected-input branch is removed: the server emits only
connected input rows. Perf fixtures now include the unconditional debris census and current
arrival shape, with no retired-count fallback.

MEASURED after merging main, before the landing fixes: the full library suite passed once,
2,846 passed, 50 ignored, zero failed in 407.30 seconds. Formatting, CI clippy, 434 net tests,
three doc-citation tests, both Python suites and shellcheck of `scripts/aws/machine.sh` passed.

MEASURED on the landing fixes, under the same unattended serial-cargo feature set:

| Check | Result |
|---|---|
| Formatting and CI clippy | Passed |
| `net::`, including protocol pins and the copy-budget witness | 435 passed, 7 ignored, zero failed |
| `tests::doc_citations` | 3 passed, zero failed |
| `python3 scripts/aws/test_harness.py` | 23 passed |
| `python3 scripts/perf/test_online_cost.py` | 16 passed |
| Supplied fractional-window counterexample | The offline whole-run view-clock test passed with a temporary `view_window_s = 1.5` policy file |
| `shellcheck scripts/aws/machine.sh` | Passed; no shell script was edited |

The full library suite was not repeated after these fixes; their changed Rust behavior is covered
by the final net tests. All Python subprocess boundaries used offline fakes. No AWS calls or push
occurred. `AGENTS.md` is unchanged.
