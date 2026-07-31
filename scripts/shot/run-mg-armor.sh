#!/bin/zsh
# Production-path MG damage-confirmation measurement: dedicated server, stationary target client,
# and firing client. The strict analyzer fails on missing or duplicate shooter confirmations.

set -eu
# A failed glob is fatal under zsh defaults, so the rm below kills the run when OUT is fresh.
setopt null_glob

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
# A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit so the
# EXIT trap does the cleanup exactly once and the script actually terminates.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# SPIKE_SPAWN_POSE: the lane-spawn default sits at world origin, which on the heightmap world is
# a slope — the tanks slide apart and the hull-local -8,0,0 aim never touches armor (verified:
# zero damage rows in a full run). This pose is the flattest 40 m patch on the shipped heightmap
# (4 cm relief, center x=149.3 z=293.9); y = surface + 2 m spawn clearance. Lane 1 puts the
# shooter at +8 x, so aim -8,0,0 points back at the target; identity rotation.
env SPIKE_PERTURB=0 SPIKE_SPAWN_POSE="149.3,8.75,293.9,0,0,0,1" \
  SPIKE_SHOT_TRACE="$OUT/server" BEVY_ASSET_ROOT="$REPO" \
  "$SERVER" >"$OUT/server.log" 2>&1 &
SERVER_PID=$!

for _ in {1..150}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  sleep 0.2
done
grep -q "listening" "$OUT/server.log"

# OVERMATCH_SERVER: without it the client dials the BAKED PRODUCTION droplet, not the local
# server this script just started.
# SPIKE_SIM_WINDOWED: a headless client cannot load the mipped-KTX2 tank glb — bevy_image 0.19
# slices the UASTC source with the RGBA32 destination block geometry on the uncompressed
# transcode path, the path taken when no RenderApp exists (ktx2.rs:209). Windowed transcodes to
# a 4x4-block GPU format and loads fine, so captures are windowed-only until the upstream fix.
# DERIVED: the target runs 128 fixed ticks longer so it remains present for the later-started shooter.
env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_SIM_IDLE=1 SPIKE_SIM_TICKS="$((TICKS + 128))" \
  SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 SPIKE_SHOT_TRACE="$OUT/target" \
  BEVY_ASSET_ROOT="$REPO" "$CLIENT" >"$OUT/target.log" 2>&1 &
TARGET_PID=$!

# Lane assignment is connect-order: the first client to CONNECT gets lane 0, and the shooter's
# hull-local -8,0,0 aim assumes the target holds lane 0. A fixed sleep raced that under load
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

env SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
  SPIKE_SIM_TICKS="$TICKS" SPIKE_FIRE_SECONDARY=1 \
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
