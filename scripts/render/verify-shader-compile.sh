#!/bin/zsh
# =============================== RUNBOOK ===============================
# verify-shader-compile.sh — do this tree's shaders COMPILE on a real device?
#
# A WGSL file that parses is not a WGSL file that runs. Everything a shader can get wrong against
# the pipeline it is bound into — a binding the layout does not carry, a texture declared at the
# wrong dimension, a `textureSample` under non-uniform control flow, a struct whose layout does not
# match the uniform — is caught by wgpu at `create_render_pipeline`, on a GPU, at run time. Bevy
# logs that as a rendering error and quits; nothing in `cargo test` can see it.
#
# So this boots the real client against the real map on the real adapter, and fails on any ERROR.
# The window is HIDDEN (`SPIKE_SIM_WINDOWED=1`, no `SPIKE_SIM_VISIBLE`): frame times from a hidden
# window are fiction, but pipeline creation is not — the pipelines are created, validated and
# compiled exactly as they are for a visible one. NEVER read a frame number out of this.
#
#   usage: scripts/render/verify-shader-compile.sh [seconds]      (default 30)
#   needs: cargo build --locked --bin overmatch    (any profile; $BIN selects, default target/debug)
#
# THE MUTANT. A gate that cannot fail is not a gate. To prove this one bites, break a shader in a
# way that PARSES — the sharpest is a dimension lie, because WGSL alone cannot see it:
#
#   assets/shaders/terrain_blend.wgsl:  texture_2d_array<f32>  ->  texture_2d<f32>
#     (and drop the array index from that texture's textureSampleGrad call)
#
# and re-run. It must report FAILED with `Shader global ResourceBinding { group: 3, binding: 101 }
# is not available in the pipeline layout`. Restore the file afterwards.
# =======================================================================
set -eu

SECONDS_TO_RUN="${1:-30}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"
BIN="${BIN:-target/debug}"
CLIENT="$REPO/$BIN/overmatch"
LOG="$(mktemp -t overmatch-shader-compile)"
# An isolated config dir: this must never read or write the developer's own settings.
CONFIG="$(mktemp -d -t overmatch-shader-cfg)"

[[ -x "$CLIENT" ]] || {
  echo "no client binary at $CLIENT — cargo build --locked --bin overmatch" >&2
  exit 1
}

CLIENT_PID=""
cleanup() {
  [[ -n "$CLIENT_PID" ]] && kill "$CLIENT_PID" 2>/dev/null || true
  [[ -n "$CLIENT_PID" ]] && wait "$CLIENT_PID" 2>/dev/null || true
  rm -rf "$CONFIG"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

echo "== booting $CLIENT hidden for ${SECONDS_TO_RUN}s (log: $LOG)"
SPIKE_SIM_WINDOWED=1 BEVY_ASSET_ROOT="$REPO" OVERMATCH_CONFIG_DIR="$CONFIG" \
  "$CLIENT" --offline >"$LOG" 2>&1 &
CLIENT_PID=$!

waited=0
while (( waited < SECONDS_TO_RUN )); do
  sleep 1
  waited=$(( waited + 1 ))
  # A validation failure makes bevy quit, so an early death IS the failure signal — report it the
  # moment it happens instead of waiting out the clock.
  kill -0 "$CLIENT_PID" 2>/dev/null || break
done

alive=0
kill -0 "$CLIENT_PID" 2>/dev/null && alive=1
kill "$CLIENT_PID" 2>/dev/null || true
wait "$CLIENT_PID" 2>/dev/null || true
CLIENT_PID=""

errors=$(grep -c 'ERROR' "$LOG" || true)
if (( errors > 0 )); then
  echo "FAILED — $errors error line(s) after ${waited}s:"
  grep -E 'ERROR' -A 6 "$LOG" | head -40
  exit 1
fi
if (( alive == 0 )); then
  echo "FAILED — the client exited before ${SECONDS_TO_RUN}s with no ERROR line; last output:"
  tail -20 "$LOG"
  exit 1
fi
# Proof the ground was actually built: an app that never reached the terrain never specialized its
# pipeline, and a silent log would read as a pass.
grep -q 'terrain surface:' "$LOG" || {
  echo "FAILED — the terrain never spawned, so no ground pipeline was ever created:"
  tail -20 "$LOG"
  exit 1
}
echo "OK — every pipeline the first ${waited}s created compiled; no rendering errors."
grep -E 'terrain surface:' "$LOG"
