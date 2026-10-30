#!/bin/zsh
# local-curve.sh <out-dir> — the local debris curve: the bay's scripts/perf/run-debris-stress.sh
# with the alpha.3 profiling binaries of bin/A (trace_chrome compiled in, spans filtered off at
# their callsites by RUST_LOG, the chrome writer pointed at /dev/null), 29 bots + 1 idle GPU-less
# client, a palindrome over five lever levels. Run under the machine lock.
set -eu
here="${0:A:h}"
export BIN="$here/bin/A" ASSET_ROOT="$here/bin/root"
export RUST_LOG="bevy_ecs=warn,bevy_app=warn" TRACE_CHROME=/dev/null OVERMATCH_MUTE=1
export WARMUP_S="${WARMUP_S:-45}" DURATION_S="${DURATION_S:-180}"
L=(off 12:2:8192:512 50:2:8192:512 100:2:8192:512 200:2:8192:512)
exec /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/perf/run-debris-stress.sh "$1" \
  $L[1] $L[2] $L[3] $L[4] $L[5] $L[5] $L[4] $L[3] $L[2] $L[1]
