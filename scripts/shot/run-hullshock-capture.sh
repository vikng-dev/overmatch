#!/bin/zsh
# One seeded HullShock delivery capture: dedicated server, idle target client, MG shooter client,
# with SPIKE_TRACE + per-fact telemetry on every process. This is the runner behind the ADR-0032
# gate reading (.agents/docs/design/hullshock-delivery-capture-2026-07-31.md) and the REV-25 A/B.
#
# Usage: run-hullshock-capture.sh <jitter-seed> <out-dir>
# Build first (once, before a seed loop): cargo build --locked --bin overmatch --bin overmatch-server
#
# Differences from run-mg-armor.sh: jitter seed per run, 3840 ticks (~60 s of belts), SPIKE_TRACE
# with sim fields on all three processes (DISTINCT prefixes — a shared prefix silently clobbers
# trace.client.jsonl), RUST_LOG=overmatch=debug on the target, non-strict analyzer.

set -eu
setopt null_glob

SEED="$1"
OUT="$2"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/debug}"
TICKS="${TICKS:-3840}"
SERVER="$REPO/$BIN/overmatch-server"
CLIENT="$REPO/$BIN/overmatch"

mkdir -p "$OUT"
rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/summary.json

SERVER_PID=""
TARGET_PID=""
cleanup() {
  [[ -n "$TARGET_PID" ]] && kill "$TARGET_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && kill "$SERVER_PID" 2>/dev/null || true
  [[ -n "$TARGET_PID" ]] && wait "$TARGET_PID" 2>/dev/null || true
  [[ -n "$SERVER_PID" ]] && wait "$SERVER_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

# SPIKE_SPAWN_POSE: the flattest 40 m patch on the shipped heightmap (4 cm relief); the lane
# default at world origin is a slope where the tanks slide apart and the -8,0,0 aim misses.
env SPIKE_PERTURB=0 SPIKE_SPAWN_POSE="149.3,8.75,293.9,0,0,0,1" SPIKE_SHOT_TRACE="$OUT/server" \
  SPIKE_TRACE="$OUT/server-trace.jsonl" SPIKE_TRACE_SIM_FIELDS=1 \
  BEVY_ASSET_ROOT="$REPO" \
  "$SERVER" >"$OUT/server.log" 2>&1 &
SERVER_PID=$!

for _ in {1..150}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "listening" "$OUT/server.log"

# Target: idle, outlives the shooter by 128 ticks (same DERIVED margin as run-mg-armor.sh).
# SPIKE_SIM_WINDOWED + OVERMATCH_SERVER: see the comments in run-mg-armor.sh (headless KTX2
# transcode defect; baked production default).
env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_SIM_IDLE=1 SPIKE_SIM_TICKS="$((TICKS + 128))" \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_JITTER_SEED="$SEED" \
  SPIKE_SHOT_TRACE="$OUT/target" \
  SPIKE_TRACE="$OUT/target-trace.jsonl" SPIKE_TRACE_SIM_FIELDS=1 \
  RUST_LOG=overmatch=debug \
  BEVY_ASSET_ROOT="$REPO" "$CLIENT" >"$OUT/target.log" 2>&1 &
TARGET_PID=$!

sleep 1

env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_SIM_TICKS="$TICKS" SPIKE_FIRE_SECONDARY=1 \
  SPIKE_AIM_POINT="-8,0,0" SPIKE_SIM_RANGE=12 \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_JITTER_SEED="$SEED" \
  SPIKE_SHOT_TRACE="$OUT/shooter" \
  SPIKE_TRACE="$OUT/shooter-trace.jsonl" SPIKE_TRACE_SIM_FIELDS=1 \
  BEVY_ASSET_ROOT="$REPO" \
  "$CLIENT" >"$OUT/shooter.log" 2>&1

wait "$TARGET_PID"
TARGET_PID=""
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

# Shot analyzer as a sanity check that hits actually happened; non-strict, non-fatal.
cd "$REPO"
uv run scripts/shot/analyze.py \
  --client "$OUT/shooter.client.jsonl" \
  --server "$OUT/server.server.jsonl" \
  --samples 0 --json >"$OUT/summary.json" || echo "seed $SEED: shot analyzer nonzero exit" >>"$OUT/warnings.txt"

echo "seed $SEED capture complete -> $OUT"
