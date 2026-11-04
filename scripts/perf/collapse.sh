#!/usr/bin/env bash
# The video game's environment destruction instrument: real match or tankless diagnosis.
# Match: scripts/perf/collapse.sh --target local --schedule scripts/perf/match-collapse.json
# Local plumbing only: add --rehearsal (allocation is recorded, not priced).
# Hosted: --target IP:PORT --deployment ID; the operator starts the server with the environment
# printed by --print-server-env for the same --schedule/--tanks/--clients. No deploy or stop here.
# Diagnostic: --duration 60 --bursts 20 --period 1 [--sample] [--out DIR]
# Cargo checks freshness once, then the run takes the same machine lock without nesting it.
set -euo pipefail
cd "$(dirname "$0")/../.."
exec python3 scripts/perf/collapse.py "$@"
