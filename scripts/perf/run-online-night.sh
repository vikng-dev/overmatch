#!/usr/bin/env bash
# =============================== RUNBOOK ===============================
# The online profile, hands-off: a real Edgegap server, this Mac's scripted client, client frame
# cost + client bytes + the server's own stats at rising tank counts. One command:
#
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock env VISIBLE=1 BIN=<dir> ASSET_ROOT=<dir> \
#     scripts/perf/run-online-night.sh <out-dir> <edgegap-version>
#
# The lock keeps compiles and other measurements off the machine; the script holds the machine and
# the display awake itself (`caffeinate -dims -w`) for as long as it runs.
#
# BIN holds `overmatch` built from the commit the version's server image was built from (the
# handshake folds the version, PROTOCOL_REV and the map/materials content: a mismatched client is
# refused); ASSET_ROOT holds its `assets/`. VISIBLE=1 puts the measured window on the owner's screen
# (run-online-cost.sh's runbook: frame-sweep preconditions, hands off); agents run OFFSCREEN=1.
#
# The chain, once per bot count in BOTS (default `1 10 28`: 2, 11 and 29 tanks with the measured
# client), because the bot lever is a deployment's environment, fixed at its start:
#   session.sh deploy <version> --bots N --stats-s 1 (a `profile-` tag, city fallback, watchdog armed
#   at MAX_MIN) → READY → [the first deployment only: a warm-up condition, discarded: it warms the
#   Metal shader cache and the path] → the condition `botsN` (run-online-cost.sh) → logs → metrics →
#   stop → confirmed gone. A count the server did not honour (fewer `spawned … bot` lines than N)
#   ends the chain: the larger counts would measure the same server again.
# DEBRIS (the debris stress lever's values, src/net/debris_stress.rs; `off` for none) makes the
# chain one deployment per bot count AND value, each deployed with `--debris <value>`, named
# `bots<N>-d<NN>-<value>`: the server's price per falling wall fragment on the 1 vCPU host, the
# same conditions scripts/perf/run-debris-stress.sh runs locally. Tonight's:
#
#   /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock env OFFSCREEN=1 BIN=<dir> ASSET_ROOT=<dir> \
#     BOTS=29 DEBRIS='off 10:5 35:5 200:5 200:5 35:5 10:5 off' \
#     scripts/perf/run-online-night.sh <out-dir> <edgegap-version>
#
# THREADS (the server's task-pool thread lever, src/net/server_host.rs; `default` for Bevy's own
# sizing) is the same kind of axis, deployed with `--threads <value>` and named `-t<value>`: the A/B
# of 1 thread against the default on the 1 vCPU host. It combines with BOTS and DEBRIS, e.g.
# `BOTS=29 THREADS='default 1 default 1'`; the server table carries the throttle counters
# (`throttled_usec`, `nr_throttled`) per condition.
#
# The measured client renders the fragments too, so its frame table is their client price as well.
# Then the tables over every condition (client frames, client bytes, server stats), and the final
# sweep: every live `profile-` deployment is stopped (only this kind of run makes them, and the
# machine lock admits one at a time) and the count left must be 0.
#
# EVERY exit path stops what this run deployed: the EXIT trap (normal end, failure, INT, TERM)
# pulls each unstopped deployment's logs, stops it, and runs the final sweep. Behind it, each
# deployment's own watchdog (session.sh) stops it at MAX_MIN even if this process is killed -9.
#
# Products: <out>/deployments.tsv (id, bots, debris, threads, status line), <out>/warmup/, <out>/<condition>/
# (the runner's products, plus server.log: the whole container log, and metrics.json: Edgegap's
# `?raw=true` series), <out>/summary.txt (the three tables, headed by where the server ran; with
# DEBRIS, the debris table after them, from <out>/conditions.tsv).
#
# Wall clock: per condition ~2.5 min (deploy 5–15 s, startup + WARMUP_S 20 + DURATION_S 60 +
# COOLDOWN_S 5, logs and stop ~15 s), plus ~1.5 min of warm-up: ~9 min for the default three, ~22
# min for one bot count and eight debris values.
# Cost: ~10 deployment-minutes at 1 vCPU, ~$0.01 (DERIVED, $0.069/h list); ~$0.03 for the eight.
# Validity: the runner's gates, per condition; a condition that fails them is reported and the
# chain goes on to the next count (the deployment is still stopped).
# =======================================================================
#
# Knobs (env): BOTS ("1 10 28"), DEBRIS (unset: no debris lever; else its values, see above),
# THREADS (unset: no thread lever; else its values),
# LOAD_CLIENTS (0: window-less scripted clients added to every
# condition), MAX_MIN (30: each deployment's watchdog), DEPLOY_ENV (extra K=V words for every
# deployment's server env), EDGEGAP_CITIES (session.sh's fallback list, default
# "Singapore Tokyo Mumbai"), WARMUP_S (20), DURATION_S (60), COOLDOWN_S (5), WARMUP_RUN_S (20: the
# warm-up condition's measured span), plus the runner's (BIN, ASSET_ROOT, CLIENT_WORKLOAD, …).
set +x
set -euo pipefail

