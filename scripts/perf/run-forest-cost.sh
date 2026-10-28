#!/bin/zsh
# =============================== RUNBOOK ===============================
# Issue 123, the forest's rendered frame cost: the shipped map's 635 segmented trees (each fir id
# resolving to `Tree(asset: "fir_1")`) against the graybox cones they replaced (`Fir(..)`: a trunk
# cylinder and a canopy cone), same binary, same scene, same settings. The whole frame is
# measured, so a tree condition also carries its sim's share: a convex collider per segment
# against a cylinder per fir (the tick alone is measured by `forest_tick_cost_bed`,
# src/headless_test.rs). Offline single-player path, no server.
#
# The cones: an asset root built per run under <out-dir>/root-cones, every entry a symlink into
# ASSET_ROOT except the map's prototypes.ron, rewritten so the three fir ids name the graybox rows
# that file records (fir_tree_01_a/b/c: Fir(18.96, 6.5), Fir(14.08, 5.8), Fir(14.55, 6.2)), the
# substitution the tick-cost bed makes. The cone code path is the one the map drew up to d16a458
# (the last commit whose map resolved its firs to cones); 88c346f switched the map to trees and
# kept the graybox prototype for this comparison.
#
# Scenes:
#   forest  OVERMATCH_START=540,570,0: the player's tank in the map's north belt; the camera
#           spawns behind it looking +Z, pitched 27° down, over ~48 trunks within 150 m (DERIVED
#           from level.json with a 72° horizontal field) — the forest filling the frame.
#   duel    the default offline duel; the camera looks +Z across the valley and no fir stands in
#           frame — the forest's cost when nobody looks at it (its entities, culling, its share
#           of the sun's cascades).
# Conditions: <scene>-<forest>, forest cones or trees. Default order: forest-cones forest-trees
# duel-cones duel-trees. Deltas print per scene against its cones condition.
#
# Validity: every precondition and gate of the frame sweep (run-frame-sweep.sh's runbook, code in
# frame-run.zsh), plus this runner's own, per condition: the scatter placed the forest the
# condition names (cones: graybox firs and no trees; trees: trees and no graybox firs), no asset
# failed to load, and the forest scene's start lever took. The game is muted (OVERMATCH_MUTE=1)
# and does not pause on a focus loss (OVERMATCH_NO_FOCUS_PAUSE=1) in every condition.
#
# Expected wall-clock: 4 conditions x (startup + 15 s warmup + 60 s measure + 15 s cooldown)
# ~ 7-8 min. SMOKE (hidden, numbers fiction): two conditions, ~1.5 min:
#   SMOKE=1 scripts/perf/run-forest-cost.sh /tmp/forest-smoke
# =======================================================================
#
# Usage: run-forest-cost.sh <out-dir> [condition ...]
# Knobs (env): BIN (directory holding `overmatch`, absolute or relative to the repo; default
#   target/release), ASSET_ROOT (directory holding `assets/`, the trees; default the repo),
#   DURATION_S (60), WARMUP_S (15), COOLDOWN_S (15), MIN_ROWS, SMOKE, STARTUP_GRACE_S (60).

set -eu
setopt null_glob

OUT="${1:?usage: run-forest-cost.sh <out-dir> [condition ...]}"
shift
# Absolute, because the cones condition passes `$OUT/root-cones` as BEVY_ASSET_ROOT and the client
# resolves a relative root against itself a second time.
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
ASSET_ROOT="$(cd "$ASSET_ROOT" && pwd)"
SMOKE="${SMOKE:-}"
WARMUP_S="${WARMUP_S:-15}"
if [[ -n "$SMOKE" ]]; then
  DURATION_S="${DURATION_S:-10}"
  COOLDOWN_S="${COOLDOWN_S:-2}"
else
  DURATION_S="${DURATION_S:-60}"
  COOLDOWN_S="${COOLDOWN_S:-15}"
fi
MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"
# The shipped map, its prototype library, and the forest scene's start spot (fixture data).
MAP=kalinovo
FOREST_START="540,570,0"

