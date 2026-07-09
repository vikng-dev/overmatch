# Handoff — MP correctness slice

State as of 2026-07-09. Branch `mp-correctness-slice`, 20 commits, **not pushed, not merged**. Gates
green on both feature sets; both net bins build. Delete this file when consumed.

Reasoning is not repeated here — it lives in the doctrine this branch wrote:

- **[ADR-0016](.agents/docs/adr/0016-replicate-causes-derive-consequences.md)** — the three tests that
  decide derive-vs-replicate: is the cause complete, is the interaction one-way, are the dynamics
  contractive.
- **[ADR-0017](.agents/docs/adr/0017-mutual-contact-resolves-on-the-authority.md)** — mutual contact
  resolves on the authority; non-owned tanks stay interpolated. **This branch first argued the
  opposite**; the rejected argument is preserved there under *Considered options*, so nobody re-derives it.
- **[design/timelines-and-shear.md](.agents/docs/design/timelines-and-shear.md)** — the four tick
  indices, measured, and why ramming, un-learnable aim lead, the incoherent tracer and the unfelt hit
  are one phenomenon.
- **[.agents/GLOSSARY.md](.agents/GLOSSARY.md)** — *shear*, *complete cause*, *contractive/expansive*,
  *misprediction vs divergence*, the tick indices.

`git log --oneline main..HEAD` is the record of what landed; the commit bodies carry the full story.

## What shipped

**The only genuine bug in the multiplayer slice was input-edge starvation** (`701d0a7`). lightyear
extrapolates a starved input stream by holding the last `ActionState` forever, and the native path's
`decay_tick` is an unoverridable blanket no-op — so `TankCommand`'s *edges* (`fire_primary`,
`crew_swap`) re-latched every tick, defeating `consume_edges`. A client whose uplink starved on the
tick it fired made **the server fire an unrequested shot per reload cycle**; a held `crew_swap`
re-armed itself every `SWAP_SECONDS` without bound.

Also: remote barrel recoil, derived rather than replicated (`71987cf`); the opponent's shell given a
time index (`8783520`); the macOS `.dmg` startup crash (`2e5fba9` — the bake and the asset server
disagreed on where `assets/` lives); and the retirement of the `hc=0` divergence metric (`ca54288`),
which was never measuring what it was cited for, and whose numbers predated the fix by an hour.

**No contact-restore barrier to predicting non-owned tanks remains.** A different barrier does, and it
is Layer-1: ADR-0015's continuity rule binds tank-tank contact, and sharp oriented boxes are its named
bad class. Check what the hull proxies actually are before any two tanks touch — worth doing regardless
of ADR-0017, since it is a determinism prerequisite too.

## Next, in order

1. **Divergence instrument.** A per-tick state hash (*did anything differ?*) plus per-component error
   magnitudes (*by how much?*). Measures the divergence error class directly — the quantity a
   determinism effort drives to zero — and would have answered this branch's timeline questions with a
   number instead of three agents arguing from source. Day-one infrastructure for that session.
2. **Hit feel.** `on_hit_impulse` (`ballistics.rs`) is gated on `Res<ClientReplica>`, a *whole-client*
   gate, so the client never applies a hit shove even to its own predicted tank. And the ~0.14 m/s shove
   is ~1.1 cm over 80 ms against a 5 cm `ROLLBACK_POSITION_M`: it is **never delivered to the client's
   sim**, not smoothed away. Deliver it as a view-layer cue — predict what you author, replicate what is
   authored against you.
3. **Remote tanks `Static` → `Kinematic`**, so a ram imparts a correct-relative-velocity shove. Cheap,
   disposable, does **not** reduce the shear. Community-recommended, never primary-sourced. Prototype.
4. **Windows client** (below) — the only thing standing between you and a two-human fight.

## Open items not touched

- **lat0 client hang at connect** (`design/sim-divergence-and-determinism.md` §7). Reproducible,
  zero-latency only (80/10 completed 6/6); not OOM, not a crash, not thermal — all three verified.
  Freezes right after `rollback enabled`, ~14 s, then SIGKILL. Hypothesis: the zero-prediction-margin
  regime `net/watchdog.rs` documents — which the timeline measurement showed we sit close to even at
  80 ms (`P − S` ≈ +1 tick). **Unresolved.** It caps lat0 sample size.
- **Two drafted upstream bug reports, unfiled**, in `.agents/scratch/upstream-reports/`:
  `lightyear-avian-blanket-apply-pos-to-transform.md` and `lightyear-avian-restore-assumes-enlarged-aabb.md`.
  Filing publishes under your name — **your call, not an agent's.** Both are ADR-0015 Layer-2 removal
  conditions. A third, `avian-solver-constraint-order.md`, is the determinism anchor.
- **`cargo clean`** — `target/` reached 56 GB. `.cursorignore` keeps Cursor's indexer out of it;
  rust-analyzer's ~3 GB is its Bevy graph and is unaffected by either.

## The friend-fight punch list

Everything remaining is about the **second human**: *"my friend on Windows joins the DO server, we
fight, we both respawn."*

- **Windows client distribution.** No CI build exists for the net client bin, and a double-clicked
  `.exe` has no `OVERMATCH_SERVER`. Needs a Windows `--bin overmatch --features net` build (mirror the
  server workflow), a **baked default server address** falling back to the DO IP, and a **debug build**
  so the debug keys work. Fully parallel to the sim work. See `DEPLOY.md` for the droplet + redeploy
  runbook — redeploy after any protocol or `.replicate()` change, or a new client will not handshake.
- **Two-human-input validation.** lightyear's per-client input demux with two live `ActionState`
  authors is the one untested netcode path; the friend joining *is* the test. Audit the client-side
  unscoped `With<Tank>` queries and the single-tank diagnostics in `src/net/diagnostics.rs`
  (`watch_turret_pose`, `log_sim_evidence`).
- **Player respawn** (yours, not the bot's): death screen + respawn key when your own crew all die.
  `schedule_bot_respawn`/`respawn_dead_bots` is the reference; a player respawn needs a new
  `TankCommand` field and a server despawn/respawn of the client's tank.

## Verification

```bash
cargo clippy --all-targets --features net -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --features net           # 36 lib tests
cargo test --no-default-features    # 22 lib tests
cargo build --features net --bin overmatch --bin overmatch-server
```

Harness — **macOS has no `timeout`**; background the client and `kill` it:

```bash
pkill -f target/debug/overmatch-server; lsof -nP -i :5888     # before AND after
BEVY_ASSET_ROOT=$PWD SPIKE_PERTURB=0 OVERMATCH_BOT=1 ./target/debug/overmatch-server &  # wait ~10 s
BEVY_ASSET_ROOT=$PWD SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SIM_LONG=1 ./target/debug/overmatch &
```

`NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` must all be 0. **Avoid `SPIKE_LATENCY_MS=0`** until the hang
above is understood. Rollback counts are jitter-chaotic and are **not** a regression signal.

A client run *without* `SPIKE_SIMULATE_INPUT` never fires locally, so every `SHELL-SPAWN` it logs must
have arrived as a `FireEvent`. That is the end-to-end proof of the opponent-fire path, and it is how
the recoil guard was shown not to over-match.
