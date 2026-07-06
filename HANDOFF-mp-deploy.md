# Handoff — MP deployment: punch list → Edgegap cloud test → Milestone B

For: a fresh session (agent + Yan). State as of 2026-07-07: everything committed and pushed,
`main` at `87d19be` + this handoff's commit, working tree clean, all gates green. This file is
COMMITTED (unlike prior handoffs) — delete it when consumed. Supersedes
`HANDOFF-mp-architecture-review.md` (consumed) and `HANDOFF-suspension-feel.md` (resolved);
`HANDOFF-phase2-tank-bake.md` remains valid as its own independent deferred slice.

## What the 2026-07-06/07 session shipped (all on main, full stories in commit bodies)

| Commit | What |
|---|---|
| `8ae795c` | Rollback watchdog — lightyear's receive-time check starves at zero prediction margin (LAN + `balanced()` delay): state rollback was silently DEAD, measured 35–50 m runaways. Backstop forces rollbacks at any margin. |
| `588c3b1` | Harness: `SPIKE_SIM_FORWARD` + `ancm` anchor bitmask in tick trace |
| `dc56e61` | Friction force-law continuity (static↔kinetic blend + LuGre anchor relax) — feel-PASSED by Yan |
| `1b9c216` | ADR-0015 divergence doctrine + determinism-doc corrections + scaffolding status in code |
| `8a08d60` | `SPIKE_CONTACT_PROBE` — per-tick pair/moved-set/AABB/attachment coherence probe |
| `33cc4e4` | Attachment-poison fix — lightyear_avian's blanket `ApplyPosToTransform` let render state ratchet child colliders off the rig (hull proxy measured 2.8 m adrift). THE fix for "hull-stuck never settles". |
| `f4a24c2` | Sphere-cast ground distance from witness geometry — parry GJK TOI relative tolerance = ~200 mm one-sided noise vs the 1000 m slab; was the at-rest limit cycle (gunner-sight shake) AND a standing divergence amplifier. |
| `87d19be` | Force-arrow gizmos mounted in the net client (G to toggle, X x-ray, F camera detach; debug builds) |

Yan's feel verdicts: cliff-dive "feels like SP", "no teleportation or jitters", standing jitter
gone, washboard "genuinely smooth" (confirmed with force arrows). MP single-client is DONE.

## The path (Yan-approved order)

1. **Punch list (hours):**
   - Bump-stop NaN panic: deterministic repro `SPIKE_SPAWN_POSE="0,0.85,-39.88,0.1736,0,0,0.9848"`
     (spawn intersecting terrain) — POSSIBLY already fixed by f4a24c2's penetrating-start guard;
     re-run the repro before touching anything.
   - Feature-gate `net/harness.rs` + `net/diagnostics.rs` (always-compiled test scaffolding;
     pre-ship item).
   - Know-only: dev auth token is hardcoded (fine for friend playtest).
2. **Linux server build + Edgegap deploy + solo cloud feel test** with `SPIKE_TRACE` on both
   ends. This IS the cross-platform divergence measurement (macOS ARM client vs Linux x86_64
   server: libm + codegen differ; everything to date is same-machine). The local noise floor is
   now 2–4 rollbacks where it used to be 61–192, so the cross-platform delta will be cleanly
   visible. If rates disappoint: `enhanced-determinism` (libm) is the pre-identified one-feature
   A/B dial. Server ships WITH the glb (phase-2 bake not required for this).
3. **Milestone B:** second client / interpolated remotes — adopt Rocket League's remote-input
   decay during resim (replay last-known remote input decaying to neutral over ~150 ms); shells
   `PreSpawned`; combat over the wire.
4. **Friend playtest.**

## Hard-won facts — do not re-learn

- **All historical rollback baselines are OBSOLETE.** The old "chaotic bands" (washboard 1–147,
  beached 61–192) were mostly poison- and cast-noise-driven. Post-f4a24c2 baselines: beached
  80/10 ≈ 2–4 rollbacks; idle rest p.y spread 0.026 mm; washboard per-tick |Δload| p95 6.5 kN.
  Re-baseline anything you A/B.
