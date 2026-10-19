#!/bin/zsh
# =============================== RUNBOOK ===============================
# The pen cam's frame cost in the game: what each surface costs while it is on screen — the
# shooter's corner, the victim's glass, the kill cam — offline and over the network. Every stream
# carries its own surface marks (debug lines the client writes on the frame stream's clock,
# `Time<Real>`; src/damage_cam/pen_cam/game.rs `log_presence`, game/killcam.rs `log_run`), so each
# number is a same-session comparison: frames with the surface on screen against frames without,
# same clock, same thermals, same background load. The paired `-off` condition runs the same
# scripted schedule with the surface's setting Off, and the analyzer applies the `-on` run's
# windows to it (DERIVED alignment — the same seconds hold the same workload minus the surface).
#
# Usage: run-pencam-cost.sh <out-dir> [condition ...]
# Conditions (default: all eight, each -on before its -off):
#   corner-on/off   offline target-ahead duel; the scripted trigger fires the player's guns
#                   (SPIKE_AUTO_FIRE: MG 20-50 s, main gun 60-75 s). PEN CAM CORNER All / Off.
#   glass-on/off    offline target-fires: the target's main gun into the player's flank at
#                   25/40/55 s. VICTIM GLASS PEEK On + the glass held 65-95 s
#                   (OVERMATCH_GLASS_HOLD) / Off and no hold.
#   killcam-on/off  offline target-kills: the target fires into the player's ammunition from 25 s
#                   every 10 s until the knockout. KILL CAM On / Off. Offline never respawns, so
#                   the stream ends on the death screen.
#   mp-on/off       a local dedicated server and the latency runner's duel
#                   (scripts/shot/run-pencam-latency.sh): the MEASURED client connects first and
#                   runs VISIBLE (SPIKE_SIM_VISIBLE=1), the opponent hidden. All three settings
#                   On / Off. The opponent's rounds knock the measured tank out about 30 s in
#                   (MEASURED 2026-10-19), and the scripted client never respawns: the analysis
#                   stops at the kill cam run's start (the on-run's mark, for both runs), where
#                   the frames turn into the death screen. The MP numbers are the corner's and
#                   the glass's over the net before that, nothing more.
#
# Validity conditions are the frame sweep's (scripts/perf/run-frame-sweep.sh): release binaries,
# the game window VISIBLE, frontmost and unoccluded on the primary display at 2560x1440 physical,
# machine quiet, hands off, warm shader cache (discard the first run after a rebuild). Enforced:
# the injected settings loaded, the scenario armed, the uncapped present mode proven, the shared
# analyzer's stream gates (monotonic, span, occlusion, display, surface), for the MP conditions
# exactly one connection per client, and for every `-on` condition at least one on-screen mark
# of its surface. The game is muted (OVERMATCH_MUTE=1) in every condition, on and off alike.
# =======================================================================

set -eu
setopt null_glob

OUT="${1:?usage: run-pencam-cost.sh <out-dir> [condition ...]}"
shift || true
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
CLIENT="$REPO/$BIN/overmatch"
SERVER="$REPO/$BIN/overmatch-server"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"
WARMUP_S=5
# The marks are `debug!` lines: the default filter (INFO, wgpu=error, naga=warn) plus them.
LOG_FILTER="info,wgpu=error,naga=warn,overmatch::damage_cam::pen_cam=debug"

[[ -x "$CLIENT" ]] || { echo "no client binary at $CLIENT — build it first (cargo build --locked --release --bin overmatch --bin overmatch-server)" >&2; exit 1; }

