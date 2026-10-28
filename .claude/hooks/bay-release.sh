#!/bin/bash
# WorktreeRemove: Claude Code is done with a worktree bay-lease.sh handed it, so the bay is
# released, never deleted: its target/ is the pool's point (scripts/bays/lib.sh).
#
# hooks.md (WorktreeRemove, CLAIMED): exit 0 counts as removed and Claude Code reads nothing else, so
# the directory may stay; a nonzero exit with the directory still present fails the removal and
# leaves it on disk. `release` exits 1 for a bay holding uncommitted work, so that work stays where
# it is, and for any path that is not a bay.
set -euo pipefail
path=$(jq -r '.worktree_path // empty')
[[ -n "$path" ]] || { echo "bay-release: the hook input carries no worktree_path" >&2; exit 1; }
exec "$(cd "$(dirname "$0")/../.." && pwd -P)/scripts/bays/release" "$path"
