# Handoff — MP correctness slice, and the predict-everyone fork

For: Yan + a fresh session. State as of 2026-07-09. Branch `mp-correctness-slice`, 8 commits,
**not pushed, not merged**. Gates green both feature sets; both net bins build. Delete this file when
consumed.

## The one decision waiting for you

**Predict non-owned tanks, or don't. Current recommendation: DON'T** — reversed on 2026-07-09 after
research, having first argued the opposite in this very file. See §4, which now records why. The
contact-restore *gate* is genuinely clear (§3); the *case for walking through it* collapsed.

## 1. What landed

| Commit | What |
|---|---|
| `71987cf` | Barrel recoil derived from `FireEvent`, not replicated |
| `701d0a7` | **The real bug**: starved input streams re-latched command edges |
| `0fa6cd8` | Named the tick-alignment invariant `apply_net_health` relies on |
| `da1eaf5` | ADR-0016: replicate causes, derive consequences |
| `2907d75` | `.cursorignore` (indexer, not rust-analyzer — the comment says why) |
| `7a93a98` | `.vscode/settings.json`: editor stops spawning compilers |
| `ca54288` | Retired the contact-restore divergence term **and the metric behind it** |
| `1b8df19` | Review fixes — two of the three correctness bugs were introduced by this slice |

**The only genuine bug in the multiplayer slice was the input-edge starvation.** lightyear extrapolates
a starved input stream by holding the last `ActionState` forever (`get_predict` → `get_last()`), and
the native path's `decay_tick` is an unoverridable blanket no-op. `TankCommand`'s *edges*
(`fire_primary`, `crew_swap`) were re-latched every tick, defeating `consume_edges`. A client whose
uplink starved on the tick it fired made **the server fire an unrequested shot per reload cycle**; a
held `crew_swap` re-armed itself every `SWAP_SECONDS` without bound. Server-side only (the client's
own tank authors an input every tick; remote tanks carry no `ActionState`).

Barrel recoil was a **missing feature, not a defect**, and became the worked example of ADR-0016.

## 2. Three things I asserted that were wrong

Recorded because the pattern matters more than the items.

1. **"`NetHealth` tick-staleness manufactures divergence."** It does not. State rollback runs
   `RollbackMode::Check`, which starts every rollback at `last_confirmed_tick` *and only on a mismatch
   there* — and the tank matches at pre-death ticks. No replay window begins before the death tick. The
   fix I proposed would not have worked anyway: the drive gate rides the `Dead` marker, never rolled
   back, not health. Retracted; the test that "confirmed" it was deleted.

2. **"Replay contact re-formation is the gate before predicting remotes."** Stale document. The
   `hc=0` finding was superseded the same day it was written (`SPIKE_CONTACT_PROBE`, `8a08d60`) and
   fixed (`AuthoredLocalTransform` shield, `33cc4e4`) — but §5's prose never caught up, and ADR-0015
   repeated it as a *measured* ranked cause. My warm-start hypothesis was refuted three ways.

3. **"Drain the `FireEvent` on the fixed clock."** Would have dropped every message arriving on a
   zero-tick frame — `lightyear_messages` clears undrained receivers in `Last` (`plugin.rs:161`).

Each was caught by an agent told to disbelieve me. **Two of the three correctness findings in the final
review were bugs this slice introduced**, both of them doc comments asserting something the code did
not guarantee. That is the failure mode ADR-0016 names.

## 3. Divergence: the gate is clear

Re-measured 2026-07-09 (design doc §6). The raw `hc=0` rate **went up** at 80/10 (55% → 100%) — and it
does not matter, because **the metric never discriminated the defect**: it conflates "no hull contact
because the tank rides on its wheels or is airborne" (correct, common) with "contact failed to re-form".

The discriminating metric — client `hc=0` while the server holds `hc>0` — is **0 across all 88
server-joined replay ticks**. Contact re-forms wherever the hull is genuinely grounded. `leaf_dvg =
0.000000`; the 2.8 m proxy levitation is gone. Solo rollbacks are **2–4 per 20 s run**, down from a
storm. ADR-0015's ranked cause #2 is **retired, not replaced** — no term was promoted, because no new
ranking was measured.

