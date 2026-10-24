#!/bin/bash
# Deny a compiling cargo command run bare inside a subagent: compiles go through
# scripts/serial-cargo, the machine's build queue, which also warms a fresh worktree (AGENTS.md).
# agent_id is present in hook input only for subagent tool calls, never the main thread. Only a
# cargo in command position counts (line start, or after && || ; | ( `then` `do`, optionally
# behind env assignments or nohup/time/env/caffeinate), so text that mentions cargo passes.
input=$(cat)
agent_id=$(jq -r '.agent_id // empty' <<<"$input")
[[ -n "$agent_id" ]] || exit 0
cmd=$(jq -r '.tool_input.command // empty' <<<"$input")
queue=$(cd "$(dirname "$0")/../.." && pwd -P)/scripts/serial-cargo
[[ -x "$queue" ]] || exit 0
lead='(^|&&|\|\||;|\||\(|\$\(|then[[:space:]]|do[[:space:]])[[:space:]]*'
prefix='(([A-Za-z_][A-Za-z0-9_]*=[^[:space:]]*|nohup|time|/usr/bin/time|-l|env|caffeinate|-i)[[:space:]]+)*'
sub='(build|b|check|c|clippy|test|t|run|r|bench|doc|rustc|nextest|mutants|armor|track|tracy|install)'
if grep -Eq "${lead}${prefix}cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?${sub}([[:space:]]|$)" <<<"$cmd"; then
  jq -n --arg q "$queue" '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:("Compile through the build queue: " + $q + " <cargo args>, run from your checkout (it builds the checkout it is run in), e.g. " + $q + " test --lib --features dev_ui,bitprobe ballistics. It queues on the machine-wide lock with no polling, waits out a build started elsewhere, and warms a fresh worktree from main on its first call. Scripts that compile (mutant runners) call it too. cargo fmt --all --check, cargo tree and cargo metadata need no queue.")}}'
fi
exit 0
