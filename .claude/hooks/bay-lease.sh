#!/bin/bash
# WorktreeCreate: an agent that would get a worktree of its own gets a leased bay instead
# (scripts/bays/lib.sh), named after the hook's `name`, on branch `lane/<name>`. With no bay free,
# `lease` exits 1 and the spawn fails: no building agent runs outside the three bays.
#
# What the harness does, from https://code.claude.com/docs/en/hooks.md (WorktreeCreate,
# WorktreeRemove) and https://code.claude.com/docs/en/worktrees.md, read 2026-10-28 against Claude
# Code 2.1.312 (CLAIMED by those docs, not exercised here):
# - It fires for `isolation: "worktree"` subagents, workflow agents, `claude --worktree`,
#   EnterWorktree, background sessions and `claude remote-control` in worktree mode, and replaces
#   Claude Code's own `git worktree add` entirely (`.worktreeinclude` is not processed). It does not
#   fire for a `git worktree add` run through Bash.
# - Input: the common fields plus `name`, a slug (given, or generated like `bold-oak-a3f2`).
# - Output: the last non-empty stdout line is the path; it must be absolute, with no `.`/`..` and
#   no symlink below the repository root. Any nonzero exit fails the creation.
# - When a subagent or workflow agent finishes, Claude Code keeps the worktree this hook made and
#   does not fire WorktreeRemove, so the orchestrator releases a lane's bay after it merges the
#   lane. WorktreeRemove (bay-release.sh) fires for an interactive `--worktree` exit, ExitWorktree,
#   a background-session delete and remote-control cleanup.
# - The periodic worktree sweep keeps every worktree without Claude Code's marker in its git
#   metadata, and a hook-made one has none, so the sweep never removes a bay.
# Still open (the docs are silent):
# - Whether this hook's input carries `agent_id` when a subagent is spawned. Nothing here reads it.
# - Whether Claude Code takes its `git worktree lock` on a hook-made worktree while the agent runs.
#   The pool keeps its lease in its own file, not in a git lock, so either answer is safe;
#   `scripts/bays/status` shows any lock it finds.
# - Whether SubagentStop's `cwd` is the subagent's worktree. Release does not depend on it.
set -euo pipefail
name=$(jq -r '.name // empty')
[[ -n "$name" ]] || { echo "bay-lease: the hook input carries no name" >&2; exit 1; }
exec "$(cd "$(dirname "$0")/../.." && pwd -P)/scripts/bays/lease" "$name"
