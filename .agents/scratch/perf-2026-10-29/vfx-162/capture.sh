#!/bin/zsh
# Hidden, fixed-step frames of the scripted fire sequence (`SPIKE_AUTO_FIRE=1`, the trigger
# `scripts/perf/run-fire-capture.sh` drives: MG held 20-50 s, main gun 60-75 s) for a before/after
# pixel comparison. No frame-time recording: the window is hidden and the clock is virtual.
#
# Usage: capture.sh <client binary> <out dir>
# ASSET_ROOT overrides the asset root (a binary built before a shader change needs that shader);
# CAPTURE_AT_FILE names a file of frame times (comma-separated app seconds) to use instead.
set -eu
setopt null_glob

BIN="${1:?usage: capture.sh <client binary> <out dir>}"
OUT="${2:?usage: capture.sh <client binary> <out dir>}"
REPO="$(cd "$(dirname "$0")/../../../.." && pwd)"

# Frame times (app seconds): every other 60 Hz frame through the first second of each fire phase,
# then a sparser tail through the main gun's lingering smoke and haze.
at=()
for i in {0..30}; do at+=("$(printf '%.4f' $((20.0 + i / 30.0)))"); done
at+=(25 30 40 49.5)
for i in {0..30}; do at+=("$(printf '%.4f' $((60.0 + i / 30.0)))"); done
at+=(61.5 62 63 64 66 68 70 72 74)
if [[ -n "${CAPTURE_AT_FILE:-}" ]]; then
  at=(${(s:,:)"$(cat "$CAPTURE_AT_FILE")"})
fi

mkdir -p "$OUT/cfg"
rm -f "$OUT"/*.png "$OUT"/run.log
# The shipped shadow distance, as the fire capture runs it.
printf '(\n    version: 1,\n    shadow_distance: M350,\n    vsync_mode: Off,\n)\n' >"$OUT/cfg/video.ron"

env -u SPIKE_SIM_VISIBLE -u SPIKE_FRAME_COST -u SPIKE_RENDER_COST \
  OVERMATCH_CONFIG_DIR="$OUT/cfg" \
  OVERMATCH_MUTE=1 \
  OVERMATCH_MUZZLE_SHADOWS=main-only \
  SPIKE_AUTO_FIRE=1 \
  SPIKE_SIM_WINDOWED=1 \
  BEVY_ASSET_ROOT="$(cd "${ASSET_ROOT:-$REPO}" && pwd)" \
  OVERMATCH_CAPTURE="$OUT/f.png" \
  OVERMATCH_CAPTURE_AT="${(j:,:)at}" \
  OVERMATCH_CAPTURE_STEP_S=0.016666666666666666 \
  OVERMATCH_CAPTURE_SIZE=1280x720 \
  OVERMATCH_CAPTURE_UI=0 \
  OVERMATCH_CAPTURE_GIZMOS=0 \
  /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$BIN" --offline >"$OUT/run.log" 2>&1
echo "frames: $(ls "$OUT"/*.png | wc -l | tr -d ' ') -> $OUT"