- **lat0 rollback counts from pre-watchdog builds are NOT a metric** (they measured check
  starvation). lat0 |Δp| tick-row divergence remains valid.
- Rollbacks in a solo game are a defect indicator, target ~zero (ADR-0015). The two-layer
  doctrine governs new work: sim continuity is permanent; netcode scaffolding maps to upstream
  defects (see `.agents/scratch/upstream-reports/` — 7 filable items, each self-contained).
- Hard landings now dip ~3 cm deeper than pre-f4a24c2 (honest distance; the TOI bias was
  phantom spring force). Documented deep-press regime py ≈ −0.14. If Yan reports "sinks too
  far" on drops, it's a bump-stop engage retune, not a bug.
- Deferred feel note (Yan): turret servo reads near-instant at fine-aim scales post-wobble-fix —
  law unchanged (trapezoidal, 70°/s², tank.rs:1336-1354), tuning options measured in memory
  `mp-suspension-feel-deferred`. Not now.
- Track-model rule (binding, ADR-0015): contact primitives must be divergence-continuous; also
  tile large static colliders ≤10 m (cast error scales with extent).
- Instrumentation levers: `SPIKE_TRACE` (role-suffixed JSONL), `SUSP_TRACE` (per-wheel
  suspension/drive terms), `SPIKE_CONTACT_PROBE` (pair/AABB/attachment coherence),
  `SUSPENSION_PROBE=ray|sphere` (sim-affecting, must match ends), `scripts/jitter/analyze.py` +
  `anatomy.py`. Force arrows: G in a debug client window.

## Working mode (Yan's, standing)

One slice → verify with the harness → report → WAIT for his feel confirmation on anything
sim/feel-touching (objective bug fixes ship on gates + review). Subagents write code, the
orchestrator verifies and commits: imperative summary + full-story body + verification paragraph
+ Claude `Co-Authored-By` trailer, one concern per commit. `/code-review high` (workflow-backed)
before any substantial diff lands — it caught real bugs in EVERY slice this session (Yan may ask
for the panel on Opus: edit the persisted workflow script's agent() opts, add `model: "opus"`).
**Push to main directly and merge PRs — Yan's standing authorization 2026-07-06** (memory
`push-main-authorized`); never force-push. Feel decisions → `.agents/scratch/playtest-forks/`.
Sessions run in a worktree; harness runs need `pkill -f target/debug/server` + port 5888 free +
`BEVY_ASSET_ROOT=$PWD` + `SPIKE_PERTURB=0` on the server, ~10 s bake before the client.

## Verification protocol (gates unchanged)

```bash
cargo clippy -q --all-targets && cargo clippy -q --features net --all-targets
cargo test -q && cargo test -q --features net       # 15 lib + 4 spherecast
cargo build -q --bin server --bin client --features net
```
Harness recipes and current baselines: see "Hard-won facts" above; grep `SHADOW-BAKE ok` both
ends, `NAN-TRIPWIRE|FIXED-NAN|panicked|B0004` = 0. Drop test now part of the repertoire:
`SPIKE_SPAWN_POSE="0,6,0,0,0,0,1"` + `SPIKE_SIM_IDLE=1` (lands 16/16, settles to rest).

## Primary references

- `.agents/docs/adr/0015-divergence-doctrine.md` — the governing doctrine (+ 0014 sim/view split).
- `.agents/docs/design/sim-divergence-and-determinism.md` — corrected + §5 dated findings.
- `.agents/scratch/upstream-reports/` — 7 upstream report candidates, one file each, with
  workaround removal conditions (parry item has an automatic test tripwire).
- Memories: `mp-architecture-review-2026-07` (session log + verdicts), `push-main-authorized`,
  `mp-suspension-feel-deferred` (servo tuning note + limit-cycle record), `mp-jitter-instrumentation`.
- `HANDOFF-phase2-tank-bake.md` — the independent deferred bake slice (server glb-independence).
