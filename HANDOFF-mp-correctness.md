# Handoff — MP correctness slice, and the predict-everyone fork

For: Yan + a fresh session. State as of 2026-07-09. Branch `mp-correctness-slice`, 18 commits,
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
| `2e5fba9` | **macOS `.dmg` fix**: the bake and the asset server disagreed on where `assets/` lives |
| `a96e9fd` | Corrected `apply_net_health`'s invariant — the trigger is *input* rollback, not state |
| `57f1405`/`8783520` | `FireEvent` carries its fire tick; shell anchored at the predicted index (2nd commit reverses the 1st) |
| `cc5dc7e` | Replaced the derived catch-up figure with the measured one |
| `4c2b95a` | GLOSSARY vocabulary; ADR-0016 rewritten and shortened; **ADR-0017** new |
| `63cd526` | Stopped the docs teaching that determinism means lockstep |
| `bfbf53a` | **`design/timelines-and-shear.md`** — the timeline model, measured |

**The only genuine bug in the multiplayer slice was the input-edge starvation.** lightyear extrapolates
a starved input stream by holding the last `ActionState` forever (`get_predict` → `get_last()`), and
the native path's `decay_tick` is an unoverridable blanket no-op. `TankCommand`'s *edges*
(`fire_primary`, `crew_swap`) were re-latched every tick, defeating `consume_edges`. A client whose
uplink starved on the tick it fired made **the server fire an unrequested shot per reload cycle**; a
held `crew_swap` re-armed itself every `SWAP_SECONDS` without bound. Server-side only (the client's
own tank authors an input every tick; remote tanks carry no `ActionState`).

Barrel recoil was a **missing feature, not a defect**, and became the worked example of ADR-0016.

## 2. What I asserted that was wrong

Recorded because the pattern matters more than the items: **every one was a confident claim sitting
next to code or prose that did not support it** — the failure mode ADR-0016 names for the wire.
Each was caught by an agent told to disbelieve me.

| Claim | Verdict |
|---|---|
| "`NetHealth` tick-staleness manufactures divergence" | **No.** State rollback (`Check` *and* `Always`) starts at `last_confirmed_tick`; no replay window precedes the death tick. My proposed fix would not have worked either — the drive gate rides the never-rolled-back `Dead` marker, not health. |
| "Replay contact re-formation is the gate before predicting remotes" | **Stale document.** Superseded the same day it was written; the `hc=0` metric never discriminated the defect. |
| "Drain the `FireEvent` on the fixed clock" | **Would drop messages** on zero-tick frames — `lightyear_messages` clears undrained receivers in `Last`. |
| "The opponent's tracer lags a constant ~64 m" | **Wrong reference frame.** Measured: 6 of 7 shots need zero catch-up; the defect is an intermittent late packet. |
| "Anchor the caught-up shell to the confirmed index `C`" | **No** — it would converge on where the victim *was*. Anchored at `P`, co-indexed with the hull it hits (`8783520` reverses `57f1405`). |
| "`Tick` is a wrapping `u16`" | It is a wrapping **`u32`**; `Sub` returns `i32` through `i64`. |
| "`I < C`" | **Backwards at shipping latency — and there is no fixed ordering.** They cross over near one-way ≈ 27 ms and are not commensurable. |
| "Catch-up is ~10 ticks / ~125 m" | **Derived, never measured.** Measured ≈4 ticks / ~49 m at RTT ≈ 91 ms (`cc5dc7e`). |
| "Parallel reduction order blocks determinism" | **Not in avian 0.7.** Edge colouring gives disjoint per-colour writes, and the repo had already measured that `parallel` off changes nothing. Three blockers, not four. |
| "ADR-0004 rejected lockstep" | **It contains no netcode at all.** Phantom citation, written by me into ADR-0016. |

**Two of the three correctness findings in the final code review were bugs this slice introduced**,
both of them doc comments asserting something the code did not guarantee.

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

**This reasoning now lives in doctrine, where it will survive this file being deleted.**

- **[ADR-0017](.agents/docs/adr/0017-mutual-contact-resolves-on-the-authority.md)** — the decision,
  the three exits from a mutual continuous interaction, and the two lightyear-source blockers (the bot
  has no input to rebroadcast; reliable remote fire needs input rollback, which breaks
  `apply_net_health`).
