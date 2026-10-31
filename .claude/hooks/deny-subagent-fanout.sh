#!/bin/bash
# Deny Agent and Workflow calls inside subagents: one level of agents only (the
# orchestrator and its subagents). agent_id is present in hook input only for
# subagent tool calls, never the main thread.
input=$(cat)
agent_id=$(jq -r '.agent_id // empty' <<<"$input")
if [[ -n "$agent_id" ]]; then
  jq -n '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:"One level of agents only: a subagent does its own work and spawns no agents or workflows. Do the reading yourself; if the task is too large for one agent, commit or write what you have and report the remainder for the orchestrator to dispatch."}}'
fi
exit 0
