# 0033 — Geometry mipmapping: the declarative LOD ladder

Status: DRAFT (accepted doctrine 2026-08-01; pipeline v2 in flight — e₁ pending the
collapse-vs-planar measurement, then this ADR finalizes with the ratified number)
(amended by ADR-0035: packaging, primitive-granularity seam, per-tank certificate,
transcription deleted)
(amended by ADR-0036: directed search replaces exhaustive Pareto enumeration; render
gate retired; validity gates re-scoped to measurement needs)

Terms: `.agents/GLOSSARY.md` § Geometry LOD. This ADR records the decisions and the
adversarial-review triage that shaped them.

## Decision

Every renderable mesh gets a mip-style chain, generated at export, selected at runtime by
screen-space error. No manual LOD steps exist for any asset.

1. **L0 = the source**, defined precisely as the *stored modifier-free mesh* of the artist's
   object (instances realized, transforms normalized) — the surface that ships is the
   surface that anchors deviation, and it has to be the surface a human can open the file
   and inspect. Modifiers are not part of source identity: an export-bound object carries
   none, enabled or not (`L1.MODIFIER_STACK`), so there is no evaluated snapshot that
   differs from what the file holds. Source hygiene (re-encoding an over-tessellated
   source) is authoring, done by a human in the .blend, and the result becomes the source.
2. **One global octave grid**: deviation targets e₁·2^(N−1), e₁ declared once game-wide
   (ratified from the left-wall measurement of a reference asset). Per-asset chains are
   SPARSE SUBSETS of the grid: each asset starts at the first rung whose candidate sheds a
   declared fraction of triangles (skip-empty-rungs), and stops past the right wall. This
   keeps "no per-asset tuning" while admitting that intrinsic left walls differ per asset.
3. **Right wall** = the maximum renderable camera-to-surface distance (far plane / fog /
   streaming contract — the map's *diagonal* bounds it, not its radius). One runtime
   constant, consumed by generation.
4. **Switch thresholds derive from the measured pairwise deviation between ADJACENT levels**,
   not from source-relative deviation alone: adjacent levels may deviate from the source in
   opposite directions, so their separation can reach e_{N−1}+e_N = 1.5·e_N. Source-relative
   deviation remains the quality bound; pairwise deviation prices the pop.
5. **Generation searches unique integer triangle targets** (never a ratio binary-search:
   measured error is not monotone in collapse ratio, and plateaus alias many ratios to one
   mesh). Candidates are cached by output hash; the Pareto-minimal valid candidate wins.
   — superseded by ADR-0036: the search is directed with budgeted Boolean verdicts;
   the certificate, not enumeration, carries the guarantee.
6. **Certification order is sacred**: generate → cleanup → export → decode the shipped GLB →
   measure everything on those bytes. A metric taken before serialization certifies nothing.
7. **Gates, all numeric, all on the shipped bytes**:
   - two-way positional deviation with a BOUNDED sampling miss (per-patch covering-radius
     upper bound; accept only when the upper bound clears the target),
   - component-count survival (a vanished small part has near-zero Hausdorff distance),
   - mesh validity: duplicate faces, non-finite attributes, orientation consistency,
     manifold edges, tangent presence and UV-area degeneracy — re-certified AFTER cleanup,
   - **rendered-difference gate as the authoritative attribute check**: render candidate vs
     parent level at the switch distance under the shipped materials and lighting, gate on
     image difference. This subsumes normal/UV/tangent thresholds no fixed number can
     honestly claim (specular sensitivity depends on roughness, maps, lighting). Worst
     normal angle is reported as a diagnostic, not a gate. — retired by ADR-0036 (it was
     disarmed in effect and 41 % of the run); re-arm conditions recorded there.
8. **A versioned manifest is the single seam**: asset hashes, per-level measured deviations
   (source-relative and pairwise), gate results, generator + Blender version, level list.
   The runtime chain is generated/validated FROM the manifest — hand-written ledgers already
   drifted once (exporter narrated 223.7 m for the same level the runtime derives at
   335.5 m).
9. **Projection math**: `D = dev_m · height_px / (2·tan(vfov/2) · budget_px)` — the small
   angle shortcut is 5.5% wrong at the commander fov and dies here. Origin-anchored
   VisibilityRange under-reports surface distance by up to the asset's bounding radius;
   negligible for a 13 cm link, added as conservative slack (+r) for larger assets.
10. **Explicit refusals (fail loud, not degrade)**: skinned/morph meshes (bind-pose metrics
    do not bound deformed error), multi-material/multi-primitive meshes (the chain loader
    enforces one primitive; lifting this waits for an HLOD tier) — retired by ADR-0035:
    chains key on the primitive, matching the render atom. The exporter names the
    refusal; nothing silently passes through.
11. **Runtime selection stays discrete**: stepped threshold profiles recomputed on
    HUMAN-RATE events (optic toggle, settings change, resolution change) with hysteresis —
    bevy's render table retains every distinct VisibilityRange slot for app lifetime, so
    continuous per-frame threshold mutation is off the table by construction.

## Simplifier choice

Blender collapse is the v2 simplifier IF it passes the gates on the reference asset
(measurement in flight). Its known gap — no vertex/edge locks, no attribute-aware cost —
is covered by the gates plus refusal: an asset that cannot both simplify and pass declares
"unsupported by Blender collapse" rather than shipping a bad level. The named successor when
that refusal starts firing on real assets: meshoptimizer (locks, attribute weights,
deterministic) driven over the decoded GLB buffers. Toolchain is pinned: preflight asserts
the Blender version, the manifest records it, upgrades trigger a corpus regeneration review.

## Parked (with re-arm conditions)

- **HLOD / merged far proxies + impostors** — re-arms when far instances measurably cost
  (bigger maps); today 28 far tanks cost 0.04 ms in main view. `NoCpuCulling` benchmark is
  a standing cheap task.
- **Nanite-style cluster hierarchies, CGAL-certified Hausdorff, Simplygon casting** — out of
  scale; the doctrine's per-object bands + bounded sampling are sufficient at our asset
  sizes. Revisit only if per-object granularity measurably fails.
- **Crossfade** — abrupt stays; a sub-budget switch needs no fade. Boundary oscillation gets
  hysteresis/dwell tests when the budget setting lands.

## Consequences

The eyeball leaves the ladder entirely (rendered-difference gate replaces it); it remains
only where it always was — on the source, which the artist touches anyway. Legacy deleted
with this ADR's branch: the planar L0 export stage, scripts/tank/diet, the alt964 mesh,
every hand-written tier table. e₁ is low-stakes by construction: everything regenerates
mechanically and the manifest re-derives the runtime constants.
