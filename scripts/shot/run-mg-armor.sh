#!/bin/zsh
# Subject: the game's damage-model test tooling, game state only.
# Production-path MG damage-confirmation measurement: dedicated server, stationary target client,
# and firing client. The strict analyzer fails on missing or duplicate shooter confirmations.

set -eu
# A failed glob is fatal under zsh defaults, so the rm below kills the run when OUT is fresh.
setopt null_glob

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
source "$REPO/scripts/lib/harness-start.zsh"
# Build in the queue, then hold one machine lock for the server and both clients.
if [[ "${1:-}" != --holding-lock ]]; then
  export OVERMATCH_CARGO_LOCK_WAIT_S=10800
  (cd "$REPO" && scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch --bin overmatch-server)
  exec /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$0" --holding-lock "$@"
fi
shift
export OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden
OUT="${OUT:-/tmp/overmatch-shot-mg}"
BIN="${BIN:-target/debug}"
TICKS="${TICKS:-700}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
SERVER="$BIN/overmatch-server"
CLIENT="$BIN/overmatch"
READY="$OUT/workload.ready"

mkdir -p "$OUT"
cd "$REPO"

rm -f "$READY" "$OUT"/*.jsonl "$OUT"/*.log "$OUT/summary.json"

SERVER_PID=""
TARGET_PID=""
SHOOTER_PID=""
cleanup() {
  local pid
  for pid in "$SHOOTER_PID" "$TARGET_PID" "$SERVER_PID"; do
    [[ -z "$pid" ]] || { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
}
# A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit so the
# EXIT trap does the cleanup exactly once and the script actually terminates.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The game's damage-model fixture: the final aim is on the target hull. During traverse,
# fire also crosses its lowered forward barrel, reducing that module's HP pool. The trace
# records the actual struck volumes so the report can distinguish armour from modules.
env OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_HARNESS="$REPO/scripts/harness/mg-armor.ron" \
  SPIKE_SHOT_TRACE="$OUT/server" \
  "$SERVER" >"$OUT/server.log" 2>&1 &
SERVER_PID=$!

for _ in {1..150}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "listening" "$OUT/server.log"

# OVERMATCH_SERVER: the local server this script just started, named explicitly (a scripted
# client with no target falls back to the same address).
# Resolution: a fixed 128-tick observation tail avoids cross-process settlement plumbing
# just for this capture. MEASURED over three runs: last shooter receipt 22-47 ticks
# after last trigger input, cursor lag at receipt 9.5-27.2 ticks. DERIVED conservative need:
# ceil(47 + 27.2) = 75 ticks; 128 leaves 53 ticks of scheduling/retransmission margin.
# Both clients keep the same script length. The watchdog bounds stalled execution; strict
# analysis fails loudly if delivery or playback remains unfinished at the capture boundary.
QUIET_TICKS=128
printf '(drive: Idle, aim: Point((0, -160, -800)), range_m: 12, ticks: %s)\n' "$TICKS" >"$OUT/target.harness.ron"
env SPIKE_WORKLOAD_READY="$READY" OVERMATCH_HARNESS="$OUT/target.harness.ron" OVERMATCH_WINDOW=hidden OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SHOT_TRACE="$OUT/target" \
  "$CLIENT" >"$OUT/target.log" 2>&1 &
TARGET_PID=$!

# Lane assignment is connect-order: the first client to CONNECT gets lane 0; the fixture aim
# assumes the target holds lane 0. A fixed sleep raced that under load
# (shooter connected first, took lane 0, aimed away — zero hits, gate-failed run).
#
# COLD-START MODE: the first capture after a rebuild can stall >10 s on Metal pipeline
# compilation — the client's ESTABLISHED connection dies on keepalive, each auto-reconnect burns
# a lane, and the run fails the validity gate loudly. The fix is simply to re-run (one throwaway
# run warms the cache). Deliberately NOT auto-retried here: an evidence pipeline should fail
# loud, not silently loop.
for _ in {1..150}; do
  grep -q "client connected" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "client connected" "$OUT/server.log"

printf '(drive: Fire(secondary: true), fire_until: %s, aim: Point((-8.5, 1.6, -8.5)), range_m: 12, ticks: %s)\n' "$((TICKS - QUIET_TICKS))" "$TICKS" \
  >"$OUT/shooter.harness.ron"
env SPIKE_WORKLOAD_READY="$READY" OVERMATCH_HARNESS="$OUT/shooter.harness.ron" OVERMATCH_WINDOW=hidden OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 \
  SPIKE_SHOT_TRACE="$OUT/shooter" \
  "$CLIENT" >"$OUT/shooter.log" 2>&1 &
SHOOTER_PID=$!

await_harness_slots "$READY" "$TARGET_PID" "$OUT/target.log" "$SHOOTER_PID" "$OUT/shooter.log" || exit 1
wait "$SHOOTER_PID"
SHOOTER_PID=""
wait "$TARGET_PID"
TARGET_PID=""
# Only a SIGTERM requested here is a successful server shutdown. A crash after the final
# shot must not be hidden by otherwise complete traces.
shutdown_sent=0
kill -TERM "$SERVER_PID" 2>/dev/null && shutdown_sent=1
server_status=0
wait "$SERVER_PID" 2>/dev/null || server_status=$?
SERVER_PID=""
if [[ "$shutdown_sent" != 1 || "$server_status" != 143 ]]; then
  echo "server exited unexpectedly (status=$server_status, shutdown_sent=$shutdown_sent); see $OUT/server.log" >&2
  exit 1
fi

uv run scripts/shot/analyze.py \
  --client "$OUT/shooter.client.jsonl" \
  --server "$OUT/server.server.jsonl" \
  --samples 0 --json --strict >"$OUT/summary.json"

python3 - "$OUT/summary.json" <<'PYTHON'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
if json.loads(path.read_text())["shooter_armour_contacts"] == 0:
    sys.exit(f"MG fixture reached no armour plate; adjust fixture aim and recapture: {path}")
PYTHON

echo "MG shot verification passed -> $OUT/summary.json"
