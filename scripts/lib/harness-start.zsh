# Cohort admission for scripted clients (zsh). The file releases script clocks, never the Battle.
# Arguments: start file, primary PID/log, optional second PID/log.
# Handle failure explicitly: zsh errexit inside a function can skip an EXIT trap.
await_harness_slots() {
  local ready="$1" first_pid="$2" first_log="$3" second_pid="${4:-}" second_log="${5:-}" poll
  # ASSUMED 15 s admission allowance, below harness::WATCHDOG_SLACK's 20 s launch bound.
  for poll in {1..75}; do
    if ! kill -0 "$first_pid" 2>/dev/null; then
      echo "primary client exited before admission; see $first_log" >&2
      return 1
    fi
    if [[ -n "$second_pid" ]] && ! kill -0 "$second_pid" 2>/dev/null; then
      echo "second client exited before admission; see $second_log" >&2
      return 1
    fi
    if grep -q 'harness: input slot' "$first_log" 2>/dev/null &&
       { [[ -z "$second_pid" ]] || grep -q 'harness: input slot' "$second_log" 2>/dev/null; }; then
      touch "$ready" || return 1
      return 0
    fi
    sleep 0.2
  done
  echo "clients did not all take input slots before the tool barrier; see $first_log $second_log" >&2
  return 1
}
