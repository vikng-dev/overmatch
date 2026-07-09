# Handoff — MP correctness slice, and the predict-everyone fork

For: Yan + a fresh session. State as of 2026-07-09. Branch `mp-correctness-slice`, 8 commits,
**not pushed, not merged**. Gates green both feature sets; both net bins build. Delete this file when
consumed.

## The one decision waiting for you

**Predict non-owned tanks, or don't.** Everything this slice found says the trade is now better than
when it was deferred (memory: `opponent-prediction-open-tab`, 2026-07-08). It is a structural change
and it was deliberately not made. See §4.

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

**No contact-restore barrier to predicting non-owned tanks remains.**

## 4. The fork: predict-everyone

**What it buys.** Not lower latency for the opponent — that is impossible, their input arrives with
their state. It puts them on *your* timeline. Today your tank runs ~½ RTT *ahead* of the server and the
opponent ~1.7 send-intervals *behind* it; they are in your physics world at different instants. Predict-
everyone makes the offset uniform. That is what tank-tank collision needs (a predicted Dynamic body
shoved into a `Static` interpolated one will not resolve like two real bodies), and it is the choice
*coherent with having no lag compensation* — an extrapolated opponent is an estimate of the frame the
server will actually evaluate in, where an interpolated one is a full RTT stale.

**How it works.** Confirmed state is the anchor; last-known input is the slope. lightyear holds the last
input (`InputBuffer::get_predict`). True inputs only drive their own tick during rollback replay, where
the replay is re-running the past and they arrive "in time". Tanks are high-inertia with analog
controls, so repeat-last-input is an excellent estimator — the thing that makes extrapolation useless
for an FPS does not exist here. War Thunder ships exactly this and says so; Rocket League predicts all
cars because they collide.

**The two hard constraints found in source:**

1. **Input-side `RollbackMode` must stay `Disabled`.** `TankCommand.aim` is an analog `Vec3` that
   changes every tick, so `Check` would mismatch — and roll back — *every single tick*, resimming the
   full `FixedMain` including avian. Let STATE rollback correct remotes; the 5 cm `ROLLBACK_POSITION_M`
   threshold catches a bad extrapolation, and a 57 t body does not wander 5 cm in 80 ms.
   The comment at `net/client.rs:255` calls this arm "a permanent no-op". It is a statement about
   today's config, not a permanent property. Fix that comment when you flip `rebroadcast_inputs`.

2. **Edges must not ride the replicated input under hold-last extrapolation.** Latent today (needs
   starvation); **structural** under rebroadcast, where hold-last is the steady state for every remote
   tank and they would ghost-fire routinely. `701d0a7` fixes the *symptom* correctly for the current
   config, but under predict-everyone the bridge's `get(tick).is_none()` will be true constantly for a
   remote tank. **Re-derive the edge rule before flipping this on** — see `TankCommand::clear_edges`,
   which is now the single definition of which fields are edges.

**What it deletes.** `ServoAngles` and `FireEvent` both. Both are *consequences* on the wire, present
only because a remote tank has no inputs. Give it inputs and `drive_aim_servos` and `fire()` run
locally: servo lay, tracer, barrel kick, reload timer and hull shove all fall out of the shared sim.
`FireEvent` is best read as a symptom of not having remote inputs.

**Also revisit** `apply_net_health` (`0fa6cd8`). Its tick-agnosticism is safe *only* because
`RollbackMode::Check` gates every rollback start on a mismatch. Predicting non-owned tanks multiplies
the entities feeding the global `last_confirmed_tick` frontier. The doc comment spells out what breaks.

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
