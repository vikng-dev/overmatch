#!/bin/zsh
# clientless-ab.sh <out-dir> <version>... — a client-less release A/B on Edgegap: per version one
# deployment, BOTS bots (default 30: 30 tanks without a client), the debris lever off, THREADS for
# the thread lever when set, WARMUP_S then DURATION_S of wall time after READY, then the container
# log (retried on a refusal), the metrics, the stop. The marker is the stats line WARMUP_S lines in.
# Stops only the ids it made, never sweeps.
set -u
here="${0:A:h}"
S="$here/session-retry.sh"
REPO=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3
OUT="$1"; shift
WARMUP_S="${WARMUP_S:-45}"; DURATION_S="${DURATION_S:-180}"; BOTS="${BOTS:-30}"
mkdir -p "$OUT"
: >"$OUT/conditions.tsv"
: >"$OUT/deployments.tsv"
LIVE=""
on_exit() {
  if [[ -n "$LIVE" ]]; then "$S" logs "$LIVE" "$OUT/$LIVE.at-exit.server.log" >/dev/null; "$S" stop "$LIVE"; fi
}
trap on_exit EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
caffeinate -ims -w $$ </dev/null >/dev/null 2>&1 &
n=0
for version in "$@"; do
  n=$((n + 1))
  name="$(printf 'ab%02d-%s' "$n" "$version")${THREADS:+-t$THREADS}"
  dir="$OUT/$name"
  mkdir -p "$dir"
  echo "== $name $(date +%T): deploying $version (bots $BOTS, debris off, no client)" >&2
  targs=()
  [[ -n "${THREADS:-}" ]] && targs=(--threads "$THREADS")
  if ! id="$("$S" deploy "$version" --bots "$BOTS" --stats-s 1 --debris off "${targs[@]}" --max-min 15)"; then
    echo "$name: no deployment turned READY" >&2
    continue
  fi
  LIVE="$id"
  printf '%s\t%s\t%s\n' "$id" "$version" "$("$S" status "$id")" >>"$OUT/deployments.tsv"
  sleep $((WARMUP_S + DURATION_S))
  "$S" logs "$id" "$dir/$name.server.log" >/dev/null || echo "$name: no log" >&2
  "$S" metrics "$id" "$dir/metrics.json" >/dev/null || echo "$name: no metrics" >&2
  "$S" stop "$id" && LIVE=""
  if [[ -f "$dir/$name.server.log" ]]; then
    grep 'server_stats' "$dir/$name.server.log" | sed -n "${WARMUP_S}p" >"$dir/$name.marker"
    (cd "$REPO" && uv run scripts/perf/online_cost.py server-window --label "$name" \
      --after-marker "$dir/$name.marker" --json "$dir/$name.server.json" "$dir/$name.server.log")
    printf '%s\t%s\t%s\n' "$name" "$version" "$name/$name.server.json" >>"$OUT/conditions.tsv"
  fi
done
echo "clientless A/B complete -> $OUT" >&2
