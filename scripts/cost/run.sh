#!/bin/zsh
# Machine-gun-march cost harness — drives the headless server + scripted-fire client(s) through the
# MG-cost scenarios and records per-fixed-tick cost traces (SPIKE_COST_TRACE, src/cost.rs) for
# scripts/cost/analyze.py. The sim-cost question: what does sustained 7.9 mm fire cost the server's
# authoritative FixedUpdate tick, and the client's cosmetic march?
#
#   usage: scripts/cost/run.sh [scenario ...]        (no args = all scenarios)
#   env:   OUT   output dir (default /tmp/mg-cost)
#          BIN   target dir (default target/debug — deps are opt-3 even in dev, see the report)
#          SERVER_BIN / CLIENT_BIN  per-binary overrides of BIN, so the server can run the release
#                profile (matching the droplet) while clients stay dev (matching local playtest
#                reality):  SERVER_BIN=target/release scripts/cost/run.sh idle1 fire1 fire2
#          TICKS long-run script length (default 2560 ≈ 40 s); WARMUP recorder skip (default 384)
#
# Scenarios (each: fresh server, so server/client cost files pair 1:1):
#   idle1   1 tank, stationary, no fire      — the baseline tick
#   fire1   1 tank, stationary, dual-MG held — 1 tank firing (rounds strike terrain ~40 m out)
#   fire1sc fire1 + SPIKE_MG_SHORTCIRCUIT    — A/B: resolution machinery skipped (matters on armor)
#   fire2   2 tanks firing                   — 2 tanks firing (server cost only; client files split)
#   armor1  A fires into a stationary target B at ~8 m — rounds strike ARMOR (full penetration march)
#   armor1sc armor1 + SPIKE_MG_SHORTCIRCUIT  — A/B arm for the armor path
#   loft1   short run, lofted aim, no landing — a 0→~150 projectile-population SWEEP for the per-shell
#           march slope (rounds never impact, so keep it SHORT; a long loft run leaks shells)
set -u
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${OUT:-/tmp/mg-cost}"
BIN="${BIN:-target/debug}"
TICKS="${TICKS:-2560}"
WARMUP="${WARMUP:-384}"
SERVER="$REPO/${SERVER_BIN:-$BIN}/overmatch-server"
CLIENT="$REPO/${CLIENT_BIN:-$BIN}/overmatch"
mkdir -p "$OUT"
cd "$REPO" || exit 1
[ -x "$SERVER" ] && [ -x "$CLIENT" ] || { echo "build first: cargo build --bin overmatch --bin overmatch-server" >&2; exit 1; }

# Which scenarios to run: the positional args, or all of them.
SCENARIOS=("$@")
[ ${#SCENARIOS[@]} -eq 0 ] && SCENARIOS=(idle1 fire1 fire1sc fire2 armor1 armor1sc loft1)
want() { [[ " ${SCENARIOS[*]} " == *" $1 "* ]]; }

# Boot a fresh server. $1=base(SPIKE_COST_TRACE) ; extra env as further args.
start_server() {
  local base="$1"; shift
  rm -f "$OUT/$base.server.jsonl" "$OUT/$base.server.log"
  env "$@" SPIKE_PERTURB=0 SPIKE_COST_TRACE="$OUT/$base.jsonl" SPIKE_COST_WARMUP="$WARMUP" \
    BEVY_ASSET_ROOT="$REPO" "$SERVER" > "$OUT/$base.server.log" 2>&1 &
  SERVER_PID=$!
  local t
  for t in $(seq 1 150); do grep -q "listening" "$OUT/$base.server.log" 2>/dev/null && break; sleep 0.2; done
}
stop_server() { kill "$SERVER_PID" 2>/dev/null; wait "$SERVER_PID" 2>/dev/null; }

# Run one client to completion (foreground). $1=costbase $2=ticks ; extra env as further args.
run_client() {
  local base="$1"; local ticks="$2"; shift 2
  env "$@" SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_TICKS="$ticks" SPIKE_LATENCY_MS=80 SPIKE_JITTER_MS=10 \
    SPIKE_COST_TRACE="$OUT/$base.jsonl" SPIKE_COST_WARMUP="$WARMUP" BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" > "$OUT/$base.client.log" 2>&1
}

if want idle1; then
  echo "== idle1 =="
  start_server idle1
  run_client idle1 "$TICKS" SPIKE_SIM_IDLE=1
  stop_server
fi

if want fire1; then
  echo "== fire1 =="
  start_server fire1
  run_client fire1 "$TICKS" SPIKE_FIRE_SECONDARY=1
  stop_server
fi

if want fire1sc; then
  echo "== fire1sc (MG short-circuit A/B) =="
  start_server fire1sc SPIKE_MG_SHORTCIRCUIT=1
  run_client fire1sc "$TICKS" SPIKE_FIRE_SECONDARY=1 SPIKE_MG_SHORTCIRCUIT=1
  stop_server
fi

if want fire2; then
  echo "== fire2 (2 tanks firing; server cost is the target) =="
  start_server fire2
  # Client B (the second tank) writes its own cost base so the two clients never clobber one .client file.
  run_client fire2b "$TICKS" SPIKE_FIRE_SECONDARY=1 &
  CB=$!
  run_client fire2 "$TICKS" SPIKE_FIRE_SECONDARY=1
  wait "$CB" 2>/dev/null
  stop_server
fi

if want armor1; then
  echo "== armor1 (A fires into stationary target B at ~8 m — full penetration march) =="
  start_server armor1
  run_client armor1b "$TICKS" SPIKE_SIM_IDLE=1 &            # B: stationary target, no fire
  CB=$!
  sleep 1                                                    # let B spawn on lane 0 first
  run_client armor1 "$TICKS" SPIKE_FIRE_SECONDARY=1 SPIKE_AIM_POINT="-8,0,0" SPIKE_SIM_RANGE=12
  wait "$CB" 2>/dev/null
  stop_server
fi

if want armor1sc; then
  echo "== armor1sc (armor A/B arm) =="
  start_server armor1sc SPIKE_MG_SHORTCIRCUIT=1
  run_client armor1scb "$TICKS" SPIKE_SIM_IDLE=1 &
  CB=$!
  sleep 1
  run_client armor1sc "$TICKS" SPIKE_FIRE_SECONDARY=1 SPIKE_MG_SHORTCIRCUIT=1 SPIKE_AIM_POINT="-8,0,0" SPIKE_SIM_RANGE=12
  wait "$CB" 2>/dev/null
  stop_server
fi

if want loft1; then
  echo "== loft1 (short projectile-population sweep, 640 ticks) =="
  start_server loft1
  run_client loft1 640 SPIKE_FIRE_SECONDARY=1 SPIKE_AIM_POINT="30,50,-800" SPIKE_SIM_RANGE=800
  stop_server
fi

echo "ALL DONE -> $OUT"
