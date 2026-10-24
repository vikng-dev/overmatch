#!/bin/bash
# Deny run_in_background Bash inside subagents: their shells die at turn-end
# (claude-code#50572) and nothing can wake the agent afterward. agent_id is
# present in hook input only for subagent tool calls, never the main thread.
input=$(cat)
agent_id=$(jq -r '.agent_id // empty' <<<"$input")
bg=$(jq -r '.tool_input.run_in_background // false' <<<"$input")
if [[ -n "$agent_id" && "$bg" == "true" ]]; then
  jq -n '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:"Backgrounded shells die when a subagent ends its turn (claude-code#50572), and no notification can wake a finished subagent. Run the command foreground with a generous timeout, or detach it (nohup cmd > log 2>&1 & echo $! > pid) and wait in-turn with chained foreground polls of at most ~590 s each (for i in $(seq 118); do kill -0 $(cat pid) 2>/dev/null || break; sleep 5; done; there is no timeout command on macOS). Never end your turn with a shell in flight; if the wait does not fit your task, commit what you have and report the command for the orchestrator to run."}}'
fi
exit 0
