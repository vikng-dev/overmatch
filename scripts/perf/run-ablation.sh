#!/bin/zsh
# =============================== RUNBOOK ===============================
# Frame-cost ablations: what one render setting or feature costs, by difference, in the default
# offline duel (or at OVERMATCH_PROBE_TANKS tanks). Each condition is spelled on the command line,
# so one runner prices any lever that a video.ron field or an env var reaches:
#
#   OFFSCREEN=1 scripts/perf/run-ablation.sh <out-dir> \
#     'base-a|' 'msaa-off-a|msaa: Off,' 'msaa-off-b|msaa: Off,' 'base-b|'
#
# A condition is `name|video fields|VAR=value VAR=value …`: the name is the analyzer's label (unique
# per run), the fields go into the condition's video.ron beside `vsync_mode: Off`, and the
# assignments reach the client's environment. Order the conditions A/B/B/A so a linear drift (heat,
# the owner's load) cancels from the averaged delta; the analyzer prints each condition's delta
# against the first.
#
# Run it OFFSCREEN=1 (frame-run.zsh): a hidden window, nothing on anyone's screen, every frame row
# proving the 3D camera drew. A visible run takes the frame sweep's preconditions and gates
# (run-frame-sweep.sh's runbook). The game is muted and does not pause on a focus loss.
#
# Useful levers: `msaa:`, `render_scale:`, `shadow_distance:`, `shadow_cascades:`,
# `shadow_resolution:`, `track_prints:` (video.ron, src/settings.rs); OVERMATCH_PROBE_TANKS;
# SPIKE_ABLATE=bloom,env_light,depth_prepass,ui and SPIKE_PHASE_COUNTS=1 (src/render_cost.rs).
# This runner gates on the probe tanks having spawned and on the client naming its ablation.
# =======================================================================
#
# Usage: run-ablation.sh <out-dir> <condition> ...
# Knobs (env): BIN (directory holding `overmatch`, absolute or relative to the repo; default
#   target/release), ASSET_ROOT (default the repo), OFFSCREEN, DURATION_S (30), WARMUP_S (15),
#   COOLDOWN_S (5), MIN_ROWS (DURATION_S*20), STARTUP_GRACE_S (60).

set -eu
setopt null_glob

OUT="${1:?usage: run-ablation.sh <out-dir> <condition> ...}"
shift
(( $# > 0 )) || { echo "usage: run-ablation.sh <out-dir> <condition> ..." >&2; exit 1; }
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
SMOKE="${SMOKE:-}"
DURATION_S="${DURATION_S:-30}"
WARMUP_S="${WARMUP_S:-15}"
COOLDOWN_S="${COOLDOWN_S:-5}"
MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"
CONDITIONS=("$@")

source "$REPO/scripts/perf/frame-run.zsh"
frame_run_begin

condition_gates() {
  local cond="$1" log="$2" assignment
  for assignment in ${=envs}; do
    case "$assignment" in
      OVERMATCH_PROBE_TANKS=*)
        local tanks="${assignment#*=}"
        if (( tanks > 2 )); then
          grep -q "offline: spawned $((tanks - 2)) probe tanks" "$log" \
            || frame_run_invalid "$cond" "probe-tank spawn missing (see $log)"
        fi ;;
      SPIKE_ABLATE=*)
        grep -q "render_cost: ablating" "$log" \
          || frame_run_invalid "$cond" "the client never named its ablation (see $log)" ;;
    esac
  done
  echo "$cond: video fields [${fields}] env [${envs}]" >>"$OUT/manifest.txt"
}

for spec in "${CONDITIONS[@]}"; do
  cond="${spec%%|*}"
  rest="${spec#*|}"
  fields="${rest%%|*}"
  envs=""
  [[ "$rest" == *"|"* ]] && envs="${rest#*|}"
  frame_run_condition "$cond" "$fields" OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 ${=envs}
done

frame_run_analyze "${CONDITIONS[1]%%|*}" "$OUT"/*.client.jsonl

echo "ablation run complete -> $OUT"
