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
export OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden OVERMATCH_MUTE=1
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

# The flat patch keeps the fixture hulls at rest. The target holds its main barrel forward and down;
# the shooter's fixture aim crosses that exposed module ahead of the armored hull. Its HP pool
# can be reduced by MG fire. The nonempty damage gate verifies the fixture still exercises it.
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
# OVERMATCH_WINDOW=hidden: a headless client cannot load the mipped-KTX2 tank glb — bevy_image 0.20.1
# slices the UASTC source with the RGBA32 destination block geometry on the uncompressed
# transcode path, the path taken when no RenderApp exists (ktx2.rs:209). Windowed transcodes to
# a 4x4-block GPU format and loads fine, so captures are windowed-only until the upstream fix.
# Fixture capture: the final 128 ticks contain no trigger demand. Both clients stay for the
# same script length; the quiet tail lets the final echoes and paths reach the presentation cursor.
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

printf '(drive: Fire(secondary: true), fire_until: %s, aim: Point((-6.9, 1.6, -10.6)), range_m: 12, ticks: %s)\n' "$((TICKS - 128))" "$TICKS" \
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
kill "$SERVER_PID" 2>/dev/null || true
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

uv run scripts/shot/analyze.py \
  --client "$OUT/shooter.client.jsonl" \
  --server "$OUT/server.server.jsonl" \
  --samples 0 --json --strict --require-shooter-damage >"$OUT/summary.json"

echo "MG shot verification passed -> $OUT/summary.json"
