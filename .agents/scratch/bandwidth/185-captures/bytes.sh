#!/bin/zsh
# Client download per remote tank, #185: a local headless server with BOTS bots plus ONE
# window-less, GPU-less, muted scripted client (SPIKE_SIM_IDLE=1: its own tank parks, the bots
# circle). The client's cumulative socket bytes from nettop at the window's two edges, over the
# wall time between them. The scout's method (run-online-cost.sh net_edge), stripped to one client.
#
# Usage (under the machine lock):
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock bytes.sh <bin-dir> <out-dir> <bots>...
set -eu
setopt null_glob
zmodload zsh/datetime

BIN="$1"; OUT="$2"; shift 2
REPO="${REPO:-$(cd "$(dirname "$0")/../../../.." && pwd)}"
WARMUP_S="${WARMUP_S:-15}"
DURATION_S="${DURATION_S:-40}"
mkdir -p "$OUT"

alive() { kill -0 "$1" 2>/dev/null; }

sample() {
  nettop -P -p "$1" -L 1 -x -J bytes_in,bytes_out 2>/dev/null |
    awk -F, -v pid="$1" 'NR > 1 && $1 ~ ("\\." pid "$") { print $2 "\t" $3; exit }' || true
}

for BOTS in "$@"; do
  cond="bots$BOTS"
  if lsof -nP -iUDP:5888 >/dev/null 2>&1; then
    echo "$cond: UDP 5888 already bound, skipping" | tee -a "$OUT/manifest.txt"
    continue
  fi
  env BEVY_ASSET_ROOT="$REPO" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1 \
    OVERMATCH_BOT="$BOTS" OVERMATCH_MUTE=1 "$BIN/overmatch-server" >"$OUT/$cond.server.log" 2>&1 &
  SPID=$!
  for _ in {1..300}; do
    grep -q "listening" "$OUT/$cond.server.log" 2>/dev/null && break
    alive $SPID || break
    sleep 0.2
  done
  env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE SPIKE_SIMULATE_INPUT=1 \
    OVERMATCH_SERVER=127.0.0.1:5888 OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    SPIKE_SIM_TICKS=$(((WARMUP_S + DURATION_S + 200) * 64)) BEVY_ASSET_ROOT="$REPO" \
    SPIKE_SIM_IDLE=1 "$BIN/overmatch" >"$OUT/$cond.client.log" 2>&1 &
  CPID=$!
  start=$EPOCHREALTIME
  ok=1
  until grep -q -- '— input slot' "$OUT/$cond.client.log" 2>/dev/null; do
    if ! alive $CPID || (( EPOCHREALTIME - start > 120 )); then ok=0; break; fi
    sleep 0.5
  done
  if (( ok )); then
    sleep "$WARMUP_S"
    grep 'server_stats' "$OUT/$cond.server.log" | tail -1 >"$OUT/$cond.marker" || true
    t0=$EPOCHREALTIME; s0="$(sample $CPID)"
    sleep "$DURATION_S"
    t1=$EPOCHREALTIME; s1="$(sample $CPID)"
    alive $CPID || ok=0
    alive $SPID || ok=0
    printf '%s\t%s\t%s\t%s\t%s\n' "$cond" "$t0" "$s0" "$t1" "$s1" >>"$OUT/edges.tsv"
  fi
  kill $CPID $SPID 2>/dev/null || true
  sleep 1
  kill -9 $CPID $SPID 2>/dev/null || true
  wait $CPID $SPID 2>/dev/null || true
  echo "$cond: ok=$ok" | tee -a "$OUT/manifest.txt"
  sleep 1
done
