# Own turret on the server stream — closing corrections

Subject: the game's turret presentation, netcode and damage-model presentation; game state only.
Continues `0514f045` and `c2d61eae` on `lane/own-turret-on-stream`. Each requested finding was
checked against source before editing. The earlier report remains in git.

## Item dispositions

| Item | Disposition | Evidence and result |
|---|---|---|
| 1 | Fixed | `src/tank/servo.rs:380` queries authority state with optional volumes, mechanism and crank speed. Missing volumes panic with the spawn invariant; powered slots alone require `ServoMechanism` and `EngineRpm` (`:413`, `:429`). The tail skips positively identified streamed roots and panics for any other missing authority (`:450`). Tests at `:777` and `:811` cover broken construction and hand-only operation. The aim fixture now carries only its explicit servo state (`src/aim.rs:853`), without `authority_state()` padding. |
| 2 | Fixed; full-turn premise refuted | `src/tank/servo.rs:589` accumulates continuous travel without wrapping; its test at `:838` requires that convention. A full-turn receive bound would refuse valid server state. `src/net/servo_wire.rs:48` instead admits finite unwrapped f32 angles for continuous slots and the data-owned inclusive stops for limited slots. Outside input beyond those stops reaches typed `ServerGone::Servo(OutOfTravel)` before history. `src/net/protocol.rs:755` uses an f64 weighted convex blend, so even admitted opposite finite f32 extremes stay finite and endpoints remain exact. This changes blend rounding; it adds no angle quantisation or travel cap. |
| 3 | Fixed | The duplicate test decoder and `check_count` wrapper are deleted. Production registration and unit tests call the single `admit(bytes, slots)` owner (`src/net/servo_wire.rs:48`, `:74`). The installed production codec and matchmaking lifecycle are witnessed over real UDP with wrong counts, a correctly sized non-finite pose, and a correctly sized out-of-travel pose (`src/net/servo_wire/tests.rs:14`), including visible lobby cause and no reconnect. |
| 4 | Fixed | `src/net/servo_wire.rs:27` mounts receive systems only when Replicon's client plugin is present. Rig slots and limits publish on `TankTemplate` addition, rather than being reinserted each frame (`:37`). |
| 5 | Fixed | `src/net/rig.rs:5` says every servo; the redundant `With<ServoAngles>` and comment are gone. State-source variables in servo, sight and trace now say `streamed`, including the own tank's pose. |
| 6 | Fixed | `src/sight.rs:325` retains the optic-clamping reason and points at `selected` for ownership. Trace drops the repeated source mapping (`src/trace.rs:548`). The shallow `servo_rotation` wrapper is deleted and all callers, including its test caller, use `servo_pose`. DERIVED: nine comment/helper lines removed by these three cleanups, before other changes. |
| 7 | Fixed | `.agents/AGENTS.md:74` owns the exact requested One-timeline principle. The motion contract and ADR-0037 amendment point there; ADR-0030's amendment points at ADR-0037 instead of repeating it. ADR-0037's amendment mentions the game's tracer once. Historical ADR rulings remain historical. |
| 8 | Left for #267, report only | The composition still has one global rig (`src/net/servo_wire.rs:12`, `:43`). A second tank type with a different slot count would have every pose refused (`:56`). The unconditional claim that *any* second type is refused is too broad: an equal-count rig could pass the count check and have the wrong slot limits applied (`:62`). Per-tank rig admission belongs to multi-tank work, not this lane. |

DERIVED numerical safety: limited samples lie within their authored stops; continuous samples are
finite f32 unwrapped travel. Lightyear 0.31.0's `InterpolationSampleContext::from_ticks`
(`lightyear_interpolation/src/rules.rs`) clamps the fraction to `[0, 1]`. The f64 weighted blend
therefore stays within the finite f32 endpoint interval. The admitted-extremes witness also checks
its finite midpoint and exact endpoint (`src/net/servo_wire/tests.rs:205`). No invented full-turn
bound or change to the server's continuous mechanism is introduced.

The earlier lane's deferred narrow-optic playtest question and `ServoCommand` attachment-marker
decision remain unchanged; this correction adds no tuning or capture evidence.

## Wire verdict

Revision **107**, as requested for admission-only changes: no `WIRE_TYPE_DEFS` definition changed.
`ServoAngles` and `ServoAnglesWire` retain their existing `Vec<f32>` layout and serde attributes.
`WIRE_SURFACE_HASH` remains **`0xb35d6c7c3c7bbe9e`**.

The type-hash tripwire also folds whole codec sources, including `servo_wire.rs`
(`src/net/protocol.rs:1888`). Its first run caught that admission-source change, despite unchanged
wire types. `WIRE_TYPES_HASH` is consequently re-pinned to **`0xfdf12b8afdcf2c7e`**; the handshake
fingerprint changes with it. Revision remains 107 under the explicit admission-only instruction.
The final protocol run passes both pins. No bump to 108 is needed.

## Verification

MEASURED in this bay; all compilation ran through `scripts/serial-cargo`:

| Check | Result |
|---|---|
| `cargo fmt --all --check`; `git diff --check` | Passed |
| `clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings` | Passed, warning-free |
| Targeted servo, aim, sight, trace, rig, protocol, servo-wire and stream-presence tests | 102 passed, zero failed |
| `check --no-default-features --bin overmatch-server` | Passed; two pre-existing unused client-audio-field warnings in `Weapon::report_clips` and `EngineSound` |
| `check --release --bin overmatch` | Passed, warning-free |
| `test --lib --features dev_ui,bitprobe` | 2,873 passed, zero failed, 51 ignored |

The first Clippy run found a test call to the removed wrapper; it was inlined before the passing
run. The first protocol run failed only the codec-source hash tripwire, resolved as recorded above.
Test linking retains the existing macOS `__eh_frame` size warning. Server-only audio warnings are
outside the turret/netcode change; the prior lane report already recorded them.

No timing runs, screen access, GitHub comments or push. The orchestrator owns independent six-lens
review, merging and bay release.
