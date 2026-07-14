#!/bin/zsh
# Production-path MG damage-confirmation measurement: dedicated server, stationary target client,
# and firing client. The strict analyzer fails on missing or duplicate shooter confirmations.

set -eu

REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/overmatch-shot-mg}"
BIN="${BIN:-target/debug}"
TICKS="${TICKS:-700}"
SERVER="$REPO/$BIN/overmatch-server"
CLIENT="$REPO/$BIN/overmatch"

mkdir -p "$OUT"
cd "$REPO"

# Incremental when unchanged, and prevents a stale binary from producing a convincing trace.
cargo build --locked --bin overmatch --bin overmatch-server

rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT/summary.json"

SERVER_PID=""
TARGET_PID=""
cleanup() {
  [[ -n "$TARGET_PID" ]] && kill "$TARGET_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$TARGET_PID" ]] && wait "$TARGET_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

env SPIKE_PERTURB=0 SPIKE_SHOT_TRACE="$OUT/server" BEVY_ASSET_ROOT="$REPO" \
  "$SERVER" >"$OUT/server.log" 2>&1 &
SERVER_PID=$!

for _ in {1..150}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "listening" "$OUT/server.log"

# DERIVED: the target runs 128 fixed ticks longer so it remains present for the later-started shooter.
env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_IDLE=1 SPIKE_SIM_TICKS="$((TICKS + 128))" \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SHOT_TRACE="$OUT/target" \
  BEVY_ASSET_ROOT="$REPO" "$CLIENT" >"$OUT/target.log" 2>&1 &
TARGET_PID=$!

sleep 1

env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_TICKS="$TICKS" SPIKE_FIRE_SECONDARY=1 \
  SPIKE_AIM_POINT="-8,0,0" SPIKE_SIM_RANGE=12 SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 \
  SPIKE_SHOT_TRACE="$OUT/shooter" BEVY_ASSET_ROOT="$REPO" \
  "$CLIENT" >"$OUT/shooter.log" 2>&1

wait "$TARGET_PID"
TARGET_PID=""
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

uv run scripts/shot/analyze.py \
  --client "$OUT/shooter.client.jsonl" \
  --server "$OUT/server.server.jsonl" \
  --samples 0 --json --strict >"$OUT/summary.json"

echo "MG shot verification passed -> $OUT/summary.json"
