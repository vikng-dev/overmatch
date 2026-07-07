# Handoff — opponent fire replication, then the friend-fight punch list

For: a fresh session (agent + Yan). **Yan wants to START by discussing opponent fire replication**
(the `FireEvent` slice) before any code — lead with the design conversation, not an implementation.
State as of 2026-07-07: everything below is committed + pushed to `main` (`164a1c7`), working tree
clean except this file. This file is COMMITTED — delete it when consumed. Supersedes
`HANDOFF-mp-deploy.md` (fully consumed: MP single-client + cloud deploy done — that file can be
deleted). `HANDOFF-phase2-tank-bake.md` remains a valid independent deferred slice.

## The goal in view
"**My friend (on Windows) joins the DO server, we fight, we both respawn.**" Server-authoritative
combat, turret toss, and bot auto-respawn all landed this session. What's left is entirely about the
**second human**. Opponent fire replication is the piece Yan wants to design first.

## What this session shipped (all on `main`, full stories in commit bodies — don't re-read the diffs unless needed)

| Commit | What |
|---|---|
| `a3697f6` | Ownerless circling test-bot (`OVERMATCH_BOT`, default off) + per-client spawn lanes |
| `f85afe7`/`9cbf340` | `[BOT]` nameplate (replicated `NetBot`) + net-client debug HUD (ping/FPS/frame) |
| `fa79164`/`b5e22e9` | On-screen HUD split (name-only nameplates + own-tank vitals panel) + debug-panel fixed width |
| `818f7bb` | **Server-authoritative combat**: damage/turret physics gated to authority via `ClientReplica`; per-volume health replicated as `NetHealth(Vec<f32>)` (ServoAngles idiom); death emergent from replicated health |
| `c73934a` | Fix cooked-off turret launching from a double-transformed Position (authority-shared bug) |
| `cea37b6` | **Turret toss synced**: `LaunchedTurretPose` replicated, client drives its rig turret kinematically (Approach A) |
| `6d68292` | `On<Remove, Rig>` observer sweeps a detached turret when its root despawns (client leak fix) |
| `164a1c7` | **Bot auto-respawn 5s after death** (`schedule_bot_respawn`/`respawn_dead_bots`) |

Full solo combat loop works + is Yan-confirmed: shoot bot → crew/modules degrade → dies → turret
tosses (correct spot, no leak) → 5s → fresh bot drives in. Server-authoritative, durable across
reconnect, cross-platform (ARM client / x86 DO server).

## ⚠️ Pending mechanical step: finish the respawn redeploy
The DO server is still running the pre-respawn build (`cea37b6`). Server build run `28883882730` was
in-flight at handoff. When green: `gh run download <id> -n overmatch-server-x86_64-linux` → scp →
`systemctl restart` per **`DEPLOY.md`** (the redeploy runbook). No protocol change since `cea37b6`, so
it's just so the DO bot actually respawns. Verify the bot circles + respawns in `journalctl`.

## THE FIRST CONVERSATION — opponent fire replication (`FireEvent`)

Design context already established this session (see memory `mp-architecture-review-2026-07` and the
combat discussion): combat is **server-authoritative, favor-the-server** (tanks are slow → no
lag-comp), **cosmetic shells** (Yan chose this over predict-and-reconcile), death emergent from
replicated health. Damage is **already** server-authoritative and done — `FireEvent` is **purely
presentation** (letting a player SEE the opponent's shell in flight); a lost FireEvent = a missing
tracer, never a missing hit.

Why it's still needed: the shooter already sees their **own** shell (spawned locally from local fire
input, cosmetic — client damage is gated off by `ClientReplica`). But a remote tank is interpolated
and **doesn't fire locally** (no `ActionState`), so the opponent's shots are currently invisible to
you. `FireEvent` carries the shot to the *other* client.

The open fork to discuss with Yan (recommend, let him steer):
- **(i) Cosmetic fire broadcast:** replicate a lightweight "shot fired from pose X (params, tick,
  shooter)"; each client spawns a local cosmetic tracer via existing `integrate_projectiles`
  (War-Thunder-style deterministic parallel flight). Cheap, robust to packet loss, ~one message per
  shot. **Likely the right first cut.**
- **(ii) Replicated shell entities:** server-authoritative shells replicated to all (truthful
  trajectory, dodging). This is **Approach B** from the turret-sync research — the *same* structural
  problem the turret research already validated (mid-game `Replicate` add via `on_insert` hook,
  `DisableReplicateHierarchy` on the entity, explicit `Position` before `RigidBody`). The turret used
  Approach A; shells are Approach B's natural home. More work + pulls in the deferred `PreSpawned`
  problem for the shooter's own predicted shell.

Relevant deferred hazard: shells spawned inside the predicted timeline **double-spawn on rollback**
(the wart documented in `src/net/diagnostics.rs` `count_shell_spawns`); `PreSpawned` (hash-match +
replay-idempotency + unmatched-cleanup) is the fix, only needed if the shooter's shell becomes a
predicted/rollback entity. Cosmetic view-entity tracers (option i) sidestep it entirely.

Research already done and reusable (from earlier subagents this session — findings are in the
conversation that produced this handoff, not a file): FPS lag-comp canon, WoT/War-Thunder tank-combat
authority, determinism-vs-reconciliation, and the lightyear-0.28 mid-game-replicated-dynamic-entity
mechanics (the turret-sync research explicitly flagged Approach B as the shell pattern). If deeper
lightyear API detail is needed, spin a **Sonnet** research subagent (see working mode).

