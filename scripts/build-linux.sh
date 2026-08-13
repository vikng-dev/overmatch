#!/usr/bin/env bash
# Build a Linux x86_64 release bundle of overmatch using Docker, so it can run
# on a friend's Linux box without installing a Linux toolchain on macOS.
#
# Output: dist/overmatch-linux-x86_64.tar.gz
#   contains overmatch/  ->  the binary + the assets/ folder next to it.
#
# Friend runs:  tar xzf overmatch-linux-x86_64.tar.gz && cd overmatch && ./overmatch
set -euo pipefail

TARGET="x86_64-unknown-linux-gnu"
NAME="overmatch"

# Repo root = parent of this script's dir, regardless of where it's invoked from.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

echo ">> Building $NAME for $TARGET inside Docker (linux/amd64)..."
# --platform linux/amd64 makes the build native-x86_64 (emulated on Apple
# Silicon via QEMU), avoiding a cross C toolchain for the ALSA/udev deps.
# Emulated compiles are slow but correct.
docker run --rm \
  --platform linux/amd64 \
  -v "$ROOT":/app \
  -v "${NAME}-cargo-registry":/usr/local/cargo/registry \
  -w /app \
  rust:latest bash -c "\
    set -e && \
    apt-get update && \
    apt-get install -y --no-install-recommends \
      pkg-config libasound2-dev libudev-dev \
      libwayland-dev libxkbcommon-dev libx11-dev && \
    rustup target add $TARGET && \
    CARGO_BUILD_JOBS=4 cargo build --release --target $TARGET && \
    chown -R $(id -u):$(id -g) target"

echo ">> Packaging bundle..."
STAGE="dist/$NAME"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp "target/$TARGET/release/$NAME" "$STAGE/"
# Ship only runtime assets: the game loads .glb meshes (see asset_server.load
# calls). Blender source files (.blend/.blend1) are build-time only, so prune
# them to keep the bundle small. cc.txt attribution files are kept.
cp -r assets "$STAGE/"
find "$STAGE/assets" -type f \( -name '*.blend' -o -name '*.blend1' -o -name '.DS_Store' \) -delete

tar -czf "dist/$NAME-linux-x86_64.tar.gz" -C dist "$NAME"

echo ">> Done: dist/$NAME-linux-x86_64.tar.gz"
echo "   Send it over; they run: tar xzf $NAME-linux-x86_64.tar.gz && cd $NAME && ./$NAME"
