#!/bin/zsh
# Terrain shader pricing for #160; OFFSCREEN=1 throughout.
# Usage: BIN=<directory holding overmatch> scripts/perf/run-terrain-pricing.sh <out-dir>
# Knobs: BIN (required), ASSET_ROOT, DURATION_S (ASSUMED default 60 s), WARMUP_S
# (ASSUMED default 15 s), COOLDOWN_S (ASSUMED default 5 s); run-ablation.sh owns the gates.
# Each lever (SPIKE_ABLATE=terrain_macro, terrain_layers; src/render_cost.rs) has its own
# A/B/B/A block at 2 and 30 tanks: base-a, lever-a, lever-b, base-b. The levers are instruments,
# not product settings: the picture goes flatter. The run-ablation gate requires the client's
# 'render_cost: ablating' line; the shader-def arm logs 'terrain_blend: pipeline specialized'.
set -u
OUT="${1:?usage: run-terrain-pricing.sh <out-dir>}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${BIN:?set BIN to the directory holding the overmatch binary under test}"
export BIN
export OFFSCREEN=1
export OVERMATCH_MUTE=1
unset SMOKE SPIKE_SIM_WINDOWED SPIKE_SIM_VISIBLE SPIKE_ABLATE
DURATION="${DURATION_S:-60}"
mkdir -p "$OUT"
STATUS="$OUT/battery-status.txt"
: >"$STATUS"
failed=0

part() {
  local name="$1"
  shift
  echo "== $name ($(date '+%H:%M:%S'))" | tee -a "$STATUS"
  if "$@" >"$OUT/$name.out" 2>&1; then
    echo "   $name OK" | tee -a "$STATUS"
  else
    echo "   $name FAILED (see $OUT/$name.out)" | tee -a "$STATUS"
    failed=1
  fi
}

abba() {
  local tanks="$1" lever="$2"
  local common="OVERMATCH_PROBE_TANKS=$tanks"
  DURATION_S="$DURATION" WARMUP_S="${WARMUP_S:-15}" COOLDOWN_S="${COOLDOWN_S:-5}" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock \
    "$HERE/run-ablation.sh" "$OUT/$tanks-$lever" \
    "base-a||$common" "$lever-a||$common SPIKE_ABLATE=$lever" \
    "$lever-b||$common SPIKE_ABLATE=$lever" "base-b||$common"
}

for tanks in 2 30; do
  for lever in terrain_macro terrain_layers; do
    part "$tanks-$lever" abba "$tanks" "$lever"
  done
done

echo "terrain pricing complete -> $OUT" | tee -a "$STATUS"
exit "$failed"
