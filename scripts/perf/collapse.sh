#!/usr/bin/env bash
# The video game's environment destruction benchmark: no client, tank, screen or installs.
# Usage: scripts/perf/collapse.sh --duration 60 --bursts 20 --period 1 [--sample] [--out DIR]
# Cargo checks freshness once, then the run takes the same machine lock without nesting it.
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/perf/collapse.py "$@"
