#!/usr/bin/env bash
# capture.sh <binary> <png-dir> <arm> [scene ...] — one hidden, muted run per scene, each under the
# machine's build lock and with a fresh config directory. Both arms take the same levers; only the
# binary differs.
set -euo pipefail

bin=$1 out=$2 arm=$3
shift 3
scenes=("$@")
[[ ${#scenes[@]} -gt 0 ]] || scenes=(duel close probes30)

levers() {
  case $1 in
    duel) ;;
    # A three-quarter view of the controlled Tiger, near enough for its nearest rung.
    close) echo "OVERMATCH_CAPTURE_FOLLOW=6,2.5,-6" ;;
    # 30 tanks: the duel plus the probe grid 48..99 m in front of the default camera.
    probes30) echo "OVERMATCH_PROBE_TANKS=30" ;;
    *) echo "unknown scene $1" >&2; exit 2 ;;
  esac
}

mkdir -p "$out/$arm" "$out/logs" "$out/config"
for scene in "${scenes[@]}"; do
  config="$out/config/$arm-$scene"
  rm -rf "$config" && mkdir -p "$config"
  # shellcheck disable=SC2046
  env OVERMATCH_MUTE=1 OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_CONFIG_DIR="$config" OVERMATCH_CAPTURE_SIZE=1920x1080 OVERMATCH_CAPTURE_UI=0 \
    OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125 OVERMATCH_CAPTURE_AT=10 \
    OVERMATCH_CAPTURE="$out/$arm/$scene.png" $(levers "$scene") \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$bin" >"$out/logs/$arm-$scene.log" 2>&1 \
    || echo "$arm $scene: exit $?" >&2
  echo "$arm $scene: $(ls "$out/$arm/$scene.png" 2>/dev/null || echo MISSING)"
done