- **[design/timelines-and-shear.md](.agents/docs/design/timelines-and-shear.md)** — the four tick
  indices, the measured offsets, and why ramming, un-learnable aim lead, the incoherent tracer and the
  unfelt hit are one phenomenon.
- **[ADR-0016](.agents/docs/adr/0016-replicate-causes-derive-consequences.md)** — the three tests
  (complete cause, one-way vs mutual, contractive vs expansive) that decide derive-vs-replicate.

The short version: predicting opponents does not fix ramming, it replaces shear with *mutual
misprediction* fed through the one part of the sim that expands perturbations. Determinism comes
before predict-everyone, not after. An earlier revision of this file argued the opposite, at length;
that argument is recorded in ADR-0017's *Considered options* so nobody re-derives it.
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

**On determinism** (Yan's stated next direction): ADR-0016 is already the on-ramp — deriving every
consequence *reduces what has to be reconciled*, but it does **not** turn the wire into inputs-only
lockstep. State replication and the authority's re-anchor stay (dropping them is what lockstep does,
and that is exactly the rejected quadrant — one divergence would desync permanently with nothing to
re-anchor from). The endpoint is **deterministic + server-authoritative**, not lockstep. Determinism
is the doctrine's fixed point, not a rewrite away from it. And note: you do not need bit-exactness,
you need **divergence below the rollback threshold** (ADR-0015 says as much), which is continuous and
buyable incrementally.

That last point matters because the deployment is **ARM client against x86 server**. But
cross-architecture bit-exactness does **not** require fixed-point physics. IEEE-754 specifies
`+ − × ÷ √` as correctly rounded — bit-identical on ARM and x86 — and Rust neither enables fast-math
nor implicitly contracts FMA, so the classic C `a*b+c` pitfall is off the table by default. The real
blockers are enumerable: **(1)** transcendentals (`glam` defaults to `std` libm,
`glam-0.30.10/Cargo.toml:48`; avian's `enhanced-determinism` swaps in the Rust `libm` crate);
**(2)** FMA contraction, only if a `mul_add` path is actually emitted as FMA; **(3)** the serial,
entity-index-keyed constraint colouring order (`.agents/scratch/upstream-reports/avian-solver-constraint-order.md`,
avian #480/#734). **"Parallel reduction order" is NOT a blocker in avian 0.7** — measured: rebuilt
without the `parallel` feature, same divergence (|Δav| 0.154 vs 0.155), because the parallel step
writes disjoint bodies per colour by construction. Once (1)–(3) land, bit-exact cross-arch float is
plausibly reachable *without* fixed-point — and in any case what the netcode needs is divergence
below the rollback threshold, not bit-exactness. Fixing the named upstream bugs is a ratchet on what
exists.

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

**The friend-fight punch list** (carried over from the now-consumed `HANDOFF-opponent-fire.md`; the
`FireEvent`/opponent-fire slice it led with has shipped — see §1 `71987cf` and §4b). The remaining
work is entirely about the **second human** ("my friend on Windows joins the DO server, we fight, we
both respawn"):
- **Windows client distribution** — there is NO CI build for the net client bin, and a
  double-clicked `.exe` has no `OVERMATCH_SERVER` env. Needs: a Windows `--bin overmatch --features net`
  CI build (mirror the server build workflow) + a **baked default server address** (fall back to the
  DO IP when `OVERMATCH_SERVER` is unset) + Yan wants a **debug build** so the debug keys work. Fully
  parallel to the sim work (no collision). Friend is on **Windows**. See **`DEPLOY.md`** for the DO
  droplet + redeploy runbook (redeploy after any protocol/`.replicate()` change or a new client won't
  handshake).
- **Two-human-input validation** — lightyear's per-client input demux with two live `ActionState`
  authors is the one untested netcode path; the friend joining IS the test. Also audit client-side
  unscoped `With<Tank>` queries and the single-tank diagnostics in `src/net/diagnostics.rs`
  (`watch_turret_pose`, `log_sim_evidence`).
- **Player respawn** (deferred, not the bot's): death screen + respawn key when your own crew all die
  (emergent). Bot respawn (`schedule_bot_respawn`/`respawn_dead_bots`) is the reference; a player
  respawn needs a new `TankCommand` field + server despawn/respawn of the client's tank.

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
