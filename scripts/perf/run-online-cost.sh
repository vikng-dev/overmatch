#!/bin/zsh
# =============================== RUNBOOK ===============================
# The online client's frame cost, its network bytes and the server's own stats, per condition,
# against a dedicated server: a remote one (TARGET=<ip:port>, an Edgegap deployment started by
# scripts/edgegap/session.sh) or a local one this runner starts (TARGET unset). The measured client
# is the scripted one (SPIKE_SIMULATE_INPUT=1) with the game's full presentation stack
# (SPIKE_SIM_WINDOWED=1, src/net/client.rs), so its frame rows are the online game's frames.
#
#   VISIBLE=1 TARGET=1.2.3.4:31085 DEPLOYMENT=<id> scripts/perf/run-online-cost.sh out/ \
#     'alone|' 'plus4||LOAD_CLIENTS=4'
#   OFFSCREEN=1 SERVER_ENV='OVERMATCH_BOT=1' scripts/perf/run-online-cost.sh out/   # local rehearsal
# Run it under the machine's lock, like every game binary: /usr/bin/lockf -k -t 10800
# /tmp/overmatch-cargo.lock <the command> (no compile and no other measurement shares the machine).
#
# A condition is `name|video fields|VAR=value …`, run-ablation.sh's grammar: the name labels the
# tables, the fields go into its video.ron after BASE_FIELDS and beside `vsync_mode: Off`, the
# assignments reach the measured client after CLIENT_WORKLOAD. BASE_FIELDS turns the kill cam and
# the pen cam's corner off: bots fire every reload (src/net/server.rs `drive_bot`), and either
# surface on screen would be its own workload in the frames. One assignment is the runner's own: LOAD_CLIENTS=K starts
# K window-less scripted clients (GPU-less, nothing on any screen) once the measured client holds
# its input slot, so it keeps lane 0 (lanes are connect order). They add tanks and links; they also
# share this Mac's CPU with the measured client, so a condition with load is not the same machine
# as one without (ASSUMED non-trivial: a 2 ms run loop each).
#
# MODE — exactly one, no default, so nothing takes a screen by omission:
#   VISIBLE=1    the window on screen: every presentation precondition and gate of the frame sweep
#                (run-frame-sweep.sh's runbook — primary 120 Hz display, 2560x1440, unoccluded,
#                hands off). The owner's screen only.
#   OFFSCREEN=1  a hidden window rendering into a 2560x1440 image (frame-run.zsh). Agents use this.
#   SMOKE=1      a hidden window, free-running: plumbing only, every number fiction.
#
# Per condition, frame-run.zsh's gates (settings loaded, uncapped, the analyzer's stream gates
# including drawn frames) plus this runner's:
#   * the measured client connected exactly once (a reconnect burns a lane: cold shader cache) and
#     held its input slot before the window opened;
#   * every load client held its input slot before the window opened, was alive when it closed, and
#     connected exactly once.
# NOT gated, and not observable: the measured tank's knockout. No log line marks a player tank's
# knockout, on the client or the server; with the kill cam off its debug mark is gone too. Its
# frames would then show the death screen. The server's bot knockouts in the window are counted.
# Not gated: the server. Its numbers are its own `server_stats` lines inside the window, parsed by
# scripts/perf/online_cost.py, whose docstring states the line it expects, the one function to
# change when the format moves, and how the window is cut from a log whose lines carry no clock
# (a marker line saved just before the window, the log saved just after it). No stats lines means
# an empty server row, not a failure.
#
# Client network: the measured client process's cumulative socket bytes from `nettop` at the
# window's two edges (the first poll at or past each edge, on the stream's own clock), divided by
# the stream span between them. MEASURED method: .agents/scratch/hosting-2026-10/risk-test.md.
#
# Products in <out-dir>: <cond>.client.jsonl, <cond>.log, <cond>.load<i>.log, <cond>.window.tsv
# (the edges: this Mac's epoch, stream span, nettop bytes), <cond>.marker (the server's last stats
# line ~2 s before the window), <cond>.server.log (the server's log at the window's close: a copy of
# server.log locally, a container-log pull with DEPLOYMENT), <cond>.server.json, manifest.txt, and
# on stdout the client frame table, the network table and the server table (tables.txt keeps the
# last two).
#
# Wall clock: per condition, startup (5–15 s; longer on a cold shader cache) + WARMUP_S + DURATION_S
# + COOLDOWN_S, ~1.5–2 min at the defaults; a local server adds ~5 s.
# =======================================================================
#
# Usage: run-online-cost.sh <out-dir> [condition ...]   (default condition: `online|`)
# Knobs (env): TARGET (ip:port; unset or `local` starts $BIN/overmatch-server on 127.0.0.1:5888),
#   DEPLOYMENT (Edgegap id: its container log is pulled at each window's edges), BIN (directory
#   holding `overmatch` and, for a local server, `overmatch-server`; default target/release),
#   ASSET_ROOT (default the repo), DURATION_S (60), WARMUP_S (20), COOLDOWN_S (5), MIN_ROWS (DURATION_S*20),
#   STARTUP_GRACE_S (90), CLIENT_WORKLOAD and LOAD_WORKLOAD (client env words; default
#   `SPIKE_SIM_IDLE=1 SPIKE_SIM_AIM_SWEEP=1`: parked, turret sweeping, no fire), BASE_FIELDS
#   (`pen_cam_corner: Off, kill_cam: Off,`), SERVER_ENV (env words for a LOCAL server, e.g.
#   `OVERMATCH_BOT=10`; it always gets OVERMATCH_SERVER_BIND=127.0.0.1, so it never listens on the
#   network and the host firewall has nothing to ask), STATS_LEVER / STATS_PERIOD_S (the server
#   stats lever, default OVERMATCH_SERVER_STATS_S=1, local server only: a deployment gets it from
#   session.sh deploy).