**No contact-restore barrier to predicting non-owned tanks remains.** One *other* barrier does, and it
is Layer-1, not netcode: ADR-0015's continuity rule binds tank-tank contact, and sharp oriented boxes are
its named bad class. Two mispredicting bodies meeting at a sharp edge bifurcate rather than converge.
Check what the hull collision proxies actually are before any two tanks touch. This is worth doing
regardless of the §4 decision — it is a determinism prerequisite too.

## 4. The fork: predict-everyone — recommended AGAINST

An earlier revision of this file argued *for* it, on the grounds that tank-tank collision requires it.
Research (2026-07-09) refuted that. Recorded honestly, because the argument is seductive and someone
will re-derive it.

**What is still true.** Prediction cannot make an opponent less latent — their input arrives with their
state. What it does is put them on *your* timeline. Today your tank runs ~½ RTT *ahead* of the server
and the opponent ~1.7 send-intervals *behind* it; they occupy your physics world at different instants.
That misalignment is real and it is why a ram feels wrong.

**Why predicting them does not fix it.** War Thunder ships exactly this model — extrapolate remote
vehicles from last-known controls — and says on the record: *"The main (and obvious) exception where it
does not work so well is physical collisions between vehicles… there is actually no good online
solution for a colliding solution anyway."* With two predicted Dynamic bodies, **both** mispredict, and
each feeds the other's contact solver. Fiedler's answer to two clients contesting one contact is
single-authority-per-contact, not free collision. Epic's Chaos "replicate velocity and let the client
collide" mode is documented as *"does not handle interactions gracefully."* Nobody ships two freely
colliding predicted bodies without determinism — and Rocket League (fixed-tick Bullet) and Photon
Quantum (fixed-point) have it; avian across two worlds does not (ADR-0015: bit-exact only in flat
cruise). **Predict-everyone relocates the artifact and buys a resim bill.**

**And hit feel is settled the other way.** Overwatch's GDC 2017 deck lists what the client rolls back:
local entity state, input, aim, poses. Explicitly excluded: *"Variables and states on remote Entities"*
and *"Data from other Entity Components (such as for health)."* They ship a `Suppress Movement
Prediction` node for knockback. The rule is: predict effects you **author**; replicate effects authored
**against** you. This corroborates memory `mp-hit-feel-view-layer`.

**Three hard constraints found in source, if you ever revisit it:**

1. **The bot cannot be predicted.** It has no `ControlledBy` and no client authoring its input;
   `drive_bot` writes `TankCommand` directly server-side, so no input message names it, so lightyear
   never inserts `ActionState` on the client replica, so a predicted bot coasts on a default command.
   Only a `HostClient` can rebroadcast server-authored inputs. Predict-everyone is really
   **predict-every-player**, mixed mode — so `ServoAngles` and `FireEvent` are **NOT deletable**. An
   earlier revision of this file claimed they were. Wrong.

2. **Reliable remote fire requires input rollback; input rollback breaks `apply_net_health`.**
   `run_rollback` replays from `rollback_start_tick + 1`, so a remote's fire at the confirmed tick is
   never re-executed — with input rollback off, derived remote fire is unreliable by construction. With
   it on, rollbacks target `last_confirmed_input.tick` (`Always`) or `earliest_mismatch - 1` (`Check`),
   neither gated on state confirmation, so a replay window can precede a death tick and
   `apply_net_health` writes post-death HP onto pre-death ticks. You cannot have both. (Note: **state**
   rollback in either mode always starts at `server_confirmed_tick` and is safe; and predicting more
   entities does NOT widen the frontier — `last_confirmed_tick` is global. `0fa6cd8` originally claimed
   both of those wrong things; corrected in `a96e9fd`.)

3. **`TankCommand.aim` is an analog `Vec3`**, so input-side `RollbackMode::Check` would mismatch, and
   roll back, *every single tick*. And edges under hold-last: `701d0a7` clears them when
   `get(tick).is_none() && get_last().is_some()`, which for a remote tank is true almost every tick, so
   it would never fire locally. See `TankCommand::clear_edges`, now the single definition of which
   fields are edges.

**Also:** the comment at `net/client.rs:255` calls the input-rollback arm "a permanent no-op". That is
a statement about today's config, not a permanent property. Fix it if you ever flip `rebroadcast_inputs`.

## 4b. What to do instead

