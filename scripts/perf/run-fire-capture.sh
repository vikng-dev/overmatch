#!/bin/zsh
# =============================== RUNBOOK ===============================
# The sustained-fire frame-cost capture: what does holding the MG trigger cost, and where does it
# go? Offline single-player, 2 tanks, ONE stream per condition containing BOTH an idle window and a
# firing window on the same clock — so firing is always compared against a same-session baseline
# rather than against absolutes from another run (thermals and background load move absolutes).
#
# Fire is driven by the dev-only scripted trigger (SPIKE_AUTO_FIRE=1, src/command.rs `auto_fire`);
# its hardcoded schedule IS the window map below. A human holding the key would also be feeding the
# window input, which changes the workload the sweep is trying to measure. The hook is mounted ONLY
# by the offline root (`command::offline_auto_fire_plugin`, mounted in `run_offline`), which is why
# every condition below launches `--offline`: under any network client the env var does nothing.
#
#   idle 0-20s | MG held 20-50s | idle 50-60s | main gun 60-75s | idle 75s+
#
# Usage: run-fire-capture.sh <out-dir> [condition ...]
# Conditions are `name:MUZZLE_SHADOWS` pairs; default is the shipped lever vs the off lever, which
# is the A/B that isolates muzzle-light shadow cost (bevy has no shadow-pass diagnostic span, so
# SPIKE_RENDER_COST cannot see it — the lever is the only instrument).
#
# Validity conditions are the frame sweep's, for the same reasons (see run-frame-sweep.sh):
# visible unoccluded frontmost window, machine quiet, hands off, warm shader cache. Gates enforced
# below: injected settings loaded, uncapped present mode proven, no occlusion overlapping the
# measurement window, monotonic full-span stream.
# =======================================================================

set -eu
setopt null_glob

OUT="${1:?usage: run-fire-capture.sh <out-dir> [name:shadowlever ...]}"
shift || true
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
CLIENT="$REPO/$BIN/overmatch"
# The auto-fire schedule's end plus a tail, in app-elapsed seconds.
RUN_S="${RUN_S:-82}"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"

if [[ -n "${SPIKE_SIM_WINDOWED:-}" ]]; then
  echo "REFUSING: SPIKE_SIM_WINDOWED is exported — a hidden window free-runs and every number" >&2
  echo "would be fiction. unset it." >&2
  exit 1
fi
[[ -x "$CLIENT" ]] || { echo "no client binary at $CLIENT — build it first" >&2; exit 1; }