set -eu
setopt null_glob
zmodload zsh/datetime

OUT="${1:?usage: run-online-cost.sh <out-dir> [condition ...]}"
shift
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
VISIBLE="${VISIBLE:-}"
OFFSCREEN="${OFFSCREEN:-}"
SMOKE="${SMOKE:-}"
DURATION_S="${DURATION_S:-60}"
WARMUP_S="${WARMUP_S:-20}"
COOLDOWN_S="${COOLDOWN_S:-5}"
MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-90}"
TARGET="${TARGET:-local}"
DEPLOYMENT="${DEPLOYMENT:-}"
CLIENT_WORKLOAD="${CLIENT_WORKLOAD:-SPIKE_SIM_IDLE=1 SPIKE_SIM_AIM_SWEEP=1}"
LOAD_WORKLOAD="${LOAD_WORKLOAD:-SPIKE_SIM_IDLE=1 SPIKE_SIM_AIM_SWEEP=1}"
SERVER_ENV="${SERVER_ENV:-}"
BASE_FIELDS="${BASE_FIELDS:-pen_cam_corner: Off, kill_cam: Off,}"
STATS_LEVER="${STATS_LEVER:-OVERMATCH_SERVER_STATS_S}"
STATS_PERIOD_S="${STATS_PERIOD_S:-1}"
# A stand-in with session.sh's subcommands, for testing the chain by day (run-online-night.sh).
SESSION="${EDGEGAP_SESSION:-$REPO/scripts/edgegap/session.sh}"
# The scripted client exits after its ticks (64 Hz, counted from its input slot); the runner ends it
# first, at the window's close, so the script must outlast the deadline.
SIM_TICKS=$(((WARMUP_S + DURATION_S + STARTUP_GRACE_S + 60) * 64))
INPUT_SLOT='— input slot'

