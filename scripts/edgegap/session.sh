#!/usr/bin/env bash
# =============================== RUNBOOK ===============================
# session.sh — one profiling deployment of an EXISTING Edgegap app version, driven straight through
# the Edgegap API: no matchmaking Worker, no hosted mode. The scripted client cannot go through the
# Worker (simulate mode always dials a fixed address with the all-zero dev key, src/net/client.rs),
# so the server must run non-hosted: no `OVERMATCH_SERVER_ID`, none of the Worker's env
# (backend/contract/server-env.json's keys are refused), all-zero netcode key.
#
#   D=$(scripts/edgegap/session.sh deploy 0.6.1 --bots 10)      # prints the id; READY line on stderr
#   TARGET=$(scripts/edgegap/session.sh address "$D")             # ip:port
#   scripts/edgegap/session.sh logs "$D" out/                     # BEFORE any stop: logs die at stop
#   scripts/edgegap/session.sh metrics "$D" out/
#   scripts/edgegap/session.sh stop "$D"                          # DELETE, then confirms it is gone
#   scripts/edgegap/session.sh stop-all-profile                   # every `profile-` deployment, then 0 left
#
# Subcommands (stdout carries only the machine-readable answer; progress goes to stderr):
#   deploy <version> [--city C]... [--bots N] [--stats-s S] [--debris V] [--env K=V]... [--max-min M]
#       POST /v1/deploy with a `profile-<utc>-<hex>` tag and a city filter, falling back through the
#       cities (default: Singapore Tokyo Mumbai, or $EDGEGAP_CITIES) on a refusal or an ERROR
#       deployment, until one turns READY. Arms the watchdog the moment an id exists. Prints the id.
#       --bots N    sets the bot lever ($BOT_LEVER, default OVERMATCH_BOT) to N; 0 or absent: unset.
#       --stats-s S sets the server-stats lever ($STATS_LEVER, default OVERMATCH_SERVER_STATS_S) to
#                   S seconds (default 1); 0 leaves it unset. A server without the lever ignores it.
#       --debris V  sets the debris stress lever ($DEBRIS_LEVER, default OVERMATCH_DEBRIS_STRESS) to
#                   V, `<pieces>:<period_s>[:<live_cap>[:<frozen_cap>]]` (src/net/debris_stress.rs);
#                   `off` or absent: unset. A server older than the lever ignores it.
#       --env K=V   any other server env var.
#       --max-min M the watchdog's limit in minutes (default 90, or $EDGEGAP_PROFILE_MAX_MIN).
#   status <id> [--json]    one line: phase, ip:port, city, seconds since READY, Edgegap max_duration.
#   address <id> [--wait S] ip:port once READY, waiting up to S seconds (default 0: answer now).
#   logs <id> <file|dir>    the container's stdout+stderr, to <file> or <dir>/<id>.server.log.
#   metrics <id> <file|dir> [--steps 1m]   `?raw=true` metrics JSON, to <file> or <dir>/<id>.metrics.json.
#   stop <id>               DELETE /v1/stop/<id>; polls until Edgegap reports it terminated or gone and
#                           the live list no longer holds it; disarms its watchdog. Exit 1 unconfirmed.
#   stop-all-profile        stops every live deployment carrying a `profile-` tag, then confirms none
#                           remain. Touches no other deployment (the Worker's are tagged `om-`).
#   list-profile            the live `profile-` deployments, one per line, and the live total.
#   watchdog <id> <minutes> (re-)arms the detached stopper by hand; `deploy` already arms it.
#
# THE COST GUARD. A non-hosted server never exits on its own, the Worker's sweep ignores anything
# not tagged `om-` (backend/src/sweep.ts), and Edgegap ends a deployment only at its VERSION's
# `max_duration` — 1440 min for the release versions (scripts/ci/edgegap-version.sh), about $1.66
# per vCPU at list price (DERIVED). Three layers stand in front of that:
#   1. the caller's trap (scripts/perf/run-online-night.sh stops every id it deployed, on any exit);
#   2. the watchdog: a detached process in its own session (it survives the caller, the terminal and
#      a process-group kill) that pulls the logs and stops the deployment at the limit, judged on the
#      wall clock so a sleeping Mac stops it on wake. Its log: $EDGEGAP_SESSION_DIR/<id>.watchdog.log;
#   3. Edgegap's own cap. The API offers no per-deployment duration: neither /v1/deploy nor
#      /v2/deployments takes one (Edgegap OpenAPI v1 and v2, read 2026-10-30); only an app version
#      carries `max_duration`. For a tighter Edgegap-side cap, register a profiling version with one:
#        EDGEGAP_MAX_DURATION=120 scripts/ci/edgegap-version.sh 0.6.1-profile \
#          ghcr.io/vikng-dev/overmatch-server 0.6.1-<sha7>
#      and deploy `0.6.1-profile`; the deployment's handshake is the image's, not the version name's.
#   `deploy` prints the max_duration Edgegap holds for the deployment, so the cap in force is on record.
#
# THE TOKEN is read from ~/.config/overmatch/edgegap.token (or $EDGEGAP_TOKEN_FILE) into a shell
# variable and reaches curl only through a process-substitution pipe (`-H @<(printf …)`, a builtin):
# never an argv, a file, a log line or the terminal. Never run this script under `set -x` or
# `bash -x`: a trace would print it. The script turns tracing off at its top for that reason.
#
# Wall clock: deploy 5–15 s to READY (MEASURED: 3.3 s warm, 7–12.5 s cold, Singapore 4.6 s;
# .agents/scratch/hosting-2026-10/risk-test.md, decisions.md); a city without capacity errors
# within ~1.5 s (MEASURED, Frankfurt). stop: Edgegap reports TERMINATED within seconds.
# Cost: $0.069/h at 1 vCPU plus $0.10/GB egress (CLAIMED, edgegap.com/pricing, 2026-10-26).
# Validity: the only Edgegap writes this script makes are a deploy tagged `profile-` and a stop of
# a deployment it has read back carrying a `profile-` tag.
# =======================================================================
set +x
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd -P)"
self="$here/$(basename "$0")"
api="${EDGEGAP_API:-https://api.edgegap.com}"
app="${EDGEGAP_APP:-Overmatch}"
port_name="${EDGEGAP_PORT_NAME:-game}"
token_file="${EDGEGAP_TOKEN_FILE:-$HOME/.config/overmatch/edgegap.token}"
session_dir="${EDGEGAP_SESSION_DIR:-${TMPDIR:-/tmp}/overmatch-edgegap}"
contract="$here/../../backend/contract/server-env.json"
STATS_LEVER="${STATS_LEVER:-OVERMATCH_SERVER_STATS_S}"
BOT_LEVER="${BOT_LEVER:-OVERMATCH_BOT}"
DEBRIS_LEVER="${DEBRIS_LEVER:-OVERMATCH_DEBRIS_STRESS}"
TAG_PREFIX="profile-"