OUT="${1:?usage: run-online-night.sh <out-dir> <edgegap-version>}"
VERSION="${2:?usage: run-online-night.sh <out-dir> <edgegap-version>}"
[[ "$OUT" == /* ]] || OUT="$PWD/$OUT"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
# EDGEGAP_SESSION: a stand-in with session.sh's subcommands, for testing this chain by day against
# a local server; never set for a measurement.
SESSION="${EDGEGAP_SESSION:-$REPO/scripts/edgegap/session.sh}"
RUNNER="$REPO/scripts/perf/run-online-cost.sh"
BOTS="${BOTS:-1 10 28}"
LOAD_CLIENTS="${LOAD_CLIENTS:-0}"
MAX_MIN="${MAX_MIN:-30}"
DEPLOY_ENV="${DEPLOY_ENV:-}"
STATS_PERIOD_S="${STATS_PERIOD_S:-1}"
export WARMUP_S="${WARMUP_S:-20}"
export DURATION_S="${DURATION_S:-60}"
export COOLDOWN_S="${COOLDOWN_S:-5}"
export MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"
WARMUP_RUN_S="${WARMUP_RUN_S:-20}"

modes=0
for mode in "${VISIBLE:-}" "${OFFSCREEN:-}" "${SMOKE:-}"; do
  if [[ -n "$mode" ]]; then modes=$((modes + 1)); fi
done
if ((modes != 1)); then
  echo "REFUSING: set exactly one of VISIBLE=1, OFFSCREEN=1 or SMOKE=1 (run-online-cost.sh's runbook)" >&2
  exit 1
fi

mkdir -p "$OUT"
LEDGER="$OUT/deployments.tsv"
: >"$LEDGER"
STOPPED=" "
FAILED=()
CHILD=""

say() { printf '%s\n' "$*" >&2; }

# A runner in the background, waited on: bash runs a trap only once its foreground child exits, and
# `wait` is the one place a signal interrupts at once. The trap then ends the runner itself.
run_waited() {
  local status=0
  "$@" &
  CHILD=$!
  wait "$CHILD" || status=$?
  CHILD=""
  return "$status"
}

on_exit() {
  local status=$? id rest
  trap - EXIT
  set +e
  if [[ -n "$CHILD" ]]; then
    kill -TERM "$CHILD" 2>/dev/null
    wait "$CHILD" 2>/dev/null
  fi
  while read -r id rest; do
    [[ -n "$id" && "$STOPPED" != *" $id "* ]] || continue
    say "exit path: pulling the logs of $id, then stopping it"
    "$SESSION" logs "$id" "$OUT/$id.at-exit.server.log" >/dev/null || true
    "$SESSION" stop "$id" && STOPPED="$STOPPED$id "
  done <"$LEDGER"
  say "final sweep: every live profile- deployment"
  "$SESSION" stop-all-profile || status=1
  "$SESSION" list-profile >"$OUT/profile-left.txt" 2>>"$OUT/profile-left.txt" || status=1
  say "$(tail -1 "$OUT/profile-left.txt")"
  if ((${#FAILED[@]} > 0)); then
    say "conditions that failed their gates: ${FAILED[*]}"
    status=1
  fi
  exit "$status"
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# A sleeping Mac or a dark display would end the deployments' runs by the watchdog and fail the
# visible window's gates: held awake until this process exits.
caffeinate -dims -w $$ </dev/null >/dev/null 2>&1 &

deploy_args=()
for kv in $DEPLOY_ENV; do deploy_args+=(--env "$kv"); done

# The chain's conditions, in order: bot count, then debris value, then thread value. Without DEBRIS
# or THREADS: one per bot count, `bots<N>`. DEBRIS appends `-d<NN>-<value, x for :>`, THREADS
# appends `-t<value>`; the debris conditions are listed in conditions.tsv for the debris table.
DEBRIS="${DEBRIS:-}"
THREADS="${THREADS:-}"
chain_bots=()
chain_debris=()
chain_threads=()
chain_names=()
for bots in $BOTS; do
  n=0
  for debris in ${DEBRIS:-off}; do
    n=$((n + 1))
    for threads in ${THREADS:-default}; do
      chain_bots+=("$bots")
      chain_debris+=("$debris")
      chain_threads+=("$threads")
      name="bots$bots"
      [[ -n "$DEBRIS" ]] && name+="$(printf -- '-d%02d-%s' "$n" "${debris//:/x}")"
      [[ -n "$THREADS" ]] && name+="-t$threads"
      chain_names+=("$name")
    done
  done
done
if [[ -n "$DEBRIS" ]]; then
  : >"$OUT/conditions.tsv"
  for i in "${!chain_names[@]}"; do
    printf '%s\t%s\t%s\n' "${chain_names[$i]}" "${chain_debris[$i]}" \
      "${chain_names[$i]}/${chain_names[$i]}.server.json" >>"$OUT/conditions.tsv"
  done
fi

first=1
baseline=""
for i in "${!chain_names[@]}"; do
  bots="${chain_bots[$i]}"
  debris="${chain_debris[$i]}"
  threads="${chain_threads[$i]}"
  name="${chain_names[$i]}"
  dir="$OUT/$name"
  debris_args=()
  [[ -n "$DEBRIS" ]] && debris_args=(--debris "$debris")
  threads_args=()
  [[ -n "$THREADS" ]] && threads_args=(--threads "$threads")
  say "== $name: deploying $VERSION (bots $bots, debris $debris, threads $threads)"
  if ! id="$("$SESSION" deploy "$VERSION" --bots "$bots" --stats-s "$STATS_PERIOD_S" \
    ${debris_args[@]+"${debris_args[@]}"} ${threads_args[@]+"${threads_args[@]}"} --max-min "$MAX_MIN" ${deploy_args[@]+"${deploy_args[@]}"})"; then
    say "$name: no deployment turned READY; stopping the chain"
    FAILED+=("$name(deploy)")
    break
  fi
  printf '%s\t%s\t%s\t%s\t%s\n' "$id" "$bots" "$debris" "$threads" "$("$SESSION" status "$id")" >>"$LEDGER"
  target="$("$SESSION" address "$id" --wait 60)"

  if [[ -n "$first" ]]; then
    say "== warm-up condition (discarded): shader cache and path"
    run_waited env DURATION_S="$WARMUP_RUN_S" MIN_ROWS=$((WARMUP_RUN_S * 20)) TARGET="$target" \
      DEPLOYMENT="$id" "$RUNNER" "$OUT/warmup" 'warmup|' ||
      say "the warm-up condition failed its gates (discarded either way)"
    first=""
  fi

  spec="$name|"
  ((LOAD_CLIENTS > 0)) && spec="$name||LOAD_CLIENTS=$LOAD_CLIENTS"
  if run_waited env TARGET="$target" DEPLOYMENT="$id" "$RUNNER" "$dir" "$spec"; then
    [[ -n "$baseline" ]] || baseline="$name"
  else
    FAILED+=("$name")
  fi

  "$SESSION" logs "$id" "$dir/server.log" >/dev/null || say "$name: the full log could not be pulled"
  "$SESSION" metrics "$id" "$dir/metrics.json" >/dev/null || say "$name: no metrics"
  if "$SESSION" stop "$id"; then STOPPED="$STOPPED$id "; fi

  if ((bots > 1)); then
    spawned="$(cd "$REPO" && uv run scripts/perf/online_cost.py bots "$dir/server.log")"
    if ((spawned < bots)); then
      say "$name: the server spawned $spawned bot(s) — the N-bot lever did not take; the conditions after it would repeat this one"
      echo "$name: OVERMATCH_BOT=$bots spawned $spawned bot(s)" >>"$OUT/notes.txt"
      break
    fi
  fi
done

{
  echo "online profile: Edgegap version $VERSION, from $(hostname -s), $(date '+%F %H:%M %Z')"
  cut -f2- "$LEDGER" | sed 's/^/deployment (bots, debris, threads, status at READY): /'
  [[ -f "$OUT/notes.txt" ]] && cat "$OUT/notes.txt"
} >"$OUT/summary.txt"
streams=("$OUT"/bots*/bots*.client.jsonl)
if [[ -n "$baseline" && -e "${streams[0]}" ]]; then
  relax=()
  [[ -n "${SMOKE:-}" ]] && relax=(--occluded-ok --display-ok --surface-ok --undrawn-ok)
  (cd "$REPO" && uv run scripts/perf/analyze.py --warmup-s "$WARMUP_S" --min-rows "$MIN_ROWS" \
    --expected-duration-s "$DURATION_S" ${relax[@]+"${relax[@]}"} --baseline "$baseline" "${streams[@]}") |
    tee -a "$OUT/summary.txt" || FAILED+=("analysis")
  (cd "$REPO" && uv run scripts/perf/online_cost.py tables "$OUT"/bots*) | tee -a "$OUT/summary.txt"
fi
if [[ -n "$DEBRIS" ]]; then
  (cd "$REPO" && uv run scripts/perf/online_cost.py stress "$OUT/conditions.tsv") | tee -a "$OUT/summary.txt"
fi
say "online night complete -> $OUT/summary.txt"
