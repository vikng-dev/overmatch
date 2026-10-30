#!/usr/bin/env bash
# capture.sh <bin-dir> <png-dir> <arm> [scene ...] — one hidden, muted run per scene, each under the
# machine's build lock and with a fresh config directory. Both arms take the same levers; only the
# binaries differ. Assets resolve from the bay (BEVY_ASSET_ROOT).
set -euo pipefail

bins=$1 out=$2 arm=$3
shift 3
scenes=("$@")
[[ ${#scenes[@]} -gt 0 ]] || scenes=(duel close side mid sandbox_close sandbox_mid)
bay=$(cd "$(dirname "$0")" && git rev-parse --show-toplevel)

levers() {
  case $1 in
    duel) echo "OVERMATCH_CAPTURE_AT=10" ;;
    # Three-quarter view of the controlled tank from 6 m, 2.5 m up.
    close) echo "OVERMATCH_CAPTURE_AT=10 OVERMATCH_CAPTURE_FOLLOW=6,2.5,-6" ;;
    # The mantlet and the glacis from about 3 m: cast beside rolled.
    macro) echo "OVERMATCH_CAPTURE_AT=10 OVERMATCH_CAPTURE_FOLLOW=1.2,2.4,-4.2" ;;
    # Its left flank from 7 m.
    side) echo "OVERMATCH_CAPTURE_AT=10 OVERMATCH_CAPTURE_FOLLOW=-7,2,0.5" ;;
    # Three-quarter view from about 24 m.
    mid) echo "OVERMATCH_CAPTURE_AT=10 OVERMATCH_CAPTURE_FOLLOW=16,6,-16" ;;
    # The armour sandbox's range target (root at 0,2,-12): about 9 m, then about 22 m.
    sandbox_close) echo "OVERMATCH_CAPTURE_AT=8 OVERMATCH_RANGE_POSE=6,4.5,-5,0,3,-12" ;;
    sandbox_mid) echo "OVERMATCH_CAPTURE_AT=8 OVERMATCH_RANGE_POSE=-14,8,4,0,2.5,-12" ;;
    # The same two with the x-ray off, the target in its own surface (a lever only the after arm has).
    sandbox_close_plain) echo "OVERMATCH_CAPTURE_AT=8 OVERMATCH_RANGE_XRAY=0 OVERMATCH_RANGE_POSE=6,4.5,-5,0,3,-12" ;;
    sandbox_mid_plain) echo "OVERMATCH_CAPTURE_AT=8 OVERMATCH_RANGE_XRAY=0 OVERMATCH_RANGE_POSE=-14,8,4,0,2.5,-12" ;;
    *) echo "unknown scene $1" >&2; exit 2 ;;
  esac
}

binary() {
  case $1 in
    sandbox_*) echo "$bins/armor_sandbox" ;;
    *) echo "$bins/overmatch" ;;
  esac
}

mkdir -p "$out/$arm" "$out/logs" "$out/config"
for scene in "${scenes[@]}"; do
  config="$out/config/$arm-$scene"
  rm -rf "$config" && mkdir -p "$config"
  # shellcheck disable=SC2046
  env OVERMATCH_MUTE=1 OVERMATCH_OFFLINE=1 SPIKE_SIM_WINDOWED=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    BEVY_ASSET_ROOT="$bay" OVERMATCH_CONFIG_DIR="$config" OVERMATCH_CAPTURE_SIZE=1920x1080 \
    OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 OVERMATCH_CAPTURE_STEP_S=0.0078125 \
    OVERMATCH_CAPTURE="$out/$arm/$scene.png" $(levers "$scene") \
    /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock "$(binary "$scene")" \
    >"$out/logs/$arm-$scene.log" 2>&1 || echo "$arm $scene: exit $?" >&2
  echo "$arm $scene: $(ls "$out/$arm/$scene.png" 2>/dev/null || echo MISSING)"
done
