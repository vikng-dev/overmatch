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
| pre-push | the asset door, the LOD chain, then fmt + clippy + ordinary tests, excluding the exact 30-receiver stress probe | CI's asset, LOD, fmt, clippy, and ordinary-test lanes | a Blender launch + a MEASURED minute of texture encode per changed asset; otherwise compile, warm cache = tens of seconds |

The **asset lane** runs `scripts/tank/asset_door.py verify` — the one door — on every asset the push
changes, and on every asset it discovers when the push changes the shared surface the door's verdict
is computed from (the material library, the toolchain pin, the source pass, the door, the encoder,
the derivation verifier, the Rust consumer contract). `verify` re-cuts the asset from its own stored
source and holds the tracked glb against the result section by section — byte for byte wherever the
chain is deterministic, and by stated KTX2 header facts over the texture payloads, whose bytes
`basisu` varies with the SIMD it was built for. That subsumes the mip gate this lane replaced: a
tracked glb the compare law holds against a candidate the derivation verifier passed is mipped KTX2
by construction.

Assets are **discovered**, never listed: an asset is a sibling trio `<id>.blend`, `<id>.tank.ron`,
`<id>.glb` in one directory. Adding a second vehicle needs no hook edit. Discovery and hydration
live in `scripts/hooks/pushed_assets.sh`, sourced by the hook and driven over synthetic revisions by
`sh scripts/hooks/test_pushed_assets.sh` — by hand, and by the CI slice when there is one.

It reads the **pushed revisions**, not the work tree: the trio's bytes come out of the pushed commit
(its git-lfs pointers resolved against this clone's object store), because the work tree is a
different, mutable thing from what the remote is about to receive. If an object or the linked
`assets/materials/materials.blend` is not there, the lane REFUSES and names what is missing — it
never falls back to the work tree and never passes on absence.

A green pre-push does not certify the separately bounded CI stress lane; CI remains authoritative.

## Escape hatch — name the lane, never `--no-verify`

```sh
OVERMATCH_SKIP=lod git push          # skip one gate lane
OVERMATCH_SKIP=lod,test git push     # or several: assets, lod, fmt, clippy, test
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