say() { printf '%s\n' "$*" >&2; }
die() {
  say "session.sh: $*"
  exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

token=""
load_token() {
  [[ -r "$token_file" ]] || die "no Edgegap token at $token_file"
  token="$(<"$token_file")"
  token="${token#token }"
  token="${token//[$'\t\r\n ']/}"
  [[ -n "$token" ]] || die "the token file $token_file is empty"
}

# One API call: `call METHOD PATH [BODY]` prints the HTTP status (000 when no answer arrived); the
# response body lands in $work/out. The authorization header travels only through the pipe.
call() {
  local method="$1" path="$2" body="${3:-}"
  local args=(-sS --max-time 60 -o "$work/out" -w '%{http_code}' -X "$method" "${api}${path}")
  [[ -n "$body" ]] && args+=(--data-binary "$body")
  : >"$work/out"
  curl "${args[@]}" \
    -H @<(printf 'authorization: token %s\ncontent-type: application/json\naccept: application/json\n' "$token") \
    2>"$work/curl.err" || true
}

# The `message` of an error body, never the whole body.
api_message() { jq -r '.message // .detail // empty' "$work/out" 2>/dev/null | head -c 300 || true; }

# `phase ip port tags_csv city max_duration elapsed` for one deployment, from /v1/status. Phase is
# ready | pending | error | terminated | gone | unknown (no answer). The Worker's `interpretStatus`
# (backend/src/edgegap.ts) reads the error flag first; here termination comes first, because a
# deployment that failed placement keeps `error: true` after Edgegap has terminated it (MEASURED
# 2026-10-30: `Status.TERMINATED`, `error: true`, "Not enough space on locations…"), and a stop
# must read that as done.
status_fields() {
  local id="$1" code
  code="$(call GET "/v1/status/$id")"
  case "$code" in
    200) ;;
    404 | 410) echo "gone - - - - - -"; return ;;
    *) echo "unknown - - - - - -"; return ;;
  esac
  jq -r --arg port "$port_name" '
    (.current_status // "" | ascii_upcase) as $s
    | (.ports[$port].external // null) as $p
    | (if ($s | contains("TERMINATED")) or ($s | contains("STOPPED")) then "terminated"
       elif (.error // false) or ($s | contains("ERROR")) then "error"
       elif (.running // false) and (.public_ip // "") != "" and $p != null then "ready"
       else "pending" end) as $phase
    | [$phase, (.public_ip // "-"), ($p // "-" | tostring),
       ((.tags // []) | join(",") | if . == "" then "-" else . end),
       (.location.city // "-" | gsub(" "; "_")),
       (.max_duration // "-" | tostring), (.elapsed_time // "-" | tostring)]
    | join(" ")' "$work/out"
}

# Every live deployment as `id tags_csv`, all pages. Returns 1 (message on stderr) when any page
# fails: a partial list must never read as "absent".
list_all() {
  local page=1 code
  while :; do
    code="$(call GET "/v1/deployments?page=$page&limit=100")"
    if [[ "$code" != 200 ]]; then
      say "listing deployments failed (HTTP $code) $(api_message)"
      return 1
    fi
    jq -r '.data // [] | .[] | "\(.request_id) \((.tags // []) | join(",") | if . == "" then "-" else . end)"' "$work/out"
    [[ "$(jq -r '.pagination.has_next // false' "$work/out")" == true ]] || return 0
    page=$((page + 1))
    if ((page > 50)); then
      say "more than 50 pages of deployments; refusing to guess"
      return 1
    fi
  done
}

# The live deployments carrying a `profile-` tag, `id tags_csv` per line; returns 1 when the list
# could not be read.
profile_lines() {
  local all
  all="$(list_all)" || return 1
  awk -v p="$TAG_PREFIX" 'NF { n = split($2, t, ","); for (i = 1; i <= n; i++) if (index(t[i], p) == 1) { print; break } }' <<<"$all"
}

has_profile_tag() { [[ ",$1," == *",$TAG_PREFIX"* ]]; }

out_path() { # <file|dir> <id> <suffix>
  if [[ -d "$1" || "$1" == */ ]]; then
    mkdir -p "$1"
    echo "${1%/}/$2.$3"
  else
    mkdir -p "$(dirname "$1")"
    echo "$1"
  fi
}

cmd_status() {
  local id="${1:?usage: status <id> [--json]}"
  load_token
  if [[ "${2:-}" == --json ]]; then
    local code
    code="$(call GET "/v1/status/$id")"
    [[ "$code" == 200 ]] || die "status $id: HTTP $code $(api_message)"
    cat "$work/out"
    echo
    return
  fi
  local phase ip port tags city maxd elapsed
  read -r phase ip port tags city maxd elapsed <<<"$(status_fields "$id")"
  echo "$id $phase $ip:$port city=$city since_ready_s=$elapsed max_duration_min=$maxd tags=$tags"
}

# Poll until READY; prints `ip:port`. Fails on error/terminated/gone, or when `wait_s` runs out.
wait_ready() {
  local id="$1" wait_s="$2" start=$SECONDS last="" phase ip port tags city maxd elapsed
  while :; do
    read -r phase ip port tags city maxd elapsed <<<"$(status_fields "$id")"
    if [[ "$phase" != "$last" ]]; then
      say "  $id: $phase after $((SECONDS - start)) s"
      last="$phase"
    fi
    case "$phase" in
      ready)
        echo "$ip:$port"
        return 0
        ;;
      error | terminated | gone) return 1 ;;
    esac
    ((SECONDS - start < wait_s)) || return 1
    sleep 1
  done
}

cmd_address() {
  local id="${1:?usage: address <id> [--wait S]}" wait_s=0
  [[ "${2:-}" == --wait ]] && wait_s="${3:?--wait needs seconds}"
  load_token
  wait_ready "$id" "$wait_s" || die "$id is not READY"
}

# Stop one deployment and confirm it is gone. Refuses an id whose live record carries no
# `profile-` tag (it is not ours to stop), and one whose record cannot be read (its tag cannot be
# checked). Returns 1 when the stop was refused or could not be confirmed.
stop_one() {
  local id="$1" phase ip port tags city maxd elapsed code listed start=$SECONDS
  read -r phase ip port tags city maxd elapsed <<<"$(status_fields "$id")"
  case "$phase" in
    gone | terminated)
      say "stop $id: already $phase"
      disarm_watchdog "$id"
      return 0
      ;;
    unknown)
      say "stop $id: no status answer, so its ${TAG_PREFIX} tag cannot be checked; not stopping"
      return 1
      ;;
  esac
  if ! has_profile_tag "$tags"; then
    say "refusing to stop $id: its tags ($tags) carry no ${TAG_PREFIX} tag"
    return 1
  fi
  code="$(call DELETE "/v1/stop/$id")"
  case "$code" in
    200 | 202 | 404 | 410) ;;
    *) say "stop $id: HTTP $code $(api_message)" ;;
  esac
  while ((SECONDS - start < 120)); do
    read -r phase ip port tags city maxd elapsed <<<"$(status_fields "$id")"
    if [[ "$phase" == gone || "$phase" == terminated ]] && listed="$(list_all)" &&
      ! awk '{print $1}' <<<"$listed" | grep -qx -- "$id"; then
      say "stopped $id: $phase, absent from the live list, after $((SECONDS - start)) s"
      disarm_watchdog "$id"
      return 0
    fi
    sleep 2
    # A deployment still DEPLOYING when the DELETE landed stops once it reaches READY or ERROR
    # (Edgegap's stop contract); re-sending the stop is idempotent and covers a lost first one.
    if (((SECONDS - start) % 20 < 2)); then call DELETE "/v1/stop/$id" >/dev/null; fi
  done
  say "stop $id: NOT CONFIRMED after 120 s (last phase $phase) — run: $self stop $id"
  return 1
}

cmd_stop() {
  local id="${1:?usage: stop <id>}"
  load_token
  stop_one "$id"
}

cmd_list_profile() {
  load_token
  local all mine
  all="$(list_all)" || die "cannot list deployments"
  mine="$(profile_lines)" || die "cannot list deployments"
  if [[ -n "$mine" ]]; then echo "$mine"; fi
  say "live deployments: $(awk 'NF' <<<"$all" | wc -l | tr -d ' ') in total, $(awk 'NF' <<<"$mine" | wc -l | tr -d ' ') tagged ${TAG_PREFIX}*"
}

cmd_stop_all_profile() {
  load_token
  local lines id failed=0 left
  lines="$(profile_lines)" || die "cannot list deployments"
  [[ -n "$lines" ]] || say "no live ${TAG_PREFIX} deployments"
  while read -r id _; do
    [[ -n "$id" ]] || continue
    stop_one "$id" </dev/null || failed=1
  done <<<"$lines"
  lines="$(profile_lines)" || die "cannot list deployments to confirm"
  left="$(awk 'NF' <<<"$lines" | wc -l | tr -d ' ')"
  say "${TAG_PREFIX} deployments remaining: $left"
  ((failed == 0 && left == 0))
}

cmd_logs() {
  local id="${1:?usage: logs <id> <file|dir>}" dest="${2:?usage: logs <id> <file|dir>}" code path
  load_token
  path="$(out_path "$dest" "$id" server.log)"
  code="$(call GET "/v1/deployment/$id/container-logs")"
  [[ "$code" == 200 ]] || die "logs $id: HTTP $code $(api_message) — logs are deleted when a deployment stops"
  jq -r '.logs // ""' "$work/out" >"$path.tmp"
  if [[ "$(jq -r '.crash_logs // empty' "$work/out")" != "" ]]; then
    {
      echo "--- crash_logs ---"
      jq -r '.crash_logs' "$work/out"
      echo "--- crash_data ---"
      jq -c '.crash_data' "$work/out"
    } >>"$path.tmp"
  fi
  mv "$path.tmp" "$path"
  say "logs $id: $(wc -l <"$path" | tr -d ' ') lines, $(wc -c <"$path" | tr -d ' ') B -> $path"
  echo "$path"
}

cmd_metrics() {
  local id="${1:?usage: metrics <id> <file|dir> [--steps S]}" dest="${2:?usage: metrics <id> <file|dir>}" query="raw=true" code path
  [[ "${3:-}" == --steps ]] && query="$query&steps=${4:?--steps needs a value}"
  load_token
  path="$(out_path "$dest" "$id" metrics.json)"
  code="$(call GET "/v1/metrics/deployment/$id?$query")"
  [[ "$code" == 200 ]] || die "metrics $id: HTTP $code $(api_message)"
  mv "$work/out" "$path"
  say "metrics $id -> $path"
  echo "$path"
}

disarm_watchdog() {
  local pidfile="$session_dir/$1.watchdog.pid" pid
  [[ -f "$pidfile" ]] || return 0
  pid="$(<"$pidfile")"
  # Only a process that is still this script's watchdog for this id (a pid can be reused), and never
  # the watchdog itself, which reaches here through its own stop.
  if [[ "$pid" != "$$" ]] && ps -o command= -p "$pid" 2>/dev/null | grep -q -- "_watchdog_loop $1 "; then
    kill "$pid" 2>/dev/null || true
  fi
  rm -f "$pidfile"
}

arm_watchdog() { # <id> <minutes>
  local id="$1" minutes="$2" deadline
  [[ "$minutes" =~ ^[0-9]+$ && "$minutes" -ge 1 ]] || die "watchdog minutes must be a positive integer, got '$minutes'"
  mkdir -p "$session_dir"
  disarm_watchdog "$id"
  deadline=$(($(date +%s) + minutes * 60))
  # setsid: its own session, so neither the caller's exit nor a kill of its process group ends it.
  nohup /usr/bin/perl -e 'use POSIX qw(setsid); setsid(); exec @ARGV or die "exec: $!"' -- \
    "$self" _watchdog_loop "$id" "$deadline" </dev/null >>"$session_dir/$id.watchdog.log" 2>&1 &
  echo $! >"$session_dir/$id.watchdog.pid"
  say "watchdog armed for $id: stops it at $(date -r "$deadline" '+%H:%M:%S') ($minutes min); log $session_dir/$id.watchdog.log"
}

cmd_watchdog() {
  local id="${1:?usage: watchdog <id> <minutes>}" minutes="${2:?usage: watchdog <id> <minutes>}"
  arm_watchdog "$id" "$minutes"
}

# The detached stopper. Wall-clock deadline, so a Mac that slept past it stops the deployment on
# wake. Exits early once the deployment is gone; at the deadline it pulls the logs first (they die
# with the deployment), then stops and confirms, retrying for 30 min if the network is down.
watchdog_loop() {
  local id="$1" deadline="$2" now phase rest
  # Nothing may end this loop early but the deployment's own end: no errexit here.
  set +e
  load_token
  echo "$(date '+%F %T') watchdog for $id: deadline $(date -r "$deadline" '+%F %T')"
  local next_check=0
  while :; do
    now=$(date +%s)
    if ((now >= deadline)); then break; fi
    if ((now >= next_check)); then
      read -r phase rest <<<"$(status_fields "$id")"
      if [[ "$phase" == gone || "$phase" == terminated ]]; then
        echo "$(date '+%F %T') $id is $phase before the deadline; watchdog done"
        rm -f "$session_dir/$id.watchdog.pid"
        return 0
      fi
      next_check=$((now + 120))
    fi
    sleep 20
  done
  echo "$(date '+%F %T') DEADLINE: pulling logs, then stopping $id"
  "$self" logs "$id" "$session_dir/$id.watchdog.server.log" || echo "logs pull failed (continuing to the stop)"
  local give_up=$((deadline + 1800))
  until stop_one "$id"; do
    (($(date +%s) < give_up)) || {
      echo "$(date '+%F %T') GAVE UP stopping $id — stop it by hand: $self stop $id"
      return 1
    }
    sleep 30
  done
  echo "$(date '+%F %T') watchdog stopped $id"
  rm -f "$session_dir/$id.watchdog.pid"
}

# The deployment whose deploy answer was lost, found by its unique tag (empty when none or unread).
find_by_tag() {
  local tag="$1" all
  all="$(list_all)" || return 0
  awk -v t="$tag" '{ n = split($2, a, ","); for (i = 1; i <= n; i++) if (a[i] == t) { print $1; exit } }' <<<"$all"
}

cmd_deploy() {
  local version="${1:?usage: deploy <version> [--city C]... [--bots N] [--stats-s S] [--debris V] [--env K=V]... [--max-min M]}"
  shift
  local cities=() envs=() bots=0 stats_s=1 debris=off max_min="${EDGEGAP_PROFILE_MAX_MIN:-90}" ready_wait="${EDGEGAP_READY_WAIT_S:-120}"
  while (($# > 0)); do
    case "$1" in
      --city) cities+=("${2:?--city needs a name}"); shift 2 ;;
      --bots) bots="${2:?--bots needs a count}"; shift 2 ;;
      --stats-s) stats_s="${2:?--stats-s needs seconds}"; shift 2 ;;
      --debris) debris="${2:?--debris needs a value or off}"; shift 2 ;;
      --env) envs+=("${2:?--env needs K=V}"); shift 2 ;;
      --max-min) max_min="${2:?--max-min needs minutes}"; shift 2 ;;
      *) die "deploy: unknown argument '$1'" ;;
    esac
  done
  if ((${#cities[@]} == 0)); then
    read -r -a cities <<<"${EDGEGAP_CITIES:-Singapore Tokyo Mumbai}"
  fi
  [[ "$bots" =~ ^[0-9]+$ ]] || die "--bots must be a non-negative integer"
  [[ "$stats_s" =~ ^[0-9]+$ ]] || die "--stats-s must be a non-negative integer"
  # The server refuses a malformed debris value at launch, which a deployment shows only as an
  # ERROR: refused here instead.
  [[ "$debris" == off || "$debris" =~ ^[0-9]+:[0-9]+(\.[0-9]+)?(:[0-9]+){0,2}$ ]] ||
    die "--debris wants <pieces>:<period_s>[:<live_cap>[:<frozen_cap>]] or off, got '$debris'"
  ((bots > 0)) && envs+=("$BOT_LEVER=$bots")
  ((stats_s > 0)) && envs+=("$STATS_LEVER=$stats_s")
  [[ "$debris" != off ]] && envs+=("$DEBRIS_LEVER=$debris")

  # The hosted contract's keys are refused: any one of them flips the server into hosted mode (or
  # half of it), which a scripted client cannot join.
  [[ -f "$contract" ]] || die "no hosted env contract at $contract"
  local refused kv key env_json='[]'
  refused="$(jq -r 'keys[]' "$contract")"
  for kv in ${envs[@]+"${envs[@]}"}; do
    [[ "$kv" == *=* ]] || die "--env wants K=V, got '$kv'"
    key="${kv%%=*}"
    if grep -qx -- "$key" <<<"$refused"; then
      die "refusing $key: it belongs to the Worker's hosted contract ($contract)"
    fi
    env_json="$(jq -c --arg k "$key" --arg v "${kv#*=}" '. + [{key: $k, value: $v, is_hidden: false}]' <<<"$env_json")"
  done

  local tag
  tag="${TAG_PREFIX}$(date -u +%Y%m%dt%H%M%Sz)-$(od -An -N3 -tx1 /dev/urandom | tr -d ' \n')"
  load_token
  say "deploy $app/$version tag $tag env $(jq -r 'map("\(.key)=\(.value)") | join(" ")' <<<"$env_json")"
  local city body code id addr phase ip port tags where maxd elapsed
  for city in "${cities[@]}"; do
    body="$(jq -nc --arg app "$app" --arg version "$version" --arg city "$city" --arg tag "$tag" \
      --argjson env "$env_json" '{app_name: $app, version_name: $version,
        filters: [{field: "city", values: [$city], filter_type: "any"}],
        tags: [$tag], env_vars: $env}')"
    code="$(call POST /v1/deploy "$body")"
    id=""
    if [[ "$code" == 200 ]]; then
      id="$(jq -r '.request_id // empty' "$work/out")"
    elif [[ "$code" == 000 || "$code" == 408 || "$code" -ge 500 ]]; then
      # Unknown outcome: a deployment may exist. Its tag finds it.
      say "  $city: HTTP $code — looking the deployment up by its tag"
      sleep 3
      id="$(find_by_tag "$tag")"
    else
      say "  $city: refused, HTTP $code $(api_message)"
      continue
    fi
    [[ -n "$id" ]] || {
      say "  $city: no deployment"
      continue
    }
    mkdir -p "$session_dir"
    echo "$id $tag $(date -u +%FT%TZ) $city" >>"$session_dir/deployments.log"
    arm_watchdog "$id" "$max_min"
    if addr="$(wait_ready "$id" "$ready_wait")"; then
      read -r phase ip port tags where maxd elapsed <<<"$(status_fields "$id")"
      say "$id READY $addr in $where — tags $tags; Edgegap max_duration ${maxd} min (the version's); watchdog ${max_min} min"
      echo "$id"
      return 0
    fi
    say "  $city: $id never turned READY — stopping it, next city"
    stop_one "$id" || say "  WARNING: $id not confirmed stopped; its watchdog stays armed"
  done
  die "no city turned READY (${cities[*]})"
}

main() {
  case "${1:-}" in
    deploy) shift; cmd_deploy "$@" ;;
    status) shift; cmd_status "$@" ;;
    address) shift; cmd_address "$@" ;;
    logs) shift; cmd_logs "$@" ;;
    metrics) shift; cmd_metrics "$@" ;;
    stop) shift; cmd_stop "$@" ;;
    stop-all-profile) shift; cmd_stop_all_profile ;;
    list-profile) shift; cmd_list_profile ;;
    watchdog) shift; cmd_watchdog "$@" ;;
    _watchdog_loop) shift; watchdog_loop "$@" ;;
    *)
      sed -n '/^# Subcommands/,/^# THE COST GUARD/p' "$0" | sed '$d' >&2
      exit 2
      ;;
  esac
}

# One line, parsed whole before it runs: bash reads a script as it goes, so a watchdog that outlives
# an edit of this file (a checkout, a merge) must never read past this line into the new text.
main "$@"; exit $?
