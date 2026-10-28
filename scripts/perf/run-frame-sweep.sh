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
# Overnight runs: `caffeinate -u` does NOT reset the screensaver timer, and backlight 0 means
# the window is occluded — either way frames are fiction. Keep the display on and lit.
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
#   * the surface must be 2560x1440 PHYSICAL pixels — the second, quieter half of the same story.
#     The two panels have different scale factors (built-in 2.0, ARZOPA 1.0) and bevy's window is
#     1280x720 LOGICAL, so the very same window is a 2560x1440 surface on the built-in panel and a
#     1280x720 one on the external: a QUARTER of the shaded pixels, for a ladder that reads
#     uniformly ~3x too fast. MEASURED with the refresh gate above already passing: m350-2 read
#     4.19 ms against 11.83 ms for identical code captured on the built-in panel. Frame cost is
#     proportional to pixels, so this is not a noisier measurement of the same thing — it is a
#     different machine, and no ladder captured at one size may be compared with one captured at
#     the other.
#     ENFORCED the same way and in the same places: the client pins its window to 1280x720 logical
#     at startup and re-pins whenever the scale factor changes (a monitor move updates the factor
#     without necessarily resizing the surface, which silently halves the logical size), records
#     `{t, surface_w, surface_h}` rows, and the analyzer fails any condition whose measurement
#     window is uncovered by a surface row or covered by one that is not exactly 2560x1440.
#     Cross-check in the client's own log: `render scale: native — the 3D main pass fills the
#     [2560, 1440] target`.
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
#     window, plus one >=100000 mHz monitor and one 2560x1440 surface covering all of it (the
#     display and surface preconditions above).
#
# SMOKE mode (plumbing validation ONLY — hidden window, so every number is free-run garbage
# by construction; it proves rows flow and gates fire, nothing else; the output dir gets a
# SMOKE-PLUMBING-ONLY.txt marker and the manifest says the same):
#   SMOKE=1 scripts/perf/run-frame-sweep.sh /tmp/frame-sweep-smoke
# =======================================================================
#
# The gates and SMOKE mode above are code in scripts/perf/frame-run.zsh, which this runner shares
# with run-track-prints-cost.sh and run-forest-cost.sh; their story stays here.
#
# Usage: run-frame-sweep.sh <out-dir>
# Knobs (env): BIN (directory holding `overmatch`, absolute or relative to the repo; default
#   target/release), ASSET_ROOT (directory holding `assets/`; default the repo), DURATION_S (60),
#   WARMUP_S (10), COOLDOWN_S (15), MIN_ROWS (DURATION_S*20 — a real run at 60 fps writes
#   ~60*DURATION_S), SMOKE (off), STARTUP_GRACE_S (60 — how much process startup the span deadline
#   forgives).
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
[[ "$BIN" == /* ]] || BIN="$REPO/$BIN"
ASSET_ROOT="${ASSET_ROOT:-$REPO}"
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
# deadline forgives before a condition is called stalled.
STARTUP_GRACE_S="${STARTUP_GRACE_S:-60}"
# There is deliberately no window-parking knob here: the client parks its own window and records
# the monitor that presented it (see the display precondition in the runbook), so the sweep's
# display gate lives in the client and the analyzer, where it can observe what it claims.

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

source "$REPO/scripts/perf/frame-run.zsh"
frame_run_begin

# The sweep's own gate: a condition that never spawned its probe tanks measured the duel alone.
condition_gates() {
  if [[ "$tanks" -gt 2 ]]; then
    grep -q "offline: spawned $((tanks - 2)) probe tanks" "$2" \
      || frame_run_invalid "$1" "probe-tank spawn missing (see $2)"
  fi
}

for entry in "${CONDITIONS[@]}"; do
  cond="${entry%%:*}"
  rest="${entry#*:}"
  shadow="${rest%%:*}"
  tanks="${rest#*:}"
  frame_run_condition "$cond" "shadow_distance: $shadow," OVERMATCH_PROBE_TANKS="$tanks"
done

frame_run_analyze "${CONDITIONS[1]%%:*}" "$OUT"/*.client.jsonl

echo "sweep complete -> $OUT"
