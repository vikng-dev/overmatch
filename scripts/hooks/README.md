# Git hooks

Local pre-commit / pre-push gates that mirror CI's ordinary checks, so formatting/lint drift is
caught before it reaches `main`. (MEASURED: CI sat red for two days in July 2026 from formatting
drift that a pre-commit hook would have blocked at the source.)

## One-time setup (per clone)

```sh
git config core.hooksPath scripts/hooks
```

That's the whole wire-up. Coming from JS: there is no `npm install`/husky `postinstall` step in
cargo, so this single `git config` replaces it. The path is **relative and versioned**, so it
resolves per working tree — every `git worktree` uses its own copy automatically, and the hooks
travel with the repo instead of living in the un-versioned `.git/hooks`.

## What runs

| Hook | Command | Mirrors | Cost |
|------|---------|---------|------|
| pre-commit | `cargo fmt --all --check` | CI `fmt` job | no compile |
| pre-push | tank glb mip gate, then fmt + clippy + ordinary tests, excluding the exact 30-receiver stress probe | release workflow's KTX2 gate; CI fmt, clippy, and ordinary-test lanes | ~30 ms + compile; warm cache = tens of seconds |

The **tank glb mip gate** (`scripts/tank/glb_ktx2.py verify`) asserts every texture embedded in
`assets/tiger_1/tiger_1.glb` is mipped KTX2. A glb exported straight out of Blender carries PNG/JPEG,
which bevy uploads with one mip level — a shimmering tank that looks like a renderer bug. The bake is
folded into the one asset door (`scripts/tank/asset_door.py`), so this only fires on a glb that
came from somewhere else; `release.yml` runs the same check, which is the gate `--no-verify` cannot
skip. It reads the committed bytes out of the local git-lfs object cache, not the work tree.

A green pre-push does not certify the separately bounded CI stress lane; CI remains authoritative.

## Escape hatch — name the lane, never `--no-verify`

```sh
OVERMATCH_SKIP=lod git push          # skip one gate lane
OVERMATCH_SKIP=lod,test git push     # or several: mip, lod, fmt, clippy, test
```

Every skip prints loudly, and CI re-runs the skipped lane on the pushed commit.

**Do not use `git push --no-verify`**: the pre-push hook is the only git-lfs transport
(`core.hooksPath` replaces `.git/hooks`, so the hook git-lfs installs never runs). Skipping the
whole hook pushes pointers without objects. Recovery: `git lfs push --all origin` — the plain
per-branch form computes an empty delta and uploads nothing.

(`git commit --no-verify` remains fine — pre-commit is only a format check.)

## Trimming

If clippy-on-every-push feels heavy, drop the clippy/test lines from `pre-push` and keep only the
fmt check — that still prevents the drift class that actually bit us; clippy/test then rely on CI.
