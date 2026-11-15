#!/bin/zsh
# tick-trace.sh — per-system cost per server tick, from Bevy's chrome trace (game state only).
#
#   usage: scripts/perf/tick-trace.sh <out-dir> [label]
#   build: CARGO_PROFILE_PERF_STRIP=false scripts/serial-cargo build --locked --profile perf \
#            --features dev_ui,bitprobe,bevy/trace_chrome --bin overmatch --bin overmatch-server
#   env:   BIN (default target/perf), BOTS (default 29), EMPTY_WARMUP_S (5), EMPTY_S (30),
#          COMBAT_WARMUP_S (15), COMBAT_S (120)
#
# One run, two windows: the server before any client joins (its bots spawned at launch, the battle
# not yet in combat), then one parked scripted client joins, the battle enters combat, and the
# combat window opens after the client's input slot. Each window is profiled into
# <out>/<label>.<phase>.profile.json by scripts/perf/trace_profile.py. The trace is deleted once
# both profiles are written, unless KEEP_TRACE=1 (a 29-bot trace is tens of GB); a failed profile
# keeps it.
#
# A `perf` time compares with other `perf` times only. Before/after pairs run back to back in one
# lock hold, A B B A: the machine mixes core types and one run's tick p50 spans a factor of two.
set -eu
zmodload zsh/datetime

if [[ "${1:-}" != --holding-lock ]]; then
  exec /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$0" --holding-lock "$@"
fi
shift
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?usage: tick-trace.sh <out-dir> [label]}"; LABEL="${2:-$(git -C "$REPO" rev-parse --short HEAD)}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
BIN="${BIN:-target/perf}"
SERVER="$REPO/$BIN/overmatch-server"; CLIENT="$REPO/$BIN/overmatch"
BOTS="${BOTS:-29}"
EMPTY_WARMUP_S="${EMPTY_WARMUP_S:-5}"; EMPTY_S="${EMPTY_S:-30}"
COMBAT_WARMUP_S="${COMBAT_WARMUP_S:-15}"; COMBAT_S="${COMBAT_S:-120}"
[[ -x "$SERVER" && -x "$CLIENT" ]] || { echo "build first (see the header)" >&2; exit 1; }
mkdir -p "$OUT"
TRACE="$OUT/$LABEL.trace.json"; SLOG="$OUT/$LABEL.server.log"; CLOG="$OUT/$LABEL.client.log"
SCRIPT="$OUT/$LABEL.harness.ron"
# The client parks for both warm-ups and the combat window, plus slack; 64 Hz fixed ticks.
print "(drive: Idle, ticks: $(( (COMBAT_WARMUP_S + COMBAT_S + 60) * 64 )))" >"$SCRIPT"

SPID= CPID=
cleanup() {
  [[ -z "$CPID" ]] || { kill "$CPID" 2>/dev/null || true; wait "$CPID" 2>/dev/null || true; }
  if [[ -n "$SPID" ]]; then
    kill -INT "$SPID" 2>/dev/null || true
    for _ in {1..40}; do kill -0 "$SPID" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$SPID" 2>/dev/null || true; wait "$SPID" 2>/dev/null || true
  fi
  SPID= CPID=
}
trap cleanup EXIT; trap 'exit 130' INT; trap 'exit 143' TERM

if lsof -nP -iUDP:5888 >/dev/null 2>&1; then echo "UDP 5888 is busy" >&2; exit 1; fi
window() { # phase w0 w1
  printf '{"label":"%s","phase":"%s","w0_us":%.0f,"w1_us":%.0f,"bots":%s}\n' \
    "$LABEL" "$1" "$(( ($2 - launch) * 1e6 ))" "$(( ($3 - launch) * 1e6 ))" "$BOTS" \
    >"$OUT/$LABEL.$1.window.json"
}

launch=$EPOCHREALTIME
(cd "$REPO" && exec env OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
   OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1 OVERMATCH_BOT="$BOTS" \
   TRACE_CHROME="$TRACE" "$SERVER" >"$SLOG" 2>&1) &
SPID=$!
for _ in {1..300}; do grep -q 'listening on' "$SLOG" 2>/dev/null && break; sleep 0.2; done
grep -q 'listening on' "$SLOG" || { echo "the server never listened; see $SLOG" >&2; exit 1; }

sleep "$EMPTY_WARMUP_S"; w0=$EPOCHREALTIME; sleep "$EMPTY_S"; w1=$EPOCHREALTIME
kill -0 "$SPID" || { echo "the server died in the empty window, before any client" >&2; exit 1; }
window empty "$w0" "$w1"

# The client is the same build with its spans filtered off at their callsites; its own trace file
# is the near-empty proof.
(cd "$REPO" && exec env OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless OVERMATCH_MUTE=1 \
   OVERMATCH_SERVER=127.0.0.1:5888 OVERMATCH_HARNESS="$SCRIPT" \
   TRACE_CHROME="$OUT/$LABEL.client.trace-off.json" \
   RUST_LOG="bevy_ecs=warn,bevy_app=warn" "$CLIENT" >"$CLOG" 2>&1) &
CPID=$!
start=$EPOCHREALTIME
until grep -q 'harness: input slot' "$CLOG" 2>/dev/null; do
  kill -0 "$CPID" 2>/dev/null || { echo "the client exited before its input slot; see $CLOG" >&2; exit 1; }
  (( EPOCHREALTIME - start < 1200 )) || { echo "the client took no input slot" >&2; exit 1; }
  sleep 0.5
done
sleep "$COMBAT_WARMUP_S"; w0=$EPOCHREALTIME; sleep "$COMBAT_S"; w1=$EPOCHREALTIME
kill -0 "$SPID" || { echo "the server died in the combat window" >&2; exit 1; }
kill -0 "$CPID" || { echo "the client died in the combat window" >&2; exit 1; }
window combat "$w0" "$w1"
cleanup; trap - EXIT

for phase in empty combat; do
  python3 "$REPO/scripts/perf/trace_profile.py" "$TRACE" "$OUT/$LABEL.$phase.window.json" \
    "$OUT/$LABEL.$phase.profile.json" \
    || { echo "the $phase profile failed; the trace is kept: $TRACE" >&2; exit 1; }
done
[[ -n "${KEEP_TRACE:-}" ]] || rm -f "$TRACE"
{ print "commit: $(git -C "$REPO" rev-parse HEAD)"; print "command: $0 $OUT $LABEL"
  print "BIN=$BIN BOTS=$BOTS EMPTY=$EMPTY_WARMUP_S+$EMPTY_S COMBAT=$COMBAT_WARMUP_S+$COMBAT_S"
} >"$OUT/$LABEL.run.txt"
echo "$LABEL: $OUT/$LABEL.{empty,combat}.profile.json"