## The rest of the friend-fight punch list (after opponent fire)
- **Windows client distribution** — there is NO CI build for the net `client` bin, and a
  double-clicked `.exe` has no `OVERMATCH_SERVER` env. Needs: a Windows `--bin client --features net`
  CI build (mirror `.github/workflows/server-build.yml`) + a **baked default server address**
  (fall back to the DO IP when `OVERMATCH_SERVER` is unset) + Yan wants a **debug build** so the debug
  keys work. Fully parallel to the sim work (no collision). Friend is on **Windows**.
- **Two-human-input validation** — lightyear's per-client input demux with two live `ActionState`
  authors is the one untested netcode path; the friend joining IS the test. Also audit client-side
  unscoped `With<Tank>` queries + the two single-tank diagnostics noted in `a3697f6`'s commit
  (`watch_turret_pose`, `log_sim_evidence` in `src/net/diagnostics.rs`).
- **Player respawn** (deferred, not the bot's): death screen + respawn key when your own crew all die
  (emergent). Bot respawn (`164a1c7`) is the reference; a player `respawn` needs a new `TankCommand`
  field + server despawn/respawn of the client's tank.

## Working mode (Yan's, standing — see memory `overmatch-orchestration-mode`)
- **Yan co-directs**: surface forks, recommend, let him steer. Start with discussion on anything
  design-shaped (he explicitly wants to discuss opponent fire before code).
- **Model routing**: **Opus** subagents implement; **Sonnet** subagents research (libs/APIs/docs).
- **Orchestrator owns live verification, agents run STATIC gates only.** Hard-won lesson: two agents
  each running live servers collide on `pkill`/port 5888 — never run two live-verifying agents at
  once. A single agent may run live only when it's the *only* one running.
- **Implement first, document after** each slice lands + feels right. Minimalism on WIP/debug features
  — hardcode, no config knobs, minimal branching.
- **Feel/sim confirmation before commit**: sim/feel-touching changes wait for Yan's windowed
  confirmation; objective fixes ship on gates + review. Verify visible changes with a windowed client
  (agents can't see the screen; the money path always needs Yan's eye or an entity-count proof).
- **`/code-review high`** (workflow-backed) before substantial sim/net diffs land — it caught real
  bugs in the combat slice this session. Scope its args to the changed files; ignore untracked files.
- **Push to main directly** (memory `push-main-authorized`); never force-push. Commits: imperative
  summary + full-story body + verification paragraph + `Co-Authored-By: Claude Opus 4.8 (1M context)`
  trailer; one concern per commit.
- **Redeploy loop** after any protocol/server change: `gh workflow run "Server build"` → download →
  scp → restart (`DEPLOY.md`). A new `.replicate()` component changes the wire protocol → a new client
  won't handshake with the old server, so redeploy before Yan tests over DO. Local testing (both ends
  same build) works immediately.

## Verification protocol
```bash
cargo fmt --all --check
cargo clippy -q --all-targets && cargo clippy -q --features net --all-targets   # CI uses -D warnings
cargo test -q && cargo test -q --features net                                    # 15 lib + 4 spherecast
cargo build -q && cargo build -q --bin server --bin client --features net
```
Live smoke (orchestrator): `pkill -f target/debug/server`; `OVERMATCH_BOT=1 BEVY_ASSET_ROOT=$PWD
SPIKE_PERTURB=0 cargo run --bin server --features net` (~10s bake); windowed client
`BEVY_ASSET_ROOT=$PWD cargo run --bin client --features net` (loopback) or headless
`... --bin client ... -- --simulate-input`. To exercise combat/cookoff without aiming, a temporary
env-gated force-death system that zeroes the bot's `Ammo` `ComponentHealth` is the proven pattern
(REMOVE it before committing). Grep `SHADOW-BAKE ok` both ends; `panicked|B0004|NAN-TRIPWIRE` = 0.

## Primary references (read these, don't have this doc repeat them)
- **`DEPLOY.md`** — the DO droplet (IP, SSH, systemd, `OVERMATCH_BOT=1` drop-in) + redeploy runbook.
  Temporary infra; migration to Fly.io/PlayFlow is noted there.
- **`src/net/protocol.rs`** — the wire contract + the `ServoAngles`/`NetHealth`/`LaunchedTurretPose`
  "root datum → apply to local rig" idiom the FireEvent design should weigh against Approach B.
- **`src/ballistics.rs`** — `FireShell` event (the fire seam), `integrate_projectiles` (flight +
  authority-gated damage), the `ClientReplica` gate.
- **`src/net/diagnostics.rs`** — `count_shell_spawns` (the rollback double-spawn / `PreSpawned` note).
- Memories: `mp-architecture-review-2026-07`, `overmatch-orchestration-mode`, `push-main-authorized`,
  `mp-suspension-feel-deferred`, `mp-jitter-instrumentation`.
- ADR `.agents/docs/adr/0015-divergence-doctrine.md` (governing doctrine) + `0014` (sim/view split).

## Suggested skills
- **`code-review`** (`/code-review high`) — before any substantial FireEvent/shell diff lands.
- **`verify`** / **`run`** — to drive a windowed client and observe the shell visuals end-to-end.
- Spawn **Sonnet** subagents for lightyear/avian API research (per working mode) rather than a skill.
