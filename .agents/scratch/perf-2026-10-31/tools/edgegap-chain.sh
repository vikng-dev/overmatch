#!/bin/zsh
# edgegap-chain.sh <out-dir> <edgegap-version> [threads...] — the debris curve on Edgegap through the
# bay's scripts/perf/run-online-night.sh: OFFSCREEN, 29 bots, the alpha.4 client of bin/A4 (spans
# filtered off at their callsites, chrome writer at /dev/null), a palindrome over five lever levels.
# The zero is `1:100000:8192:512`: one piece at the first tick, then none, so the stats line carries
# the debris census (the bots' house rubble) under the same frozen cap as the armed levels.
set -eu
here="${0:A:h}"
export OFFSCREEN=1 BIN="$here/bin/A4" ASSET_ROOT="$here/bin/A4/root"
export RUST_LOG="bevy_ecs=warn,bevy_app=warn,bevy_render=warn" TRACE_CHROME=/dev/null OVERMATCH_MUTE=1
export BOTS=29 WARMUP_S="${WARMUP_S:-45}" DURATION_S="${DURATION_S:-180}" MAX_MIN="${MAX_MIN:-20}"
z=1:100000:8192:512 a=12:2:8192:512 b=50:2:8192:512 c=100:2:8192:512 d=200:2:8192:512
export DEBRIS="${DEBRIS:-$z $a $b $c $d $d $c $b $a $z}"
out="$1"; version="$2"; shift 2
if (( $# > 0 )); then export THREADS="$*"; fi
exec /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/perf/run-online-night.sh "$out" "$version"
