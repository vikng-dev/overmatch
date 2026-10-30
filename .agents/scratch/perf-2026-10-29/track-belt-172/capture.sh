#!/usr/bin/env bash
# Hidden before/after stills for issue #172. Usage: capture.sh <binary> <out-dir>
# Every run is one hidden, offline game process under the machine's build lock; it exits after
# its last screenshot. Scene inputs are capture configuration (ASSUMED), not tank logic.
set -euo pipefail
bin=$1
out=$2
mkdir -p "$out"
common=(OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_MUTE=1
  OVERMATCH_CAPTURE_SIZE=1280x720 OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0
  OVERMATCH_CAPTURE_STEP_S=0.0078125)
run() {
  local name=$1
  shift
  mkdir -p "$out/$name"
  env "${common[@]}" "$@" OVERMATCH_CAPTURE="$out/$name/belts.png" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/$name.log" 2>&1
}
washboard=(OVERMATCH_MAP=track-fit-fixture OVERMATCH_TRACK_PRINTS_DEMO=0,-84,0 OVERMATCH_CAPTURE_AT=12.5,14)
run near "${washboard[@]}" OVERMATCH_CAPTURE_FOLLOW=8,1.6,2
run mid "${washboard[@]}" OVERMATCH_CAPTURE_FOLLOW=35,5,10
run far "${washboard[@]}" OVERMATCH_CAPTURE_FOLLOW=85,8,20
run xray "${washboard[@]}" OVERMATCH_CAPTURE_FOLLOW=8,1.6,2 OVERMATCH_XRAY=11-20
run printed OVERMATCH_MAP=kalinovo OVERMATCH_TRACK_PRINTS_DEMO=70,-20,90 \
  OVERMATCH_CAPTURE_AT=12.5,16 OVERMATCH_CAPTURE_FOLLOW=2,1.6,8
