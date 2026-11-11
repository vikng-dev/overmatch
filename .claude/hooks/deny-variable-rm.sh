#!/bin/bash
# Deny an rm whose path holds an unguarded shell variable ($VAR or ${VAR}). Claude Code's built-in
# check answers such a command with a permission dialog that no allow rule clears, and a background
# agent stalls on it until the owner answers; a denial here returns the reason to the agent instead.
# Only an rm in command position counts, and only variables inside that same simple command, so
# text that mentions rm mid-sentence passes. An rm opening a quoted string counts too: the built-in
# check reads those as commands that run later. ${VAR:?} is guarded and passes.
cmd=$(jq -r '.tool_input.command // empty')
lead='(^|&&|\|\||;|\||\(|\$\(|then[[:space:]]|do[[:space:]]|["'"'"'])[[:space:]]*'
prefix='((sudo|command|nohup|xargs)[[:space:]]+)*'
unguarded='\$([A-Za-z_]|\{[A-Za-z_][A-Za-z0-9_]*\})'
if grep -Eq "${lead}${prefix}rm[[:space:]][^;&|]*${unguarded}" <<<"$cmd"; then
  jq -n '{hookSpecificOutput:{hookEventName:"PreToolUse",permissionDecision:"deny",permissionDecisionReason:"rm with a shell variable in its path is refused: it raises a permission dialog that stalls the agent. Delete with literal absolute paths (several per command is fine, no loops), or guard every variable as \"${DIR:?}/${name:?}.md\", then retry."}}'
fi
exit 0