CONDITIONS=("$@")
if (( ${#CONDITIONS[@]} == 0 )); then
  CONDITIONS=("shipped:on" "mgshadow-off:off")
fi

mkdir -p "$OUT"
rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/manifest.txt
rm -rf "$OUT"/cfg-*

CLIENT_PID=""
cleanup() {
  [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
  [[ -n "$CLIENT_PID" ]] && wait "$CLIENT_PID" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

{
  echo "commit: $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD 2>/dev/null || echo ' (dirty)')"
  echo "run_s: $RUN_S  bin: $BIN  conditions: ${CONDITIONS[*]}"
  echo "schedule: idle 0-20 | MG 20-50 | idle 50-60 | main gun 60-75 | idle 75+"
  echo "binary: $CLIENT  $(stat -f 'mtime=%Sm size=%z' "$CLIENT")"
  echo "host: $(uname -m) $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
} >"$OUT/manifest.txt"

for entry in "${CONDITIONS[@]}"; do
  cond="${entry%%:*}"
  lever="${entry#*:}"

  cfg="$OUT/cfg-$cond"
  mkdir -p "$cfg"
  # The shipped shadow default (M350) so the measurement is of the game as played; vsync Off so
  # frame times are the workload's, not the display's.
  printf '(\n    version: 1,\n    shadow_distance: M350,\n    vsync_mode: Off,\n)\n' >"$cfg/video.ron"

  echo "== $cond: OVERMATCH_MUZZLE_SHADOWS=$lever (${RUN_S}s) =="
  env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE \
    OVERMATCH_CONFIG_DIR="$cfg" \
    OVERMATCH_MUZZLE_SHADOWS="$lever" \
    SPIKE_AUTO_FIRE=1 \
    SPIKE_FRAME_COST="$OUT/$cond.jsonl" \
    SPIKE_RENDER_COST="$OUT/$cond.render.jsonl" \
    BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" --offline >"$OUT/$cond.log" 2>&1 &
  CLIENT_PID=$!

  stream="$OUT/$cond.client.jsonl"
  # `role_path` (src/trace.rs) inserts the role before the extension, so the recorders write
  # `<cond>.client.jsonl` and `<cond>.render.client.jsonl` from the values exported above.
  render_stream="$OUT/$cond.render.client.jsonl"
  deadline=$((SECONDS + RUN_S + STARTUP_GRACE_S))
  span_ok=""
  while (( SECONDS < deadline )); do
    sleep 1
    state="$(ps -o state= -p "$CLIENT_PID" 2>/dev/null || true)"
    if [[ -z "$state" || "$state" == *Z* ]]; then
      exit_status=0
      wait "$CLIENT_PID" 2>/dev/null || exit_status=$?
      CLIENT_PID=""
      echo "$cond INVALID: client exited (status $exit_status) before the stream spanned ${RUN_S}s (see $OUT/$cond.log)" >&2
      exit 1
    fi
    [[ -f "$stream" ]] || continue
    t_pat='s/.*"t"[[:space:]]*:[[:space:]]*\([0-9.eE+-]*\).*/\1/p'
    t_first="$(head -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    t_last="$(tail -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    if [[ -n "$t_first" && -n "$t_last" ]] && (( t_last - t_first >= RUN_S )); then
      span_ok=1
      break
    fi
  done
  if [[ -z "$span_ok" ]]; then
    echo "$cond INVALID: stream never spanned ${RUN_S}s within the deadline (see $OUT/$cond.log)" >&2
    exit 1
  fi
  kill "$CLIENT_PID" 2>/dev/null || true
  wait "$CLIENT_PID" 2>/dev/null || true
  CLIENT_PID=""

  [[ -f "$stream" ]] || { echo "$cond INVALID: no frame stream at $stream" >&2; exit 1; }
  grep -q "settings: loaded $cfg/video.ron" "$OUT/$cond.log" || { echo "$cond INVALID: injected settings were not loaded" >&2; exit 1; }
  grep -q "auto_fire: armed" "$OUT/$cond.log" || { echo "$cond INVALID: the scripted trigger never armed — nothing fired" >&2; exit 1; }
  # The render-cost half was requested, so its absence is a failed run, not a quiet one: before
  # 2026-07-31 the offline root never mounted `render_cost::client_plugin` and this stream was
  # never written, while the capture still reported success.
  if [[ ! -s "$render_stream" ]]; then
    echo "$cond INVALID: SPIKE_RENDER_COST was requested but $render_stream is missing or empty (see $OUT/$cond.log)" >&2
    exit 1
  fi
  grep -q "render_cost: recording rows to" "$OUT/$cond.log" || { echo "$cond INVALID: the render-cost recorder never armed (see $OUT/$cond.log)" >&2; exit 1; }
  grep -q "frame_cost: effective present mode Immediate, frame cap off" "$OUT/$cond.log" \
    || { echo "$cond INVALID: no proof the run was uncapped (see $OUT/$cond.log)" >&2; exit 1; }
  if grep -qE 'settings: vsync .* is not supported by this surface' "$OUT/$cond.log"; then
    echo "$cond INVALID: vsync Off was normalized away — the run was display-paced" >&2; exit 1
  fi
  # Shared stream-shape gates (monotonic, span, min rows, occlusion-free measurement window).
  ( cd "$REPO" && uv run scripts/perf/analyze.py --validate-only --warmup-s 5 \
      --min-rows $((RUN_S * 20)) --expected-duration-s $((RUN_S - 5)) "$stream" ) \
    || { echo "$cond INVALID: frame stream failed the analyzer's validity gates (see above)" >&2; exit 1; }
  {
    echo "$cond: $(grep -m1 -o 'frame_cost: effective present mode.*' "$OUT/$cond.log")"
    echo "$cond: occlusion transitions in stream: $(grep -c '"occluded"' "$stream" || true)"
    echo "$cond: auto-fire phases: $(grep -c 'auto_fire: phase' "$OUT/$cond.log" || true)"
    echo "$cond: render-cost rows: $(wc -l <"$render_stream" | tr -d ' ')"
  } >>"$OUT/manifest.txt"
  echo "   $cond OK: $(wc -l <"$stream" | tr -d ' ') rows"
  sleep 5
done

echo "capture complete -> $OUT"
