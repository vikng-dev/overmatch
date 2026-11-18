# Own turret on the server stream — final lens follow-up

Subject: the game's turret presentation, netcode and damage-model presentation; game state only.
Continues `63d675ee` on `lane/own-turret-on-stream`. The preceding corrections and their evidence
remain in that commit's report. Each requested item below was verified against source.

## Item dispositions

| Item | Disposition | Evidence and result |
|---|---|---|
| 1 | Fixed | `src/net/rig_admission.rs:31` now admits any declared servo ancestor, matching the complete servo stream published at `src/net/protocol.rs:661`. The gate remains because `TankSpec::articulated_nodes` (`src/spec.rs:1807`) also includes recoiling barrels and roadwheels, which have no parent-local pose in `ServoAngles`. The production-plugin test at `src/net/rig_admission.rs:50` admits servo ancestors above either aim axis and still refuses roadwheel ancestors. |
| 2 | Fixed | `TankTemplate::servo_travel_limits` (`src/tank/template.rs:394`) reads the assembled servo entries in the same order that `assemble` assigns `ServoIndex`. `ServoSlots::from_template` (`src/net/servo_wire.rs:15`) collects those limits without reconstructing or sorting spec entries. The test at `src/net/servo_wire/tests.rs:12` compares receive limits against the spawned servos' indices and data-owned stops. |
| 3 | Fixed | `src/tank/spawn.rs:151` now contains the requested two-sentence comment, with the repeated opening removed. |

Decision: retain admission for non-servo moving ancestors, rather than deleting the entire gate.
No finding was refuted. DERIVED: existing slot assignment and travel limits are unchanged because
both the removed reconstruction and `resolve_rig_nodes` sort the same unique servo names; the
receiver now consumes the assembled order directly.

The whole-root launch guards in `publish_servo_angles` and `net::rig::present_aim`, and the serial
`drive_aim_servos`, are untouched. Earlier deferred multi-tank admission (#267), narrow-optic
playtesting and the `ServoCommand` attachment-marker decision remain unchanged.

## Wire verdict

Revision **107**, as requested: no wire type definition, serde attribute, registration or angle
encoding changed. `ServoAngles` and `ServoAnglesWire` retain their `Vec<f32>` layout.
`WIRE_SURFACE_HASH` remains **`0xb35d6c7c3c7bbe9e`**.

The whole-source codec fold includes `servo_wire.rs`, so removing its duplicate sort changed that
hash despite unchanged wire types. The first protocol run passed its other tests and reported the
new pin. `WIRE_TYPES_HASH` is re-pinned to **`0xb12be3bccccba258`**, changing the handshake fingerprint.
The final protocol run passes both pins; revision remains 107 under the explicit instruction.

## Verification

MEASURED in this bay; every compile ran through `scripts/serial-cargo`:

| Check | Result |
|---|---|
| `cargo fmt --all --check`; `git diff --check` | Passed |
| `clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings` | Passed, warning-free |
| Targeted `net::rig_admission`, `net::servo_wire`, `tank::template`, `tank::spawn`, `net::protocol`, each with `test --lib --features dev_ui,bitprobe` | 36 passed, zero failed; protocol accounts for 17 |
| `check --no-default-features --bin overmatch-server` | Passed; the two previously recorded unused client-audio-field warnings in `Weapon::report_clips` and `EngineSound` remain |
| `check --release --bin overmatch` | Passed, warning-free |
| `test --lib --features dev_ui,bitprobe` | 2,874 passed, zero failed, 51 ignored |

Test linking retains the previously recorded macOS `__eh_frame` size warning. The initial protocol
failure was solely the codec-source hash tripwire, resolved by the re-pin above. Verification logs
are under `target/own-turret-followup-*.log`.

No timing runs, screen access, GitHub comments or push. The orchestrator owns independent six-lens
review, merging and bay release.
