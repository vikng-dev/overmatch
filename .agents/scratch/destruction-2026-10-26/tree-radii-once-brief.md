# Brief: a tree's trunk stores each boundary radius once (GitHub #141)

Overmatch is a video game. This task is about a tree asset's schema: the trunk a tree in the game's destruction model is built from. No change to the damage model's laws, the tree state word or the wire.

## Decision (owner, 2026-10-27 — implement, do not reopen)

A trunk segment authors `height_m`, `r_bottom_m` and `r_top_m`, so every inner boundary radius is stored twice (segment k's top, segment k+1's bottom). The runtime loader refuses a mismatch by exact f32 equality, the build tolerates 1e-4 m, and a tree the build certifies can stop the map loading. Instead of reconciling the two checks, make the mismatch unrepresentable:

```ron
TreeAsset(
    species: "norway_spruce",
    segments: [
        Segment(height_m: 1.8013, r_bottom_m: 0.208),
        Segment(height_m: 1.8013, r_bottom_m: 0.1969),
        // …
    ],
    r_tip_m: 0.0123,
)
```

Segment k's top radius is segment k+1's `r_bottom_m`, the last segment's is `r_tip_m`. Both adjacency checks (runtime and build) and the tests that drove them are deleted; every other check stays (positive finite dimensions, the segment count bound, the build's hold of each segment's height and both ring radii against the glb).

## Scope

1. **Runtime, `src/trees.rs`.** `Segment { height_m, r_bottom_m }`, `TreeAsset { species, segments, r_tip_m }`, both still `deny_unknown_fields`. Give the trunk ONE way to read a segment's top radius (a method on the resolved kind, or a resolved ring list — your call, say why), used by `radius_at`, `segment_collider` and anything else that read `r_top_m`. `r_tip_m` must be positive and finite (the refusal names it). Delete the continuity check and its test; keep the malformed-asset test table complete for the new field (a missing or non-positive `r_tip_m` is refused by name). Fixtures (`fixture_asset` and any other `Segment { … }` literal anywhere in `src/`) move to the new shape with the same radii, so every pinned bit stays where it is: the fixture's resolved geometry is unchanged, and the pins prove it (they must pass untouched).
2. **Generator, `scripts/prop/plan.py` and `scripts/prop/gen_tree.py`.** The plan may keep whatever in-memory rows suit the mesh builder; the RON it writes carries each boundary once in the shape above. `gen_tree.py` keeps building the same tubes and caps (a segment's top ring at the next boundary's radius, the tip's at `r_tip_m`).
3. **Build, `scripts/prop/build.py`.** `ron_failures` reads the new shape: drop the adjacency comparison, hold each segment's bottom ring against its `r_bottom_m` and its top ring against the next boundary's radius (or `r_tip_m`) within `TOL_M` as now. Update the module docstring's list of what the build checks.
4. **Tests, `scripts/prop/test_prop.py`.** Move fixtures to the new shape; delete the adjacency cases; keep each glb-hold case naming the field it moved.
5. **The asset, `assets/trees/fir_1/fir_1.tree.ron`.** Rewritten to the new shape, same tree, no re-roll: prove the bytes are EXACTLY what the generator's RON writer now emits for this tree's plan (run the pure-Python writer in `plan.py` on the same species, seed and inputs the header records; no Blender), and that every boundary radius equals the old file's. The glb and the `.blend` do not change. Do NOT touch `fir_1.lod.json`: its `sim_ron_sha` goes stale by design and the orchestrator re-stamps it on main.
6. **Docs.** `.agents/GLOSSARY.md` if it states the segment schema; the module docs in `src/trees.rs` and `scripts/prop/build.py`. Leave `.agents/scratch/**` alone (the orchestrator amends the slice contract).

## Rules (binding)

- Work only in the worktree you were given, absolute paths. Commit locally; **never push**. Never `git checkout`, `git restore`, `git stash`, `git clean`.
- Rust compiles only through `scripts/serial-cargo … --features dev_ui,bitprobe`. Before any Python suite, put a `cargo` shim that exits 1 first on PATH (`mkdir -p $SCRATCH/shim && printf '#!/bin/sh\necho cargo blocked >&2; exit 1\n' > $SCRATCH/shim/cargo && chmod +x $SCRATCH/shim/cargo`).
- **No Blender, no binary launches; never open a window or take the screen.** The Blender-gated prop cases may skip; the orchestrator runs the real build on main.
- Never bare `cargo fmt`; `rustfmt --edition 2024 <file>`; `cargo fmt --all --check` as a check is fine.
- Do not spawn subagents of your own; if this needs a fan-out, report that and stop. Never end your turn while owning a live shell (detach with `nohup … & echo $! > pid`, wait with chained foreground `timeout 590 bash -c 'while kill -0 $(cat pid) 2>/dev/null; do sleep 5; done'`).
- **Freeze line.** You may change only: `src/trees.rs` (and any other `src/` file whose only change is a `Segment`/`TreeAsset` literal), `scripts/prop/{plan,gen_tree,build,test_prop}.py`, `assets/trees/fir_1/fir_1.tree.ron`, `.agents/GLOSSARY.md`. No new smoke, gate or guard rail.
- Comments state the current invariant, never edit history (no "was", "used to", "now", issue numbers or dates). Label numbers MEASURED or DERIVED.
- Commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Gates

```
cargo fmt --all --check
scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings
scripts/serial-cargo test --lib --features dev_ui,bitprobe -- trees net::destruction scatter map
PATH=<shim>:$PATH python3 scripts/prop/test_prop.py
```

plus the headless tests that load trees (`grep -n 'trees::' src/headless_test.rs`; run them by name).

## Report

The top-radius accessor you chose and why; every file changed; the byte-identity proof for `fir_1.tree.ron` and the boundary-radius equality; that every pinned test passed untouched; the gate result lines; the commit hash. No agent ids, no timings.
