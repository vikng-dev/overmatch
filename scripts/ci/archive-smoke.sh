#!/usr/bin/env bash
# archive-smoke.sh — boot a server, run a client against it headlessly, and prove they connected.
#
#     usage: scripts/ci/archive-smoke.sh <server-binary> <client-binary>
#
# THE POINT is the ARCHIVES' OWN BYTES. In `.github/workflows/release.yml` both binaries are the
# ones a user downloads, extracted from the release archives with no path surgery, so the assets
# each side digests are the shipped assets. A `level.json` whose line endings were rewritten
# anywhere in build or packaging changes the client's map digest, and the map digest is half of the
# netcode `protocol_id` (ADR-0018) — the connect token then decrypts to nothing and the server logs
# a bad protocol id. That release shipped three times before this lane existed.
#
# WHAT IS OVERRIDDEN, and nothing else: `OVERMATCH_SERVER`, so the client dials this machine
# instead of the baked droplet, and the `SPIKE_*` harness contract (`src/net/harness.rs`), which is
# how a client with no window, no GPU and no human runs a scripted session and exits on its own.
# `SPIKE_SIMULATE_INPUT` without `SPIKE_SIM_WINDOWED` selects the GPU-less composition root
# (`net::client::run` → `gpu_less_default_plugins`: wgpu `backends: None`, `WinitPlugin` disabled),
# which is what makes this runnable on a headless runner at all.
#
# THE ASSERTION is the client's own observable evidence, in order:
#   1. `client: connected` — the netcode handshake completed (a poisoned digest never reaches it)
#   2. `— input slot`      — the server replicated a tank this client controls
#   3. `simulation script complete` + exit status 0 — it played its script out and left cleanly
# A wedged client is bounded twice: the in-app watchdog (`harness::simulate_watchdog`, script
# length / 64 + 20 s) and the wall-clock kill below.
#
# Env:
#   SMOKE_SERVER_ASSET_ROOT  `BEVY_ASSET_ROOT` for the server only (a server binary that is not
#                            packaged beside its assets — see the Windows lane's note in
#                            release.yml). Unset: the server resolves them next to itself, which is
#                            the shipped tarball's layout.
#   SMOKE_TICKS              scripted client run length in 64 Hz ticks (default 256 ≈ 4 s).
#   SMOKE_DEADLINE           wall-clock seconds the client gets before it is killed (default 120).
#   SMOKE_LOGS               directory for `server.log` / `client.log` (default a temp dir).
set -uo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <server-binary> <client-binary>" >&2
    exit 2
fi

server_bin=$1
client_bin=$2
ticks=${SMOKE_TICKS:-256}
deadline=${SMOKE_DEADLINE:-120}
logs=${SMOKE_LOGS:-$(mktemp -d)}
mkdir -p "$logs"
server_log="$logs/server.log"
client_log="$logs/client.log"

[ -x "$server_bin" ] || { echo "smoke: $server_bin is not executable" >&2; exit 1; }
[ -x "$client_bin" ] || { echo "smoke: $client_bin is not executable" >&2; exit 1; }

# Both logs, whatever happened — the runner is gone the moment this job ends, and a failure whose
# evidence was not printed is a failure nobody can diagnose.
dump() {
    echo "===== server.log ====="
    cat "$server_log" 2>/dev/null
    echo "===== client.log ====="
    cat "$client_log" 2>/dev/null
    echo "====================="
}

# The server is started from its own directory: with no `BEVY_ASSET_ROOT` and no
# `CARGO_MANIFEST_DIR`, `crate::assets::asset_root` resolves `assets/` beside the executable, which
# is exactly how the shipped tarball is laid out (and how the droplet runs it).
echo "smoke: starting $server_bin"
(
    cd "$(dirname "$server_bin")" || exit 1
    if [ -n "${SMOKE_SERVER_ASSET_ROOT:-}" ]; then
        export BEVY_ASSET_ROOT="$SMOKE_SERVER_ASSET_ROOT"
    fi
    exec "./$(basename "$server_bin")"
) > "$server_log" 2>&1 &
server_pid=$!
trap 'kill "$server_pid" 2>/dev/null' EXIT

# The server prints this once the netcode listener is up (`net::server::run`). Waiting on the line
# rather than on a sleep keeps a slow runner from being read as a failed boot.
for _ in $(seq 1 120); do
    grep -q "server: starting, listening" "$server_log" 2>/dev/null && break
    kill -0 "$server_pid" 2>/dev/null || { echo "smoke: the server exited during boot" >&2; dump; exit 1; }
    sleep 0.5
done
if ! grep -q "server: starting, listening" "$server_log" 2>/dev/null; then
    echo "smoke: the server never announced its listener" >&2
    dump
    exit 1
fi

# The client, exactly as shipped except for the address and the scripted-session contract. Started
# from its own directory for the same exe-relative asset reason as the server.
echo "smoke: running $client_bin against 127.0.0.1:5888 for $ticks ticks"
(
    cd "$(dirname "$client_bin")" || exit 1
    exec env OVERMATCH_SERVER=127.0.0.1:5888 \
        SPIKE_SIMULATE_INPUT=1 \
        SPIKE_SIM_IDLE=1 \
        SPIKE_SIM_TICKS="$ticks" \
        "./$(basename "$client_bin")"
) > "$client_log" 2>&1 &
client_pid=$!

elapsed=0
while kill -0 "$client_pid" 2>/dev/null && [ "$elapsed" -lt "$deadline" ]; do
    sleep 1
    elapsed=$((elapsed + 1))
done
if kill -0 "$client_pid" 2>/dev/null; then
    kill -9 "$client_pid" 2>/dev/null
    wait "$client_pid" 2>/dev/null
    echo "smoke: the client was still running after ${deadline}s — killed" >&2
    dump
    exit 1
fi
wait "$client_pid"
client_status=$?

failed=0
# `client: connected` is the netcode handshake; a protocol-id mismatch (the CRLF class) dies here.
grep -q "client: connected" "$client_log" || { echo "smoke: the client never connected" >&2; failed=1; }
# The server replicated a tank this client owns — a connect that never becomes a session is not one.
grep -q -- "— input slot" "$client_log" || { echo "smoke: the client never got an input slot" >&2; failed=1; }
# It played the script out rather than being cut short by the in-app watchdog.
grep -q "simulation script complete" "$client_log" ||
    { echo "smoke: the client never completed its script" >&2; failed=1; }
[ "$client_status" -eq 0 ] || { echo "smoke: the client exited $client_status" >&2; failed=1; }

if [ "$failed" -ne 0 ]; then
    dump
    exit 1
fi

echo "smoke: connected, took an input slot, played the script out, exited 0"
