#!/bin/sh
# sweep.sh — reap stale build artifacts across every overmatch checkout on this box.
#
# Two leaks, two reapers, over the main tree and every `git worktree list` entry:
# - rustc on macOS leaves a superseded set of `*.rcgu.o` in `target/*/deps` on every rebuild
#   (rust-lang/rust#161824). `split-debuginfo = "off"` stops new ones in the dev profile; profiles
#   that keep "unpacked" (dev-full) still leak. Cargo does not fingerprint these files, so deleting
#   ones older than 12 h leaves every unit Fresh and is safe while a build runs.
# - Fingerprints for dependency versions and rustc releases no longer used: `cargo sweep --maxsize`
#   keeps each target under a ceiling by evicting least-recently-used artifacts.
#
# Runs only while a Claude Code session is running: that is when builds happen and disk fills, and an
# idle box has nothing to reap. Run by launchd every 6 h (com.overmatch.target-sweep.plist) or by
# hand:  scripts/disk/sweep.sh   (FORCE=1 skips the session check)
set -u

MAIN=/Users/Yan/github/vikng-dev/overmatch

if [ "${FORCE:-0}" != 1 ] && ! pgrep -f '/\.local/bin/claude' >/dev/null 2>&1; then
  echo "target-sweep $(date '+%Y-%m-%d %H:%M:%S') — no Claude session running, skipped"
  exit 0
fi

echo "target-sweep $(date '+%Y-%m-%d %H:%M:%S') — rcgu.o > 12 h, maxsize 25GB per checkout"
git -C "$MAIN" worktree list --porcelain | sed -n 's/^worktree //p' | while read -r dir; do
  if [ ! -d "$dir/target" ]; then
    continue
  fi
  before=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  find "$dir/target" -path '*/deps/*.rcgu.o' -mmin +720 -delete 2>/dev/null
  cargo sweep --maxsize 25GB "$dir" >/dev/null 2>&1
  after=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  echo "  swept $dir  ${before} -> ${after}"
done
echo "target-sweep done"
