#!/bin/zsh
# =============================== RUNBOOK ===============================
# The 30-tank frame-budget sweep: is 15v15 (30 tanks) inside the frame budget on this machine,
# and what does the shadow-distance ladder buy — the evidence behind the ShadowDistance::Off
# product question. Offline single-player path, no server.
#
# REAL sweep (the measured evidence run):
#   cargo build --locked --release --bin overmatch
#   scripts/perf/run-frame-sweep.sh /tmp/frame-sweep-$(date +%Y%m%d)
#
# Preconditions — each is a VALIDITY condition, not a nicety:
#   * machine quiet: no builds, no browser video, no other GPU load; plugged in; not thermally
#     pre-loaded (an already-hot chassis measures the throttled machine, not the game)
#   * the game window must be VISIBLE, frontmost and unoccluded for the whole sweep. A hidden or
#     fully-occluded window's present returns SurfaceError::Occluded, nothing ever vsync-blocks,
#     and the frame loop FREE-RUNS — frame times from a hidden window are fiction. The sweep
#     steals focus once per condition (window churn); leave the machine alone while it runs.
#   * warm shader cache: the FIRST launch after a rebuild stalls >10 s on Metal pipeline
#     compilation. Discard the first sweep after every rebuild (or do one throwaway run first).
#   * hands off keyboard/mouse during conditions — input changes the workload.
#
# Expected wall-clock: 6 conditions x (10 s warmup + 60 s measure + 15 s cooldown) ~ 9 min.
# Products: $OUT/<cond>.client.jsonl raw frame streams, manifest.txt, analyzer table on stdout
# (re-run any time: uv run scripts/perf/analyze.py --baseline m350-2 $OUT/*.client.jsonl).
#
# SMOKE mode (plumbing validation ONLY — hidden window, so every number is free-run garbage
# by construction; it proves rows flow and gates fire, nothing else):
#   SMOKE=1 scripts/perf/run-frame-sweep.sh /tmp/frame-sweep-smoke
# =======================================================================
#
# Usage: run-frame-sweep.sh <out-dir>
# Knobs (env): BIN (default target/release), DURATION_S (60), WARMUP_S (10), COOLDOWN_S (15),
#   MIN_ROWS (DURATION_S*20 — a real run at 60 fps writes ~60*DURATION_S), SMOKE (off).
# Per-run graphics settings are injected via OVERMATCH_CONFIG_DIR: each condition gets its own
# scratch config dir with a generated video.ron (vsync_mode: Off so frames are never
# refresh-clamped), so the sweep never touches the developer's real settings. Tank count rides
# the existing OVERMATCH_PROBE_TANKS lever (total tanks; the 2 duel Tigers + N-2 idle probes on
# the valley floor — see src/tank/scenario.rs).

set -eu
setopt null_glob

OUT="${1:?usage: run-frame-sweep.sh <out-dir>}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/release}"
CLIENT="$REPO/$BIN/overmatch"
SMOKE="${SMOKE:-}"
if [[ -n "$SMOKE" ]]; then
  DURATION_S="${DURATION_S:-15}"
  WARMUP_S="${WARMUP_S:-5}"
  COOLDOWN_S="${COOLDOWN_S:-2}"
else
  DURATION_S="${DURATION_S:-60}"
  WARMUP_S="${WARMUP_S:-10}"
  COOLDOWN_S="${COOLDOWN_S:-15}"
fi
MIN_ROWS="${MIN_ROWS:-$((DURATION_S * 20))}"

[[ -x "$CLIENT" ]] || { echo "no client binary at $CLIENT — build it first (see runbook)" >&2; exit 1; }

# The ladder: shadow-distance rungs (Off / shipped default / whole-map) x tank counts (duel / 15v15).
# name:shadow_distance:total_tanks — names are the analyzer's condition labels.
CONDITIONS=(
  "m350-2:M350:2"
  "m350-30:M350:30"
  "off-2:Off:2"
  "off-30:Off:30"
  "m1000-2:M1000:2"
  "m1000-30:M1000:30"
)
if [[ -n "$SMOKE" ]]; then
  # Two short hidden conditions: enough to prove config injection, the probe lever and the gates.
  CONDITIONS=("m350-5:M350:5" "off-5:Off:5")
fi

mkdir -p "$OUT"
rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/manifest.txt
rm -rf "$OUT"/cfg-*

