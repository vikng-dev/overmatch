#!/bin/zsh
# Batch connect-defect verifier — doc §7 (connect hang) + §10 (constant-offset runaway).
#
# Runs N headless scripted client connects at 80/10 against a fresh local server per run
# (fresh server so client/server SPIKE_TRACE files pair 1:1 for scripts/divergence/analyze.py),
# classifies each run from its client log, and appends one TSV row per run to $OUT/summary.tsv.
#
#   usage: scripts/connect/batch.sh [N=24] [START=1]
#   env:   OUT   output dir (default /tmp/connect-verify)
#          LOAD  set to 1 to saturate all cores with busy-loops for the batch —
#                REQUIRED to reproduce the §7 hang (0/48 quiet vs 4/12 loaded, 2026-07-11)
#
# Classes: OK | HANG_TIMEOUT_KILLED (external 90 s kill) | HANG_NO_INPUT_SLOT (in-app 40 s
# watchdog) | ANOMALOUS (nonzero exit without script completion — §7 hangs OS-SIGKILLed
# before 90 s land here, exit 137). Offset runaway needs the trace post-pass:
#   uv run scripts/divergence/analyze.py --client $OUT/runN.client.jsonl --server $OUT/runN.server.jsonl
set -u
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/connect-verify}"
N=${1:-24}
START=${2:-1}
CLIENT_TIMEOUT=90   # healthy run exits ~11-25 s; a wedged main loop never exits on its own

mkdir -p "$OUT"
cd "$REPO" || exit 1
[ -x target/debug/overmatch ] && [ -x target/debug/overmatch-server ] || {
  echo "build first: cargo build --bin overmatch --bin overmatch-server" >&2; exit 1; }

LOAD_PIDS=()
if [ "${LOAD:-0}" = "1" ]; then
  for c in $(seq 1 "$(sysctl -n hw.ncpu)"); do yes > /dev/null & LOAD_PIDS+=($!); done
  trap '[ ${#LOAD_PIDS[@]} -gt 0 ] && kill $LOAD_PIDS 2>/dev/null' EXIT
  echo "cpu load: ${#LOAD_PIDS[@]} busy-loops"
fi

SUMMARY="$OUT/summary.tsv"
[ -f "$SUMMARY" ] || echo "run\texit\tclass\tconnected\tinput_slot\tscript_complete\twd_timeout\tsnap_lines\trollback_fired\twatchdog_fired\tdur_s\tlast_line" > "$SUMMARY"

for i in $(seq $START $((START + N - 1))); do
  base="$OUT/run$i"
  rm -f "$base".*.jsonl "$base".*.log

  SPIKE_PERTURB=0 SPIKE_TRACE="$base.jsonl" BEVY_ASSET_ROOT="$REPO" \
    ./target/debug/overmatch-server > "$base.server.log" 2>&1 &
  SERVER_PID=$!
  for t in $(seq 1 100); do
    grep -q "listening" "$base.server.log" 2>/dev/null && break
    sleep 0.2
  done

  t0=$(date +%s)
  SPIKE_SIMULATE_INPUT=1 SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 \
    SPIKE_TRACE="$base.jsonl" BEVY_ASSET_ROOT="$REPO" \
    ./target/debug/overmatch > "$base.client.log" 2>&1 &
  CLIENT_PID=$!

  elapsed=0
  while kill -0 $CLIENT_PID 2>/dev/null && [ $elapsed -lt $CLIENT_TIMEOUT ]; do
    sleep 1; elapsed=$((elapsed+1))
  done
  if kill -0 $CLIENT_PID 2>/dev/null; then
    kill -9 $CLIENT_PID 2>/dev/null
    wait $CLIENT_PID 2>/dev/null
    ec=137; killed=1
  else
    wait $CLIENT_PID; ec=$?; killed=0
  fi
  dur=$(( $(date +%s) - t0 ))

  kill $SERVER_PID 2>/dev/null; wait $SERVER_PID 2>/dev/null

  log="$base.client.log"
  connected=$(grep -c "client: connected" "$log" || true)
  slot=$(grep -c "input slot" "$log" || true)
  complete=$(grep -c "simulation script complete" "$log" || true)
  wdto=$(grep -c "watchdog timeout" "$log" || true)
  snaps=$(grep -c "ROLLBACK-SNAP" "$log" || true)
  rbfired=$(grep -c "ROLLBACK fired" "$log" || true)
  wdfired=$(grep -c "watchdog: receive-time" "$log" || true)
  last=$(tail -1 "$log" | cut -c1-160)

  if [ $killed -eq 1 ]; then cls=HANG_TIMEOUT_KILLED
  elif [ "$wdto" -gt 0 ]; then cls=HANG_NO_INPUT_SLOT
  elif [ $ec -eq 0 ] && [ "$complete" -gt 0 ]; then cls=OK
  else cls=ANOMALOUS
  fi

  echo "run$i\t$ec\t$cls\t$connected\t$slot\t$complete\t$wdto\t$snaps\t$rbfired\t$wdfired\t$dur\t$last" >> "$SUMMARY"
  echo "run$i -> $cls (exit=$ec dur=${dur}s)"
done
echo "BATCH DONE -> $SUMMARY"
