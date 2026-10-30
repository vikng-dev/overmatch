#!/usr/bin/env bash
# capture.sh <binary> <asset root> <png dir> <before|after> [scene] — one hidden, muted offline run per
# scene, each under the machine's build lock, each with a fresh config directory.
#
# The house scenes reuse `houses-pbr/capture.sh`'s poses (a camera 2.6 m over the ground near a
# house, rows at 45–55 m, the western village from 170 m), frame at app time 12 s on a fixed 1/128 s
# step. `window` looks through house_0027's east and west windows (house_1's spec: windows at
# z 1.0–2.0 m and 0.5–1.5 m, 0.9–2.025 m up). `tracers-*` spawn the player's Tiger 25 m east of
# house_0027 facing its east wall (`OVERMATCH_START`), aim it at the frame's centre on the wall's
# first pier, and fire on `SPIKE_AUTO_FIRE`'s real-time schedule (MG 20–50 s, main gun 60–75 s);
# those frames are timed by the app clock, which tracks real time, so they are not reproducible to
# the frame. Poses are world metres from `poses.py` (house frames, terrain heights).
set -euo pipefail

bin=$1 root=$2 out=$3 arm=$4

scenes=(
  "near-a 12 -445.7,30.8,172.8,-433.4,28.9,177.3"
  "near-b 12 -405.8,29.5,169.1,-414.2,28.3,176.2"
  "mid-a 12 -468.8,32.9,135.1,-433.4,28.9,177.3"
  "mid-b 12 -543.5,38.1,248.6,-521.0,33.8,209.6"
  "wide 12 -346.7,71.4,89.7,-514.1,29.0,60.2"
  "roofs 12 -421.8,43.1,155.5,-414.2,27.1,176.2"
  "window 12 -410.85,28.65,181.64,-416.58,28.55,175.55"
  "tracers-behind 30,30.25,30.5,59.1,59.25,59.5,59.75,60.25 -388.73,31.26,201.01,-414.70,28.34,179.53"
  "tracers-side 30,30.1,30.2,59.3,59.4,59.5,59.6,59.7,59.8,59.9,60,60.1,60.2,60.4 -412.35,29.17,187.36,-414.70,28.34,179.53"
)
only=${5:-}

mkdir -p "$out/$arm" "$out/logs" "$out/config"
for scene in "${scenes[@]}"; do
  read -r name at pose <<<"$scene"
  [[ -z $only || $name == $only ]] || continue
  config="$out/config/$arm-$name"
  rm -rf "$config" && mkdir -p "$config"
  extra=(OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_SIZE=1920x1080)
  if [[ $name == tracers-* ]]; then
    # The HUD and the pen cam's corner stay up behind the tank; the side view hides them. The
    # third-person aim picks along the WINDOW's centre ray (`aim::commit_aim`), so the frame is
    # captured at the hidden window's own logical size (1280 x 720 at the capture's scale 2) for
    # the gun to lay on the frame's centre. The main gun fires at real 60 s, about app 59 s.
    ui=$([[ $name == tracers-behind ]] && echo 1 || echo 0)
    extra=(OVERMATCH_START=-396.32,195.23,229.5 SPIKE_AUTO_FIRE=1 OVERMATCH_CAPTURE_UI=$ui
      OVERMATCH_CAPTURE_SIZE=2560x1440)
  fi
  env BEVY_ASSET_ROOT="$root" \
    OVERMATCH_MUTE=1 OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_CONFIG_DIR="$config" OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_AT="$at" "${extra[@]}" \
    OVERMATCH_CAPTURE_POSE="$pose" OVERMATCH_CAPTURE="$out/$arm/$name.png" \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/logs/$arm-$name.log" 2>&1 \
    || echo "$arm $name: exit $?" >&2
  echo "$arm $name: $(ls "$out/$arm/$name"*.png 2>/dev/null | wc -l | tr -d ' ') frame(s)"
done
