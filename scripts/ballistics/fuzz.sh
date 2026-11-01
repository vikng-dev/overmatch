#!/usr/bin/env bash
# The game's damage-model gate, game state only. Asset authors run the CLI at bake scale or replay
# a report's named ray. The machine build/run lock covers the whole process, with no window or audio.
set -euo pipefail
repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
cd "$repo"
export OVERMATCH_CARGO_LOCK_WAIT_S=10800 OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=headless
exec scripts/serial-cargo run --locked --profile edit --features dev_ui,bitprobe --bin ballistic_fuzzer -- "$@"
