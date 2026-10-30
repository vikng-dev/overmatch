#!/bin/zsh
# battery-chain.sh <out-dir> — the battery's offscreen blocks, each its own lock hold, a block
# started only when its DERIVED duration still ends before DEADLINE (default 07:40 local).
set -u
here="${0:A:h}"
OUT="$1"
DEADLINE="${DEADLINE:-07:40}"
L=(/usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock)
deadline_epoch=$(date -j -f '%Y-%m-%d %H:%M' "$(date +%F) $DEADLINE" +%s)
# block:minutes (DERIVED from the battery's runbook: startup + warmup + measure + cooldown)
for spec in ${=BLOCKS:-cal-start:5 battle-30:32 duel-2:48 cal-end:5 forest-fwd:14 forest-rev:14}; do
  block="${spec%%:*}"; mins="${spec##*:}"
  if (( $(date +%s) + mins * 60 > deadline_epoch )); then
    echo "$block skipped at $(date +%T): $mins min would pass $DEADLINE"
    continue
  fi
  echo "== $block $(date +%T)"
  "${L[@]}" "$here/battery-block.sh" "$OUT" "$block" >"$OUT.$block.out" 2>&1
  echo "   $block exit $? $(date +%T)"
done
