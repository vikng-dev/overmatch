# Own turret on the server stream

Subject: the game's turret presentation and netcode; rounds are game objects and the game's damage model is game state only.

Own and remote attached turrets now present the same `ServoAngles` at the hull's view cursor.
The barrel trails the player's aim by delivery plus the view delay; the tracer leaves the
streamed barrel. Server servo laws, input transport and authored vehicle data are unchanged.
No timing, capture, screen or audio run was performed.

## Removed

- `net::rig::present_aim`'s controlled-tank exclusion; the stream poses both ownership cases.
- The controlled-replica integration arm in `tank::servo::drive_servos` and its mutable access
  to `RemoteServos`. Only authoritative `TankServos` integrates; replicas only supply poses.
- Replica `ServoMechanism` construction in `attach_replicated_tank_body`. Its transmission
  accessor now exists only for the spawn test that reads it.
- Replica command consumption in `aim::drive_aim_servos`, now selected by `TankServos`.
- The harness's copy of outgoing aim/range into the own tank's client servo command.
- `the_controlled_replica_integrates_its_own_aim`, the owner-replica coast test arm and its
  run-comparison bookkeeping, `a_replica_view_interpolates_its_remote_state` (local fixed-step
  smoothing), and `a_scripted_client_lays_its_own_tank_like_a_player`.
- `missing_remote_slot` and the volume relationship it created only for client integration.
- The fire harness's `gun_yaw_minus_cursor_yaw_deg` field, its computation and lead columns.
  The fire/tracer/muzzle-coherence and pivot diagnostics still have users and remain.

No files were deleted. No turret prediction/rollback registration, reconciliation or local-to-stream
smoother remains to remove: protocol registration already interpolated `ServoAngles`, and the
client mounts no prediction manager. The stale restoration/trace comments now name servo state.

## Local input and shared presentation

`camera::OrbitCamera`, `aim::CommittedAim`, device commands, ranging, mouse-authored
`sight::GunnerIntent`, aim reticles and the trigger click remain local input feedback. The
optic camera still computes its existing stateless blend of committed intent and the drawn
sight line: its eye and gun-side bearing now read the streamed gun. The optic margin reads the
sampled lay; no client servo moves it. The camera/intent state never becomes turret state.

`aim::update_bore_indicator` and `sight::reticle::update_ranging_reticle` already resolve view
nodes, so actual-pointing marks now read the streamed gun. The intent reticle still projects
`CommittedAim`. `client::present_released_shot`, cursor-held fire events and pending recoil
kicks already serve own and remote shots alike; muzzle, tracer and recoil needed no ownership
branch removed. Their source remains the server answer.

`RemoteServos` remains the shared replica presentation store; `ServoState::presented` gives
identical blend endpoints and zero velocity. Authority/offline servo interpolation and its laws
remain. Replica `ServoCommand` components still identify live view bindings in `bind_tank_view`
and are removed by detached-assembly integrity handling; they have no client drive consumer.
General servo state-hash/trace fields still diagnose server state and sampled poses. Aim/sight
sensitivity and camera-blend data still serve local input feedback. None serves a local turret
integrator. These are current users; nothing is blocked or deferred.

## Docs and wire

Updated the rulebook's one-timeline rule, ADR-0037, the motion contract, ADR-0030's supersession
notice, ADR-0038's consumer table, the battle-cycle and servo-law narratives, the sim-body glossary
and affected comments. The net debt report's own-turret row is resolved. Historical captures and
superseded ADR bodies retain their original evidence; `turret-cursor-3a.md`,
`turret-remeasure-2026-11-18.md` and `input-arrival-review.md` describe their recorded baselines.

No wire type definition or registration changed. `WIRE_TYPE_DEFS`, its hash and `PROTOCOL_REV`
remain unchanged at revision **106**.

The unrelated release warning has two real users: `TrackGear::bitprobe_startup` under
`bitprobe`, and the test-only track-scorecard turn probe. `steering_speed_at_governor` is kept
under `cfg(any(test, feature = "bitprobe"))`.

## Evidence

The production stream/barrel proof now covers own and remote rigs, fractional cursors,
holds and a forward cut using `Timing::view_fixture()`. The fixed-step proof rejects moving
either ownership case from a command or stored momentum; the presenter proof covers both and
the game's launched-assembly exception. Generic authority coast and aiming laws remain tested.

DERIVED from `git show 0a1d9e0b:<path>` and `git diff --numstat`, physical lines before/after:

| File | Before | After | Net |
|---|---:|---:|---:|
| `scripts/shot/run-turret-cursor.py` | 485 | 479 | -6 |
| `src/aim.rs` | 1607 | 1607 | +0 |
| `src/net/battle.rs` | 398 | 398 | +0 |
| `src/net/harness.rs` | 1771 | 1722 | -49 |
| `src/net/protocol.rs` | 2334 | 2333 | -1 |
| `src/net/rig.rs` | 176 | 166 | -10 |
| `src/net/tests/stream_presence.rs` | 624 | 653 | +29 |
| `src/sight.rs` | 1038 | 1036 | -2 |
| `src/tank/servo.rs` | 1396 | 1313 | -83 |
| `src/tank/spawn.rs` | 756 | 755 | -1 |
| `src/tank/template.rs` | 1430 | 1431 | +1 |
| `src/trace.rs` | 744 | 744 | +0 |
| `src/track/transmission.rs` | 2290 | 2291 | +1 |

All modified existing files, docs included: **19466 → 19342**, net **-124** lines
(DERIVED, excluding this new report).

## Checks

MEASURED in this bay, compilation exclusively through `scripts/serial-cargo`:

| Check | Result |
|---|---|
| `cargo fmt --all --check`; `git diff --check`; Python runner syntax | Passed |
| Locked workspace/all-targets Clippy, `dev_ui,bitprobe`, `-D warnings` | Passed |
| Targeted servo, rig, stream-presence, harness, protocol, aim, sight, spawn, template and transmission tests | All passed; 223 tests across the module filters |
| Default-feature `check --release --bin overmatch` | Passed, no warnings |
| `check --no-default-features --bin overmatch-server` | Passed; two existing unused-audio-metadata warnings |
| Full library suite, `dev_ui,bitprobe`, run once | 2,870 passed, 51 ignored, zero failed |
| Final fixture/comment cleanup: servo, spawn, rig, sight and stream-presence tests | 64 passed, zero failed; Clippy and both configuration checks repeated successfully |

The full run preceded the final removal of unused test-fixture volume setup and corrections to
integrator wording; the focused pass above verifies those final edits. Test linking reports the
macOS `__eh_frame` size warning. The server-only warnings name `Weapon::report_clips` and
`EngineSound` audio fields, which remain client audio inputs and are outside this cleanup.
