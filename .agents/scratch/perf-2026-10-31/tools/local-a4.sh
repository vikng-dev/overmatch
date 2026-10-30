#!/bin/zsh
# local-a4.sh — after the Edgegap chains, under one lock hold each: the alpha.4 server locally at
# 30 tanks (spans filtered), beside alpha.3's p2, so the host and the release can be told apart.
set -u
here="${0:A:h}"
L=(/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock)
R="$here/runs/profile"
echo "a4-plain $(date +%T)"
"${L[@]}" env SERVER="$here/bin/A4/overmatch-server" CLIENT="$here/bin/A4/overmatch" ROOT="$here/bin/A4/root" \
  BOTS=29 "$here/profile-run.sh" "$R" p5-30t-plain-alpha4
echo "a4-plain-t1 $(date +%T)"
"${L[@]}" env SERVER="$here/bin/A4/overmatch-server" CLIENT="$here/bin/A4/overmatch" ROOT="$here/bin/A4/root" \
  BOTS=29 THREADS=1 "$here/profile-run.sh" "$R" p6-30t-plain-alpha4-t1
echo "local-zero $(date +%T)"
"${L[@]}" "$here/local-extra.sh" "$here/runs/local-zero" 1:100000:8192:512 1:100000:8192:512
echo "end $(date +%T)"
