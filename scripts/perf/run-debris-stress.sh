#!/bin/zsh
# =============================== RUNBOOK ===============================
# The dedicated server's price per falling wall fragment, for the game's environment destruction:
# whether the server should simulate the fragments that break off a wall. A local headless server
# with BOTS server-side bots plus CLIENTS window-less scripted clients (default 29 + 1 = 30 tanks),
# a fresh server per condition, each condition one setting of the debris stress lever
# (OVERMATCH_DEBRIS_STRESS, src/net/debris_stress.rs): off, or a breach of N fragments every
# PERIOD_S beside a tank. Nothing takes a screen: the server is headless, the clients GPU-less.
#
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock scripts/perf/run-debris-stress.sh <out-dir>
#
# The lock keeps compiles and other measurements off the machine; the script holds the machine
# awake itself (`caffeinate -ims -w`) for as long as it runs.
#
# Conditions (default): off 10:5 35:5 200:5 200:5 35:5 10:5 off. Every lever value runs twice, as
# a palindrome, so a drift linear in time cancels from each value's mean: the A/B/B/A design over
# four levels. A condition is a lever value: `off`, or `<pieces>:<period_s>[:<live_cap>[:
# <frozen_cap>]]`, the lever's own format (live cap default 256; frozen cap default none, so the
# rubble persists and the frozen count climbs through a condition; every stats line carries it).
#
# Why a client at all: the server replicates only to connected links, and the price under test
# includes sending the fragments. The clients sit idle (SPIKE_SIM_IDLE=1): their tanks stay parked
# in their lanes, while the bots circle and fire. Bots have no link, so with one client the
# outbound cost is one link's; CLIENTS=K shows the slope per link, but each client also loads this
# Mac's CPU beside the server.
#
# Per condition: the server starts (OVERMATCH_SERVER_BIND=127.0.0.1, so the firewall has nothing to
# ask; OVERMATCH_SERVER_STATS_S=1), the clients connect and take their input slots, WARMUP_S passes
# and the server's last stats line becomes the marker, DURATION_S passes, the server's log is copied
# at the window's close, then every process is killed. The window is the stats lines after the
# marker in that copy (online_cost.py server-window).
# Gates per condition (a failed one marks it INVALID in manifest.txt; the run goes on):
#   * the server listened and spawned BOTS bots;
#   * every client took its input slot within STARTUP_GRACE_S and was alive at the window's close;
#   * the server was alive at the window's close and printed stats lines inside the window;
#   * an armed lever released fragments inside the window.
#
# Products in <out-dir>: c<NN>-<label>.server.log (the whole log), .window.log (the log at the
# window's close), .marker, .server.json, .client<i>.log per condition; conditions.tsv (condition,
# lever, json); manifest.txt; summary.txt (online_cost.py stress: per condition the tick p50/p95,
# the physics step and outbound brackets, CPU and the live/frozen fragments; then per lever, with
# its change from `off`).
#
# A day run or a SMOKE quotes no number: other builds share the machine by day.
# SMOKE=1: two short conditions (off, 35:5) of WARMUP_S 5 + DURATION_S 10, plumbing only.
#
# Wall clock: per condition ~5 s server start + ~10 s client connect (longer on a cold cache) +
# WARMUP_S 20 + DURATION_S 60 + ~2 s, ~1.6 min: ~13 min for the default eight.
# =======================================================================
#
# Usage: run-debris-stress.sh <out-dir> [condition ...]
# Knobs (env): BIN (the directory holding `overmatch-server` and `overmatch`; default
#   target/release), ASSET_ROOT (default the repo), BOTS (29), CLIENTS (1), PERIOD_S (5: the default
#   conditions' period), WARMUP_S (20), DURATION_S (60), STARTUP_GRACE_S (90), CLIENT_WORKLOAD
#   (`SPIKE_SIM_IDLE=1`), SERVER_ENV (extra K=V words for every server), SMOKE.

set -eu
setopt null_glob
zmodload zsh/datetime

