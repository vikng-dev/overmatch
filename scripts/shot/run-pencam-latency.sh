#!/bin/zsh
# Pen cam delivery latency over the network: a dedicated server and two scripted clients parked
# side by side, each firing its main gun and holding its machine guns into the other's engine bay,
# every process tracing the shot lifecycle. `scripts/shot/pencam_latency.py` joins the traces.
#
#   LAT=<ms> JIT=<ms> [SEED=1] [RUN_S=200] [OUT=dir] scripts/shot/run-pencam-latency.sh
#
# LAT/JIT condition each client's RECEIVE path (server → client), the direction an explanation
# travels; with both nonzero the jitter is seeded by SEED. Both clients run hidden (SPIKE_SIM_WINDOWED
# without SPIKE_SIM_VISIBLE: no window, macOS activation revoked) and muted.

set -eu
# A failed glob is fatal under zsh defaults, so the rm below kills the run when OUT is fresh.
setopt null_glob

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
LAT="${LAT:-0}"
JIT="${JIT:-0}"
SEED="${SEED:-1}"
RUN_S="${RUN_S:-200}"
OUT="${OUT:-/tmp/overmatch-pencam-latency-${LAT}-${JIT}}"
BIN="${BIN:-target/debug}"
SERVER="$REPO/$BIN/overmatch-server"
CLIENT="$REPO/$BIN/overmatch"
# DERIVED: 64 Hz fixed ticks.
TICKS=$((RUN_S * 64))

mkdir -p "$OUT"
cd "$REPO"

# Incremental when unchanged, and prevents a stale binary from producing a convincing trace.
cargo build --locked --bin overmatch --bin overmatch-server

rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT/summary.json"

SERVER_PID=""
FIRST_PID=""
cleanup() {
  [[ -n "$FIRST_PID" ]] && kill "$FIRST_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$FIRST_PID" ]] && wait "$FIRST_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
}
# A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit so the
# EXIT trap does the cleanup exactly once and the script actually terminates.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# The flat patch `run-mg-armor.sh` documents; lane 1 spawns 8 m along +x from lane 0, both facing −z.
env OVERMATCH_MUTE=1 SPIKE_SPAWN_POSE="149.3,8.75,293.9,0,0,0,1" \
  SPIKE_SHOT_TRACE="$OUT/server" BEVY_ASSET_ROOT="$REPO" \
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
  env -u SPIKE_SIM_VISIBLE OVERMATCH_MUTE=1 \
    SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
    SPIKE_SIM_TICKS="$4" SPIKE_FIRE_PRIMARY=1 SPIKE_FIRE_SECONDARY=1 \
    SPIKE_FIRE_TICK="$3" SPIKE_FIRE_INTERVAL=256 \
    SPIKE_AIM_POINT="$2" SPIKE_SIM_RANGE=12 \
    SPIKE_LATENCY_MS="$LAT" SPIKE_JITTER_MS="$JIT" SPIKE_JITTER_SEED="$SEED" \
    SPIKE_SHOT_TRACE="$OUT/$1" BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" >"$OUT/$1.log" 2>&1
}

# Lane assignment is connect-order: the first client to CONNECT takes lane 0, and each aim assumes
# its lane. The second client starts only once the server logged the first connection. A cold
# Metal shader cache (the first run after a rebuild) can stall a client past keepalive and burn a
# lane on reconnect. The analyzer refuses such a run from the server trace's `link` rows (more
# connects than clients, or a disconnect before the last send) and exits non-zero, which this
# script, its last command, returns; a rerun is warm.
# DERIVED: the first client runs 128 ticks longer so it outlives the later-started second; their
# primaries are 128 ticks (half an interval) apart so the two main guns do not fire together.
client a "8,1.2,2.2" 320 "$((TICKS + 128))" &
FIRST_PID=$!
for _ in {1..150}; do
  grep -q "client connected" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "client connected" "$OUT/server.log"

client b "-8,0.8,-2.2" 448 "$TICKS"

wait "$FIRST_PID"
FIRST_PID=""
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

uv run scripts/shot/pencam_latency.py --server "$OUT/server.server.jsonl" \
  --client "$OUT/a.client.jsonl" --client "$OUT/b.client.jsonl" \
  --label "${LAT}/${JIT} ms" --json "$OUT/summary.json"
