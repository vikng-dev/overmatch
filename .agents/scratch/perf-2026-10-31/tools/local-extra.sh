#!/bin/zsh
# local-extra.sh <out-dir> <lever>... — more local debris conditions, the same setup as local-curve.sh
# (alpha.3 bin/A, spans filtered, 29 bots + 1 idle client, 45 s warmup, 180 s window). Under the lock.
set -eu
here="${0:A:h}"
export BIN="$here/bin/A" ASSET_ROOT="$here/bin/root"
export RUST_LOG="bevy_ecs=warn,bevy_app=warn" TRACE_CHROME=/dev/null OVERMATCH_MUTE=1
export WARMUP_S="${WARMUP_S:-45}" DURATION_S="${DURATION_S:-180}"
out="$1"; shift
exec /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/perf/run-debris-stress.sh "$out" "$@"