OUT="${1:?usage: run-debris-stress.sh <out-dir> [condition ...]}"
shift
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
ASSET_ROOT="$(cd "$ASSET_ROOT" && pwd)"
SMOKE="${SMOKE:-}"
BOTS="${BOTS:-29}"
CLIENTS="${CLIENTS:-1}"
PERIOD_S="${PERIOD_S:-5}"
if [[ -n "$SMOKE" ]]; then
  WARMUP_S="${WARMUP_S:-5}"
  DURATION_S="${DURATION_S:-10}"
else
  WARMUP_S="${WARMUP_S:-20}"
  DURATION_S="${DURATION_S:-60}"
fi
STARTUP_GRACE_S="${STARTUP_GRACE_S:-90}"
CLIENT_WORKLOAD="${CLIENT_WORKLOAD:-SPIKE_SIM_IDLE=1}"
SERVER_ENV="${SERVER_ENV:-}"
ADDR="127.0.0.1:5888"
# The scripted client exits after its ticks (64 Hz, counted from its input slot); the runner ends it
# first, at the window's close, so the script must outlast the condition.
SIM_TICKS=$(((WARMUP_S + DURATION_S + STARTUP_GRACE_S + 60) * 64))
INPUT_SLOT='— input slot'

CONDITIONS=("$@")
if (( ${#CONDITIONS[@]} == 0 )); then
  if [[ -n "$SMOKE" ]]; then
    CONDITIONS=(off "35:$PERIOD_S")
  else
    CONDITIONS=(off "10:$PERIOD_S" "35:$PERIOD_S" "200:$PERIOD_S" "200:$PERIOD_S" "35:$PERIOD_S" "10:$PERIOD_S" off)
  fi
fi

for exe in overmatch-server overmatch; do
  [[ -x "$BIN/$exe" ]] || { echo "no $exe at $BIN/$exe" >&2; exit 1; }
done
mkdir -p "$OUT"
rm -f "$OUT"/*.server.json "$OUT"/*.marker "$OUT/conditions.tsv" "$OUT/summary.txt"

SERVER_PID=""
CLIENT_PIDS=()

alive() {
  local state
  state="$(ps -o state= -p "$1" 2>/dev/null || true)"
  [[ -n "$state" && "$state" != *Z* ]]
}

stop_all() {
  local pid
  for pid in "${CLIENT_PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${CLIENT_PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done
  CLIENT_PIDS=()
  # SIGKILL: the server does not stop on SIGINT/SIGTERM cleanly, and nothing of it is kept but its log.
  if [[ -n "$SERVER_PID" ]]; then
    kill -KILL "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""
  fi
}
trap stop_all EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
caffeinate -ims -w $$ </dev/null >/dev/null 2>&1 &

{
  echo "debris stress: $(hostname -s), $(date '+%F %H:%M %Z'), $(cd "$REPO" && git rev-parse --short HEAD)"
  echo "server: $BIN/overmatch-server  env [OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1 OVERMATCH_BOT=$BOTS $SERVER_ENV]"
  echo "clients: $CLIENTS x GPU-less $BIN/overmatch, workload [$CLIENT_WORKLOAD]"
  echo "window: WARMUP_S $WARMUP_S, DURATION_S $DURATION_S; conditions: ${CONDITIONS[*]}"
  [[ -n "$SMOKE" ]] && echo "SMOKE: plumbing only, every number fiction"
} >"$OUT/manifest.txt"

# A failed gate: recorded, and the condition's row leaves the tables ("-").
invalid() {
  echo "   $1: INVALID — $2" >&2
  echo "$1: INVALID — $2" >>"$OUT/manifest.txt"
  rm -f "$OUT/$1.server.json"
}

# One condition. Returns 1 (after recording why) when a gate fails.
run_condition() {
  local cond="$1" lever="$2" i start log
  log="$OUT/$cond.server.log"
  if lsof -nP -iUDP:5888 >/dev/null 2>&1; then
    invalid "$cond" "UDP 5888 is already bound (lsof -nP -iUDP:5888): another server would take the clients"
    return 1
  fi
  local lever_env=()
  [[ "$lever" != off ]] && lever_env=("OVERMATCH_DEBRIS_STRESS=$lever")
  env BEVY_ASSET_ROOT="$ASSET_ROOT" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1 \
    OVERMATCH_BOT="$BOTS" OVERMATCH_MUTE=1 "${lever_env[@]}" ${=SERVER_ENV} \
    "$BIN/overmatch-server" >"$log" 2>&1 &
  SERVER_PID=$!
  for _ in {1..150}; do
    grep -q "listening" "$log" 2>/dev/null && break
    alive "$SERVER_PID" || break
    sleep 0.2
  done
  if ! { alive "$SERVER_PID" && grep -q "listening" "$log"; }; then
    invalid "$cond" "the server never listened (see $log)"
    return 1
  fi

  for (( i = 1; i <= CLIENTS; i++ )); do
    env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE -u SPIKE_FRAME_COST_OFFSCREEN -u SPIKE_FRAME_COST \
      SPIKE_SIMULATE_INPUT=1 OVERMATCH_SERVER="$ADDR" OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
      SPIKE_SIM_TICKS="$SIM_TICKS" BEVY_ASSET_ROOT="$ASSET_ROOT" ${=CLIENT_WORKLOAD} \
      "$BIN/overmatch" >"$OUT/$cond.client$i.log" 2>&1 &
    CLIENT_PIDS+=($!)
  done
  start=$EPOCHREALTIME
  for (( i = 1; i <= CLIENTS; i++ )); do
    until grep -q -- "$INPUT_SLOT" "$OUT/$cond.client$i.log" 2>/dev/null; do
      if ! alive "${CLIENT_PIDS[$i]}" || (( EPOCHREALTIME - start > STARTUP_GRACE_S )); then
        invalid "$cond" "client $i took no input slot within ${STARTUP_GRACE_S}s (see $OUT/$cond.client$i.log)"
        return 1
      fi
      sleep 0.5
    done
  done

  sleep "$WARMUP_S"
  grep 'server_stats' "$log" | tail -1 >"$OUT/$cond.marker" || true
  sleep "$DURATION_S"
  cp "$log" "$OUT/$cond.window.log"
  alive "$SERVER_PID" || { invalid "$cond" "the server exited inside the window (see $log)"; return 1; }
  for (( i = 1; i <= CLIENTS; i++ )); do
    alive "${CLIENT_PIDS[$i]}" \
      || { invalid "$cond" "client $i exited inside the window (see $OUT/$cond.client$i.log)"; return 1; }
  done
  stop_all

  local spawned
  spawned="$(cd "$REPO" && uv run scripts/perf/online_cost.py bots "$OUT/$cond.window.log")"
  (( spawned == BOTS )) || { invalid "$cond" "the server spawned $spawned bots, not $BOTS"; return 1; }
  ( cd "$REPO" && uv run scripts/perf/online_cost.py server-window --label "$cond" \
      --after-marker "$OUT/$cond.marker" --json "$OUT/$cond.server.json" "$OUT/$cond.window.log" )
  local window
  window="$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); print(r["stats_lines"], r["stats"].get("debris.released", {}).get("last_minus_first", 0))' "$OUT/$cond.server.json")"
  (( ${window%% *} > 0 )) || { invalid "$cond" "no stats lines inside the window"; return 1; }
  if [[ "$lever" != off ]] && (( ${window##* } <= 0 )); then
    invalid "$cond" "the lever $lever released no fragment inside the window"
    return 1
  fi
  echo "$cond: lever $lever, valid" >>"$OUT/manifest.txt"
}

n=0
for lever in "${CONDITIONS[@]}"; do
  n=$((n + 1))
  cond="$(printf 'c%02d-%s' "$n" "${lever//:/x}")"
  echo "== $cond (OVERMATCH_DEBRIS_STRESS=$lever)" >&2
  printf '%s\t%s\t%s\n' "$cond" "$lever" "$cond.server.json" >>"$OUT/conditions.tsv"
  run_condition "$cond" "$lever" || true
  stop_all
done

( cd "$REPO" && uv run scripts/perf/online_cost.py stress "$OUT/conditions.tsv" ) | tee "$OUT/summary.txt"
grep -q INVALID "$OUT/manifest.txt" && echo "some conditions are INVALID: see $OUT/manifest.txt" >&2
echo "debris stress complete -> $OUT/summary.txt"
