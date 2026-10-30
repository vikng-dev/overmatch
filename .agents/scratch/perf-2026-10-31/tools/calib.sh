#!/bin/zsh
# calib.sh <out-dir> <lever>... — the debris lever's live-piece count per value: a headless server
# (SERVER, default bin/A with its spans filtered off; no client), 29 bots, stats lever on, CAL_S
# seconds after it listens; then stats.py over the lines after SKIP_S. Under the machine lock, one
# invocation.
set -eu
zmodload zsh/datetime
here="${0:A:h}"
OUT="${1:?out}"; shift
CAL_S="${CAL_S:-75}"; SKIP_S="${SKIP_S:-30}"
mkdir -p "$OUT"
PID=""
stop_server() {
  if [[ -n "$PID" ]]; then
    kill -KILL "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
    PID=""
  fi
}
trap stop_server EXIT
caffeinate -ims -w $$ </dev/null >/dev/null 2>&1 &
for lever in "$@"; do
  log="$OUT/cal-${lever//:/x}.server.log"
  if lsof -nP -iUDP:5888 >/dev/null 2>&1; then echo "UDP 5888 busy" >&2; exit 1; fi
  env BEVY_ASSET_ROOT="$here/bin/root" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1 \
    OVERMATCH_BOT=29 OVERMATCH_MUTE=1 OVERMATCH_DEBRIS_STRESS="$lever" \
    TRACE_CHROME="$OUT/cal-${lever//:/x}.trace-off.json" RUST_LOG="bevy_ecs=warn,bevy_app=warn" \
    "${SERVER:-$here/bin/A/overmatch-server}" >"$log" 2>&1 &
  PID=$!
  for _ in {1..300}; do grep -q listening "$log" 2>/dev/null && break; sleep 0.2; done
  sleep "$CAL_S"
  stop_server
  python3 "$here/stats.py" --skip "$SKIP_S" "$lever" "$log"
done
