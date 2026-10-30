#!/bin/zsh
# battery-block.sh <out-dir> <block> — one offscreen block of scripts/perf/run-night-battery.sh,
# spelled as that script spells it (cal, duel-2, battle-30, forest-fwd, forest-rev), so each block
# is one lock hold. Skipped: vis-off (visible windows) and prints-* (the track-print demo needs the
# `track_prints_demo` feature these binaries lack). BIN: bin/A4 (alpha.4 profiling client, trace
# feature compiled in, spans filtered off at their callsites, chrome writer at /dev/null).
set -u
here="${0:A:h}"
R=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/perf
export BIN="${BIN:-$here/bin/A4}" ASSET_ROOT="${ASSET_ROOT:-$here/bin/A4/root}"
export RUST_LOG="bevy_ecs=warn,bevy_app=warn,bevy_render=warn" TRACE_CHROME=/dev/null OVERMATCH_MUTE=1
unset SMOKE SPIKE_SIM_WINDOWED SPIKE_SIM_VISIBLE
DURATION=60
OUT="$1"; block="$2"
mkdir -p "$OUT"

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
  OFFSCREEN=1 DURATION_S="$DURATION" WARMUP_S=15 COOLDOWN_S=5 "$R/run-ablation.sh" "$out" "${conds[@]}"
}

case "$block" in
  cal-*)
    OFFSCREEN=1 DURATION_S=30 WARMUP_S=15 COOLDOWN_S=5 "$R/run-ablation.sh" "$OUT/$block" \
      'cal-2||SPIKE_PHASE_COUNTS=1' 'cal-30||OVERMATCH_PROBE_TANKS=30 SPIKE_PHASE_COUNTS=1' ;;
  duel-2)
    abba "$OUT/duel-2" 2 \
      'rs75|render_scale: Percent75,' 'rs50|render_scale: Percent50,' 'msaa-off|msaa: Off,' \
      'shadows-off|shadow_distance: Off,' 'cascades-2|shadow_cascades: Two,' \
      'res-2048|shadow_resolution: X2048,' 'no-bloom||SPIKE_ABLATE=bloom' \
      'no-env-light||SPIKE_ABLATE=env_light' 'no-prepass||SPIKE_ABLATE=depth_prepass' \
      'no-ui||SPIKE_ABLATE=ui' ;;
  battle-30)
    abba "$OUT/battle-30" 30 \
      'shadows-off|shadow_distance: Off,' 'cascades-2|shadow_cascades: Two,' \
      'res-2048|shadow_resolution: X2048,' 'msaa-off|msaa: Off,' 'rs50|render_scale: Percent50,' \
      'no-env-light||SPIKE_ABLATE=env_light' ;;
  forest-fwd)
    OFFSCREEN=1 DURATION_S="$DURATION" "$R/run-forest-cost.sh" "$OUT/forest-fwd" \
      forest-cones forest-trees duel-cones duel-trees ;;
  forest-rev)
    OFFSCREEN=1 DURATION_S="$DURATION" "$R/run-forest-cost.sh" "$OUT/forest-rev" \
      duel-trees duel-cones forest-trees forest-cones ;;
  *) echo "unknown block $block" >&2; exit 2 ;;
esac