CONDITIONS=("$@")
(( ${#CONDITIONS[@]} > 0 )) || CONDITIONS=("online|")

modes=0
for m in "$VISIBLE" "$OFFSCREEN" "$SMOKE"; do [[ -n "$m" ]] && modes=$((modes + 1)); done
if (( modes != 1 )); then
  echo "REFUSING: set exactly one of VISIBLE=1 (the window on screen), OFFSCREEN=1 (hidden, measured)" >&2
  echo "or SMOKE=1 (hidden, plumbing only)" >&2
  exit 1
fi
# The hidden modes get the windowed pair from frame-run.zsh; the visible one asks for it here, per
# condition, never exported (frame_run_begin refuses an exported SPIKE_SIM_WINDOWED in a real run).
MODE_ENV=()
[[ -n "$VISIBLE" ]] && MODE_ENV=(SPIKE_SIM_WINDOWED=1 SPIKE_SIM_VISIBLE=1)

if [[ -n "$DEPLOYMENT" && "$TARGET" == local ]]; then
  echo "DEPLOYMENT is set but TARGET is not: give the deployment's ip:port (session.sh address)" >&2
  exit 1
fi
if [[ "$TARGET" == local ]]; then
  [[ -x "$BIN/overmatch-server" ]] || { echo "no server binary at $BIN/overmatch-server" >&2; exit 1; }
  ADDR="127.0.0.1:5888"
else
  ADDR="$TARGET"
fi

FRAME_RUN_CLIENT_ARGS=()
source "$REPO/scripts/perf/frame-run.zsh"
frame_run_begin
rm -f "$OUT"/*.window.tsv "$OUT"/*.marker "$OUT"/*.server.json "$OUT"/tables.txt

SERVER_PID=""
LOAD_PIDS=()
load_count=0
connected=""
edge_start=""
edge_end=""
marked=""
collected_exit=""

stop_load() {
  local pid
  for pid in "${LOAD_PIDS[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${LOAD_PIDS[@]}"; do wait "$pid" 2>/dev/null || true; done
  LOAD_PIDS=()
}

run_cleanup() {
  stop_load
  # SIGKILL: the server does not stop on SIGINT/SIGTERM cleanly, and nothing of it is kept but its log.
  if [[ -n "$SERVER_PID" ]]; then
    kill -KILL "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
    SERVER_PID=""
  fi
  # An early exit (an INVALID condition, a signal) keeps the server's side of the story: the
  # container log dies with the deployment, which the caller may stop next.
  if [[ -n "$DEPLOYMENT" && -z "$collected_exit" ]]; then
    collected_exit=1
    "$SESSION" logs "$DEPLOYMENT" "$OUT/server-at-exit.log" >/dev/null || true
  fi
}

alive() {
  local state
  state="$(ps -o state= -p "$1" 2>/dev/null || true)"
  [[ -n "$state" && "$state" != *Z* ]]
}

# One window edge: this Mac's wall clock, the stream span, the measured client's cumulative bytes.
net_edge() {
  local cond="$1" edge="$2" span="$3" sample
  sample="$(nettop -P -p "$CLIENT_PID" -L 1 -x -J bytes_in,bytes_out 2>/dev/null |
    awk -F, -v pid="$CLIENT_PID" 'NR > 1 && $1 ~ ("\\." pid "$") { print $2 "\t" $3; exit }' || true)"
  [[ -n "$sample" ]] || sample=$'-\t-'
  printf '%s\t%s\t%s\t%s\n' "$edge" "$EPOCHREALTIME" "$span" "$sample" >>"$OUT/$cond.window.tsv"
}

# The server's log as it stands now, into <dest>: a copy of the local server's, or a pull of the
# deployment's container log. Returns 1 when there is none to save.
server_snapshot() {
  local dest="$1"
  if [[ "$TARGET" == local ]]; then
    cp "$OUT/server.log" "$dest"
  elif [[ -n "$DEPLOYMENT" ]]; then
    "$SESSION" logs "$DEPLOYMENT" "$dest" >/dev/null
  else
    return 1
  fi
}

launch_load() {
  local cond="$1" i
  for (( i = 1; i <= load_count; i++ )); do
    env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE -u SPIKE_FRAME_COST_OFFSCREEN -u SPIKE_FRAME_COST \
      SPIKE_SIMULATE_INPUT=1 OVERMATCH_SERVER="$ADDR" OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
      SPIKE_SIM_TICKS="$SIM_TICKS" BEVY_ASSET_ROOT="$ASSET_ROOT" ${=LOAD_WORKLOAD} \
      "$CLIENT" >"$OUT/$cond.load$i.log" 2>&1 &
    LOAD_PIDS+=($!)
  done
}

condition_launched() {
  connected=""
  edge_start=""
  edge_end=""
  marked=""
  LOAD_PIDS=()
  printf 'edge\tepoch_s\tspan_s\tbytes_in\tbytes_out\n' >"$OUT/$1.window.tsv"
}

condition_poll() {
  local cond="$1" log="$2" span="$4" i
  if [[ -z "$connected" ]] && grep -q -- "$INPUT_SLOT" "$log"; then
    connected=1
    launch_load "$cond"
  fi
  # The server window's start: its last stats line, taken inside the warmup so a container-log pull
  # never runs inside the measured window.
  if [[ -z "$marked" ]] && (( span >= WARMUP_S - 2 )); then
    marked=1
    if server_snapshot "$OUT/$cond.marker.log" 2>/dev/null; then
      grep 'server_stats' "$OUT/$cond.marker.log" | tail -1 >"$OUT/$cond.marker" || true
    fi
    rm -f "$OUT/$cond.marker.log"
  fi
  if [[ -z "$edge_start" ]] && (( span >= WARMUP_S )); then
    [[ -n "$connected" ]] || frame_run_invalid "$cond" "the measured client held no input slot by the end of the ${WARMUP_S}s warmup — unreachable server, or a slow connect (raise WARMUP_S; see $log)"
    for (( i = 1; i <= load_count; i++ )); do
      grep -q -- "$INPUT_SLOT" "$OUT/$cond.load$i.log" 2>/dev/null \
        || frame_run_invalid "$cond" "load client $i held no input slot by the end of the warmup, so the window would watch the ramp (raise WARMUP_S; see $OUT/$cond.load$i.log)"
    done
    edge_start=1
    net_edge "$cond" start "$span"
  fi
  if [[ -z "$edge_end" ]] && (( span >= WARMUP_S + DURATION_S )); then
    edge_end=1
    net_edge "$cond" end "$span"
    server_snapshot "$OUT/$cond.server.log" 2>/dev/null \
      || echo "   $cond: no server log to save; its server row stays empty" >&2
    for (( i = 1; i <= load_count; i++ )); do
      alive "${LOAD_PIDS[$i]}" \
        || frame_run_invalid "$cond" "load client $i exited before the window closed (see $OUT/$cond.load$i.log)"
    done
    stop_load
  fi
}

condition_gates() {
  local cond="$1" log="$2" connects i
  connects="$(grep -c 'client: connected (entity' "$log" || true)"
  (( connects == 1 )) || frame_run_invalid "$cond" "$connects connects for the measured client — a reconnect burned a lane (cold shader cache?); rerun warm (see $log)"
  for (( i = 1; i <= load_count; i++ )); do
    connects="$(grep -c 'client: connected (entity' "$OUT/$cond.load$i.log" || true)"
    (( connects == 1 )) || frame_run_invalid "$cond" "load client $i connected $connects times (see $OUT/$cond.load$i.log)"
  done
  (( $(wc -l <"$OUT/$cond.window.tsv") == 3 )) || frame_run_invalid "$cond" "the window's edges were not both recorded ($OUT/$cond.window.tsv)"
  echo "$cond: target $ADDR, load clients $load_count, client env [${CLIENT_WORKLOAD} ${envs}]" >>"$OUT/manifest.txt"
}

{
  echo "target: $ADDR${DEPLOYMENT:+ (Edgegap deployment $DEPLOYMENT)}"
  [[ "$TARGET" == local ]] && echo "local server: $BIN/overmatch-server  env [OVERMATCH_SERVER_BIND=127.0.0.1 $STATS_LEVER=$STATS_PERIOD_S $SERVER_ENV]"
  echo "base video fields: $BASE_FIELDS"
  echo "measured client: SPIKE_SIMULATE_INPUT=1 ${MODE_ENV[*]} workload [$CLIENT_WORKLOAD]; load clients: GPU-less, workload [$LOAD_WORKLOAD]"
  echo "client network: nettop -P -p <measured pid> -x at the window's edges, rates over the stream span"
} >>"$OUT/manifest.txt"

if [[ "$TARGET" == local ]]; then
  # Another server on the port would take the clients and make every number someone else's.
  if lsof -nP -iUDP:5888 >/dev/null 2>&1; then
    echo "REFUSING: UDP 5888 is already bound (lsof -nP -iUDP:5888) — another server is running" >&2
    exit 1
  fi
  stats_env=()
  (( STATS_PERIOD_S > 0 )) && stats_env=("$STATS_LEVER=$STATS_PERIOD_S")
  env BEVY_ASSET_ROOT="$ASSET_ROOT" OVERMATCH_SERVER_BIND=127.0.0.1 "${stats_env[@]}" ${=SERVER_ENV} \
    "$BIN/overmatch-server" >"$OUT/server.log" 2>&1 &
  SERVER_PID=$!
  for _ in {1..150}; do
    grep -q "listening" "$OUT/server.log" 2>/dev/null && break
    alive "$SERVER_PID" || break
    sleep 0.2
  done
  alive "$SERVER_PID" && grep -q "listening" "$OUT/server.log" \
    || { echo "the local server never listened (see $OUT/server.log)" >&2; exit 1; }
fi

for spec in "${CONDITIONS[@]}"; do
  cond="${spec%%|*}"
  rest="${spec#*|}"
  [[ "$spec" == *"|"* ]] || rest=""
  fields="${rest%%|*}"
  envs=""
  [[ "$rest" == *"|"* ]] && envs="${rest#*|}"
  load_count=0
  client_env=()
  for word in ${=envs}; do
    case "$word" in
      LOAD_CLIENTS=*) load_count="${word#*=}" ;;
      *) client_env+=("$word") ;;
    esac
  done
  frame_run_condition "$cond" "$BASE_FIELDS $fields" "${MODE_ENV[@]}" \
    SPIKE_SIMULATE_INPUT=1 OVERMATCH_SERVER="$ADDR" OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    SPIKE_SIM_TICKS="$SIM_TICKS" ${=CLIENT_WORKLOAD} "${client_env[@]}"
  ( cd "$REPO" && uv run scripts/perf/online_cost.py server-window --label "$cond" \
      --after-marker "$OUT/$cond.marker" --json "$OUT/$cond.server.json" "$OUT/$cond.server.log" )
done
collected_exit=1

frame_run_analyze "${CONDITIONS[1]%%|*}" "$OUT"/*.client.jsonl
( cd "$REPO" && uv run scripts/perf/online_cost.py tables "$OUT" ) | tee "$OUT/tables.txt"

echo "online run complete -> $OUT"