CONDITIONS=("$@")
if (( ${#CONDITIONS[@]} == 0 )); then
  CONDITIONS=(forest-cones forest-trees duel-cones duel-trees)
  [[ -n "$SMOKE" ]] && CONDITIONS=(forest-cones forest-trees)
fi
for cond in "${CONDITIONS[@]}"; do
  case "$cond" in
    forest-cones|forest-trees|duel-cones|duel-trees) ;;
    *) echo "unknown condition $cond" >&2; exit 1 ;;
  esac
done

source "$REPO/scripts/perf/frame-run.zsh"
frame_run_begin

# The cones' asset root: symlinks into ASSET_ROOT, the map's prototype library rewritten.
CONES="$OUT/root-cones"
rm -rf "$CONES"
mkdir -p "$CONES/assets/maps/$MAP"
for entry in "$ASSET_ROOT"/assets/*; do
  [[ "${entry:t}" == maps ]] || ln -s "$entry" "$CONES/assets/${entry:t}"
done
for entry in "$ASSET_ROOT"/assets/maps/*; do
  [[ "${entry:t}" == "$MAP" ]] || ln -s "$entry" "$CONES/assets/maps/${entry:t}"
done
for entry in "$ASSET_ROOT/assets/maps/$MAP"/*; do
  [[ "${entry:t}" == prototypes.ron ]] || ln -s "$entry" "$CONES/assets/maps/$MAP/${entry:t}"
done
sed -e 's/^\( *"fir_tree_01_a_LOD2"\): Tree(asset: "fir_1"),$/\1: Fir(height_m: 18.96, canopy_diameter_m: 6.5),/' \
    -e 's/^\( *"fir_tree_01_b_LOD2"\): Tree(asset: "fir_1"),$/\1: Fir(height_m: 14.08, canopy_diameter_m: 5.8),/' \
    -e 's/^\( *"fir_tree_01_c_LOD2"\): Tree(asset: "fir_1"),$/\1: Fir(height_m: 14.55, canopy_diameter_m: 6.2),/' \
    "$ASSET_ROOT/assets/maps/$MAP/prototypes.ron" >"$CONES/assets/maps/$MAP/prototypes.ron"
# The rewrite must have left no tree and put the three firs back; a changed library fails here.
# The file's comments name both shapes, so only its entries are read.
rows="$(grep -v '^ *//' "$CONES/assets/maps/$MAP/prototypes.ron")"
fir_rows="$(print -r -- "$rows" | grep -c ': Fir(' || true)"
if [[ "$rows" == *'Tree('* ]] || (( fir_rows != 3 )); then
  echo "the cones' prototype rewrite no longer matches $ASSET_ROOT/assets/maps/$MAP/prototypes.ron" >&2
  exit 1
fi
{
  echo "trees asset root: $ASSET_ROOT"
  echo "cones asset root: $CONES (prototypes.ron rewritten; everything else a symlink into the trees root)"
  echo "forest scene: OVERMATCH_START=$FOREST_START; duel scene: the default duel"
} >>"$OUT/manifest.txt"

condition_gates() {
  local cond="$1" log="$2"
  local scatter
  scatter="$(grep -o 'scatter: [0-9]* instances placed.*' "$log" | head -1)"
  [[ -n "$scatter" ]] || frame_run_invalid "$cond" "the scatter never placed the map (see $log)"
  # `…, <n> graybox firs (trunk colliders only), <m> trees`
  local firs="${${scatter%% graybox firs*}##* }" trees="${${scatter%% trees*}##* }"
  case "$cond" in
    *-cones) (( firs > 0 && trees == 0 )) \
      || frame_run_invalid "$cond" "the cones' map did not place cones alone: $scatter" ;;
    *-trees) (( trees > 0 && firs == 0 )) \
      || frame_run_invalid "$cond" "the trees' map did not place trees alone: $scatter" ;;
  esac
  if grep -qiE 'failed to load|path not found|could not load' "$log"; then
    frame_run_invalid "$cond" "an asset failed to load — the forest drawn may not be the one named (see $log)"
  fi
  if [[ "$cond" == forest-* ]]; then
    grep -q "OVERMATCH_START: the player's tank spawns at" "$log" \
      || frame_run_invalid "$cond" "the forest scene's start spot never took (see $log)"
  fi
  echo "$cond: $scatter" >>"$OUT/manifest.txt"
  echo "$cond: focus losses with the pause suppressed: $(grep -c 'pause suppressed' "$log" || true)" >>"$OUT/manifest.txt"
}

for cond in "${CONDITIONS[@]}"; do
  root="$ASSET_ROOT"
  [[ "$cond" == *-cones ]] && root="$CONES"
  start=()
  [[ "$cond" == forest-* ]] && start=(OVERMATCH_START="$FOREST_START")
  frame_run_condition "$cond" "" \
    OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 BEVY_ASSET_ROOT="$root" "${start[@]}"
done

for scene in forest duel; do
  streams=("$OUT/$scene"-*.client.jsonl)
  (( ${#streams[@]} > 0 )) || continue
  baseline="$scene-cones"
  [[ -f "$OUT/$baseline.client.jsonl" ]] || baseline="$(basename "${streams[1]}" .client.jsonl)"
  echo "\n#### $scene"
  frame_run_analyze "$baseline" "${streams[@]}"
done

echo "forest run complete -> $OUT"
