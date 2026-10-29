# Sourced, never run: the frame-cost runners' one launch-and-gate loop, shared by
# run-frame-sweep.sh, run-track-prints-cost.sh and run-forest-cost.sh. Each runner states its own
# question, scene and conditions; what makes ANY condition evidence is this code, and its story —
# the preconditions (visible frontmost window, primary 120 Hz display, 2560x1440 surface, warm
# shader cache, quiet machine, hands off), every gate below and SMOKE mode — is told once, in
# run-frame-sweep.sh's runbook.
#
# The runner sets, before `frame_run_begin`: OUT, REPO, BIN (the directory holding `overmatch`),
# ASSET_ROOT (the directory holding `assets/`), SMOKE, WARMUP_S, DURATION_S, COOLDOWN_S, MIN_ROWS,
# STARTUP_GRACE_S, and the CONDITIONS array (recorded in the manifest). It may define
# `condition_gates <cond> <log> <stream>`: its own gates, run after the shared ones and before the
# condition's receipt, failing through `frame_run_invalid`.

# The analyzer's presentation-gate relaxations, as an ARRAY. zsh does not word-split an unquoted
# parameter expansion, so the obvious `${SMOKE:+--occluded-ok --display-ok --surface-ok}` reaches
# argparse as ONE argument and every smoke run dies on "--occluded-ok --display-ok --surface-ok
# does not exist". Real mode never noticed: there the expansion is empty.
SMOKE_RELAX=()
[[ -n "$SMOKE" ]] && SMOKE_RELAX=(--occluded-ok --display-ok --surface-ok)
CLIENT="$BIN/overmatch"
CLIENT_PID=""

frame_run_invalid() {
  echo "$1 INVALID: $2" >&2
  exit 1
}

# The first `t` of a frame stream, straight off the JSONL (every row shape carries it). The optional
# whitespace around the colon is not pedantry: without it a writer that pretty-separates its keys
# yields an EMPTY match, the span never appears to advance, and the condition dies on the deadline
# with a "never armed" message that blames the app for a bug in this line.
FRAME_RUN_T_PAT='s/.*"t"[[:space:]]*:[[:space:]]*\([0-9.eE+-]*\).*/\1/p'
frame_run_t_first() { head -n1 "$1" 2>/dev/null | sed -n "$FRAME_RUN_T_PAT"; }
frame_run_t_last() { tail -n1 "$1" 2>/dev/null | sed -n "$FRAME_RUN_T_PAT"; }

