# 0036 — The generator proves less: directed search, and the certificate as the only gate

Status: ACCEPTED 2026-08-12. Amends ADR-0033 (§5 search, §7 gates). Grounded in measurement
(`.agents/scratch/lod-gen-measure-2026-08-12.md`) and two head-to-head prototypes
(`lod-directed-prototype-2026-08-12.md`, `lod-meshopt-spike-2026-08-12.md`).

## The problem, measured

85 % of the reference 22.9-minute link run was measurement — render gate 41 %, deviation
search 40 % — against 0.05 % spent writing shipped meshes. The rung scan had no budget of
any kind: an indecisive deviation bracket froze and was simply retried (Turret_Decor,
2 784 tris: zero of twelve rungs in 47.6 minutes, ~6.5 h projected for rung 1). The
exhaustive staircase bought a global Pareto-minimality claim the certificate never needed —
and the 5 % search tolerance leaked it anyway: the shipped link rung 4 carries 254 triangles
while a valid 248-triangle candidate existed and passes.

## Decisions

1. **Directed search with budgeted Boolean verdicts.** Per rung, bisection over realizable
   triangle budgets; each candidate gets a verdict: PROVEN_FAIL at the first sampled witness
   over target (a rejection costs ~zero nodes), PROVEN_PASS when the covering-radius bound
   closes under target, and UNDECIDED at a deterministic node budget — **UNDECIDED counts as
   FAIL**. An undecidable candidate may only cost triangles, never honesty: nothing unproven
   ever ships, and the winner's full certification on shipped bytes is unchanged.
2. **The Pareto-minimality claim is retired, deliberately.** The contract is now
   "deterministically found, certified", not "provably cheapest". What the player depends on
   — the certified deviation upper bound at the switch distance — is untouched; what is
   given up is a fewest-triangles proof nobody renders, whose enforcement was the difference
   between unfinishable and minutes. Measured on the link: identical rungs, one cheaper.
3. **The render gate is DELETED.** It cost 41 % of the run and had been disarmed in effect
   since the corpus was cut (Blender cannot decode the shipped KTX2 textures, so it compared
   renders under the .blend's materials — running, testing nothing). What remains, free: the
   geometric certificate, the numeric validity checks, and worst-normal-angle recorded per
   level as a diagnostic, not a gate. **Re-arm:** a level swap seen to pop in play, or a
   toolchain change that touches attribute handling with nothing else covering it — then a
   rendered comparison returns as a deliberate ratification audit, decoding shipped bytes.
4. **Lane validity gates re-scope to what the measurement needs**: finite attributes,
   non-degenerate, non-empty, and component-count survival (a vanished small part has
   near-zero Hausdorff distance, so the deviation bound alone cannot see it — ADR-0033 §7).
   Manifoldness is the armor pipeline's law, not this lane's —
   the deviation bound polices decimator misbehavior by construction (a mangled region
   deviates and fails; an undeviating one is invisible by definition). The UV/tangent checks
   served the deleted render gate; untextured physics-vocabulary meshes are legal. With
   ADR-0035's per-primitive seam this makes every unique tiger mesh eligible: the "37
   ineligible meshes" were an artifact of gates serving retired consumers.
5. **Blender collapse stays the engine.** meshoptimizer was measured and parked: its error
   estimate is not a bound (1.55–17.6× under the certified truth), at equal certified error
   its candidates need ~19.5 % more triangles, and its output moves under FMA contraction —
   different compilers ship different meshes. **Re-arm:** a far-LOD need below Blender's
   topology floor (meshopt reached 46 tris where Blender floors at 140), or Blender collapse
   refusing real assets.
6. **The acceptance bound gets the convex upgrade.** Measured law: acceptance cost scales as
   target⁻² and is a property of mesh and rung, not candidate (link rung 1: ~490 k nodes;
   a 72-triangle mesh still cost 2.8 min). The upgrade: for a fixed target triangle the
   distance is convex over a source triangle, so its maximum sits at a vertex — nine
   point-triangle distances bound a whole triangle pair, and near-coplanar regions accept
   with zero subdivision. **The rebuild slice carries a hard success gate:** measured
   full-tank cold projection within the minutes budget, or stop-and-raise before slice 2
   builds on it.

## Recorded forks (decided in the rebuild slice, not silently)

- **Node budget shape:** flat (one constant) vs scale-free (∝ (diagonal/e)², capped) — flat
  makes fine-rung availability depend on mesh size. Recommendation on record: scale-free.
  Constraint either way (review finding): the cap is a pinned constant, never a lever for
  the wall-clock gate — a budget-exhausted verdict may cost a rung, and that loss is
  recorded in the report distinctly from structural infeasibility, so fidelity traded for
  time is always loud.
- **The oracle self-consistency audit dies with the staircase** it re-probed. Its two real
  catches were decimator nondeterminism — still covered where it matters: winners are
  generated twice and must reproduce byte-identically at export.
