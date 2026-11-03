#!/bin/bash
# Throwaway: run the stills in chunks, each under one hold of the machine lock (the one lockf a run
# takes), releasing between chunks so other bays' compiles interleave.
# Usage: drive.sh [stem substring ...]
B=/Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-2
S=/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/impact-stills
while true; do
  /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock python3 -I "$S/run.py" "$B" "$S/out" "$S/raw" "$@"
  code=$?
  if [ "$code" -ne 3 ]; then
    echo "DRIVE-END $code $(date)"
    break
  fi
  sleep 2
done
