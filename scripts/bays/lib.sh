# lib.sh — the bay pool: exactly three fixed-path worktrees, each keeping its own warm target/.
#
# Sourced by `lease`, `release` and `status` beside it; not run on its own.
#
#     scripts/bays/lease <lane>            take a free bay, print its path   (exit 1: none free)
#     scripts/bays/release [--force] <bay> free a bay: bay-2, 2, or its path (exit 1: it holds work)
#     scripts/bays/status                  each bay's holder, branch, age and cleanliness
#
# A bay is the linked worktree `.claude/worktrees/bay-N` (N = 1..3) under the MAIN checkout's root,
# which is resolved from the common git dir, so a copy of these scripts in any worktree drives the
# same pool. Its path never changes, and that is the point: rustc discards a crate's incremental
# cache when the compiler's working directory changes (the working directory is a tracked option),
# so a worktree at a new path compiles this crate from scratch while a bay recompiles only what a
# lane changed. `target/` sits inside the bay (`/target` is ignored), so its path is as fixed as the
# bay's. MEASURED 2026-10-28 at add9115, `scripts/serial-cargo test --lib --features
# dev_ui,bitprobe --no-run`, build lock free: a new bay's first build, warmed from main's target/,
# 91.1 s; the same bay released and leased again at the same main, 1.1 s.
#
# The invariants, each one a test in test_bays.sh:
#   1. Three bays, no overflow. A lease with no bay free exits 1 naming every holder; nothing is
#      created beyond bay-3.
#   2. Work is never destroyed. A bay with uncommitted changes or untracked files (`git status
#      --porcelain --untracked-files=all` non-empty; target/ is ignored, so a build never counts)
#      is never recycled, and `release` refuses it. `release --force`, the orchestrator's call
#      after it has looked, first commits that work to `refs/bays/salvage/bay-N-<time>` and only
#      then clears it. Ignored files other than target/ are scratch by the repository's own
#      definition and go at recycle.
#   3. A lease recycles to main's tip: `git switch -C lane/<lane> main`, then
#      `git clean -ffdx -e /target`. git rewrites only the files that differ, so every other file
#      keeps its mtime and cargo's fingerprints stay true. It never resets a `lane/<lane>` that
#      holds commits main lacks; it refuses that lane name instead.
#   4. Lease and release run under one `lockf` on `<common git dir>/bays.lock`, so two concurrent
#      leases never take the same bay.
#
# Lease record: `<lane> <epoch>` in `bay-lease` inside the bay's own git admin dir
# (`.git/worktrees/<id>/`), which goes with the worktree if the worktree is ever removed. It is not
# a `git worktree lock`: Claude Code takes that lock on the worktree an agent runs in and drops it
# when the agent finishes (worktrees.md, "Worktree locks and manual removal"; whether it does so on
# a hook-made worktree is undocumented), so a lock could be taken from under a lease or refused to
# Claude Code. `release` detaches the bay's HEAD, so `lane/<lane>` keeps its commits and is free to
# be merged and deleted.
#
# Cargo.lock: when main's Cargo.lock differs from the bay's checked-out one, the lease deletes the
# bay's target/ and the first `scripts/serial-cargo` call re-warms it from main's
# (`scripts/disk/warm-target.sh`). The checked-out copy stands for the one the target last built,
# because a lane commits only what it has built (clippy runs before every commit).
#
# LFS: git runs with GIT_LFS_SKIP_SMUDGE=1, so a lease never touches the network, and then
# `git lfs checkout` writes every asset whose object is already in the shared store.
#
# macOS only, like scripts/serial-cargo: the lock is /usr/bin/lockf.

BAYS_COUNT=3

bays_here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
bays_common=$(git -C "$bays_here" rev-parse --path-format=absolute --git-common-dir)
bays_root=$(dirname "$bays_common")
bays_pool=$bays_root/.claude/worktrees
bays_lock=$bays_common/bays.lock

die() {
  printf '%s: %s\n' "$(basename "$0")" "$*" >&2
  exit 1
}

[[ $(basename "$bays_common") == .git ]] || die "$bays_common is not a checkout's .git; the pool lives under the main checkout"

# Re-run the calling script under the pool lock. Returns only inside the re-run.
bays_locked() {
  [[ ${BAYS_LOCK_HELD:-} == "$bays_lock" ]] && return 0
  BAYS_LOCK_HELD=$bays_lock exec /usr/bin/lockf -k -t "${BAYS_LOCK_WAIT_S:-120}" "$bays_lock" "$0" "$@"
}

bay_path() { printf '%s/bay-%s\n' "$bays_pool" "$1"; }

# The bay's own git admin dir. Fails when the path is not a live worktree of this repository whose
# top level is the path itself; a directory that lost its `.git` file would otherwise resolve to the
# main checkout, and every later git call would land there.
bay_admin() {
  local top
  [[ -d $1 ]] || return 1
  top=$(git -C "$1" rev-parse --show-toplevel 2>/dev/null) || return 1
  [[ $top == "$1" ]] || return 1
  [[ $(git -C "$1" rev-parse --path-format=absolute --git-common-dir) == "$bays_common" ]] || return 1
  git -C "$1" rev-parse --absolute-git-dir
}

# What a recycle would lose, one path per line; empty when the bay is clean.
bay_dirt() { git -C "$1" --no-optional-locks status --porcelain --untracked-files=all; }

# A bay named as `bay-2`, `2` or its path, printed as its path. Fails for anything else.
bay_resolve() {
  local n=${1%/} real
  n=${n##*/}
  n=${n#bay-}
  [[ $n =~ ^[1-9][0-9]*$ ]] && ((n <= BAYS_COUNT)) || return 1
  if [[ $1 == */* ]]; then
    real=$(cd "$1" 2>/dev/null && pwd -P) || return 1
    [[ $real == "$(bay_path "$n")" ]] || return 1
  fi
  bay_path "$n"
}

# Seconds since an epoch, as `1h05m`.
bay_age() {
  local s=$(($(date +%s) - ${1:-0}))
  printf '%dh%02dm' $((s / 3600)) $((s % 3600 / 60))
}
