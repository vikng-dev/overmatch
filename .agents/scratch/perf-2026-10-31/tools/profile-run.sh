#!/bin/zsh
# profile-run.sh <out-dir> <label> — one local server condition for the per-system profile:
# a headless server (SERVER, default bin/A/overmatch-server: the trace_chrome build) with BOTS bots
# and CLIENTS GPU-less idle scripted clients (CLIENT, default bin/B/overmatch), stats lever on.
# TRACE=1 writes the server's chrome trace to <out>/<label>.trace.json (TRACE_CHROME).
# THREADS sets OVERMATCH_SERVER_THREADS. Window: WARMUP_S after the client's input slot, then
# DURATION_S. Writes <label>.server.log, .client.log, .window.json (wall offsets of the window from
# the server's launch, in µs), .marker.
# Run it under the machine lock, one invocation per condition.
set -eu
zmodload zsh/datetime
here="${0:A:h}"
OUT="${1:?out}"; LABEL="${2:?label}"
SERVER="${SERVER:-$here/bin/A/overmatch-server}"
CLIENT="${CLIENT:-$here/bin/A/overmatch}"
ROOT="${ROOT:-$here/bin/root}"
BOTS="${BOTS:-29}"; CLIENTS="${CLIENTS:-1}"
WARMUP_S="${WARMUP_S:-30}"; DURATION_S="${DURATION_S:-60}"
ADDR=127.0.0.1:5888
mkdir -p "$OUT"
log="$OUT/$LABEL.server.log"
SPIDS=()
CPIDS=()
cleanup() {
  local p
  for p in "${CPIDS[@]}"; do kill "$p" 2>/dev/null || true; done
  for p in "${SPIDS[@]}"; do
    kill -INT "$p" 2>/dev/null || true
    for _ in {1..40}; do kill -0 "$p" 2>/dev/null || break; sleep 0.25; done
    kill -KILL "$p" 2>/dev/null || true
  done
  for p in "${CPIDS[@]}" "${SPIDS[@]}"; do wait "$p" 2>/dev/null || true; done
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
caffeinate -ims -w $$ </dev/null >/dev/null 2>&1 &

if lsof -nP -iUDP:5888 >/dev/null 2>&1; then echo "UDP 5888 busy" >&2; exit 1; fi
senv=(BEVY_ASSET_ROOT="$ROOT" OVERMATCH_SERVER_BIND=127.0.0.1 OVERMATCH_SERVER_STATS_S=1
      OVERMATCH_BOT="$BOTS" OVERMATCH_MUTE=1)
# The trace_chrome build writes a chrome trace whatever the filter; without TRACE the spans are
# filtered off at their callsites (RUST_LOG adds to bevy's default filter) and the near-empty file
# is kept as the proof.
if [[ -n "${TRACE:-}" ]]; then
  senv+=(TRACE_CHROME="$OUT/$LABEL.trace.json")
else
  senv+=(TRACE_CHROME="$OUT/$LABEL.trace-off.json" RUST_LOG="bevy_ecs=warn,bevy_app=warn")
fi
[[ -n "${THREADS:-}" ]] && senv+=(OVERMATCH_SERVER_THREADS="$THREADS")
[[ -n "${DEBRIS:-}" ]] && senv+=(OVERMATCH_DEBRIS_STRESS="$DEBRIS")
launch=$EPOCHREALTIME
(cd "$OUT" && exec env "${senv[@]}" "$SERVER" >"$log" 2>&1) &
SPIDS+=($!)
for _ in {1..300}; do grep -q listening "$log" 2>/dev/null && break; sleep 0.2; done
grep -q listening "$log" || { echo "server never listened" >&2; exit 1; }
ticks=$(((WARMUP_S + DURATION_S + 180) * 64))
for (( i = 1; i <= CLIENTS; i++ )); do
  env -u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE -u SPIKE_FRAME_COST_OFFSCREEN -u SPIKE_FRAME_COST \
    SPIKE_SIMULATE_INPUT=1 OVERMATCH_SERVER="$ADDR" OVERMATCH_MUTE=1 OVERMATCH_NO_FOCUS_PAUSE=1 \
    SPIKE_SIM_TICKS="$ticks" BEVY_ASSET_ROOT="$ROOT" SPIKE_SIM_IDLE=1 \
    TRACE_CHROME="$OUT/$LABEL.client$i.trace-off.json" RUST_LOG="bevy_ecs=warn,bevy_app=warn" \
    "$CLIENT" >"$OUT/$LABEL.client$i.log" 2>&1 &
  CPIDS+=($!)
done
start=$EPOCHREALTIME
for (( i = 1; i <= CLIENTS; i++ )); do
  until grep -q -- '— input slot' "$OUT/$LABEL.client$i.log" 2>/dev/null; do
    (( EPOCHREALTIME - start < 120 )) || { echo "client $i took no input slot" >&2; exit 1; }
    sleep 0.5
  done
done
sleep "$WARMUP_S"
grep server_stats "$log" | tail -1 >"$OUT/$LABEL.marker" || true
w0=$EPOCHREALTIME
sleep "$DURATION_S"
w1=$EPOCHREALTIME
cp "$log" "$OUT/$LABEL.window.log"
kill -0 "${SPIDS[1]}" || { echo "server died in the window" >&2; exit 1; }
for p in "${CPIDS[@]}"; do kill -0 "$p" || { echo "a client died in the window" >&2; exit 1; }; done
printf '{"label":"%s","w0_us":%.0f,"w1_us":%.0f,"bots":%s,"clients":%s,"threads":"%s","trace":"%s","debris":"%s"}\n' \
  "$LABEL" "$(( (w0 - launch) * 1e6 ))" "$(( (w1 - launch) * 1e6 ))" "$BOTS" "$CLIENTS" "${THREADS:-default}" "${TRACE:-}" "${DEBRIS:-off}" >"$OUT/$LABEL.window.json"
cleanup
trap - EXIT
echo "$LABEL done: window $(cat "$OUT/$LABEL.window.json")"
