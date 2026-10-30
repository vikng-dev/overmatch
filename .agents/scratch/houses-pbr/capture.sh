#!/usr/bin/env bash
# capture.sh <binary> <png-dir> <before|after> [scene] — from the checkout root, one hidden, muted
# offline run per house scene, each under the machine's build lock, each with a fresh config
# directory. Poses are a camera ~2.6 m over the ground near a house, ~3 m at mid range, and two
# elevated views; their heights come from the shipped heightmap.
set -euo pipefail

bin=$1 out=$2 arm=$3

scenes=(
  "near-a -445.7,30.8,172.8,-433.4,28.9,177.3"
  "near-b -405.8,29.5,169.1,-414.2,28.3,176.2"
  "mid-a -468.8,32.9,135.1,-433.4,28.9,177.3"
  "mid-b -543.5,38.1,248.6,-521.0,33.8,209.6"
  "wide -346.7,71.4,89.7,-514.1,29.0,60.2"
  "roofs -421.8,43.1,155.5,-414.2,27.1,176.2"
)
only=${4:-}

mkdir -p "$out/$arm" "$out/logs" "$out/config"
for scene in "${scenes[@]}"; do
  name=${scene%% *}
  pose=${scene#* }
  [[ -z $only || $only == "$name" ]] || continue
  config="$out/config/$arm-$name"
  rm -rf "$config" && mkdir -p "$config"
  env BEVY_ASSET_ROOT="$PWD" \
    OVERMATCH_MUTE=1 OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_CONFIG_DIR="$config" OVERMATCH_CAPTURE_SIZE=1920x1080 OVERMATCH_CAPTURE_UI=0 \
    OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_AT=12 \
    OVERMATCH_CAPTURE_POSE="$pose" OVERMATCH_CAPTURE="$out/$arm/$name.png" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/logs/$arm-$name.log" 2>&1 \
    || echo "$arm $name: exit $?" >&2
  echo "$arm $name: $(ls "$out/$arm/$name.png" 2>/dev/null || echo MISSING)"
done