frame_run_cleanup() {
  [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
  [[ -n "$CLIENT_PID" ]] && wait "$CLIENT_PID" 2>/dev/null || true
}
# Set here, at the sourcing script's top level: zsh scopes an EXIT trap set inside a function to
# that function. A signal trap that RETURNS resumes the script in zsh — route INT/TERM through exit
# so the EXIT trap does the cleanup exactly once and the script actually terminates.
trap frame_run_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Refusals, the out dir, the traps, the smoke marker and the manifest's provenance header.
frame_run_begin() {
  # A real run with the hidden-capture env exported would inherit it into every child, the window
  # would never be shown, and every gate below would happily pass on free-run fiction. Refuse rather
  # than silently scrub: an operator who exported it should decide whether they meant SMOKE=1.
  if [[ -z "$SMOKE" && -n "${SPIKE_SIM_WINDOWED:-}" ]]; then
    echo "REFUSING real run: SPIKE_SIM_WINDOWED is exported — a hidden window free-runs and every" >&2
    echo "number would be fiction. unset it, or run SMOKE=1 for loudly-marked plumbing validation." >&2
    exit 1
  fi
  [[ -x "$CLIENT" ]] || { echo "no client binary at $CLIENT — build it first (see the runbook)" >&2; exit 1; }
  [[ -d "$ASSET_ROOT/assets" ]] || { echo "no assets/ under ASSET_ROOT=$ASSET_ROOT" >&2; exit 1; }

  mkdir -p "$OUT"
  rm -f "$OUT"/*.jsonl "$OUT"/*.log "$OUT"/manifest.txt "$OUT"/SMOKE-PLUMBING-ONLY.txt
  rm -rf "$OUT"/cfg-*

  if [[ -n "$SMOKE" ]]; then
    echo "SMOKE MODE: hidden window (SPIKE_SIM_WINDOWED=1) — the surface is Occluded, the loop" >&2
    echo "free-runs, and every frame time below is FICTION. Plumbing validation only." >&2
    # The directory-level marker, so the numbers cannot later be mistaken for a measurement by
    # someone (or some agent) who finds the files without this terminal scrollback.
    cat >"$OUT/SMOKE-PLUMBING-ONLY.txt" <<'EOF'
SMOKE / PLUMBING-ONLY run. The window was HIDDEN (SPIKE_SIM_WINDOWED=1): the surface is
Occluded, present is discarded, and the frame loop free-runs. Every frame time in this
directory is fiction by construction. This run proves that rows flow and gates fire — nothing
else. Do not quote these numbers.
EOF
  fi

  # Evidence provenance: which code, which binary, which conditions, which mode produced this run.
  # The checkout's commit names the runner; a binary kept outside the checkout names its own source
  # in a BUILD.txt beside it.
  {
    echo "runner commit: $(git -C "$REPO" rev-parse HEAD)$(git -C "$REPO" diff --quiet HEAD 2>/dev/null || echo ' (dirty)')"
    [[ -f "$BIN/BUILD.txt" ]] && sed 's/^/binary build: /' "$BIN/BUILD.txt"
    echo "mode: $([[ -n "$SMOKE" ]] && echo 'SMOKE / PLUMBING-ONLY (hidden window — every number is free-run fiction)' || echo 'real (visible window)')"
    echo "duration_s: $DURATION_S  warmup_s: $WARMUP_S  cooldown_s: $COOLDOWN_S  min_rows: $MIN_ROWS  bin: $BIN  asset_root: $ASSET_ROOT"
    echo "presentation gates: $([[ -n "$SMOKE" ]] \
      && echo 'RELAXED — occlusion, display, surface and effective-present-mode gates SKIPPED (hidden window by design)' \
      || echo 'occlusion-free measurement window + one >=100000 mHz monitor + a 2560x1440 surface throughout + effective present mode Immediate, frame cap off')"
    echo "conditions: ${CONDITIONS[*]}"
    echo "binary: $CLIENT  $(stat -f 'mtime=%Sm size=%z' "$CLIENT")"
    echo "host: $(uname -m) $(sysctl -n machdep.cpu.brand_string 2>/dev/null || uname -p)"
  } >"$OUT/manifest.txt"
}

# One condition: `frame_run_condition <cond> <video fields> [VAR=value ...]`. The video fields are
# RON struct fields, comma-terminated (`shadow_distance: M350,`), written into the condition's own
# video.ron beside `vsync_mode: Off` (with the frame-cap default of 0, uncapped) so frame times are
# the workload's, not the display refresh's; serde treats the omitted fields as defaults. The
# assignments reach the client's environment after the shared ones, so a condition can override
# BEVY_ASSET_ROOT.
frame_run_condition() {
  local cond="$1" fields="$2"
  shift 2
  local cfg="$OUT/cfg-$cond"
  local stream="$OUT/$cond.client.jsonl"
  local log="$OUT/$cond.log"
  mkdir -p "$cfg"
  printf '(\n    version: 1,\n    %s\n    vsync_mode: Off,\n)\n' "$fields" >"$cfg/video.ron"

  echo "== $cond: ${fields} $* (${WARMUP_S}s warmup + ${DURATION_S}s measure) =="
  # Real mode SCRUBS the hidden-capture env pair from the child even though the refusal already
  # caught an exported SPIKE_SIM_WINDOWED — belt and suspenders, because run_offline's hidden mode
  # silently activating is exactly the free-run fiction a capture must never emit.
  local extra=(-u SPIKE_SIM_WINDOWED -u SPIKE_SIM_VISIBLE)
  [[ -n "$SMOKE" ]] && extra=(SPIKE_SIM_WINDOWED=1)
  env "${extra[@]}" \
    OVERMATCH_CONFIG_DIR="$cfg" SPIKE_FRAME_COST="$OUT/$cond.jsonl" BEVY_ASSET_ROOT="$ASSET_ROOT" \
    "$@" "$CLIENT" --offline >"$log" 2>&1 &
  CLIENT_PID=$!

  # Run the condition until the STREAM spans warmup+duration on its own clock — `t` is
  # app-elapsed time, so process startup (asset load, shader compilation, window creation; >10 s
  # cold) is invisible to a wall-clock sleep and would silently eat the measurement window. The
  # analyzer's span gate measures the stream, so the runner must feed it by the same clock.
  # STARTUP_GRACE_S bounds how long startup may take before the condition is declared stalled.
  local deadline=$((SECONDS + WARMUP_S + DURATION_S + STARTUP_GRACE_S))
  local span_ok="" state t_first t_last exit_status
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
      frame_run_invalid "$cond" "client exited (status $exit_status) before the stream spanned $((WARMUP_S + DURATION_S))s — early death, not evidence (see $log)"
    fi
    [[ -f "$stream" ]] || continue
    # A torn last line simply fails the extraction and this poll waits another second.
    t_first="$(frame_run_t_first "$stream")"
    t_last="$(frame_run_t_last "$stream")"
    if [[ -n "$t_first" && -n "$t_last" ]] && (( t_last - t_first >= WARMUP_S + DURATION_S )); then
      span_ok=1
      break
    fi
  done
  [[ -n "$span_ok" ]] || frame_run_invalid "$cond" "stream never spanned $((WARMUP_S + DURATION_S))s within the $((WARMUP_S + DURATION_S + STARTUP_GRACE_S))s deadline — stalled or never armed (see $log)"
  kill "$CLIENT_PID" 2>/dev/null || true
  wait "$CLIENT_PID" 2>/dev/null || true
  CLIENT_PID=""

  # Hard validity gates — a condition that never loaded its injected settings, ran display-paced,
  # or produced a short/occluded stream is NOT evidence. Fail loud.
  [[ -f "$stream" ]] || frame_run_invalid "$cond" "no frame stream at $stream (SPIKE_FRAME_COST never armed?)"
  grep -q "settings: loaded $cfg/video.ron" "$log" || frame_run_invalid "$cond" "injected settings were not loaded (see $log)"
  if [[ -z "$SMOKE" ]]; then
    # Uncapped-run proof: the client states the post-probe EFFECTIVE mode (src/frame_cost.rs);
    # only Immediate + frame cap off is a real free-of-display-pacing run. The settings-loaded
    # grep above only proves the file PARSED — normalize_vsync can still have resolved Off to On.
    grep -q "frame_cost: effective present mode Immediate, frame cap off" "$log" \
      || frame_run_invalid "$cond" "no proof the run was uncapped — expected 'frame_cost: effective present mode Immediate, frame cap off' (see $log)"
    if grep -qE 'settings: vsync .* is not supported by this surface' "$log"; then
      frame_run_invalid "$cond" "vsync Off was normalized away — the run was display-paced (see $log)"
    fi
    if grep -qE 'present-mode probe: (could not create|the surface reported no present modes)' "$log"; then
      frame_run_invalid "$cond" "the present-mode capability probe failed — AutoNoVsync may have negotiated down to Fifo (see $log)"
    fi
  fi
  # Stream-shape gates, shared with the final analysis pass (--validate-only runs the same code):
  # monotonic time, full warmup+duration SPAN, MIN_ROWS, and (real mode) no occluded interval
  # overlapping the measurement window, plus a fast-enough single monitor and a full-size surface
  # covering all of it. SMOKE relaxes all three presentation gates: its window is hidden, so it
  # neither occludes honestly nor resolves a monitor or a real surface, and its numbers are
  # declared fiction.
  ( cd "$REPO" && uv run scripts/perf/analyze.py --validate-only --warmup-s "$WARMUP_S" \
      --min-rows "$MIN_ROWS" --expected-duration-s "$DURATION_S" \
      "${SMOKE_RELAX[@]}" "$stream" ) \
    || frame_run_invalid "$cond" "frame stream failed the analyzer's validity gates (see above)"
  if (( $+functions[condition_gates] )); then
    condition_gates "$cond" "$log" "$stream"
  fi
  # A window that is never shown (behind a locked screen, for one) sends no occlusion message and its
  # 3D camera never draws, so every stream gate above passes on a frame loop that renders nothing.
  # The renderer's own `render scale:` line and an observed `occluded=false` prove the camera drew.
  if [[ -z "${SMOKE:-}" ]]; then
    grep -q 'render scale: ' "$log" \
      || frame_run_invalid "$cond" "the 3D camera never rendered (no 'render scale:' line); was the window ever shown? (see $log)"
    grep -q 'occluded=false' "$log" \
      || frame_run_invalid "$cond" "the window never reported itself visible (no 'occluded=false') (see $log)"
  fi

  # Presentation provenance into the manifest: the effective mode line verbatim, plus how many
  # occlusion transitions the client observed (0 on a clean visible run after the initial show),
  # plus which display presented it. The gate on it is the analyzer's, above; this is the
  # human-readable receipt.
  # The LAST monitor line, not the first: bevy creates the window before the client's startup park
  # runs, so on a machine with the 60 Hz panel attached the FIRST line routinely names that panel
  # and would make a perfectly valid capture read as a poisoned one. Both lines stay in the log,
  # and the row count below says whether there was more than one.
  local monitor_line surface_line scale_line
  monitor_line="$(grep -o 'frame_cost: presenting on monitor.*' "$log" | tail -1)"
  # Same tail-not-head rule for the surface, and for the same reason: the first line is the size the
  # window was born at on whichever panel bevy opened it on. The render-scale line is the renderer's
  # independent word for the same number — a cross-check from the other side of the pipeline.
  surface_line="$(grep -o 'frame_cost: surface .*' "$log" | tail -1)"
  scale_line="$(grep -o 'render scale: .*' "$log" | tail -1)"
  {
    echo "$cond: $(grep -m1 -o 'frame_cost: effective present mode.*' "$log" || echo 'effective present mode NOT REPORTED')"
    echo "$cond: occlusion transitions in stream: $(grep -c '"occluded"' "$stream" || true)"
    echo "$cond: ${monitor_line:-presenting monitor NOT REPORTED} <- in force at the end"
    echo "$cond: monitor rows in stream: $(grep -c '"monitor"' "$stream" || true) (1 = never moved; 2 is normally the startup park off the external panel, inside warmup — the analyzer gates on the state in force once warmup ends)"
    echo "$cond: ${surface_line:-surface size NOT REPORTED} <- in force at the end"
    echo "$cond: ${scale_line:-render scale NOT REPORTED} <- the renderer's own word for the same size"
    echo "$cond: surface rows in stream: $(grep -c '"surface_w"' "$stream" || true) (1 = never resized; 2 is normally the startup re-pin after the park changed the scale factor)"
  } >>"$OUT/manifest.txt"
  echo "   $cond OK: $(wc -l <"$stream" | tr -d ' ') rows (frame + occlusion + monitor + surface)"

  sleep "$COOLDOWN_S"
}

# The tables: `frame_run_analyze <baseline> <stream ...>`, the same gates as each condition's.
frame_run_analyze() {
  local baseline="$1"
  shift
  ( cd "$REPO" && uv run scripts/perf/analyze.py --warmup-s "$WARMUP_S" --min-rows "$MIN_ROWS" \
      --expected-duration-s "$DURATION_S" "${SMOKE_RELAX[@]}" --baseline "$baseline" "$@" )
}
