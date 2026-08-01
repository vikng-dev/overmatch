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
# PLACEMENT — where the probe tanks stand relative to the camera, and therefore which side of the
# shoe LOD's 500 m swap they render on (src/track/link_view.rs). Two sweeps, same runner:
#   near (default):  scripts/perf/run-frame-sweep.sh $OUT-near
#   far:             OVERMATCH_PROBE_FAR=1 scripts/perf/run-frame-sweep.sh $OUT-far
# The flag needs no plumbing here — `env` below does not clear the inherited environment, so it
# reaches the client as-is. Each condition's own log records the placement it spawned, the grid's
# extent and its min/max distance from the controlled tank, so a capture is self-identifying:
#   grep 'spawned .* probe tanks' $OUT/*.log
# The two placements answer different questions and are NOT comparable to each other: far measures
# whether the triangle win survives the extra `check_visibility_ranges` walk, near measures what
# that walk costs when the rendered geometry is unchanged (pure overhead). Both are before/after
# comparisons against the same placement on the parent commit.
#
# Preconditions — each is a VALIDITY condition, not a nicety:
#   * machine quiet: no builds, no browser video, no other GPU load; plugged in; not thermally
#     pre-loaded (an already-hot chassis measures the throttled machine, not the game)
#   * the game window must be VISIBLE, frontmost and unoccluded for the whole sweep. A hidden or
#     fully-occluded window's present returns SurfaceError::Occluded, nothing ever vsync-blocks,
#     and the frame loop FREE-RUNS — frame times from a hidden window are fiction. The sweep
#     steals focus once per condition (window churn); leave the machine alone while it runs.
#     ENFORCED, not just documented: a real run refuses to start with SPIKE_SIM_WINDOWED
#     exported (and scrubs it from the child's env regardless), the client records every
#     window-occlusion transition into the frame stream, and the analyzer fails any condition
#     whose occluded interval overlaps the measurement window.
#   * the window must be presented on the PRIMARY (built-in, 120 Hz ProMotion) display. This Mac
#     also drives a 60 Hz external panel (ARZOPA 2560x1600), and bevy opens the game window THERE
#     whenever it is connected. macOS then paces presentation on that panel to multiples of
#     16.67 ms no matter what present mode the app negotiated: every condition quantizes to ~60 fps,
#     every rung of the ladder reads the same, and the sweep is a fiction that LOOKS clean —
#     the "effective present mode Immediate, frame cap off" gate below still PASSES, because it
#     reads the app's requested mode, not the OS's pacing of the surface. Measured on the earlier
#     shadow sweep: shadows fully OFF read 16.632 ms on the external panel and 12.057 ms on the
#     built-in, same binary, same settings.
#     ENFORCED, but NOT from this script: the CLIENT parks its own window on the primary display at
#     startup and records which monitor is presenting — name, refresh in millihertz, primary flag —
#     as `{t, monitor, refresh_mhz, primary}` rows in the frame stream, at first resolution and on
#     every change (src/frame_cost.rs). The analyzer then fails any condition whose measurement
#     window is uncovered by a monitor row, covered by one below 100000 mHz, or split across two
#     monitors. So the sweep gates on the MEASURED refresh of the surface that presented the
#     frames, which is the quantity that poisons them.
#     This replaces an osascript window-park guard that could never work: System Events reports
#     ZERO windows for this unbundled winit binary on this macOS (verified with Accessibility
#     granted and the process frontmost), so the guard's own failure mode fired on every run.
#     A guard that cannot observe the thing it guards is worse than none — it teaches the operator
#     to bypass it.
#   * warm shader cache: the FIRST launch after a rebuild stalls >10 s on Metal pipeline
#     compilation. Discard the first sweep after every rebuild (or do one throwaway run first).
#   * hands off keyboard/mouse during conditions — input changes the workload.
#
# Expected wall-clock: 6 conditions x (startup + 10 s warmup + 60 s measure + 15 s cooldown)
# ~ 9-12 min — each condition runs until its STREAM spans warmup+measure on the app's own clock
# (startup does not eat the window), bounded by STARTUP_GRACE_S.
# Products: $OUT/<cond>.client.jsonl raw frame streams, manifest.txt, analyzer table on stdout
# (re-run any time: uv run scripts/perf/analyze.py --baseline m350-2 $OUT/*.client.jsonl).
#
# Per-condition validity gates (fail LOUD, never patch over — the failure class this harness
# fights is "invalid capture masquerades as a real measurement"):
#   * the client must still be ALIVE at the scheduled cutoff — a child that died early produced
#     a partial stream, not evidence;
#   * the injected video.ron must have been loaded, and the probe tanks spawned;
#   * real mode only: the client must report `frame_cost: effective present mode Immediate,
#     frame cap off` (proof the run was truly uncapped — Settings can silently normalize an
#     unsupported vsync Off back to On, and a failed capability probe negotiates down to Fifo),
#     and neither a vsync-normalization nor a probe-failure line may appear;
#   * analyzer gates (also run per condition via --validate-only): monotonic timestamps, a row
#     SPAN of at least warmup+duration (row COUNT cannot tell a 60 s run from a fast free-runner
#     that died at 10 s), and — real mode — no occluded interval overlapping the measurement
#     window, plus one >=100000 mHz monitor covering all of it (the display precondition above).
#
# SMOKE mode (plumbing validation ONLY — hidden window, so every number is free-run garbage
# by construction; it proves rows flow and gates fire, nothing else; the output dir gets a
# SMOKE-PLUMBING-ONLY.txt marker and the manifest says the same):
#   SMOKE=1 scripts/perf/run-frame-sweep.sh /tmp/frame-sweep-smoke
# =======================================================================
#
# Usage: run-frame-sweep.sh <out-dir>
# Knobs (env): BIN (default target/release), DURATION_S (60), WARMUP_S (10), COOLDOWN_S (15),
#   MIN_ROWS (DURATION_S*20 — a real run at 60 fps writes ~60*DURATION_S), SMOKE (off),
#   STARTUP_GRACE_S (60 — how much process startup the span deadline forgives).
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
# How much process startup (asset load, Metal pipeline compilation, window creation) the span
# deadline forgives before a condition is called stalled — see the poll loop below.
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"
# There is deliberately no window-parking knob here: the client parks its own window and records
# the monitor that presented it (see the display precondition in the runbook), so the sweep's
# display gate lives in the client and the analyzer, where it can observe what it claims.

