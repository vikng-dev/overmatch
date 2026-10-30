#!/bin/zsh
# =============================== RUNBOOK ===============================
# Which part of the pen cam corner's frame cost is which: the corner's target pixels, its mask
# cameras, or its bloom (#164). Every lever is a `SPIKE_ABLATE` instrument (src/render_cost.rs),
# never a product setting, and each is priced against the corner's shipped state in the same
# session by `run-pencam-cost.sh`'s corner-on condition (frames with the corner showing against
# frames without, same clock):
#   shipped       no lever: the corner as it ships (the A of every pair)
#   inset_half    the corner's target at half size; its mask views follow, keyed by size
#   inset_masks   no mask cameras for the corner (the picture loses its mask edges)
#   inset_bloom   no bloom on the corner's camera
#   off           the ceiling: corner-off, run alongside the shipped corner-on (paired windows)
# Order is A B C D D C B A: shipped, half, masks, bloom, bloom, masks, half, shipped. The two
# shipped runs also carry corner-off. Each run gets its own directory under <out-dir>, named
# <position>-<lever>; digest.txt concatenates their summaries, so each lever's two runs read
# against the two shipped runs.
# Every run has run-pencam-cost.sh's validity conditions (release binaries, window visible and
# frontmost on the primary display at 2560x1440, machine quiet, warm shader cache: discard the
# first run after a rebuild) and is muted. A run is ~90 s, so the battery is ~15 min.
#
# Usage: run-pencam-attribution.sh <out-dir>
# Build first: scripts/serial-cargo build --locked --release --features dev_ui,bitprobe --bin overmatch
# (the levers are compiled into every binary; they act only when SPIKE_ABLATE names them).
# =======================================================================

set -eu -o pipefail

OUT="${1:?usage: run-pencam-attribution.sh <out-dir>}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$OUT"
: >"$OUT/digest.txt"

order=(shipped inset_half inset_masks inset_bloom inset_bloom inset_masks inset_half shipped)
position=0
for lever in "${order[@]}"; do
  position=$((position + 1))
  dir="$OUT/$position-$lever"
  echo "=== $position/${#order[@]}: $lever ==="
  if [[ "$lever" == shipped ]]; then
    env -u SPIKE_ABLATE "$REPO/scripts/perf/run-pencam-cost.sh" "$dir" corner-on corner-off
  else
    SPIKE_ABLATE="$lever" "$REPO/scripts/perf/run-pencam-cost.sh" "$dir" corner-on
    # The lever must have acted: its log line is the proof the run measured it.
    grep -q "render_cost: $lever:" "$dir/corner-on.log" \
      || { echo "$lever INVALID: the lever never took effect (no 'render_cost: $lever:' in $dir/corner-on.log)" >&2; exit 1; }
  fi
  { echo "== $position-$lever =="; cat "$dir/summary.txt"; } >>"$OUT/digest.txt"
done

echo "attribution complete -> $OUT (digest.txt; per-run summary.txt and *.cost.json)"
