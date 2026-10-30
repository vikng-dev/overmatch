#!/bin/zsh
# Hidden, muted client captures of remote tanks moving, #185: a local headless server with BOTS
# circling bots and one hidden capture client (OVERMATCH_CAPTURE: the window is created hidden, the
# run is silent, it exits once its frames are on disk). The camera is held on the bots' home row.
# Scene inputs (pose, times) are capture configuration (ASSUMED), not tank logic.
#
# Usage (under the machine lock):
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock capture.sh <bin-dir> <out-dir>
set -eu
zmodload zsh/datetime

BIN="$1"; OUT="$2"
REPO="${REPO:-$(cd "$(dirname "$0")/../../../.." && pwd)}"
BOTS="${BOTS:-3}"
AT="${AT:-24,24.25,24.5,24.75,25,25.25,25.5,25.75,26}"
POSE="${POSE:-12,40.5,30,0,36.5,12}"
mkdir -p "$OUT" "$OUT/config"

alive() { kill -0 "$1" 2>/dev/null; }

if lsof -nP -iUDP:5888 >/dev/null 2>&1; then
  echo "UDP 5888 already bound" >&2; exit 1
fi
env BEVY_ASSET_ROOT="$REPO" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_BOT="$BOTS" OVERMATCH_MUTE=1 \
  "$BIN/overmatch-server" >"$OUT/server.log" 2>&1 &
SPID=$!
for _ in {1..300}; do
  grep -q "listening" "$OUT/server.log" 2>/dev/null && break
  alive $SPID || break
  sleep 0.2
done
env -u SPIKE_SIMULATE_INPUT -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE \
  OVERMATCH_SERVER=127.0.0.1:5888 OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
  OVERMATCH_CONFIG_DIR="$OUT/config" BEVY_ASSET_ROOT="$REPO" \
  OVERMATCH_CAPTURE="$OUT/remote.png" OVERMATCH_CAPTURE_AT="$AT" OVERMATCH_CAPTURE_POSE="$POSE" \
  OVERMATCH_CAPTURE_SIZE=1280x720 OVERMATCH_CAPTURE_UI=0 OVERMATCH_CAPTURE_GIZMOS=0 \
  "$BIN/overmatch" >"$OUT/client.log" 2>&1 &
CPID=$!
start=$EPOCHREALTIME
while alive $CPID; do
  if (( EPOCHREALTIME - start > 180 )); then echo "capture client timed out" >&2; break; fi
  sleep 1
done
kill $CPID $SPID 2>/dev/null || true
sleep 1
kill -9 $CPID $SPID 2>/dev/null || true
wait $CPID $SPID 2>/dev/null || true
ls "$OUT"/remote-*.png 2>/dev/null | wc -l
