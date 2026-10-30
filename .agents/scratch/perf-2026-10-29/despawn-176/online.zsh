#!/bin/zsh
# Loopback mass-despawn driver for #176: a local server with BOTS bots, one hidden offscreen client;
# once every replica holds its sim body the server is killed, the client times out and despawns
# every replicated tank. Usage: online.zsh <out-dir> <bin-dir> <repo>
set -u
zmodload zsh/datetime
OUT="$1"; BIN="$2"; REPO="$3"
BOTS="${BOTS:-28}"; SETTLE_S="${SETTLE_S:-20}"; AFTER_S="${AFTER_S:-12}"
mkdir -p "$OUT/cfg"
: >"$OUT/phases.txt"
mark() { print -r -- "$(date -u +%Y-%m-%dT%H:%M:%S) $1" >>"$OUT/phases.txt"; }
alive() { local s; s="$(ps -o state= -p "$1" 2>/dev/null || true)"; [[ -n "$s" && "$s" != *Z* ]]; }
if lsof -nP -iUDP:5888 >/dev/null 2>&1; then echo "REFUSING: UDP 5888 busy" >&2; exit 1; fi
env BEVY_ASSET_ROOT="$REPO" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_BOT="$BOTS" OVERMATCH_MUTE=1 \
  "$BIN/overmatch-server" >"$OUT/server.log" 2>&1 &
SERVER=$!
for _ in {1..300}; do grep -q "listening" "$OUT/server.log" 2>/dev/null && break; alive $SERVER || break; sleep 0.2; done
grep -q "listening" "$OUT/server.log" || { echo "server never listened" >&2; kill -KILL $SERVER; exit 1; }
mark "server listening"
env -u SPIKE_SIM_VISIBLE BEVY_ASSET_ROOT="$REPO" OVERMATCH_CONFIG_DIR="$OUT/cfg" \
  SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 SPIKE_FRAME_COST="$OUT/frames.jsonl" SPIKE_FRAME_COST_OFFSCREEN=1 \
  OVERMATCH_SERVER=127.0.0.1:5888 OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
  SPIKE_SIM_TICKS=100000 SPIKE_SIM_IDLE=1 \
  "$BIN/overmatch" >"$OUT/client.log" 2>&1 &
CLIENT=$!
cleanup() { kill -KILL $SERVER $CLIENT 2>/dev/null; wait $SERVER $CLIENT 2>/dev/null; }
trap cleanup EXIT INT TERM
want=$((BOTS))
deadline=$((SECONDS + 300))
while (( SECONDS < deadline )); do
  n=$(grep -c "replicated tank gets local sim body" "$OUT/client.log" 2>/dev/null || true)
  (( n >= want )) && break
  alive $CLIENT || { echo "client exited early" >&2; exit 1; }
  sleep 1
done
mark "replicas attached: $(grep -c 'replicated tank gets local sim body' "$OUT/client.log")"
sleep "$SETTLE_S"
mark "killing server"
kill -KILL $SERVER; wait $SERVER 2>/dev/null
sleep "$AFTER_S"
mark "killing client"
cleanup
trap - EXIT
echo "done -> $OUT"
