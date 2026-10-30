#!/usr/bin/env zsh
# arm.sh <bin-dir> <arm> — one arm's evidence: the hidden 30-tank removal count (the offscreen
# runner, under one lock hold), then the stills (capture.sh, one lock hold per scene). Frame times
# the runner prints are not part of this evidence. Output: runs/<arm>/ (git-ignored scratch) and the
# `mesh_removals:` lines in counts-<arm>.txt beside this file.
set -eu
here=${0:A:h}
repo=${here:h:h:h:h}
bin=$1 arm=$2
out=$repo/target/lane-logs/r173/runs/$arm
mkdir -p $out
export OVERMATCH_MUTE=1
/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock env OFFSCREEN=1 WARMUP_S=15 DURATION_S=20 \
  BIN=$bin ASSET_ROOT=$repo $repo/scripts/perf/run-ablation.sh $out/smoke \
  "$arm||OVERMATCH_PROBE_TANKS=30 SPIKE_MESH_REMOVALS=1" >$out/smoke.out 2>&1 \
  || echo "smoke exit $?" >>$out/smoke.out
{
  echo "# $arm: SPIKE_MESH_REMOVALS lines from the hidden 30-tank run ($(cat $bin/BUILD.txt))"
  grep -hE 'offline: spawned|mesh_removals:' $out/smoke/*.log | sed -E 's/^.*(offline: spawned|mesh_removals:)/\1/'
} >$here/counts-$arm.txt
$here/capture.sh $bin/overmatch $out/stills $arm >$out/capture.out 2>&1 || echo "capture exit $?" >>$out/capture.out
echo "arm $arm done" >>$out/capture.out
