# Sim divergence & the determinism landscape (bevy / avian / lightyear)

2026-07-04. Written after the step-8 rollback investigations, before the latency feel test
(slices 2/3). Two halves: what we now *know empirically* about our own client/server divergence,
and what the stack currently offers for cross-platform determinism. Sources: this repo's spike
log (measured), vendored crate sources (verified, cited by file:line), and web material (flagged).
**Updated 2026-07-06** after the architecture-review session: stale claims corrected in place
(marked "(corrected 2026-07-06: …)"), new measured rows in §2, and §5 added with the
solo-divergence model and the two-layer doctrine — whose canonical home is
[ADR-0015](../adr/0015-divergence-doctrine.md).

## 1. Why two runs of "the same sim" diverge at all

IEEE 754 is often blamed too broadly. The precise picture:

- **Basic float ops are exactly specified.** `+ - * / sqrt` on f32 give bit-identical results on
  every IEEE-conforming CPU, same rounding mode. A single-threaded sim using only these, in the
  same order, is bit-deterministic across x86/ARM.
- **What actually breaks it:**
  1. **Transcendentals** (`sin`, `cos`, `atan2`, `powf`…) are NOT specified by IEEE — each
     platform's libm returns different ulps. This is the gap avian's `enhanced-determinism`
     closes (swap system libm for the Rust `libm` crate everywhere: avian, bevy_math,
     bevy_heavy, parry — avian3d-0.7.0/Cargo.toml:73-78).
  2. **FMA and SIMD codegen.** Rust does *not* auto-contract `a*b+c` into FMA (unlike C with
     `-ffast-math`), which removes the classic C++ pitfall — but explicit SIMD paths (glam's)
     can differ by target feature level (SSE2 vs NEON vs scalar fallback), and `mul_add` calls
     are FMA-or-not depending on target.
  3. **Reduction/iteration order.** Float addition isn't associative. Any parallel sum, any
     hash-ordered ECS query iteration, any archetype-order-dependent loop makes results depend
     on entity spawn history and thread scheduling — *even on one machine*.
  4. **Engine-internal ordering.** Solver constraint order, broad-phase pair order, island
     splitting, sleeping heuristics.

## 2. What we measured in Overmatch (same machine, same binary!)

The step-8 investigations give us an unusually concrete divergence taxonomy — all of it at
zero latency, macOS-vs-macOS, identical binaries:

| Source | Magnitude | Status |
|---|---|---|
| Unanchored `propagate_collider_transforms` (schedule race after disabling `PhysicsTransformPlugin`) | ~90% of smooth-ground rollbacks (~150 → ~10/run at 100 ms), plus the bind-window NaN crashes | **FIXED** (ccbe7fc) — was a bug, not float divergence |
| Render-space (`GlobalTransform`) reads in sim systems | small but systematic, worst under high yaw/pitch rates | **FIXED** (ca5e380) — sim reads physics `Position`/`Rotation` |
| Contact transients on rough terrain (washboard/bump) | 20–60 velocity-threshold trips/s at 0.05 m/s; ~135/20 s run even post-fixes | **Irreducible class** — managed by thresholds (`ROLLBACK_VELOCITY` 0.20) |
| Everything else (flat ground, driving, slewing, firing) | ~10 rollbacks/run at 100 ms, ~0 at rest | Healthy noise floor |
| Check starvation: lightyear's receive-time mismatch check skipped at zero prediction margin and never retried (see §5) | 35–50 m divergence with fresh authority arriving every tick and **zero rollbacks**; 3,296 skip-trace lines in one run | **FIXED** (8ae795c, `net/watchdog.rs` backstop) — added 2026-07-06 |
| Rollback contact-restore defect: hull contact fails to re-form on the first replayed tick, seeding mm-scale error that re-trips the 0.05 m bar | hc=0 on 55% of replayed ticks at 80/10 (98.4% at lat0); Δlv exactly −g·dt = 0.1533 m/s at k=1 while pose restore is near-exact (\|Δp\| p50 1.5 mm); contact re-forms at k=1 in 62/69 cases where the client's abandoned timeline still had it vs fails in 80/85 where it didn't | **OPEN — mechanism CONFIRMED 2026-07-06**, fix pending: the rollback restores the root pose to server truth but collision-detection state to the client's own abandoned timeline — and for the tank's collider children not at all (no `PredictionHistory` on them, so `ColliderAabb`/`EnlargedAabb`, BVH leaves, moved-proxy set, and `ContactGraph` all keep mispredicted-timeline values; `lightyear_avian3d::restore_collider_tree_from_enlarged_aabbs` assumes "the rollback just restored EnlargedAabb" — false for child colliders). Second-order anomaly unpinned: avian's teleport-catcher should self-heal in 1–2 ticks and provably doesn't (live ticks hold hc=0 at penetrating poses for hundreds of ticks; zombie-pair `contains_key` dedup is the prime suspect). Plan: probe → repo-side post-restore reconciliation (Layer 2) → upstream patch |
| In-contact replay load chaos (friction/load through the replay, a separate machine from the row above — anti-correlated with the hc-loss signature) | per-wheel load deltas to 5.8e6 N; the multi-meter replay errors | **OPEN** — absorbed by the Layer-2 thresholds (ADR-0015) — added 2026-07-06 |

Lesson: **most of our "divergence" so far was bugs, not physics.** The genuinely irreducible
part is contact-transient noise, and it is exactly what rollback + per-component thresholds are
designed to absorb. Second lesson: every one of these was found by measuring, not by reasoning —
the same discipline applies to anything below.

Why "same machine" still diverges at all: avian ships with the `parallel` feature **on by
default** (avian3d-0.7.0/Cargo.toml:57-63) and we run defaults — solver work is threaded, so
constraint application order varies run-to-run and process-to-process. Plus ECS iteration order
differs between client and server worlds (different entity histories), feeding order-dependent
float sums. This is why bit-exactness was never on the table for the current architecture.

*(corrected 2026-07-06: the "parallel = nondeterministic solver order" half of that paragraph is
stale for avian 0.7. The dynamics step is order-invariant same-machine **by construction** —
graph-coloring with disjoint body writes, Vec-backed contact storage; upstream PRs #712/#807 made
parallel constraint generation deterministic, and avian CI enforces cross-platform bit-identity
WITH parallel enabled (2D-only test; 3D plausible but unverified). And bit-exactness IS on the
table where it matters: flat-ground cruise measured bit-exact client-vs-server over ~880-tick
windows, all fields. The two REAL divergence mechanisms are (1) same-machine replay divergence
from BVH refit topology across rollback restore — lightyear_avian restores the tree from
EnlargedAabbs but refit keeps rollback-time topology → pair-discovery order → coloring
differences at contact transients — and (2) cross-machine entity-index-keyed constraint coloring
between the two ECS worlds, irreducible by config, needs upstream canonical ordering. The ECS
iteration-order sentence stands for gameplay code.)*

## 3. What the stack offers today

- **avian `enhanced-determinism`** (off by default, off for us): libm math everywhere for
  cross-*platform* consistency of transcendentals; docs claim "improving determinism across
  architectures at a small performance cost" (avian3d-0.7.0/src/lib.rs:66; third-party material
  quotes 10–30% — unverified). It does NOT fix parallel/iteration order: for strict determinism
  you must also disable `parallel` (single-threaded solver) and take care that gameplay code
  iterates in a stable order. So "deterministic avian" = libm + single-thread + disciplined
  queries — a meaningful perf and ergonomics bill. *(corrected 2026-07-06: this is a
  cross-ARCHITECTURE dial only — libm transcendentals — irrelevant same-arch; and the "must also
  disable `parallel`" clause is stale, since avian 0.7's parallel dynamics step is order-invariant
  by construction — see the §2 correction.)*