# A real sweep with the hidden-capture env exported would inherit it into every child, the window
# would never be shown, and every gate below would happily pass on free-run fiction. Refuse rather
# than silently scrub: an operator who exported it should decide whether they meant SMOKE=1.
if [[ -z "$SMOKE" && -n "${SPIKE_SIM_WINDOWED:-}" ]]; then
  echo "REFUSING real sweep: SPIKE_SIM_WINDOWED is exported — a hidden window free-runs and every" >&2
  echo "number would be fiction. unset it, or run SMOKE=1 for loudly-marked plumbing validation." >&2
  exit 1
fi

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
rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/manifest.txt "$OUT"/SMOKE-PLUMBING-ONLY.txt
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
  # The directory-level marker, so the numbers cannot later be mistaken for a measurement by
  # someone (or some agent) who finds the files without this terminal scrollback.
  cat >"$OUT/SMOKE-PLUMBING-ONLY.txt" <<'EOF'
SMOKE / PLUMBING-ONLY sweep. The window was HIDDEN (SPIKE_SIM_WINDOWED=1): the surface is
Occluded, present is discarded, and the frame loop free-runs. Every frame time in this
directory is fiction by construction. This run proves that rows flow and gates fire — nothing
else. Do not quote these numbers.
EOF
fi

# Evidence provenance: which code, which binary, which ladder, which mode produced this sweep.
{
  echo "commit: $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD 2>/dev/null || echo ' (dirty)')"
  echo "mode: $([[ -n "$SMOKE" ]] && echo 'SMOKE / PLUMBING-ONLY (hidden window — every number is free-run fiction)' || echo 'real (visible window)')"
  echo "duration_s: $DURATION_S  warmup_s: $WARMUP_S  cooldown_s: $COOLDOWN_S  min_rows: $MIN_ROWS  bin: $BIN"
  echo "presentation gates: $([[ -n "$SMOKE" ]] \
    && echo 'RELAXED — occlusion, display and effective-present-mode gates SKIPPED (hidden window by design)' \
    || echo 'occlusion-free measurement window + one >=100000 mHz monitor throughout + effective present mode Immediate, frame cap off')"
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
  # Real mode SCRUBS the hidden-capture env pair from the child even though the refusal above
  # already caught an exported SPIKE_SIM_WINDOWED — belt and suspenders, because run_offline's
  # hidden mode silently activating is exactly the free-run fiction this sweep must never emit.
  EXTRA=(-u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE)
  [[ -n "$SMOKE" ]] && EXTRA=(SPIKE_SIM_WINDOWED=1)
  env "${EXTRA[@]}" \
    OVERMATCH_CONFIG_DIR="$cfg" OVERMATCH_PROBE_TANKS="$tanks" \
    SPIKE_FRAME_COST="$OUT/$cond.jsonl" \
    BEVY_ASSET_ROOT="$REPO" \
    "$CLIENT" --offline >"$OUT/$cond.log" 2>&1 &
  CLIENT_PID=$!

  # Run the condition until the STREAM spans warmup+duration on its own clock — `t` is
  # app-elapsed time, so process startup (asset load, shader compilation, window creation; >10 s
  # cold) is invisible to a wall-clock sleep and would silently eat the measurement window. The
  # analyzer's span gate measures the stream, so the runner must feed it by the same clock.
  # STARTUP_GRACE_S bounds how long startup may take before the condition is declared stalled.
  stream="$OUT/$cond.client.jsonl"
  deadline=$((SECONDS + WARMUP_S + DURATION_S + STARTUP_GRACE_S))
  span_ok=""
  while (( SECONDS < deadline )); do
    sleep 1
    # The child must still be ALIVE while the window fills (and not a zombie — kill -0 answers
    # yes for a zombie, so ask ps for the state): a client that died mid-condition produced a
    # partial stream whose row COUNT can still clear MIN_ROWS. Early death FAILS, loudly.
    state="$(ps -o state= -p "$CLIENT_PID" 2>/dev/null || true)"
    if [[ -z "$state" || "$state" == *Z* ]]; then
      exit_status=0
      wait "$CLIENT_PID" 2>/dev/null || exit_status=$?
      CLIENT_PID=""
      echo "$cond INVALID: client exited (status $exit_status) before the stream spanned $((WARMUP_S + DURATION_S))s — early death, not evidence (see $OUT/$cond.log)" >&2
      exit 1
    fi
    [[ -f "$stream" ]] || continue
    # First/last `t` straight off the JSONL (both row shapes carry it); a torn last line simply
    # fails the extraction and this poll waits another second. The optional whitespace around the
    # colon is not pedantry: without it a writer that pretty-separates its keys yields an EMPTY
    # match here, the span never appears to advance, and the condition dies on the deadline with a
    # "never armed" message that blames the app for a bug in this line.
    t_pat='s/.*"t"[[:space:]]*:[[:space:]]*\([0-9.eE+-]*\).*/\1/p'
    t_first="$(head -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    t_last="$(tail -n1 "$stream" 2>/dev/null | sed -n "$t_pat")"
    if [[ -n "$t_first" && -n "$t_last" ]] && (( t_last - t_first >= WARMUP_S + DURATION_S )); then
      span_ok=1
      break
    fi
  done
  if [[ -z "$span_ok" ]]; then
    echo "$cond INVALID: stream never spanned $((WARMUP_S + DURATION_S))s within the $((WARMUP_S + DURATION_S + STARTUP_GRACE_S))s deadline — stalled or never armed (see $OUT/$cond.log)" >&2
    exit 1
  fi
  kill "$CLIENT_PID" 2>/dev/null || true
  wait "$CLIENT_PID" 2>/dev/null || true
  CLIENT_PID=""

  # Hard validity gates — a condition that never loaded its injected settings, never spawned its
  # tanks, ran display-paced, or produced a short/occluded stream is NOT evidence. Fail loud.
  [[ -f "$stream" ]] || { echo "$cond INVALID: no frame stream at $stream (SPIKE_FRAME_COST never armed?)" >&2; exit 1; }
  grep -q "settings: loaded $cfg/video.ron" "$OUT/$cond.log" || { echo "$cond INVALID: injected settings were not loaded (see $OUT/$cond.log)" >&2; exit 1; }
  if [[ "$tanks" -gt 2 ]]; then
    grep -q "offline: spawned $((tanks - 2)) probe tanks" "$OUT/$cond.log" || { echo "$cond INVALID: probe-tank spawn missing (see $OUT/$cond.log)" >&2; exit 1; }
  fi
  if [[ -z "$SMOKE" ]]; then
    # Uncapped-run proof: the client states the post-probe EFFECTIVE mode (src/frame_cost.rs);
    # only Immediate + frame cap off is a real free-of-display-pacing run. The settings-loaded
    # grep above only proves the file PARSED — normalize_vsync can still have resolved Off to On.
    grep -q "frame_cost: effective present mode Immediate, frame cap off" "$OUT/$cond.log" \
      || { echo "$cond INVALID: no proof the run was uncapped — expected 'frame_cost: effective present mode Immediate, frame cap off' (see $OUT/$cond.log)" >&2; exit 1; }
    if grep -qE 'settings: vsync .* is not supported by this surface' "$OUT/$cond.log"; then
      echo "$cond INVALID: vsync Off was normalized away — the run was display-paced (see $OUT/$cond.log)" >&2; exit 1
    fi
    if grep -qE 'present-mode probe: (could not create|the surface reported no present modes)' "$OUT/$cond.log"; then
      echo "$cond INVALID: the present-mode capability probe failed — AutoNoVsync may have negotiated down to Fifo (see $OUT/$cond.log)" >&2; exit 1
    fi
  fi
  # Stream-shape gates, shared with the final analysis pass (--validate-only runs the same code):
  # monotonic time, full warmup+duration SPAN, MIN_ROWS, and (real mode) no occluded interval
  # overlapping the measurement window plus a fast-enough single monitor covering all of it.
  # SMOKE relaxes both presentation gates for the same reason: its window is hidden, so it neither
  # occludes honestly nor resolves a monitor at all, and its numbers are declared fiction.
  ( cd "$REPO" && uv run scripts/perf/analyze.py --validate-only --warmup-s "$WARMUP_S" \
      --min-rows "$MIN_ROWS" --expected-duration-s "$DURATION_S" \
      ${SMOKE:+--occluded-ok --display-ok} "$stream" ) \
    || { echo "$cond INVALID: frame stream failed the analyzer's validity gates (see above)" >&2; exit 1; }
  # Presentation provenance into the manifest: the effective mode line verbatim, plus how many
  # occlusion transitions the client observed (0 on a clean visible run after the initial show),
  # plus which display presented it. The gate on it is the analyzer's, above; this is the
  # human-readable receipt.
  # The LAST monitor line, not the first: bevy creates the window before the client's startup park
  # runs, so on a machine with the 60 Hz panel attached the FIRST line routinely names that panel
  # and would make a perfectly valid capture read as a poisoned one. Both lines stay in the log,
  # and the row count below says whether there was more than one.
  monitor_line="$(grep -o 'frame_cost: presenting on monitor.*' "$OUT/$cond.log" | tail -1)"
  {
    echo "$cond: $(grep -m1 -o 'frame_cost: effective present mode.*' "$OUT/$cond.log" || echo 'effective present mode NOT REPORTED')"
    echo "$cond: occlusion transitions in stream: $(grep -c '"occluded"' "$stream" || true)"
    echo "$cond: ${monitor_line:-presenting monitor NOT REPORTED} <- in force at the end"
    echo "$cond: monitor rows in stream: $(grep -c '"monitor"' "$stream" || true) (1 = never moved; 2 is normally the startup park off the external panel, inside warmup — the analyzer gates on the state in force once warmup ends)"
  } >>"$OUT/manifest.txt"
  echo "   $cond OK: $(wc -l <"$stream" | tr -d ' ') rows (frame + occlusion + monitor)"

  sleep "$COOLDOWN_S"
done

cd "$REPO"
uv run scripts/perf/analyze.py --warmup-s "$WARMUP_S" --min-rows "$MIN_ROWS" \
  --expected-duration-s "$DURATION_S" ${SMOKE:+--occluded-ok --display-ok} \
  --baseline "${CONDITIONS[1]%%:*}" "$OUT"/*.client.jsonl

echo "sweep complete -> $OUT"
