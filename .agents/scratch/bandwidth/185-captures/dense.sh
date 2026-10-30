#!/bin/zsh
# Dense capture: 91 frames at 1/30 s from app second 24 (at-dense.txt), close pose, for one build.
# Usage (under the machine lock): dense.sh <bin-dir> <out-dir>
D="$(cd "$(dirname "$0")" && pwd)"
export AT="$(<"$D/at-dense.txt")"
export POSE=2,39,24,6,36.5,12
exec "$D/capture.sh" "$1" "$2"
