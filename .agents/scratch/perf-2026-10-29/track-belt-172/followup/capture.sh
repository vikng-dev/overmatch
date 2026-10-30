#!/usr/bin/env bash
# Hidden stills for the #172 follow-up: the #172 lane's scenes (../capture.sh), any subset.
# Usage: capture.sh <binary> <out-dir> [scene...]   scenes: near mid far xray (default: all four)
# Every run is one hidden, muted, offline game process under the machine's build lock; it exits
# after its last screenshot. Scene inputs are capture configuration (ASSUMED), not tank logic.
set -euo pipefail
bin=$1
out=$2
shift 2
scenes=("$@")
[[ ${#scenes[@]} -gt 0 ]] || scenes=(near mid far xray)
mkdir -p "$out"
common=(OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_MUTE=1
  OVERMATCH_CAPTURE_SIZE=1280x720 OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0
  OVERMATCH_CAPTURE_STEP_S=0.0078125
  OVERMATCH_MAP=track-fit-fixture OVERMATCH_TRACK_PRINTS_DEMO=0,-84,0 OVERMATCH_CAPTURE_AT=12.5,14)
for scene in "${scenes[@]}"; do
  case $scene in
    near) extra=(OVERMATCH_CAPTURE_FOLLOW=8,1.6,2) ;;
    mid) extra=(OVERMATCH_CAPTURE_FOLLOW=35,5,10) ;;
    far) extra=(OVERMATCH_CAPTURE_FOLLOW=85,8,20) ;;
    xray) extra=(OVERMATCH_CAPTURE_FOLLOW=8,1.6,2 OVERMATCH_XRAY=11-20) ;;
    *) echo "unknown scene $scene" >&2; exit 2 ;;
  esac
  mkdir -p "$out/$scene"
  env "${common[@]}" "${extra[@]}" OVERMATCH_CAPTURE="$out/$scene/belts.png" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/$scene.log" 2>&1
done