CONDITIONS=("$@")
if (( ${#CONDITIONS[@]} == 0 )); then
  CONDITIONS=(corner-on corner-off glass-on glass-off killcam-on killcam-off mp-on mp-off)
fi
for cond in "${CONDITIONS[@]}"; do
  case "$cond" in
    corner-on|corner-off|glass-on|glass-off|killcam-on|killcam-off|mp-on|mp-off) ;;
    *) echo "unknown condition $cond" >&2; exit 1 ;;
  esac
  if [[ "$cond" == mp-* && ! -x "$SERVER" ]]; then
    echo "no server binary at $SERVER — build it first" >&2; exit 1
  fi
done

mkdir -p "$OUT"
rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/*.cost.json "$OUT"/manifest.txt "$OUT"/summary.txt
rm -rf "$OUT"/cfg-*

CLIENT_PID=""
OTHER_PID=""
SERVER_PID=""
cleanup() {
  for pid in "$CLIENT_PID" "$OTHER_PID" "$SERVER_PID"; do
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
  done
  for pid in "$CLIENT_PID" "$OTHER_PID" "$SERVER_PID"; do
    [[ -n "$pid" ]] && wait "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

{
  echo "commit: $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD 2>/dev/null || echo ' (dirty)')"
  echo "bin: $BIN  conditions: ${CONDITIONS[*]}"
  echo "client: $CLIENT  $(stat -f 'mtime=%Sm size=%z' "$CLIENT")"
  echo "host: $(uname -m) $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
} >"$OUT/manifest.txt"

invalid() {
  echo "$1 INVALID: $2" >&2
  exit 1
}

# Wait until $1 (a frame stream) spans $2 seconds, or the process $3 exits, or the deadline passes.
# Prints `spanned`, `exited` or `timeout`. The caller reaps the process and reads its status: this
# runs in a command substitution's subshell, where the client is not a child and `wait` cannot.
wait_span() {
  local stream="$1" run_s="$2" pid="$3"
  local deadline=$((SECONDS + run_s + STARTUP_GRACE_S))
  local t_pat='s/.*"t"[[:space:]]*:[[:space:]]*\([0-9.eE+-]*\).*/\1/p'
  # Declared once: zsh prints an existing local's value on a bare re-declaration, and this
  # function's stdout is the outcome.
  local state t_first t_last
  while (( SECONDS < deadline )); do
    sleep 1
    state="$(ps -o state= -p "$pid" 2>/dev/null || true)"
    if [[ -z "$state" || "$state" == *Z* ]]; then
      echo exited
      return
    fi
    [[ -f "$stream" ]] || continue
    t_first="$(head -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    t_last="$(tail -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    if [[ -n "$t_first" && -n "$t_last" ]] && (( t_last - t_first >= run_s )); then
      echo spanned
      return
    fi
  done
  echo timeout
}

for cond in "${CONDITIONS[@]}"; do
  # Per-condition shape.
  mode=offline; corner=Off; glass=Off; killcam=Off; hold=""; armed=""; surfaces=(); scen=()
  case "$cond" in
    corner-*)  run_s=82;  scen=(OVERMATCH_TARGET_AHEAD=1 SPIKE_AUTO_FIRE=1);        armed="auto_fire: armed";    surfaces=(corner) ;;
    glass-*)   run_s=100; scen=(OVERMATCH_TARGET_AHEAD=1 OVERMATCH_TARGET_FIRES=1); armed="target fires: armed"; surfaces=(glass) ;;
    killcam-*) run_s=100; scen=(OVERMATCH_TARGET_AHEAD=1 OVERMATCH_TARGET_KILLS=1); armed="target kills: armed"; surfaces=(killcam) ;;
    mp-*)      run_s=120; mode=mp;                                                  surfaces=(corner glass) ;;
  esac
  if [[ "$cond" == *-on ]]; then
    case "$cond" in
      corner-on)  corner=All ;;
      glass-on)   glass=On; hold="65-95" ;;
      killcam-on) killcam=On ;;
      mp-on)      corner=All; glass=On; killcam=On ;;
    esac
  fi

  cfg="$OUT/cfg-$cond"
  mkdir -p "$cfg"
  # The shipped shadow default (M350) so the measurement is of the game as played; vsync Off so
  # frame times are the workload's, not the display's; the primary display (the surface gate).
  printf '(\n    version: 1,\n    shadow_distance: M350,\n    vsync_mode: Off,\n    display: Primary,\n    pen_cam_corner: %s,\n    victim_glass_peek: %s,\n    kill_cam: %s,\n)\n' \
    "$corner" "$glass" "$killcam" >"$cfg/video.ron"

  stream="$OUT/$cond.client.jsonl"
  log="$OUT/$cond.log"
  echo "== $cond: $mode, corner $corner, glass $glass, kill cam $killcam${hold:+, glass held $hold s} (${run_s}s) =="

  if [[ "$mode" == offline ]]; then
    hold_env=()
    [[ -n "$hold" ]] && hold_env=(OVERMATCH_GLASS_HOLD="$hold")
    env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE \
      OVERMATCH_MUTE=1 OVERMATCH_CONFIG_DIR="$cfg" "${scen[@]}" "${hold_env[@]}" \
      SPIKE_FRAME_COST="$OUT/$cond.jsonl" RUST_LOG="$LOG_FILTER" BEVY_ASSET_ROOT="$REPO" \
      "$CLIENT" --offline >"$log" 2>&1 &
    CLIENT_PID=$!
    outcome="$(wait_span "$stream" "$run_s" "$CLIENT_PID")"
    case "$outcome" in
      spanned) kill "$CLIENT_PID" 2>/dev/null || true; wait "$CLIENT_PID" 2>/dev/null || true ;;
      exited)
        code=0; wait "$CLIENT_PID" 2>/dev/null || code=$?; CLIENT_PID=""
        invalid "$cond" "client exited ($code) before the stream spanned ${run_s}s (see $log)" ;;
      *) invalid "$cond" "stream never spanned ${run_s}s within the deadline (see $log)" ;;
    esac
    CLIENT_PID=""
    grep -q "$armed" "$log" || invalid "$cond" "the scenario never armed ('$armed' missing; see $log)"
    if [[ "$cond" == killcam-* ]]; then
      grep -q "knocked out at" "$log" || invalid "$cond" "the player's tank was never knocked out (see $log)"
    fi
  else
    # DERIVED: 64 Hz fixed ticks; the measured client runs 128 ticks longer so it outlives the
    # later-started opponent (as the latency runner's first client does).
    ticks=$((run_s * 64))
    # The flat patch the latency runner documents; lane 1 spawns 8 m along +x from lane 0.
    env OVERMATCH_MUTE=1 SPIKE_SPAWN_POSE="149.3,8.75,293.9,0,0,0,1" BEVY_ASSET_ROOT="$REPO" \
      "$SERVER" >"$OUT/$cond.server.log" 2>&1 &
    SERVER_PID=$!
    for _ in {1..150}; do
      grep -q "listening" "$OUT/$cond.server.log" 2>/dev/null && break
      sleep 0.2
    done
    grep -q "listening" "$OUT/$cond.server.log" || invalid "$cond" "the server never listened"
    # The measured client: lane 0, visible, its guns on the opponent's engine bay.
    env OVERMATCH_MUTE=1 OVERMATCH_CONFIG_DIR="$cfg" \
      SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 SPIKE_SIM_VISIBLE=1 OVERMATCH_SERVER=127.0.0.1:5888 \
      SPIKE_SIM_TICKS="$((ticks + 128))" SPIKE_FIRE_PRIMARY=1 SPIKE_FIRE_SECONDARY=1 \
      SPIKE_FIRE_TICK=320 SPIKE_FIRE_INTERVAL=256 SPIKE_AIM_POINT="8,1.2,2.2" SPIKE_SIM_RANGE=12 \
      SPIKE_FRAME_COST="$OUT/$cond.jsonl" RUST_LOG="$LOG_FILTER" BEVY_ASSET_ROOT="$REPO" \
      "$CLIENT" >"$log" 2>&1 &
    CLIENT_PID=$!
    for _ in {1..300}; do
      grep -q "client connected" "$OUT/$cond.server.log" 2>/dev/null && break
      sleep 0.2
    done
    grep -q "client connected" "$OUT/$cond.server.log" || invalid "$cond" "the measured client never connected (see $log)"
    # The opponent: lane 1, hidden and muted, its guns on the measured tank's front hull.
    env -u SPIKE_SIM_VISIBLE OVERMATCH_MUTE=1 \
      SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_SERVER=127.0.0.1:5888 \
      SPIKE_SIM_TICKS="$ticks" SPIKE_FIRE_PRIMARY=1 SPIKE_FIRE_SECONDARY=1 \
      SPIKE_FIRE_TICK=448 SPIKE_FIRE_INTERVAL=256 SPIKE_AIM_POINT="-8,0.8,-2.2" SPIKE_SIM_RANGE=12 \
      BEVY_ASSET_ROOT="$REPO" \
      "$CLIENT" >"$OUT/$cond.opponent.log" 2>&1 &
    OTHER_PID=$!
    # The scripted client exits on its own after its ticks (the usual end: `exited`, status 0);
    # `spanned` is the fallback if the stream outruns the schedule, and a kill ends it then.
    outcome="$(wait_span "$stream" "$((run_s + 10))" "$CLIENT_PID")"
    code=0
    if [[ "$outcome" == exited ]]; then
      wait "$CLIENT_PID" 2>/dev/null || code=$?
    else
      kill "$CLIENT_PID" 2>/dev/null || true; wait "$CLIENT_PID" 2>/dev/null || true
    fi
    CLIENT_PID=""
    kill "$OTHER_PID" 2>/dev/null || true; wait "$OTHER_PID" 2>/dev/null || true; OTHER_PID=""
    kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; SERVER_PID=""
    case "$outcome" in
      spanned) ;;
      exited) (( code == 0 )) || invalid "$cond" "measured client exited with status $code (see $log)" ;;
      *) invalid "$cond" "measured client: $outcome (see $log)" ;;
    esac
    connects="$(grep -c "client connected" "$OUT/$cond.server.log" || true)"
    (( connects == 2 )) || invalid "$cond" "$connects client connects for 2 clients — a lane burned (cold shader cache?); rerun warm"
  fi

  [[ -f "$stream" ]] || invalid "$cond" "no frame stream at $stream"
  grep -q "settings: loaded $cfg/video.ron" "$log" || invalid "$cond" "injected settings were not loaded"
  grep -q "frame_cost: effective present mode Immediate, frame cap off" "$log" \
    || invalid "$cond" "no proof the run was uncapped (see $log)"
  if grep -qE 'settings: vsync .* is not supported by this surface' "$log"; then
    invalid "$cond" "vsync Off was normalized away — the run was display-paced"
  fi
  ( cd "$REPO" && uv run scripts/perf/analyze.py --validate-only --warmup-s "$WARMUP_S" \
      --min-rows $((run_s * 20)) --expected-duration-s $((run_s - WARMUP_S)) "$stream" ) \
    || invalid "$cond" "frame stream failed the analyzer's validity gates (see above)"

  # The surface marks: an -on run must carry its own; an -off run takes its paired -on run's.
  marks="$(grep -c "the slot shows\|kill cam: the run" "$log" || true)"
  surface_args=()
  for s in "${surfaces[@]}"; do surface_args+=(--surface "$s"); done
  windows_args=()
  if [[ "$cond" == *-off ]]; then
    on_log="$OUT/${cond%-off}-on.log"
    [[ -f "$on_log" ]] || invalid "$cond" "paired run's log $on_log missing — run ${cond%-off}-on first"
    windows_args=(--windows-from "$on_log" --paired-json "$OUT/${cond%-off}-on.cost.json")
  elif (( marks == 0 )); then
    invalid "$cond" "no surface mark in $log — the surface never came on screen, or the debug marks are not in this binary"
  fi
  # MP: the measured tank's knockout ends the workload under measurement (see the runbook).
  until_args=()
  if [[ "$mode" == mp ]]; then
    cut="$(sed -n 's/.*kill cam: the run starts at t=\([0-9.]*\) s.*/\1/p' "$OUT/${cond%-*}-on.log" | head -n1)"
    [[ -n "$cut" ]] && until_args=(--until-s "$cut")
  fi
  ( cd "$REPO" && uv run scripts/perf/pencam_cost.py --stream "$stream" --log "$log" \
      "${surface_args[@]}" "${windows_args[@]}" "${until_args[@]}" --warmup-s "$WARMUP_S" \
      --label "$cond" --json "$OUT/$cond.cost.json" ) | tee -a "$OUT/summary.txt" \
    || invalid "$cond" "the pen cam cost analyzer failed (see above)"
  {
    echo "$cond: $(grep -m1 -o 'frame_cost: effective present mode.*' "$log")"
    echo "$cond: occlusion transitions in stream: $(grep -c '"occluded"' "$stream" || true)"
    echo "$cond: surface marks: $marks"
  } >>"$OUT/manifest.txt"
  echo "   $cond OK: $(wc -l <"$stream" | tr -d ' ') rows, $marks marks"
  sleep 5
done

echo "capture complete -> $OUT (summary.txt, *.cost.json)"
