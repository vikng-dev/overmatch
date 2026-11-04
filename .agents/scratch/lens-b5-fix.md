# Lens b5 fix disposition

Subject: the video game's environment destruction model and debris network boundary; game state only.

All assigned findings were verified against the current source. None was rejected.

| Finding | Disposition | Evidence |
|---|---|---|
| 1 — inverse inertia | Fixed in `construction: refuse unrepresentable inverse inertia (#106)` | Material-density admission checks finite positive mass and principal inertia, representable reciprocals and mass frame, and a finite nonzero inverse tensor. The refusal names construction, hull, scale and correction. MEASURED: the positive-mass fixture at scale `1e-4` has a zero inverse and failed the original gate test; both that case and mass underflow at `1e-20` are refused now. |
| 2 — finite speed overflow | Fixed in `net: refuse debris release sampling overflow (#106)` | Refuse `along_speed_m_s` when its doubled sampling range overflows, naming the file, field and fix. MEASURED: the `f32::MAX` fixture failed before the check and passes now; half-MAX remains admitted. |
| 3 — pose publication | Fixed in `net: publish owned debris poses on the compute pool (#106)` | Preserve the Publish phase, lifetime assertion and `set_if_neq`; validate distinct live carrier ownership, then use Bevy's unique-entity parallel mutable query. MEASURED: each of three fixture body orders published all 257 owners' exact poses; duplicate ownership is refused, retirement retains the final pose, and sleeping carriers stop changing. |
| 4 — lifecycle witness | Fixed in `net: witness malformed poses through match lifecycle (#106)` | The real-UDP witness enters through the shared ready-match harness, mounts the shipped protocol, lifecycle and debris installers, and reads `MatchClosed` without draining lifecycle input. It asserts link removal, the exact cause in the lobby's `Text`, and no reconnection beyond the owned policy window. MEASURED: the stronger assertion caught a manually spawned-link fixture that omitted lobby state; the corrected ready-match witness passes. |
| 5 — bake composition | Confirmed; handoff below | The test still calls `admit` directly. Its API evidence remains, but it does not exercise the bake's write entry point. |
| 6 — brevity | Fixed in `net: state debris policies once (#106)` | Remove the revision recap, point registration at its policy owner, and reuse the test's existing tolerance. |

## Ownership and execution order

DERIVED: the publication map certifies unique carrier keys before dispatch. Bevy's `EntitySet` query gives each key unique mutable access; each closure reads immutable replacement data and calls `set_if_neq` only on its own carrier. These writes commute under every worker assignment. The serial boundary certifies ownership and lifetime; it does not publish poses. Runtime cost is unmeasured; no pricing or captures were run.

The inverse behavior was verified in vendored avian3d 0.7 and its bevy_heavy 0.5 dependency. The parallel query, entity-map key iterator and message cursor APIs were verified in pinned Bevy 0.19 source.

## Bake handoff to construction-fix

`tools/src/bin/bake-constructions.rs::main` owns the production sequence: bake a spec, serialize it, call the game admission gate, then create the directory and write the artifact. The lane brief reserves `tools/` for `construction-fix`, so this lane makes no tool-source edits.

Expose that single-spec bake/admit/write entry point to the tool tests, with an output root supplied by the fixture. Feed it a valid tool construction whose serialized hull the game's law-block gate refuses; a whole thin slab between the tool's geometric resolution and the law's plane-membership tolerance is a candidate to verify. Assert the load-gate diagnostic and that no output directory or artifact was created. Removing admission from the production entry point must make the test fail. Keep the current direct admission test as API evidence.

The protocol file's existing backlog rows `net-21` and `net-22` concern the untouched servo publication/application functions and the driving scope. They remain open; this lane changes only the assigned revision/registration comments and does not claim those rows resolved.

## Validation

- MEASURED: the construction/headless/documentation gate passed 220 tests; seven measurement tests were ignored.
- MEASURED: the combined debris, match-exit and whole-tree guard gate passed 69 tests, including `doc_citations`, `ui_ascii`, `net_boundary` and `bake::`.
- Formatting and CI workspace/all-target clippy passed before each implementation commit; targeted tests were repeated at each commit boundary.
- Decisions: refuse unrepresentable release sampling before play; publish disjoint carrier writes through the existing compute pool; use the production ready-match lifecycle for the receive witness; respect the reserved tool and driving paths.
