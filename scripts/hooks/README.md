# Git hooks

Two local hooks, and between them they hold exactly one thing CI cannot: the git-lfs upload.

## One-time setup (per clone)

```sh
git config core.hooksPath scripts/hooks
```

That's the whole wire-up. Coming from JS: there is no `npm install`/husky `postinstall` step in
cargo, so this single `git config` replaces it. The path is **relative and versioned**, so it
resolves per working tree — every `git worktree` uses its own copy automatically, and the hooks
travel with the repo instead of living in the un-versioned `.git/hooks`.

## What runs

| Hook | Command | Cost |
|------|---------|------|
| pre-commit | `cargo fmt --all --check` | no compile |
| pre-push | `git lfs pre-push` — the LFS upload, and nothing else | the objects' upload time |

**Everything else is CI's, post-hoc, on the pushed commit**: fmt, clippy, the test suite, and the
asset door. The local gates that used to run here (fmt, clippy, the door, and `OVERMATCH_FULL=1`'s
full `cargo test`, all steerable with `OVERMATCH_SKIP`) are retired — at one to two developers and
no players, minutes of local wall clock to pre-empt a red badge is the expensive way to learn what
CI says anyway.

## Never `git push --no-verify`

The pre-push hook is the only git-lfs transport (`core.hooksPath` replaces `.git/hooks`, so the
hook git-lfs installs there never runs). Skipping the whole hook pushes pointers without objects,
and CI then dies at "Git LFS pull (R2)" with "object not found". Recovery: `git lfs push --all
origin` — the plain per-branch form computes an empty delta and uploads nothing.

(`git commit --no-verify` remains fine — pre-commit is only a format check.)

## `pushed_assets.sh`

Asset discovery, selection and hydration — an asset is a sibling trio `<id>.blend`,
`<id>.tank.ron`, `<id>.glb` in one directory, so adding a second vehicle needs no edit anywhere.
It lives here for historical reasons and its remaining consumer is **CI's assets job**, which asks
its two predicates over a push's whole range to decide whether to run the ~35 minute re-cut at all.
`sh scripts/hooks/test_pushed_assets.sh` drives all of it over synthetic revisions, plus the
pre-push hook over a synthetic push.

## Trimming

Trimmed three times, deliberately: `cargo test` and the LOD lane moved behind `OVERMATCH_FULL=1`
(2026-08-10), the LOD lane retired outright with the global manifest it verified (ADR 0035), and
then every gate but the transport was cut (2026-08-16) when release confidence moved to the
release pipeline's own artifact smoke.

The inventory of checks is **closed**. A new gate here or in CI needs a paid incident or a
demonstrated hole, not a plausible risk.
