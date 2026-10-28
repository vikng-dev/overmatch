#!/bin/zsh
# =============================== RUNBOOK ===============================
# The track prints' relief levels in frames (cosmetic terrain marks, issue 125): what each TRACK
# PRINTS level — LIGHTING, PARALLAX, SUN SHADOW — costs while prints fill the screen, on the mud
# pack and on the base pack. The relief is drawn only on print pixels within TRACK PRINT RANGE, so
# the scene is prints held on screen: the track-print demo (src/track_prints/demo.rs) sets the
# player's tank down on the spot at 10 s, drives it straight, stops, pivots, drives out, and from
# ~37 s holds the camera ~10 m off the pivot for good (`OVERMATCH_TRACK_PRINTS_DEMO_HOLD`), at the
# relief level and range the condition injects into video.ron. Nothing but those two settings
# differs between two conditions on one ground. Offline single-player path, no server.
#
# Conditions: <ground>-<level>; ground mud (599.9,-599.9 heading 195°) or base (70,-20 heading
# 90°) — the demo's documented spots, fixture data where one pack dominates the painted mask —
# and level lighting, parallax or sun. Default order: mud-lighting mud-parallax mud-sun
# base-lighting base-parallax base-sun. Deltas print per ground against its lighting condition.
#
# Build: an optimized build carries the demo only under the `track_prints_demo` feature, and the
# measured build takes the shipped feature set plus that one (no dev_ui, no bitprobe):
#   scripts/serial-cargo build --locked --release --bin overmatch --features track_prints_demo
#
# Validity: every precondition and gate of the frame sweep (run-frame-sweep.sh's runbook, code in
# frame-run.zsh), plus this runner's own gates, per condition:
#   * the demo armed with its hold, and the hold reached with the injected relief and range in
#     force on the player's settings (the demo forces no level while it holds);
#   * prints laid before the hold: a non-zero stamp count;
#   * the hold's start, on the frame stream's own clock, at or before the measurement window's
#     start: frames before the camera settled on the prints are warmup, never evidence.
# The game is muted (OVERMATCH_MUTE=1) and does not pause on a focus loss
# (OVERMATCH_NO_FOCUS_PAUSE=1, whose log line the manifest counts) in every condition.
#
# Expected wall-clock: 6 conditions x (startup + 50 s warmup + 60 s measure + 15 s cooldown)
# ~ 13-15 min. SMOKE (hidden, numbers fiction): two conditions, ~2.5 min:
#   SMOKE=1 scripts/perf/run-track-prints-cost.sh /tmp/track-prints-smoke
# =======================================================================
#
# Usage: run-track-prints-cost.sh <out-dir> [condition ...]
# Knobs (env): BIN (directory holding `overmatch`, absolute or relative to the repo; default
#   target/release), ASSET_ROOT (directory holding `assets/`; default the repo), VIEW (pivot or
#   drive, the demo view held; default pivot), RANGE (Near, Middle, Far or Farthest; default Far,
#   the shipped default), DURATION_S (60), WARMUP_S (50; the hold starts ~37 s in), COOLDOWN_S (15),
#   MIN_ROWS, SMOKE, STARTUP_GRACE_S (60).

set -eu
setopt null_glob

OUT="${1:?usage: run-track-prints-cost.sh <out-dir> [condition ...]}"
shift
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
VIEW="${VIEW:-pivot}"
RANGE="${RANGE:-Far}"
SMOKE="${SMOKE:-}"
WARMUP_S="${WARMUP_S:-50}"
if [[ -n "$SMOKE" ]]; then
  DURATION_S="${DURATION_S:-10}"
  COOLDOWN_S="${COOLDOWN_S:-2}"
else
  DURATION_S="${DURATION_S:-60}"
  COOLDOWN_S="${COOLDOWN_S:-15}"
fi
MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"

case "$VIEW" in
  pivot) held="PivotView(0)" ;;
  drive) held="DriveView(0)" ;;
  *) echo "VIEW must be pivot or drive, got $VIEW" >&2; exit 1 ;;
esac
case "$RANGE" in
  Near|Middle|Far|Farthest) ;;
  *) echo "RANGE must be Near, Middle, Far or Farthest, got $RANGE" >&2; exit 1 ;;
esac

CONDITIONS=("$@")
if (( ${#CONDITIONS[@]} == 0 )); then
  CONDITIONS=(mud-lighting mud-parallax mud-sun base-lighting base-parallax base-sun)
  [[ -n "$SMOKE" ]] && CONDITIONS=(mud-lighting base-sun)
fi
for cond in "${CONDITIONS[@]}"; do
  case "$cond" in
    mud-lighting|mud-parallax|mud-sun|base-lighting|base-parallax|base-sun) ;;
    *) echo "unknown condition $cond" >&2; exit 1 ;;
  esac
done

source "$REPO/scripts/perf/frame-run.zsh"
frame_run_begin
echo "scene: track-print demo held on its $VIEW view, track_print_range $RANGE" >>"$OUT/manifest.txt"

# The hold line the demo writes once its camera settles: the view, its real start, the relief and
# range in force, the stamps laid.
condition_gates() {
  local cond="$1" log="$2" stream="$3"
  grep -q "track prints demo: armed at .* hold Some($held)" "$log" \
    || frame_run_invalid "$cond" "the demo never armed with its $VIEW hold (see $log)"
  local line
  line="$(grep -o "track prints demo: holding $held from real t=.*" "$log" | head -1)"
  [[ -n "$line" ]] || frame_run_invalid "$cond" "the demo never reached its hold (see $log)"
  [[ "$line" == *"relief $relief, range $RANGE,"* ]] \
    || frame_run_invalid "$cond" "the hold ran at another relief or range than injected ($relief, $RANGE): $line"
  local stamps="${${line##*range $RANGE, }%% stamps*}"
  (( stamps > 0 )) || frame_run_invalid "$cond" "no prints were laid before the hold: $line"
  local hold_t="${${line#*real t=}%%s *}"
  local t_first
  t_first="$(frame_run_t_first "$stream")"
  (( hold_t <= t_first + WARMUP_S )) \
    || frame_run_invalid "$cond" "the hold began at t=${hold_t}s, after the measurement window opened at t=$((t_first + WARMUP_S))s — raise WARMUP_S"
  echo "$cond: $line" >>"$OUT/manifest.txt"
  echo "$cond: focus losses with the pause suppressed: $(grep -c 'pause suppressed' "$log" || true)" >>"$OUT/manifest.txt"
}

for cond in "${CONDITIONS[@]}"; do
  case "$cond" in
    mud-*) spot="599.9,-599.9,195" ;;
    base-*) spot="70,-20,90" ;;
  esac
  case "$cond" in
    *-lighting) relief=Lighting ;;
    *-parallax) relief=Parallax ;;
    *-sun) relief=SunShadow ;;
  esac
  frame_run_condition "$cond" "track_prints: $relief, track_print_range: $RANGE," \
    OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_TRACK_PRINTS_DEMO="$spot" OVERMATCH_TRACK_PRINTS_DEMO_HOLD="$VIEW"
done

for ground in mud base; do
  streams=("$OUT/$ground"-*.client.jsonl)
  (( ${#streams[@]} > 0 )) || continue
  baseline="$ground-lighting"
  [[ -f "$OUT/$baseline.client.jsonl" ]] || baseline="$(basename "${streams[1]}" .client.jsonl)"
  echo "\n#### $ground"
  frame_run_analyze "$baseline" "${streams[@]}"
done

echo "track-print relief run complete -> $OUT"
