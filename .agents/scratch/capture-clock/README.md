# Capture clock — #385

The game's capture timeline, including its damage-model presentation cues (game state only).

The root was `src/dev_capture.rs:220`: the plan selected a fixed frame step immediately,
while frames and cues read app elapsed time (`src/dev_capture.rs:532`).
`src/tank/scenario.rs:526` waits for asynchronous presentation assets before placing tanks.
`state::sim_plugin` gated gameplay systems during Loading, but neither the frame clock nor the
fixed-time accumulator stopped. A variable loading-frame count therefore changed the tank's age
at a plan second.

The chosen origin is placement readiness. `src/dev_capture/clock.rs:33` holds real, virtual
and fixed time at zero before the composition's anchor, and gates frame/key/event cues, including
zero-time cues. It restores the composition's selected pacing before Bevy's time update when
ready. A fresh real clock excludes loading wall time from automatically paced plans as well.
The offline anchor includes bound views so presentation attachment cannot consume a simulation
step. There is no operator switch or fallback to the former origin.

| Plan kind / composition | Anchor |
|---|---|
| Offline duel, named/coordinate starts, target scenarios, probe grids, battle, LOD showcase | `Playing` and every tank view bound; the tanks and map are placed before any fixed tick |
| Track-prints demo | Same offline anchor; its later placement and drive/view phases read this timeline |
| Hull-marks demo | Same offline anchor, including the parked target's bound view |
| Offline impact, wall, repair, corner-replay and shot cues | Same offline anchor; shot screenshots still count from the actual burst |
| Armour sandbox, including range-wall scripts | Placed target view bound |
| Track sandbox | Published `RigGeom`, which is the rig-construction transaction's latch |
| Windowed online client plans | Startup UI/world placement, so Join/connect captures need no possessed tank |
| Dedicated-server wall plans | Startup world placement on the server's native tick-runner clock; its primed real clock and admission strategy are preserved |

Online arrivals are live input, not a replay. Plans without a fixed frame step retain wall-paced
sampling after their anchor; the byte-repeatability evidence below is for fixed-step offline
plans. Plans without frames still acquire their composition's anchor.

No tracked plan cue or frame time changed. Loading is excluded before subjects are admitted, so
none of the existing authored cues moved ahead of its subject. The private proof-plan copies
change only `frames.out` to the cache.

The numeric ratchet in the edited capture module also removes the assumed impact-ray ceiling:
ray range derives from terrain collider bounds, with a Resolution clearance for stable normals.
The existing impact test now uses elevated fixture terrain and derives the expected surface from
its construction. Capture settling, write deadlines and cosmetic sampling carry Resolution labels.

## Repeated-frame proof

All results here are MEASURED. Machine/backend: Apple M4 / Metal (run logs).
Every run used `target/debug/overmatch --offline`, the same binary and the same plan file for its
three repetitions, `OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1`, and its own
initially empty `OVERMATCH_CONFIG_DIR`. Both plans request 1280x720 at scale 1, with an authored
fixed step of 0.015625 s. `w1d-wall` supplies forward input and an orbit camera.

Every requested frame was compared: one per plan per run. PNG dimensions were checked, all
processes exited successfully, and both scenes were visually inspected. SHA256 of PNG bytes:

| Plan/frame | Run 1 | Run 2 | Run 3 | Verdict |
|---|---|---|---|---|
| `hull-closeup`, authored 12 s | `ae7579c5465256a1a1d71aa71e414a0c0a7bc8d891b0596914d50794d62dd223` | same | same | MEASURED identical |
| `w1d-wall`, authored 12 s | `169630fa8e7a517d6ab1a6cf53ea41c6dd051352cca018aba03c8994ac58939d` | same | same | MEASURED identical |

Binary SHA256: `fbaa8191f6ba5fd15833a039c934b5229227d82833bf12609fcdce4ec579fc60` (verified unchanged after all runs).

Evidence remains outside git under `/Users/Yan/.cache/overmatch/capture-clock/`:
`proof.json` records binary/plan/frame hashes, flags, config paths, exit statuses and measured
wall durations; `*-run[123].png` are the six frames; `*-{1,2,3}.log` are the run logs.
`run-proof.py` is the runner, and the two `.ron` files are its output-path-only plan copies.

## Gates and review

Passed on the final source:

- `cargo fmt --all --check`
- `scripts/serial-cargo clippy --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`
- `scripts/serial-cargo test --lib --features dev_ui,bitprobe dev_capture`: MEASURED 32 passed.
- Capture binary: `scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch`.
  The macOS linker emitted its unwind-table size warning; the build succeeded.

The clock test checks a bound view while still Loading, Playing while the view is unbound,
zero cue execution and zero elapsed real/virtual/fixed time before readiness, and resumed pacing
after readiness. Its consumer is before/after captures; it pins no rendered frame or historical
time result. The startup test protects the server runner's clock ownership.

Self-reviewed under all six lenses: explicit waiting state and invariant failures; one clock
origin per composition; generic readiness and collider-derived bounds; one owner for clock
policy; invariant-based tests; bounded readiness/cue gates preserving server tick ownership.
No backlog row names an edited file. No agents, external comments, pushes or merges were used.

## Open observations

No remaining frame nondeterminism was observed in these plans. The crew status panel remains
visible despite `ui: false` in both proof scenes; the existing `hide_ui` implementation was not
changed by this lane. Live network arrivals and wall-paced sampling remain outside fixed-step
replay guarantees.
