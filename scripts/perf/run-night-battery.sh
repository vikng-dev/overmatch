#!/bin/zsh
# =============================== RUNBOOK ===============================
# The frame-attribution ablation battery (.agents/scratch/perf-2026-10-29/frame-attribution.md,
# "Night battery"): every render lever priced by difference in one unattended invocation, opened by
# a block that checks the offscreen mode against the real window.
#
#   BIN=<dir holding overmatch> caffeinate -dims scripts/perf/run-night-battery.sh <out-dir>
#
# The first block opens VISIBLE windows, so the owner's preconditions for a visible run hold for the
# whole battery (run-frame-sweep.sh's runbook): the built-in panel on, unlocked and lit (dimmed is
# fine, backlight 0 is not), no external monitor, hands off, the machine quiet. Every other block is
# offscreen (frame-run.zsh, OFFSCREEN): a hidden window, the cameras rendering into a 2560x1440 image
# at scale 2, every frame row proving the 3D camera drew.
#
# Blocks, each into its own subdirectory, each continuing past another's failure:
#   vis-off     visible against offscreen, 2 and 30 tanks, A/B/B/A; every gate on, the visible
#               runs keeping their `occluded=false` and `render scale:` checks
#   cal-start   the daytime calibration pair (2 and 30 tanks, offscreen, plain), to compare with day
#   duel-2      2 tanks: render scale, MSAA, shadows, cascades, resolution, bloom, sky environment
#               light, depth prepass, UI, each as an A/B/B/A block sharing its A with the next
#   battle-30   30 tanks: the levers whose cost scales with draws, same A/B/B/A shape
#   prints-*    track-print relief levels on the mud pack, forward then reverse
#   forest-*    trees against cones, forward then reverse
#   cal-end     the calibration pair again, for drift across the night
#
# Expected wall clock (DERIVED: startup ~10 s, ~15 s at 30 tanks, + warmup + measure + cooldown):
# vis-off 8 x 95 s = 13 min; cal 2 x 2 x 60 s = 4 min; duel-2 31 x 90 s = 47 min; battle-30
# 19 x 95 s = 30 min; prints 6 x 135 s = 14 min; forest 8 x 100 s = 13 min. About 2 h 05 min.
# =======================================================================
#
# Usage: run-night-battery.sh <out-dir>
# Knobs (env): BIN (required), ASSET_ROOT (default the repo), DURATION_S (60; the calibration pair
#   keeps the daytime 30).

set -u
OUT="${1:?usage: run-night-battery.sh <out-dir>}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
HERE="$(cd "$(dirname "$0")" && pwd)"
REPO="$(cd "$HERE/../.." && pwd)"
: "${BIN:?set BIN to the directory holding the overmatch binary under test}"
export BIN
unset SMOKE OFFSCREEN SPIKE_SIM_WINDOWED SPIKE_SIM_VISIBLE
DURATION="${DURATION_S:-60}"
mkdir -p "$OUT"
STATUS="$OUT/battery-status.txt"
: >"$STATUS"

part() {
  local name="$1"
  shift
  echo "== $name ($(date '+%H:%M:%S'))" | tee -a "$STATUS"
  if "$@" >"$OUT/$name.out" 2>&1; then
    echo "   $name OK" | tee -a "$STATUS"
  else
    echo "   $name FAILED (see $OUT/$name.out)" | tee -a "$STATUS"
  fi
}

# One condition, visible (`vis-*`) or offscreen, into its own directory under vis-off/.
vis_off_condition() {
  local name="$1" tanks="$2" offscreen=""
  [[ "$name" == off-* ]] && offscreen=1
  OFFSCREEN="$offscreen" DURATION_S="$DURATION" WARMUP_S=15 COOLDOWN_S=10 \
    "$HERE/run-ablation.sh" "$OUT/vis-off/$name" "$name||OVERMATCH_PROBE_TANKS=$tanks"
}