1. **Tick-stamp `FireEvent` and fast-forward the tracer.** `on_fire_shell` starts an opponent's shell at
   its origin *now*, so your copy trails the server's by one-way latency — ~64 m at 800 m/s / 80 ms —
   for the whole flight. Real bug. Precondition for any hit-feel work.
2. **Remote tanks `Static` → `Kinematic`**, driven by the interpolated pose *with velocity*, so a ram
   imparts a correct-relative-velocity shove into your predicted tank while the remote is not fought
   over. Server still resolves ram damage (World of Tanks does exactly this). **Caveat:** recommended
   repeatedly in community sources, never in a primary one. Prototype, not proven pattern.
3. **Hit feel: view-layer cue, authoritative shove on arrival.** And know *why* it currently feels like
   nothing: the ~0.14 m/s shove is ~1.1 cm over 80 ms against a 5 cm `ROLLBACK_POSITION_M`, so it is not
   smoothed away — **it is never delivered to the client's sim at all.**
4. **A standing divergence instrument** (per-tick state hash + per-component error magnitudes). Serves
   the threshold ratchet today and is day-one infrastructure for a determinism effort.

**On determinism** (Yan's stated next direction): ADR-0016 is already the on-ramp — keep deriving
consequences and what remains on the wire is only causes, which *is* deterministic lockstep. Determinism
is the doctrine's fixed point, not a rewrite away from it. But note: you do not need bit-exactness, you
need **divergence below the rollback threshold** (ADR-0015 says as much), which is continuous and
buyable incrementally. That matters because the deployment is **ARM client against x86 server**;
cross-architecture bit-exactness realistically means fixed-point physics, which avian is not. Fixing the
named upstream bugs is a ratchet on what exists. Bit-exact cross-arch determinism is a different project
wearing the same word.

## 5. Open items I did not touch

- **lat0 client hang at connect** (design doc §7). Reproducible, zero-latency only (80/10 completed
  6/6), not OOM, not a crash, not thermal — all three verified. Freezes right after `rollback enabled`,
  ~14 s, then SIGKILL. Hypothesis: the zero-prediction-margin regime already documented in
  `net/watchdog.rs`, where loopback RTT + `balanced()` input delay drives the margin to zero.
  **Unresolved and uninvestigated.** It caps lat0 sample size.
- **Two drafted upstream bug reports, unfiled**, in `.agents/scratch/upstream-reports/`:
  `lightyear-avian-blanket-apply-pos-to-transform.md` and
  `lightyear-avian-restore-assumes-enlarged-aabb.md`. Filing publishes under your name — **your call,
  not an agent's.** Both are Layer-2 removal conditions per ADR-0015.
- **`target/` was 56 GB.** `.cursorignore` now excludes it from Cursor's indexer; `cargo clean` when
  convenient. rust-analyzer's ~3 GB is its Bevy graph and is not affected by either.

## 6. Verification

```bash
cargo clippy --all-targets --features net -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --features net          # 24 lib tests
cargo build --features net --bin overmatch --bin overmatch-server
```

Harness (note: **macOS has no `timeout`** — background the client and `kill` it):
```bash
pkill -f target/debug/overmatch-server; lsof -nP -i :5888     # before AND after
BEVY_ASSET_ROOT=$PWD SPIKE_PERTURB=0 OVERMATCH_BOT=1 ./target/debug/overmatch-server &  # wait ~10 s
BEVY_ASSET_ROOT=$PWD SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SIM_LONG=1 ./target/debug/overmatch &
```
Grep `NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` = 0. **Avoid `SPIKE_LATENCY_MS=0`** until §5's hang is
understood. Rollback counts are jitter-chaotic and are not a regression signal.

**Recoil, end-to-end evidence:** a client run *without* `SPIKE_SIMULATE_INPUT` (so it never fires
locally) spawned **13 cosmetic shells**, all necessarily from the bot's `FireEvent` — proving the event
arrives, the shooter entity-maps, and the new "skip tanks that fire locally" guard does not over-match.

**What I did NOT verify:** nobody has *watched a barrel move*. The kick is covered by unit tests
(`kick_lands_on_named_slot`, `barrel_less_weapon_is_noop`) driving the real system over real ECS state,
and the `FireEvent` path is proven above — but the last link, muzzle-slot resolution on a live
replicated rig, has only been observed through those two. **Eyeball it in a rendered client.**
