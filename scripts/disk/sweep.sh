#!/bin/sh
# sweep.sh — reap stale build artifacts across every overmatch checkout on this box.
#
# The dev/ci profiles are slimmed for debuginfo (see [profile.*] in Cargo.toml), but a
# long-lived target dir still accumulates fingerprints for dependency versions and rustc
# releases we no longer use. `cargo sweep --maxsize` keeps each target dir under a ceiling
# by evicting the least-recently-used artifacts, so an idle checkout can't creep back into
# the tens-of-GB range between rebuilds.
#
# Personal-box tool: the checkout paths are hardcoded (main + the two worktree bays). If a
# path doesn't exist we skip it — no bay is mandatory. Run by the weekly launchd agent
# (com.overmatch.target-sweep.plist) or by hand:  scripts/disk/sweep.sh
set -u

# Every overmatch checkout on this machine: the main tree plus the two worktree bays.
CHECKOUTS="
/Users/Yan/Desktop/github/vikng-dev/personal/overmatch
/Users/Yan/Desktop/github/vikng-dev/personal/overmatch-bay-1
/Users/Yan/Desktop/github/vikng-dev/personal/overmatch-bay-2
"

echo "target-sweep $(date '+%Y-%m-%d %H:%M:%S') — maxsize 25GB per checkout"
for dir in $CHECKOUTS; do
  if [ ! -d "$dir/target" ]; then
    echo "  skip  $dir (no target dir)"
    continue
  fi
  before=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  cargo sweep --maxsize 25GB "$dir" >/dev/null 2>&1
  after=$(du -sh "$dir/target" 2>/dev/null | cut -f1)
  echo "  swept $dir  ${before} -> ${after}"
done
echo "target-sweep done"
