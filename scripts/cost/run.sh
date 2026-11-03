#!/bin/zsh
# The game's damage-model cost harness, game state only: headless server and scripted clients.
# Usage: scripts/cost/run.sh [idle1|fire1|fire2|armor1|loft1 ...]
# OUT selects the output directory; BIN the binary directory (default target/debug).
# SERVER_BIN / CLIENT_BIN override BIN per role. TICKS defaults to 2560; WARMUP to 384.
# ASSUMED workload lengths. The runner creates SPIKE_WORKLOAD_READY after all clients have
# input slots. Scripts start tick zero and recorders start their WARMUP from that signal,
# independently of the already-running Battle. No partial-cohort tick enters the tables.
# scripts/cost/analyze.py consumes the resulting streams.
set -u

# Hold one machine lock across the workload, including both concurrent clients. The re-entry
# argument is this script's private lock contract; child binaries never acquire a nested lock.
if [[ "${1:-}" != --holding-lock ]]; then
  exec /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$0" --holding-lock "$@"
fi
shift
export OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/mg-cost}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
BIN="${BIN:-target/debug}"
TICKS="${TICKS:-2560}"
WARMUP="${WARMUP:-384}"
SERVER="${SERVER_BIN:-$BIN}/overmatch-server"
CLIENT="${CLIENT_BIN:-$BIN}/overmatch"
[[ "$SERVER" == /* ]] || SERVER="$REPO/$SERVER"
[[ "$CLIENT" == /* ]] || CLIENT="$REPO/$CLIENT"
mkdir -p "$OUT" || exit 1
cd "$REPO" || exit 1
[[ -x "$SERVER" && -x "$CLIENT" ]] || {
  echo "build first: scripts/serial-cargo build --features dev_ui,bitprobe --bin overmatch --bin overmatch-server" >&2
  exit 1
}
SCENARIOS=("$@")
[[ ${#SCENARIOS[@]} -gt 0 ]] || SCENARIOS=(idle1 fire1 fire2 armor1 loft1)
for scenario in "${SCENARIOS[@]}"; do
  case "$scenario" in
    idle1|fire1|fire2|armor1|loft1) ;;
    *) echo "unknown cost scenario: $scenario" >&2; exit 2 ;;
  esac
done
SERVER_PID= CA= CB=
cleanup() {
  local pid
  for pid in "$CA" "$CB" "$SERVER_PID"; do
    [[ -z "$pid" ]] || { kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true; }
  done
  SERVER_PID= CA= CB=
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

start_server() {
  local base="$1" t
  READY="$OUT/$base.ready"
  rm -f "$READY" "$OUT/$base.server.jsonl" "$OUT/$base.server.log" || return 1
  env SPIKE_WORKLOAD_READY="$READY" SPIKE_COST_TRACE="$OUT/$base.jsonl" SPIKE_COST_WARMUP="$WARMUP" \
    BEVY_ASSET_ROOT="$REPO" "$SERVER" >"$OUT/$base.server.log" 2>&1 &
  SERVER_PID=$!
  # ASSUMED readiness allowance: 150 polls at 0.2 s = 30 s.
  for t in {1..150}; do
    grep -q 'listening' "$OUT/$base.server.log" && return 0
    kill -0 "$SERVER_PID" 2>/dev/null || return 1
    sleep 0.2
  done
  return 1
}

# Start a direct child so cleanup owns the actual client PID, including the second client.
start_client() {
  local base="$1" ticks="$2" fields="$3"
  printf '(ticks: %s, %s)\n' "$ticks" "$fields" >"$OUT/$base.harness.ron" || return 1
  env SPIKE_WORKLOAD_READY="$READY" OVERMATCH_HARNESS="$OUT/$base.harness.ron" SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 \
    SPIKE_COST_TRACE="$OUT/$base.jsonl" SPIKE_COST_WARMUP="$WARMUP" BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" >"$OUT/$base.client.log" 2>&1 &
  CLIENT_PID=$!
}

# Bound admission by the harness watchdog's startup allowance. The file is a tool barrier,
# never a Battle phase: an ordinary player on the same server is already in combat.
await_slots() {
  local scenario="$1" poll
  for poll in {1..75}; do # ASSUMED 15 s admission allowance, below the 20 s watchdog.
    kill -0 "$CA" 2>/dev/null || return 1
    if [[ -n "$CB" ]]; then kill -0 "$CB" 2>/dev/null || return 1; fi
    if grep -q 'harness: input slot' "$OUT/$scenario.client.log" &&
       { [[ -z "$CB" ]] || grep -q 'harness: input slot' "$OUT/${scenario}b.client.log"; }; then
      touch "$READY" || return 1
      return 0
    fi
    sleep 0.2
  done
  echo "$scenario FAILED: clients did not all take input slots before the tool barrier" >&2
  return 1
}

failed=0
for scenario in "${SCENARIOS[@]}"; do
  echo "== $scenario =="
  if ! start_server "$scenario"; then
    echo "$scenario FAILED: server did not become ready; see $OUT/$scenario.server.log" >&2
    failed=1
    cleanup
    continue
  fi
  ticks="$TICKS"
  fields='drive: Fire(secondary: true)'
  second=
  case "$scenario" in
    idle1) fields='drive: Idle' ;;
    fire2) second='drive: Fire(secondary: true)' ;;
    armor1) second='drive: Idle'; fields='drive: Fire(secondary: true), aim: Point((-8, 0, 0)), range_m: 12' ;;
    loft1) ticks=640; fields='drive: Fire(secondary: true), aim: Point((30, 50, -800)), range_m: 800' ;;
  esac
  scenario_failed=0
  if [[ -n "$second" ]]; then
    if start_client "${scenario}b" "$ticks" "$second"; then CB=$CLIENT_PID
    else scenario_failed=1; fi
    [[ "$scenario" != armor1 ]] || sleep 1 # ASSUMED target admission lead.
  fi
  if start_client "$scenario" "$ticks" "$fields"; then CA=$CLIENT_PID
  else scenario_failed=1; fi
  if [[ "$scenario_failed" -eq 0 ]] && ! await_slots "$scenario"; then
    scenario_failed=1
    cleanup
  fi
  if [[ -n "$CA" ]]; then
    if ! wait "$CA"; then
      echo "$scenario FAILED: primary client; see $OUT/$scenario.client.log" >&2
      scenario_failed=1
    fi
    CA=
  fi
  if [[ -n "$CB" ]]; then
    if ! wait "$CB"; then
      echo "$scenario FAILED: second client; see $OUT/${scenario}b.client.log" >&2
      scenario_failed=1
    fi
    CB=
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    echo "$scenario FAILED: server exited during the workload" >&2
    scenario_failed=1
  fi
  cleanup
  if [[ "$scenario_failed" -eq 0 ]]; then echo "$scenario OK"
  else failed=1; fi
done
if [[ "$failed" -eq 0 ]]; then echo "ALL DONE -> $OUT"
else echo "cost battery FAILED -> $OUT" >&2; fi
exit "$failed"
