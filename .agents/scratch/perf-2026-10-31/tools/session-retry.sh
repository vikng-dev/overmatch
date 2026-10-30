#!/usr/bin/env bash
# session-retry.sh — scripts/edgegap/session.sh, with `logs` and `metrics` retried (20, 40, 60 s
# apart) when Edgegap answers with a refusal such as HTTP 429 (its rate limit). Every other
# subcommand passes straight through. Its stdout is session.sh's.
set +x
set -uo pipefail
real=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/edgegap/session.sh
case "${1:-}" in
  logs | metrics)
    for wait_s in 0 20 40 60; do
      sleep "$wait_s"
      if out="$("$real" "$@")"; then
        printf '%s\n' "$out"
        exit 0
      fi
      echo "session-retry: $1 failed, retrying" >&2
    done
    exit 1
    ;;
  *) exec "$real" "$@" ;;
esac
