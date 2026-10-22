#!/bin/sh
# sweep.sh — reap stale build artifacts across every overmatch checkout on this box.
#
# Two leaks, two reapers, over the main tree and every `git worktree list` entry:
# - rustc on macOS leaves a superseded set of `*.rcgu.o` in `target/*/deps` on every rebuild
#   (rust-lang/rust#161824). `split-debuginfo = "off"` stops new ones in the dev profile; profiles
#   that keep "unpacked" (dev-full) still leak. Cargo does not fingerprint these files, so deleting
#   ones older than 12 h leaves every unit Fresh and is safe while a build runs.
# - Incremental state: every feature set, target kind and profile gets its own
#   `incremental/<crate>-<hash>` directory, rustc garbage-collects only the sessions INSIDE one, and
#   nothing ever removes a directory whose combination is no longer built (MEASURED 2026-10-22: 164
#   directories, 16 GB, from one day's builds). A directory untouched for a day goes; rebuilding
#   that combination costs one non-incremental compile of this crate (or of a vendored path crate),
#   never of the registry dependencies.
# - Fingerprints for dependency versions and rustc releases no longer used: `cargo sweep --maxsize`
#   keeps each target under a ceiling by evicting least-recently-used artifacts. It COUNTS
#   `incremental/` toward the ceiling but cannot evict it, so once incremental alone passes the
#   ceiling every run evicts all warm dependencies instead (MEASURED 2026-10-22: 36 GB of
#   incremental, a 9.8 min cold rebuild). The ceiling therefore sits above the incremental steady
#   state the reaper above bounds, and the reaper runs first.
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

echo "target-sweep $(date '+%Y-%m-%d %H:%M:%S') — rcgu.o > 12 h, incremental > 24 h, maxsize 40GB per checkout"
git -C "$MAIN" worktree list --porcelain | sed -n 's/^worktree //p' | while read -r dir; do
  if [ ! -d "$dir/target" ]; then
    continue
  fi
  before=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  find "$dir/target" -path '*/deps/*.rcgu.o' -mmin +720 -delete 2>/dev/null
  find "$dir/target" -mindepth 3 -maxdepth 3 -type d -path '*/incremental/*' -mmin +1440 2>/dev/null \
    | while read -r stale; do rm -rf "${stale:?}"; done
  cargo sweep --maxsize 40GB "$dir" >/dev/null 2>&1
  after=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  echo "  swept $dir  ${before} -> ${after}"
done
echo "target-sweep done"
