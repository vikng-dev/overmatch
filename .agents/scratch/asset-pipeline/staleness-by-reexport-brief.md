# Brief: staleness is the re-export compare; the certificate loses `blend_digest`; CI verifies only what a push touched

Overmatch is a video game. This task is about its asset build pipeline (Python, shell, one Rust module, CI YAML); no game-logic change.

## Decision (owner, 2026-10-27 — implement, do not reopen)

The owner wants staleness caught without redundant build time. `build.py verify` (tank and tree) already re-exports each trio from its `.blend` and byte-compares the result against the committed glb; that compare IS the staleness check. The `blend_digest` field is a proxy on top that only adds false positives (a pipeline-script edit that changes no output invalidates every stamp) and a re-stamp ceremony. So:

1. **The certificate loses `blend_digest`.** Tank (`scripts/tank/trio.py`: `CERTIFICATE_FIELDS`, `certificate`, `coherence`; `scripts/tank/build.py`), tree (`scripts/prop/build.py`: `certificate`, `coherence`, `build`, `verify`), Rust (`src/geometry_lod/certificate.rs`: the struct field, the `#[expect(dead_code)]`, the malformed-certificate panic text, test fixtures). Delete every function, constant and test that exists ONLY to compute or test that digest — expected: both `blend_digest` functions, `species_projection`, `species_of`, `_canonical`, `GENERATOR_FIELDS` and its generator-reads test, `registry_projection`, `material_names`, `_kind`, `material_library`, the `StalenessInputs` classes, the digest cases of `VerifyRefusals`, and the tank build's `sys.path` append and `import ron` for `scripts/prop/ron.py`. **Prove each by grep before deleting**: anything with another caller stays (e.g. `search_digest`/`ladder_digest` key the chain cache — keep what the cache uses; `spec_projection` may feed the canon — check).
2. **`PIPELINE_SOURCES` stays** in both builds as the build's declared dependency list — what can move an artifact — now read only by the hook suite's coverage rows. Restate its comment and the two coverage rows' wording accordingly (no "hashes"). Drop `../prop/ron.py` from the tank list if the tank build no longer imports it.
3. **The two tracked certificates** (`assets/tiger_1/tiger_1.lod.json`, `assets/trees/fir_1/fir_1.lod.json`) lose the `blend_digest` line, edited so the bytes are EXACTLY what each build's writer now emits for the same document (prove it: parse the file, run the writer — `trio.certificate_bytes`-equivalent / prop `certificate_bytes` — on the parsed document, compare bytes; say so in the report). No Blender rebuild.
4. **CI verifies only what a push touched.** Today `assets_ci_scope` (`scripts/hooks/pushed_assets.sh`) answers affected yes/no and the Assets workflow's verify step (`.github/workflows/assets.yml`, "Verify every asset through the build") then re-exports EVERY trio. Make the scope also name the stems to verify, and the verify loop read that list:
   - a changed trio (the existing `assets_changed_trios` rule, both endpoints' trios as now) is verified;
   - the shared surface splits by lane: paths only the TANK build reads re-verify every tank trio; paths only the TREE build reads (`scripts/prop/`, `assets/trees/species.ron`) re-verify every tree trio; paths both read (`assets/materials/`, `scripts/toolchain.py`, the Rust consumer contract, …) re-verify all. Assign each path in the current `assets_shared_surface` pattern to a lane by reading which build actually reads it (imports, subprocess calls, files opened); a path you cannot place is shared — **fail toward running**, as the file's existing doc says;
   - `workflow_dispatch` and every "cannot know" case still verify everything, as now;
   - the test suites step still runs whole (it is seconds).
   Write the list to `$GITHUB_OUTPUT` the way the file already writes `affected` (a multiline output or a file path — your call, say why). Add rows to `scripts/hooks/test_pushed_assets.sh` for: a tree-only change selects only tree trios; a tank-lane path selects only tank trios; a shared path selects all; a changed trio alone selects that trio. Keep the pre-push half (`assets_push_targets`) consistent if it shares the predicate.
5. **Docs, current invariant only:** `.agents/GLOSSARY.md` certificate entry (drop `blend_digest`, say staleness is the re-export compare in `build.py verify`); ADR-0035 gets ONE line under its Status: "Amended 2026-10-27: the certificate carries no `blend_digest`; staleness is `build.py verify`'s re-export compare." and its five-fields sentence becomes four with the field gone; `scripts/lod/config.py`, `src/bake.rs:~920` and the `pushed_assets.sh` / `test_pushed_assets.sh` comments restated without the digest. Leave `.agents/scratch/**` alone.

What the re-export compare does NOT catch, by design (state it once, in the `verify` docstring of the tank build): a changed LOD configuration or ladder leaves the committed rungs honest (each rung's deviation is certified against the LOD0 it came from) but not re-cut; a changed species range leaves a committed tree valid but not re-drawn. Both are re-cut by `build`, never flagged by `verify`.

## Rules (binding)

- Work only in the worktree you were given, absolute paths. Commit locally; **never push**. Never `git checkout`, `git restore`, `git stash`, `git clean`.
- **No cargo outside the queue.** Rust compiles only through `scripts/serial-cargo … --features dev_ui,bitprobe`. Before running ANY Python or shell suite, put a `cargo` shim first on PATH that prints "cargo blocked" and exits 1 (`mkdir -p $SCRATCH/shim && printf '#!/bin/sh\necho cargo blocked >&2; exit 1\n' > $SCRATCH/shim/cargo && chmod +x …; PATH=$SCRATCH/shim:$PATH python3 …`): the asset door shells out to `cargo run`, which in a worktree compiles a whole fresh target. Never run `scripts/tank/test_asset_door.py` whole or `scripts/tank/test_glb_ktx2.py`, and never `build.py build`/`verify` on a real asset.
- **Never open a window or take the screen.** Blender only ever `--background`; prefer not to run it at all (the suites skip or mock it).
- Never bare `cargo fmt`; `rustfmt --edition 2024 <file>`; `cargo fmt --all --check` as a check is fine. Python keeps the files' 100-column ceiling.
- Do not spawn subagents of your own; if this needs a fan-out, report that and stop. Never end your turn while owning a live shell (detach long runs with `nohup … & echo $! > pid`, wait with chained foreground `timeout 590 bash -c 'while kill -0 $(cat pid) 2>/dev/null; do sleep 5; done'`).
- **Freeze line.** You may change only: `scripts/tank/{build,trio,test_build}.py`, `scripts/prop/{build,test_prop}.py`, `scripts/lod/config.py` (comments), `scripts/hooks/{pushed_assets.sh,test_pushed_assets.sh}`, `.github/workflows/assets.yml` (the scope step and the verify step only), `src/geometry_lod/certificate.rs`, `src/bake.rs` (one comment), the two `.lod.json` files, `.agents/GLOSSARY.md`, `.agents/docs/adr/0035-*.md`. **`scripts/encode-tank-ktx2.sh` is another lane's file this hour: do not touch it.** No new smoke, gate or guard rail beyond the hook rows named above.
- Comments state the current invariant, never edit history (no "was", "used to", "retired", issue numbers or dates in code comments). Label numbers MEASURED or DERIVED.
- Commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Gates

```
cargo fmt --all --check
scripts/serial-cargo clippy --locked --all-targets --features dev_ui,bitprobe -- -D warnings
scripts/serial-cargo test --lib --features dev_ui,bitprobe geometry_lod
PATH=<shim>:$PATH python3 scripts/tank/test_build.py
PATH=<shim>:$PATH python3 scripts/prop/test_prop.py
PATH=<shim>:$PATH python3 scripts/tank/test_report.py && python3 scripts/tank/test_refusals.py
sh scripts/hooks/test_pushed_assets.sh
```

## Report

What was deleted (functions, tests) and what you kept because it had another caller (with the caller); the lane assignment of every shared-surface path and the evidence for each; how the stem list reaches the verify step; the byte-identity proof for the two certificates; whether an existing test pins that `verify` refuses a view glb that differs from the re-export (name it; do not add one if absent, just say so); every gate's result line; the commit hash. No agent ids, no timings.