CLIENT_PID=""
cleanup() {
  [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
  [[ -n "$CLIENT_PID" ]] && wait "$CLIENT_PID" 2>/dev/null || true
}
# A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit so the
# EXIT trap does the cleanup exactly once and the script actually terminates.
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ -n "$SMOKE" ]]; then
  echo "SMOKE MODE: hidden window (SPIKE_SIM_WINDOWED=1) — the surface is Occluded, the loop" >&2
  echo "free-runs, and every frame time below is FICTION. Plumbing validation only." >&2
fi

# Evidence provenance: which code, which binary, which ladder, which mode produced this sweep.
{
  echo "commit: $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD 2>/dev/null || echo ' (dirty)')"
  echo "mode: $([[ -n "$SMOKE" ]] && echo 'SMOKE (hidden window — numbers are fiction)' || echo 'real (visible window)')"
  echo "duration_s: $DURATION_S  warmup_s: $WARMUP_S  cooldown_s: $COOLDOWN_S  min_rows: $MIN_ROWS  bin: $BIN"
  echo "conditions: ${CONDITIONS[*]}"
  echo "binary: $CLIENT  $(stat -f 'mtime=%Sm size=%z' "$CLIENT")"
  echo "host: $(uname -m) $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
} >"$OUT/manifest.txt"

for entry in "${CONDITIONS[@]}"; do
  cond="${entry%%:*}"
  rest="${entry#*:}"
  shadow="${rest%%:*}"
  tanks="${rest#*:}"

  # Per-condition settings injection: an isolated config dir with a generated video.ron.
  # vsync_mode: Off (+ the frame-cap default of 0 = uncapped) so frame times are the workload's,
  # not the display refresh; serde treats the omitted fields as defaults.
  cfg="$OUT/cfg-$cond"
  mkdir -p "$cfg"
  printf '(\n    version: 1,\n    shadow_distance: %s,\n    vsync_mode: Off,\n)\n' "$shadow" >"$cfg/video.ron"

  echo "== $cond: shadow_distance=$shadow tanks=$tanks (${WARMUP_S}s warmup + ${DURATION_S}s measure) =="
  EXTRA=()
  [[ -n "$SMOKE" ]] && EXTRA=(SPIKE_SIM_WINDOWED=1)
  env "${EXTRA[@]}" \
    OVERMATCH_CONFIG_DIR="$cfg" OVERMATCH_PROBE_TANKS="$tanks" \
    SPIKE_FRAME_COST="$OUT/$cond.jsonl" \
    BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" --offline >"$OUT/$cond.log" 2>&1 &
  CLIENT_PID=$!

  sleep "$((WARMUP_S + DURATION_S))"
  kill "$CLIENT_PID" 2>/dev/null || true
  wait "$CLIENT_PID" 2>/dev/null || true
  CLIENT_PID=""

  # Hard validity gates — a condition that never loaded its injected settings, never spawned its
  # tanks, or produced a short stream is NOT evidence. Fail loud, never patch over.
  stream="$OUT/$cond.client.jsonl"
  [[ -f "$stream" ]] || { echo "$cond INVALID: no frame stream at $stream (SPIKE_FRAME_COST never armed?)" >&2; exit 1; }
  rows=$(wc -l <"$stream" | tr -d ' ')
  [[ "$rows" -ge "$MIN_ROWS" ]] || { echo "$cond INVALID: $rows frame rows < required $MIN_ROWS (crashed / stalled / killed early?)" >&2; exit 1; }
  grep -q "settings: loaded $cfg/video.ron" "$OUT/$cond.log" || { echo "$cond INVALID: injected settings were not loaded (see $OUT/$cond.log)" >&2; exit 1; }
  if [[ "$tanks" -gt 2 ]]; then
    grep -q "offline: spawned $((tanks - 2)) probe tanks" "$OUT/$cond.log" || { echo "$cond INVALID: probe-tank spawn missing (see $OUT/$cond.log)" >&2; exit 1; }
  fi
  echo "   $cond OK: $rows frame rows"

  sleep "$COOLDOWN_S"
done

cd "$REPO"
uv run scripts/perf/analyze.py --warmup-s "$WARMUP_S" --min-rows "$MIN_ROWS" \
  --baseline "${CONDITIONS[1]%%:*}" "$OUT"/*.client.jsonl

echo "sweep complete -> $OUT"
