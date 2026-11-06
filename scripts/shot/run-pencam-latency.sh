#!/bin/zsh
# Subject: the game's damage-model test tooling, game state only.
# Pen cam delivery latency over the network: a dedicated server and two scripted clients parked
# side by side, each firing its main gun and holding its machine guns into the other's engine bay,
# every process tracing the shot lifecycle. `scripts/shot/pencam_latency.py` joins the traces.
#
#   LAT=<ms> JIT=<ms> [SEED=1] [RUN_S=200] [OUT=dir] scripts/shot/run-pencam-latency.sh
#
# LAT/JIT condition each client's RECEIVE path (server → client), the direction an explanation
# travels; with both nonzero the jitter is seeded by SEED. Both clients run hidden
# (OVERMATCH_WINDOW=hidden: no window, macOS activation revoked, unattended).

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
LAT="${LAT:-0}"
JIT="${JIT:-0}"
SEED="${SEED:-1}"
RUN_S="${RUN_S:-200}"
OUT="${OUT:-/tmp/overmatch-pencam-latency-${LAT}-${JIT}}"
BIN="${BIN:-target/debug}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
SERVER="$BIN/overmatch-server"
CLIENT="$BIN/overmatch"
READY="$OUT/workload.ready"
# DERIVED: 64 Hz fixed ticks.
TICKS=$((RUN_S * 64))

mkdir -p "$OUT"
cd "$REPO"

rm -f "$READY" "$OUT"/*.jsonl "$OUT"/*.log "$OUT/summary.json"

SERVER_PID=""
FIRST_PID=""
SECOND_PID=""
cleanup() {
  local pid
  for pid in "$SECOND_PID" "$FIRST_PID" "$SERVER_PID"; do
    [[ -z "$pid" ]] || { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
}
# A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit so the
# EXIT trap does the cleanup exactly once and the script actually terminates.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The flat patch (scripts/harness/flat-patch.ron); lane 1 spawns 8 m along +x from lane 0, both
# facing −z.
env OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_HARNESS="$REPO/scripts/harness/flat-patch.ron" \
  SPIKE_SHOT_TRACE="$OUT/server" \
  "$SERVER" >"$OUT/server.log" 2>&1 &
SERVER_PID=$!

for _ in {1..150}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "listening" "$OUT/server.log"

# One scripted client: $1 trace/log name, $2 hull-local aim point, $3 first primary tick, $4 run
# ticks. The aims are fixture data: a's at b's engine bay (1.2 m up, 2.2 m rearward of its origin,
# as the offline target-fires scenario lays it), b's at a's front hull, so a round costs a
# component, not the tank.
client() {
  printf '(drive: Fire(primary: true, secondary: true), fire_tick: %s, fire_interval: 256, aim: Point((%s)), range_m: 12, ticks: %s)\n' \
    "$3" "$2" "$4" >"$OUT/$1.harness.ron"
  exec env SPIKE_WORKLOAD_READY="$READY" OVERMATCH_WINDOW=hidden \
    OVERMATCH_HARNESS="$OUT/$1.harness.ron" OVERMATCH_SERVER=127.0.0.1:5888 \
    SPIKE_LATENCY_MS="$LAT" SPIKE_JITTER_MS="$JIT" SPIKE_JITTER_SEED="$SEED" \
    SPIKE_SHOT_TRACE="$OUT/$1" \
    "$CLIENT" >"$OUT/$1.log" 2>&1
}

# Lane assignment is connect-order: the first client to CONNECT takes lane 0, and each aim assumes
# its lane. The second client starts only once the server logged the first connection. A cold
# Metal shader cache (the first run after a rebuild) can stall a client past keepalive and burn a
# lane on reconnect. The analyzer refuses such a run from the server trace's `link` rows (a
# connect count other than one per client) and exits non-zero, which this script, its last
# command, returns; a rerun is warm.
# DERIVED: both scripts play from the shared tool barrier; the first client
# runs 128 ticks longer so it is still present for the second's last rounds, and their primaries
# are 128 ticks (half an interval) apart so the two main guns do not fire together.
client a "8,1.2,2.2" 320 "$((TICKS + 128))" &
FIRST_PID=$!
for _ in {1..150}; do
  grep -q "client connected" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "client connected" "$OUT/server.log"

client b "-8,0.8,-2.2" 448 "$TICKS" &
SECOND_PID=$!
await_harness_slots "$READY" "$FIRST_PID" "$OUT/a.log" "$SECOND_PID" "$OUT/b.log" || exit 1
wait "$SECOND_PID"
SECOND_PID=""
wait "$FIRST_PID"
FIRST_PID=""
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

uv run scripts/shot/pencam_latency.py --server "$OUT/server.server.jsonl" \
  --client "$OUT/a.client.jsonl" --client "$OUT/b.client.jsonl" \
  --label "${LAT}/${JIT} ms" --json "$OUT/summary.json"
