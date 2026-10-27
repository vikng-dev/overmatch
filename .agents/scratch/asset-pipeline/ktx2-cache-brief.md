# Brief: the texture encoder caches by content

Overmatch is a video game. This task is about its asset build pipeline (one shell script and a small test); no game-logic change.

## Why

The tank build's slowest stage is the texture encode inside `scripts/encode-tank-ktx2.sh`: one `basisu` UASTC invocation per embedded image, the Tiger's largest three being 14–17 MB PNGs. MEASURED on main: the tank `verify` spends 67.1 s and `build` 137.1 s in the door's export chain, which runs this encode; the script's own comment calls the encode "the ~60 s". Every build and every CI verify re-encodes identical images. The owner wants no redundant build time.

## The change

In `scripts/encode-tank-ktx2.sh`, per image:

- **Key** = sha256 over, in a fixed order with separators: the source image file's bytes, the image's role, the exact `basisu` argument list used for it (the role's `set --` arguments plus every common flag the invocation passes), and the encoder binary's identity (`"$BASISU" -version`'s first line). Not the script's own bytes: a comment edit must not invalidate the cache; any flag change must.
- **Cache dir** = `${OVERMATCH_KTX2_CACHE:-${XDG_CACHE_HOME:-$HOME/.cache}/overmatch/ktx2}`, one file per key (`<key>.ktx2`).
- **Hit**: copy the cached file to `$WORK/ktx2/$idx.ktx2`, print the existing `ktx2  ▸` line with `cached` in place of the size so the GUI adapter's progress count still advances one line per image.
- **Miss**: encode as now, then publish into the cache atomically (write a temp file inside the cache dir, `mv` it to the key) — two bakes can run at once (the script's work-dir comment says why), so a reader must never see a half-written file. A failed encode publishes nothing.
- A cache dir that cannot be created or written degrades to encoding without caching, with one line saying so; it never fails the bake.
- The output is byte-identical whether hit or miss: `build.py verify` byte-compares the re-export against the committed glb, which already depends on `basisu` being deterministic for one version and argument list; the key carries both.

Comment the cache in the script's own voice (current invariant, why the key holds what it holds). Keep the script POSIX `sh` like the rest of it.

## Test

A new `scripts/tank/test_ktx2_cache.sh` (POSIX `sh`, no Blender, no cargo), run against a small synthetic glb: write one with two or three tiny PNG textures in different roles (a baseColor and a normal map at least) using `python3` and the standard library only (look at `scripts/tank/glb_ktx2.py`'s `unpack`/repack to see what the script reads), with `OVERMATCH_BASISU` pointing at a wrapper that appends one line per invocation to a log and then execs the real `basisu` (`/opt/homebrew/bin/basisu` locally; resolve it from `command -v basisu` or an existing `OVERMATCH_BASISU`), and `OVERMATCH_KTX2_CACHE` at a fresh temp dir. Invariants, each shown to fail on a named mutant:
1. A second bake of the same input invokes the encoder zero times and produces a byte-identical output glb (mutant: key includes a per-run value → second run misses).
2. The same image under a different role misses (mutant: role left out of the key).
3. A changed encoder identity misses (wrapper reports a different `-version` line; mutant: version left out of the key).
Exit non-zero on any failed invariant, printing which. If no real `basisu` is on the machine, print a skip line and exit 0.

## Rules (binding)

- Work only in the worktree you were given, absolute paths. Commit locally; **never push**. Never `git checkout`, `git restore`, `git stash`, `git clean`.
- **No cargo at all, no Blender at all.** Nothing in this task needs either. Never run `build.py` on a real asset, `scripts/tank/test_asset_door.py` or `scripts/tank/test_glb_ktx2.py`.
- **Never open a window or take the screen.**
- Do not spawn subagents of your own; if this needs a fan-out, report that and stop. Never end your turn while owning a live shell.
- **Freeze line.** You may change only `scripts/encode-tank-ktx2.sh` and add `scripts/tank/test_ktx2_cache.sh`. Another lane is editing the build scripts, the hooks and the workflows this hour: do not touch them (the orchestrator wires the CI cache step and the suite line after merge).
- Comments state the current invariant, never edit history. Label numbers MEASURED or DERIVED.
- Commit message ends with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Report

The key's exact composition; the hit/miss line format; the test's three invariants with the mutant each failed on (and that you reverted it); the test's output; wall time of the synthetic bake cold vs warm (MEASURED); the commit hash. No agent ids, no timings beyond that measurement.
