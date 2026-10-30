#!/bin/zsh
# clientless-chain.sh <out-dir> <edgegap-version> <lever>... — the server-only debris curve on
# Edgegap: per lever one deployment (29 bots, stats lever 1 s, the debris lever), no client at all,
# so it measures the simulation without any link (no replication send). WARMUP_S then DURATION_S of
# wall time after READY; then the container log (retried on a refusal), the metrics, the stop. The
# THREADS sets the server's thread lever for every deployment. The
# window's marker is the stats line WARMUP_S lines in, so regress.py and online_cost.py cut the
# window the same way as for a runner's condition. It never sweeps other deployments: it stops only
# the ids it made (its EXIT trap included), so a chain beside it is left alone.
set -u
here="${0:A:h}"
S="$here/session-retry.sh"
REPO=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3
OUT="$1"; VERSION="$2"; shift 2
WARMUP_S="${WARMUP_S:-45}"; DURATION_S="${DURATION_S:-180}"
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
for lever in "$@"; do
  n=$((n + 1))
  name="$(printf 'cl%02d-%s' "$n" "${lever//:/x}")${THREADS:+-t$THREADS}"
  dir="$OUT/$name"
  mkdir -p "$dir"
  echo "== $name $(date +%T): deploying $VERSION (bots 29, debris $lever, no client)" >&2
  targs=()
  [[ -n "${THREADS:-}" ]] && targs=(--threads "$THREADS")
  if ! id="$("$S" deploy "$VERSION" --bots 29 --stats-s 1 --debris "$lever" "${targs[@]}" --max-min 15)"; then
    echo "$name: no deployment turned READY" >&2
    continue
  fi
  LIVE="$id"
  printf '%s\t%s\t%s\n' "$id" "$lever" "$("$S" status "$id")" >>"$OUT/deployments.tsv"
  sleep $((WARMUP_S + DURATION_S))
  "$S" logs "$id" "$dir/$name.server.log" >/dev/null || echo "$name: no log" >&2
  "$S" metrics "$id" "$dir/metrics.json" >/dev/null || echo "$name: no metrics" >&2
  "$S" stop "$id" && LIVE=""
  if [[ -f "$dir/$name.server.log" ]]; then
    grep 'server_stats' "$dir/$name.server.log" | sed -n "${WARMUP_S}p" >"$dir/$name.marker"
    (cd "$REPO" && uv run scripts/perf/online_cost.py server-window --label "$name" \
      --after-marker "$dir/$name.marker" --json "$dir/$name.server.json" "$dir/$name.server.log")
    printf '%s\t%s\t%s\n' "$name" "$lever" "$name/$name.server.json" >>"$OUT/conditions.tsv"
  fi
done
echo "clientless chain complete -> $OUT" >&2
