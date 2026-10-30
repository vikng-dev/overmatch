#!/bin/zsh
# Shadow attribution and stock-control battery for #169; OFFSCREEN=1 throughout.
# Usage: BIN=<directory holding overmatch> scripts/perf/run-shadow-pricing.sh <out-dir>
# Knobs: BIN (required), ASSET_ROOT, DURATION_S (ASSUMED default 60 s), WARMUP_S
# (ASSUMED default 15 s), COOLDOWN_S (ASSUMED default 5 s); run-ablation.sh owns the gates.
# Each lever has its own A/B/B/A block at the requested 2 and 30 tank populations.
# ASSUMED probe splits/overlap below preserve the DERIVED shipped 350 m envelope;
# they are experiments, not recommended product settings. No bundled filter/split/overlap edits.
# Bevy exposes only all-view indirect draws cheaply; census per-view draws counts sets.
set -u
OUT="${1:?usage: run-shadow-pricing.sh <out-dir>}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
HERE="$(cd "$(dirname "$0")" && pwd)"
: "${BIN:?set BIN to the directory holding the overmatch binary under test}"
export BIN
export OFFSCREEN=1
unset SMOKE SPIKE_SIM_WINDOWED SPIKE_SIM_VISIBLE SPIKE_ABLATE SPIKE_SHADOW_FILTER \
  SPIKE_SHADOW_SPLITS SPIKE_SHADOW_OVERLAP OVERMATCH_SHADOW_DISTANCE_M
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
  local tanks="$1" name="$2" fields="$3" extra="$4" base_fields="${5:-}"
  local common="OVERMATCH_PROBE_TANKS=$tanks SPIKE_PHASE_COUNTS=1"
  DURATION_S="$DURATION" WARMUP_S="${WARMUP_S:-15}" COOLDOWN_S="${COOLDOWN_S:-5}" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock \
    "$HERE/run-ablation.sh" "$OUT/$tanks-$name" \
    "base-a|$base_fields|$common" "$name-a|$fields|$common $extra" \
    "$name-b|$fields|$common $extra" "base-b|$base_fields|$common"
}

for tanks in 2 30; do
  for class in terrain tree house tank; do
    part "$tanks-$class-casters" abba "$tanks" "$class-casters" "" "SPIKE_ABLATE=$class-casters"
  done
  for filter in gaussian hardware temporal; do
    part "$tanks-filter-$filter" abba "$tanks" "filter-$filter" "" "SPIKE_SHADOW_FILTER=$filter"
  done
  part "$tanks-cascades-2" abba "$tanks" cascades-2 'shadow_cascades: Two,' ""
  part "$tanks-splits-2" abba "$tanks" splits-2 'shadow_cascades: Two,' 'SPIKE_SHADOW_SPLITS=120,350' 'shadow_cascades: Two,'
  part "$tanks-splits-3" abba "$tanks" splits-3 "" 'SPIKE_SHADOW_SPLITS=60,180,350'
  part "$tanks-overlap" abba "$tanks" overlap "" 'SPIKE_SHADOW_OVERLAP=0.05'
done

echo "shadow battery complete -> $OUT" | tee -a "$STATUS"
exit "$failed"
