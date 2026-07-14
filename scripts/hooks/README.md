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
| pre-push | fmt + clippy + ordinary tests, excluding the exact 30-receiver stress probe | CI fmt, clippy, and ordinary-test lanes | compiles; warm cache = tens of seconds |

A green pre-push does not certify the separately bounded CI stress lane; CI remains authoritative.

## Escape hatch

`git commit --no-verify` and `git push --no-verify` skip the hook for one operation. Use sparingly;
CI is still the backstop.

## Trimming

If clippy-on-every-push feels heavy, drop the clippy/test lines from `pre-push` and keep only the
fmt check — that still prevents the drift class that actually bit us; clippy/test then rely on CI.
