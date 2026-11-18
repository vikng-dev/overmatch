# Own turret on the server stream — review corrections

Subject: the game's turret presentation, netcode and damage-model presentation; game state only.
Continues `0514f045` on `lane/own-turret-on-stream`. The complete independent review was read
and every finding checked against source. All findings were confirmed; none was silently skipped
or refuted. This report supersedes the earlier lane report; git retains its baseline measurements.

## Findings and decisions

| Finding | Disposition | Verified cause and result |
|---|---|---|
| P1-1 | Fixed | `ServoAngles` carried only gunner yaw/pitch although aiming lays every declared mount. It now carries every servo angle in the template's deterministic slot order. The receive codec checks finite angles and the data-owned count before interpolation. A typed refusal closes the match and tells the player why. The stream proof checks every servo node and the muzzle on own and remote tanks. Player and bot initial snapshots take their complete pose from the template's initial mechanism state, before any fixed publication tick; the empty wire default is deleted. |
| P2-2 | Fixed | `drive_servos` queried replicas, returned before integrating them, and rewrote their unchanged poses. Its tank query now requires authority mechanism state, volumes, construction and crank speed directly; the replica arm, `Or`, optional/expect pairs, copied panic and stale crank message are gone. The three tests pinning that dead path are deleted. Hand-servo and aim fixtures now provide complete authority construction/crank state; the aim fixture uses the production state constructor. |
| P2-3 | Fixed | `RemoteServos` held fake endpoints and velocity. `StreamedServoPose` now holds angles only and attaches from the received `ServoAngles`. `ServoState::presented` is deleted. Replica trace/hash inputs contain sampled angles; authority rows retain actual endpoints and velocity, so their servo hashes describe different state shapes. Impossible replica integrator fixtures are replaced. |
| P2-4 | Fixed | The view-overstep proof now targets `TankServos`: offline play and the sandbox must interpolate the shortest arc across the wrap seam, rather than take current directly or blend the long way. |
| P2-5 and P3 cousins | Fixed | ADR-0037's original ruling and historical statements are restored from the lane's main baseline. Its dated `Amended 2026-11-18 (#187)` paragraph records the owner decision and the full uplink + server slew + view-delay trail. ADR-0030's supersession notice and ADR-0038's own-turret evidence row are restored and annotated. |
| P3-6 | Fixed | The rulebook, motion contract, ADR amendment and camera comment distinguish local third-person camera/intent cursor/aim dot/trigger click from the streamed optic eye/up, blended optic look and streamed ranging graticule. No camera taste is changed. |
| P3-7 | Fixed | `DeviceCommand` snapshots before the fixed hold. The client hold serves the crew panel's drive target and `MatchCommands`; the battle code and design doc now say that. |
| P3-8 | Fixed | Sight scheduling, recording and protocol comments now name sampled replica state. The sight margin's convex travel-window rationale and rejected re-pin rationale are restored. The servo-law design narrative names the angles-only pose and all-slot stream. |
| P3-9 | Left, as directed | **Unknown:** a second tank's narrow optic and faster traverse may bind the margin against delayed lay, causing stop-go intent. DERIVED condition: slew rate × (uplink + view delay) ≥ optic margin; mixed sampled-state/restored-ancestry ages also contribute. Settle with the second tank's admitted optic geometry and traverse data, representative uplink/view-delay evidence, and a dedicated playtest of sustained traverse. No timing or capture run was performed here. |
| P3-10 | Fixed | The test-only `TankTemplate::transmission` accessor is deleted; the fixture reads the spawned authority's crank value. |
| P3-11 | Left, as directed | `ServoCommand` remains the live attached-servo marker consumed by view binding and integrity handling; its replica target needs no reader. |
| Lens ratchet | Fixed | `drive_gunner_aim` no longer retells the retired `toggle_sight` seed-on-entry story or recaps its rule; the commitment and sight-line seeding rationale remain. The serial `drive_aim_servos` finding is outside this lane and untouched. |

## Wire verdict

Revision **107**. Angles retain the existing unquantised `f32` representation. The tank template
owns the count and sorted-name slot order; no mount or vehicle name appears in presentation logic.
The current composition admits one `TankTemplate`; its data-owned slot count reaches the receive
codec before replication intake. Malformed counts never reach interpolation. The registered codec
and production lobby refusal are witnessed over real UDP, including no reconnect after refusal.

DERIVED from Postcard's sequence-length varint and four bytes per `f32`: a full servo component is
`4 × N + ULEB128_length(N)` bytes per tank per tick. The fixture rig declares four slots, so its
payload is **17 bytes**, formerly **8 bytes**: **+9 bytes per tank per tick**. These are component
payload bytes, excluding entity/component headers, packet framing and unchanged-component omission.
The byte count is also asserted by the protocol round-trip test; no bandwidth capture was run.

`WIRE_TYPES_HASH` is re-pinned to **`0x3ae1a75b22a96448`**. The ordered registrations are
unchanged, so `WIRE_SURFACE_HASH` remains **`0xb35d6c7c3c7bbe9e`**. The first protocol run
passed its other tests and failed the expected type-hash tripwire before this re-pin.

Source anchors (current tree): `src/net/protocol.rs:117` owns the vector wire layout;
`src/net/servo_wire.rs:30` owns receive admission; `src/tank/servo.rs:233` owns the
angles-only pose; `src/tank/servo.rs:373` owns authority stepping;
`src/tank/servo.rs:1203` proves the shortest-arc overstep;
`src/sight.rs:329` reads the sampled pose and `src/sight.rs:544` clamps the optic margin;
`src/damage_cam/record.rs:253` records either source's real angles.
P3-11's marker consumers are `src/tank/view.rs:54` and `src/tank/integrity.rs:26`.
No original finding was refuted.

The first full suite exposed incomplete hand-servo aim fixtures and an empty server init pose;
both are corrected above. It also exposed a pre-existing test conditioner ordering gap:
`lightyear_udp 0.31.0`'s endpoint receives in the whole `LinkSystems::Receive`, while the
input-arrival conditioner was ordered only after the `ApplyConditioner` subset. Four packet-loss
proofs reproduced the failure in isolation. The conditioner now follows the whole receive set
and precedes `ConnectionSystems::Receive`, so it sees the server packets before consumption.
Production input behavior is unchanged by that test-fixture correction.

## Verification

MEASURED in this bay, compilation exclusively through `scripts/serial-cargo`:

| Check | Result |
|---|---|
| `cargo fmt --all --check`; `git diff --check` | Passed |
| Locked workspace/all-target Clippy, `dev_ui,bitprobe`, `-D warnings` | Passed, no warnings |
| Targeted protocol, servo, rig, codec, stream, sight, spawn, template, trace, recording, battle, aim, server, hosted-join and input-arrival filters | 181 passed; one existing hosted instrument ignored |
| `check --no-default-features --bin overmatch-server` | Passed; two pre-existing unused client audio-metadata warnings |
| `check --release --bin overmatch` | Passed, no warnings |
| Final full library suite, `dev_ui,bitprobe` | 2,870 passed; zero failed; 51 ignored |

The first full run completed with 2,861 passed, eight failed and 51 ignored. The failures exposed
the incomplete aim fixtures, empty initial wire pose and packet-loss conditioner ordering gap
recorded above; the focused reruns pass after those corrections. Test linking reports the existing
macOS `__eh_frame` size warning. The server-only warnings name `Weapon::report_clips` and
`EngineSound` audio fields; these remain client audio inputs.

No timing, capture or screen access, no GitHub comments, no push. The orchestrator retains
responsibility for independent six-lens review before merging; this lane does not merge or
release its bay.
