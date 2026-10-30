#!/usr/bin/env bash
# build.sh A|B — the night's two profiling builds from the detached release tag, each copied out
# to bin/<A|B>/ with a BUILD.txt.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd -P)"
bay=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3
cd "$bay"
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
head="$(git rev-parse HEAD)"
tag="$(git rev-parse 'v0.7.0-alpha.3^{commit}')"
[[ "$head" == "$tag" ]] || { echo "HEAD $head is not the tag $tag" >&2; exit 1; }
case "$1" in
  A) features=dev_ui,bitprobe,bevy/trace_chrome; bins=(--bin overmatch-server); outs=(overmatch-server) ;;
  A2) features=dev_ui,bitprobe,bevy/trace_chrome; bins=(--bin overmatch); outs=(overmatch) ;;
  B) features=dev_ui,bitprobe; bins=(--bin overmatch --bin overmatch-server); outs=(overmatch overmatch-server) ;;
  *) echo "A or B" >&2; exit 2 ;;
esac
start=$(date +%s)
scripts/serial-cargo build --locked --profile profiling --features "$features" "${bins[@]}"
end=$(date +%s)
dest="$here/bin/${1:0:1}"
mkdir -p "$dest"
for o in "${outs[@]}"; do cp -c "target/profiling/$o" "$dest/$o"; done
{
  echo "built $(date '+%F %T %Z') from $head (tag v0.7.0-alpha.3), bay-3, $((end - start)) s including the queue"
  echo "scripts/serial-cargo build --locked --profile profiling --features $features ${bins[*]}"
  for o in "${outs[@]}"; do shasum -a 256 "$dest/$o"; done
} >>"$dest/BUILD.txt"
cat "$dest/BUILD.txt"