- **lightyear `deterministic_replication`** (we already vendor it): lockstep-style, inputs-only
  replication for entities marked `Deterministic` — "not updated via state, but only via inputs"
  (lightyear_deterministic_replication-0.28.0/src/lib.rs:1-58). This is the architecture that
  *requires* the full determinism bill above. Its `rollback_resources: true` avian mode
  (contact-graph snapshotting) exists for this world, not ours.
- **lightyear state replication + prediction** (what we run): requires only *bounded* divergence
  — the authority continuously re-anchors clients; thresholds define the tolerance band. No
  determinism requirement at all, at the cost of bandwidth (state on the wire) and rollback CPU.
- **bevy core**: no determinism guarantees; query iteration order is explicitly unstable
  (long-standing discussion, bevyengine/bevy#2480). Anything order-sensitive must sort.

**The distinction this section originally lacked (added 2026-07-06): FORWARD vs REPLAY
determinism.** *Forward* determinism — same state + same inputs → same result — is what lockstep
needs, and what Box2D v3 / Box3D actually ship. *Replay* determinism — restore a snapshot,
resimulate, and land bit-identically on the original forward path — is what prediction + rollback
needs, and **no engine sells it today**: restore paths don't reproduce internal solver state
(contact graphs, warm-start impulses, broad-phase topology), so the replay walks a different
constraint order even when the forward sim is perfectly deterministic. avian issue #734 is the
open upstream thread; lightyear's `rollback_resources: true` avian mode (contact-graph
snapshotting, mentioned above) is the ecosystem's closest existing thing. Our own dominant
divergence term (§2's contact-restore row) is precisely a replay-determinism failure, not a
forward one.

## 4. What this means for Overmatch

1. **Our architecture choice bounds the damage — it does not sidestep the problem.**
   *(reframed 2026-07-06; the original heading was "already sidesteps the hard problem", which is
   wrong in spirit.)* State replication + prediction means the macOS-client-vs-Linux-server float
   gap (libm transcendentals, NEON vs AVX) shows up as a *higher background rollback rate*, not
   as desyncs or wrongness — the server is always right, clients converge by construction. That
   part stands. But within state replication, **determinism is the ROLLBACK-KILLER**: every bit
   of forward- and replay-determinism we gain makes corrections rarer, and rollbacks in a solo
   game are a defect indicator, target ~zero (ADR-0015). The Rocket League precedent is exactly
   this shape — server-authoritative + prediction, determinism pursued as the optimization that
   makes corrections rare. Divergence work is the *active agenda*, not a rejected alternative's
   leftover concern. The cloud feel test (Edgegap, first outing) is still also our cross-platform
   divergence measurement: compare `PredictionDiagnostics` rates against the same-machine
   baselines recorded in the spike log (~10/run smooth ground at 100 ms, ~135/20 s washboard).
2. **`enhanced-determinism` is a cheap dial we can turn if cross-platform rates disappoint.**
   It narrows the client/server transcendental gap even without full lockstep discipline —
   worth an A/B on the cloud test if rates are much worse than local. Perf cost lands on both
   sim ends; measure before adopting.
3. **Disabling `parallel` is NOT recommended** for us: it buys same-machine order stability we
   don't need (thresholds already absorb it) and costs solver throughput the 10v10 aspiration
   will want. *(corrected 2026-07-06: moot on both counts — parallel isn't the divergence source
   (§2 correction), and the throughput argument was miscast: the tank is ONE rigid body — the
   wheels are external forces, not solver constraints — so even 10v10 is ~20 bodies, trivial
   solver load.)*
4. **Lockstep/deterministic replication stays rejected** for this game: 16-wheel contact-rich
   physics per tank × cross-platform × Rust ecosystem's current maturity = the expensive path,
   and its bandwidth win doesn't matter at 1v1–3v3. Revisit only if state bandwidth ever becomes
   the binding constraint (10v10 on thin pipes).
5. **Keep the divergence budget honest with the tools built this session:** `SPIKE_SIM_LONG` +
   `SPIKE_SIM_WINDOWED` + `SPIKE_SIM_AIM_SWEEP` reproduce the workload classes; `nan_tripwire`
   names corruption; per-component thresholds are the tuning surface (velocity loose, position/
   rotation tight — discrete/gameplay-binary state must never be threshold-banded).

Open thread tracked in the spike log: the residual ~25%→2/8 bind-window crash window
(`update_ray_caster_positions` NaN under the bind-burst rollbacks; suspect DisableRollback-grace
inconsistency during replay) — pre-feel-test fix candidate. Also: the set-configuration hole
(`PhysicsTransformSystems::Propagate` orphaned when `PhysicsTransformPlugin` is disabled) looks
like an upstream lightyear_avian gap worth reporting.

## 5. 2026-07-06: the solo-divergence model, check starvation, and the two-layer doctrine

Architecture-review session, all measured, none conjectured. The doctrine's canonical home is
[ADR-0015](../adr/0015-divergence-doctrine.md); this section is the measured record.

**The solo-divergence model.** With one player (client vs a static world) there is nothing to
mispredict: client and server *should agree completely*, and **rollbacks in a solo game are a
defect indicator, target ~zero**. Measured divergence causes, ranked:

1. **Contact-adjacent solver noise across two ECS worlds.** Flat-ground cruise IS bit-exact
   client-vs-server (measured over ~880-tick windows, all fields). Divergence exists only at
   contact transients, where entity-index-keyed constraint ordering differs between the worlds
   and contact chaos amplifies last-bit float differences. Irreducible by config; needs upstream
   canonical ordering.
2. **THE DOMINANT TERM: the correction machinery manufactures its own divergence.** Rollback
   restore is imperfect — hull contact fails to re-form on the first replayed tick: hc=0 on 55%
   of replayed ticks at 80/10, 98.4% at lat0, with Δlv exactly −g·dt = 0.1533 m/s vertical at k=1
   while pose restore is near-exact (|Δp| p50 1.5 mm). Each rollback thus seeds mm-scale error
   that re-trips the 0.05 m position threshold ticks later — a self-feeding engine in
   hull-contact states, felt as "the hull-stuck tank never settles". The multi-meter replay
   errors are a SEPARATE machine: in-contact friction/load chaos through the replay (per-wheel
   load deltas to 5.8e6 N), anti-correlated with the hc-loss signature.
3. **Input-timing slips under jitter** — rare. Trigger attribution ~93% Position; the cause is
   state, not input.

**The check-starvation bug (FIXED this branch, 8ae795c).** lightyear 0.28: for an entity
confirmed every tick, the only rollback-mismatch detector runs at receive time, gated strictly on
`confirmed_tick < current_tick` (lightyear_prediction-0.28.0/src/registry.rs:426-428); skipped
checks are never retried, and the `check_rollback` unchanged-entity scan excludes always-confirmed
entities (rollback.rs:583). `InputDelayConfig::balanced()` at LAN/loopback RTT absorbs all latency
into input delay → zero prediction margin → the client sits level with the server → every update
skips → state rollback permanently, silently dead. Measured: 35–50 m divergence with fresh
authority arriving and **zero rollbacks**; 3,296 skip-trace lines in one run; the falsifier
`SPIKE_INPUT_DELAY_TICKS=0` capped divergence at 0.015–0.57 m. Fixed by `src/net/watchdog.rs`
(forced-rollback backstop). **Methodology consequence:** lat0 rollback counts from pre-watchdog
builds measured check starvation, not convergence — invalid as an A/B metric. lat0 |Δp| tick-row
divergence remains valid.

**The two-layer doctrine** (summary — ADR-0015 is canonical). **Layer 1, permanent, ours:**
contact and force laws must be continuous functions of pose/velocity (divergence continuity) —
shipped applications: sphere-cast suspension (washboard rollbacks −73%), friction static↔kinetic
blend + LuGre anchor relax (wedge storm 44+ with deterministic runaway → 1 in the good regime).
Binding for all future force laws, the track model included (sharp oriented box casts are the
known bug class — rounded shapes or ray/sphere stations). **Layer 2, deliberately removable, each
piece mapped to an upstream defect:** the watchdog ↔ lightyear's skipped-check-never-retried; the
coming contact-restore fix (slice 3, in flight) ↔ lightyear_avian restore path / avian #734; the
coarse thresholds + desync-only velocity bars ↔ the divergence those defects manufacture (tighten
toward the 1 cm reference values as divergence collapses). **Permanent-but-looks-like-
scaffolding:** the render-space error layer (`net/render_error.rs`) — multiplayer reintroduces
legitimate mispredictions forever, and the layer is how ANY correction is presented; it stays.
Strategy: ship the scaffold now (upstream timelines are not ours; #734 open since May 2025), file
upstream reports, keep workarounds small and documented-removable, let the continuity work
compound.

Web sources (background, treat as secondary): [DeepWiki avian determinism](https://deepwiki.com/avianphysics/avian/10.3-determinism) (machine-generated), [avian repo](https://github.com/avianphysics/avian), [bevy determinism discussion #2480](https://github.com/bevyengine/bevy/discussions/2480).
