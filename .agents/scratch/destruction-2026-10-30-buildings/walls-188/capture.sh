#!/bin/zsh
# Hidden, muted two-peer capture of a house wall breaking, #188 (the game's environment
# destruction, game state only): a local headless server fires the wall cue
# (OVERMATCH_CAPTURE_WALL, `dev_capture::wall`) at server second FIRE, and two hidden capture
# clients (a, b) hold the camera the server logs for that wall, framing before and after; a third
# (c) holds the inside pose the server logs, looking at the wall's inner face. Scene inputs (times,
# sizes) are capture configuration (ASSUMED), not game logic.
#
# Usage (under the machine lock):
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock capture.sh <bin-dir> <out-dir> <cue>
#   <cue>: `88mm_AP`, `88mm_HE,0`, `x5` … (the part of OVERMATCH_CAPTURE_WALL before `@`)
set -eu
zmodload zsh/datetime

BIN="$1"; OUT="$2"; CUE="$3"
REPO="${REPO:-$(cd "$(dirname "$0")/../../../.." && pwd)}"
FIRE="${FIRE:-90}"
AT="${AT:-25,120,135}"
SIZE="${SIZE:-1600x900}"
mkdir -p "$OUT/a/config" "$OUT/b/config" "$OUT/c/config"

alive() { kill -0 "$1" 2>/dev/null; }

if lsof -nP -iUDP:5888 >/dev/null 2>&1; then
  echo "UDP 5888 already bound" >&2; exit 1
fi
env BEVY_ASSET_ROOT="$REPO" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_MUTE=1 \
  RUST_LOG="info,overmatch::buildings=debug" \
  OVERMATCH_CAPTURE_WALL="$CUE@$FIRE" \
  "$BIN/overmatch-server" >"$OUT/server.log" 2>&1 &
SPID=$!
POSE=""
for _ in {1..600}; do
  POSE=$(grep -o 'OVERMATCH_CAPTURE_POSE=[-0-9.,e]*' "$OUT/server.log" 2>/dev/null | head -1 | cut -d= -f2 || true)
  if [[ -n "$POSE" ]] && grep -q "listening" "$OUT/server.log"; then break; fi
  alive $SPID || break
  sleep 0.2
done
if [[ -z "$POSE" ]]; then
  echo "no wall pose from the server" >&2; kill $SPID 2>/dev/null || true; exit 1
fi
INSIDE=$(grep -o 'wall inside pose [-0-9.,e]*' "$OUT/server.log" | head -1 | cut -d' ' -f4)
echo "pose $POSE inside $INSIDE"
client() {
  env -u SPIKE_SIMULATE_INPUT -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE \
    OVERMATCH_SERVER=127.0.0.1:5888 OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    OVERMATCH_CONFIG_DIR="$OUT/$1/config" BEVY_ASSET_ROOT="$REPO" \
    OVERMATCH_CAPTURE="$OUT/$1/wall.png" OVERMATCH_CAPTURE_AT="$AT" OVERMATCH_CAPTURE_POSE="$2" \
    OVERMATCH_CAPTURE_SIZE="$SIZE" OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 \
    "$BIN/overmatch" >"$OUT/$1/client.log" 2>&1
}
client a "$POSE" &
APID=$!
sleep 2
client b "$POSE" &
BPID=$!
sleep 2
client c "$INSIDE" &
CPID=$!
start=$EPOCHREALTIME
while alive $APID || alive $BPID || alive $CPID; do
  if (( EPOCHREALTIME - start > 420 )); then echo "capture clients timed out" >&2; break; fi
  sleep 1
done
kill $APID $BPID $CPID $SPID 2>/dev/null || true
sleep 1
kill -9 $APID $BPID $CPID $SPID 2>/dev/null || true
wait $APID $BPID $CPID $SPID 2>/dev/null || true
ls "$OUT"/[abc]/wall-*.png 2>/dev/null | wc -l
