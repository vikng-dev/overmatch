#!/usr/bin/env bash
# capture.sh <binary> <png-dir> <before|after|main> [scene ...] — the README's captures, one
# hidden, muted run per scene, each under the machine's build lock. `before` is the raster terrain
# shadow (SPIKE_ABLATE=terrain-field), `after` the baked field, `main` a binary built from the
# branch's base, run from a checkout of it (no lever). Every run gets a fresh config directory.
set -euo pipefail

bin=$1 out=$2 arm=$3
shift 3
scenes=("$@")
[[ ${#scenes[@]} -gt 0 ]] || scenes=(duel hulldown-side-8deg hulldown-high-8deg ridgeline-8deg
  ridgeline-17deg treeline-17deg treeline-8deg)

tank="OVERMATCH_START=55.7,385.3,45"
levers() {
  case $1 in
    duel) ;;
    hulldown-side-8deg) echo "OVERMATCH_SUN_ELEVATION=8 $tank OVERMATCH_CAPTURE_POSE=70.7,39.7,370.3,55.7,36.5,385.3" ;;
    hulldown-high-8deg) echo "OVERMATCH_SUN_ELEVATION=8 $tank OVERMATCH_CAPTURE_POSE=40,44,372,55.7,36,385.3" ;;
    ridgeline-8deg) echo "OVERMATCH_SUN_ELEVATION=8 $tank OVERMATCH_CAPTURE_POSE=-30,50,330,60,36,395" ;;
    ridgeline-17deg) echo "$tank OVERMATCH_CAPTURE_POSE=-30,50,330,60,36,395" ;;
    treeline-17deg) echo "OVERMATCH_CAPTURE_POSE=540,19,556,548,16,640" ;;
    treeline-8deg) echo "OVERMATCH_SUN_ELEVATION=8 OVERMATCH_CAPTURE_POSE=540,19,556,548,16,640" ;;
    # The track-print demo on mud (src/track_prints/demo.rs): the drive and the pivot at each
    # relief level; the third and sixth frames are the sun-shadow level.
    prints-mud) echo "OVERMATCH_TRACK_PRINTS_DEMO=599.9,-599.9,195 OVERMATCH_CAPTURE_AT=38.5,40.5,42.5,44.5,46.5,48.5" ;;
    *) echo "unknown scene $1" >&2; exit 2 ;;
  esac
}

ablate=()
[[ $arm == before ]] && ablate=(SPIKE_ABLATE=terrain-field)
mkdir -p "$out/$arm" "$out/logs" "$out/config"
for scene in "${scenes[@]}"; do
  config="$out/config/$arm-$scene"
  rm -rf "$config" && mkdir -p "$config"
  # shellcheck disable=SC2046
  env OVERMATCH_MUTE=1 OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_CONFIG_DIR="$config" OVERMATCH_CAPTURE_SIZE=1920x1080 OVERMATCH_CAPTURE_UI=0 \
    OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_AT=10 \
    OVERMATCH_CAPTURE="$out/$arm/$scene.png" "${ablate[@]}" $(levers "$scene") \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/logs/$arm-$scene.log" 2>&1 \
    || echo "$arm $scene: exit $?" >&2
  echo "$arm $scene: $(ls "$out/$arm/$scene.png" 2>/dev/null || echo MISSING)"
done