vis_off_analysis() {
  local streams=("$OUT"/vis-off/*/*.client.jsonl)
  (( ${#streams[@]} > 0 )) || return 1
  ( cd "$REPO" && uv run scripts/perf/analyze.py --warmup-s 15 --expected-duration-s "$DURATION" \
      --min-rows $((DURATION * 20)) --baseline vis-2-a "${streams[@]}" )
}

# The daytime calibration, exactly as run on 2026-10-29: 15 s warmup, 30 s measured.
calibration() {
  OFFSCREEN=1 DURATION_S=30 WARMUP_S=15 COOLDOWN_S=5 "$HERE/run-ablation.sh" "$1" \
    'cal-2||SPIKE_PHASE_COUNTS=1' \
    'cal-30||OVERMATCH_PROBE_TANKS=30 SPIKE_PHASE_COUNTS=1'
}

# A/B/B/A blocks sharing their A: base, X, X, base, Y, Y, base, ...
abba() {
  local out="$1" tanks="$2"
  shift 2
  local env="OVERMATCH_PROBE_TANKS=$tanks SPIKE_PHASE_COUNTS=1"
  local conds=("base-0||$env") k=0 spec name fields extra
  for spec in "$@"; do
    name="${spec%%|*}"
    fields="${${spec#*|}%%|*}"
    extra="${spec##*|}"
    [[ "$spec" == *"|"*"|"* ]] || extra=""
    (( k += 1 ))
    conds+=("$name-a|$fields|$env $extra" "$name-b|$fields|$env $extra" "base-$k||$env")
  done
  OFFSCREEN=1 DURATION_S="$DURATION" WARMUP_S=15 COOLDOWN_S=5 "$HERE/run-ablation.sh" "$out" "${conds[@]}"
}

for cond in vis-2-a:2 off-2-a:2 off-2-b:2 vis-2-b:2 vis-30-a:30 off-30-a:30 off-30-b:30 vis-30-b:30; do
  part "vis-off-${cond%%:*}" vis_off_condition "${cond%%:*}" "${cond##*:}"
done
part vis-off-analysis vis_off_analysis

part cal-start calibration "$OUT/cal-start"

part duel-2 abba "$OUT/duel-2" 2 \
  'rs75|render_scale: Percent75,' \
  'rs50|render_scale: Percent50,' \
  'msaa-off|msaa: Off,' \
  'shadows-off|shadow_distance: Off,' \
  'cascades-2|shadow_cascades: Two,' \
  'res-2048|shadow_resolution: X2048,' \
  'no-bloom||SPIKE_ABLATE=bloom' \
  'no-env-light||SPIKE_ABLATE=env_light' \
  'no-prepass||SPIKE_ABLATE=depth_prepass' \
  'no-ui||SPIKE_ABLATE=ui'

part battle-30 abba "$OUT/battle-30" 30 \
  'shadows-off|shadow_distance: Off,' \
  'cascades-2|shadow_cascades: Two,' \
  'res-2048|shadow_resolution: X2048,' \
  'msaa-off|msaa: Off,' \
  'rs50|render_scale: Percent50,' \
  'no-env-light||SPIKE_ABLATE=env_light'

part prints-fwd env OFFSCREEN=1 DURATION_S="$DURATION" "$HERE/run-track-prints-cost.sh" \
  "$OUT/prints-fwd" mud-lighting mud-parallax mud-sun
part prints-rev env OFFSCREEN=1 DURATION_S="$DURATION" "$HERE/run-track-prints-cost.sh" \
  "$OUT/prints-rev" mud-sun mud-parallax mud-lighting

part forest-fwd env OFFSCREEN=1 DURATION_S="$DURATION" "$HERE/run-forest-cost.sh" \
  "$OUT/forest-fwd" forest-cones forest-trees duel-cones duel-trees
part forest-rev env OFFSCREEN=1 DURATION_S="$DURATION" "$HERE/run-forest-cost.sh" \
  "$OUT/forest-rev" duel-trees duel-cones forest-trees forest-cones

part cal-end calibration "$OUT/cal-end"

echo "battery complete -> $OUT" | tee -a "$STATUS"
