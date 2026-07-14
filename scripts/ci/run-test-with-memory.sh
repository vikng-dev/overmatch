#!/usr/bin/env bash

set -euo pipefail

if (( $# == 0 )); then
  echo "usage: $0 TEST_BINARY [TEST_ARGUMENT ...]" >&2
  exit 2
fi

cgroup=/sys/fs/cgroup
expected_memory_max_bytes="${EXPECTED_MEMORY_MAX_BYTES:-}"
sample_seconds="${SAMPLE_SECONDS:-5}"
timeout_seconds="${TEST_TIMEOUT_SECONDS:-300}"
kill_after_seconds="${TEST_KILL_AFTER_SECONDS:-10}"
sampler=""

for value_name in sample_seconds timeout_seconds kill_after_seconds; do
  value="${!value_name}"
  if [[ ! "${value}" =~ ^[1-9][0-9]*$ ]]; then
    echo "${value_name} must be a positive integer" >&2
    exit 1
  fi
done
if [[ -z "${expected_memory_max_bytes}" ]]; then
  echo "EXPECTED_MEMORY_MAX_BYTES is required" >&2
  exit 1
fi

for control in memory.current memory.events memory.max memory.peak memory.stat memory.swap.max; do
  if [[ ! -r "${cgroup}/${control}" ]]; then
    echo "required container cgroup-v2 control is unavailable: ${control}" >&2
    exit 1
  fi
done

actual_memory_max_bytes="$(<"${cgroup}/memory.max")"
actual_swap_max_bytes="$(<"${cgroup}/memory.swap.max")"
if [[ "${actual_memory_max_bytes}" != "${expected_memory_max_bytes}" ]]; then
  echo "container memory ceiling does not match the requested limit: ${actual_memory_max_bytes}" >&2
  exit 1
fi
if [[ "${actual_swap_max_bytes}" != 0 ]]; then
  echo "container swap is not disabled: memory.swap.max=${actual_swap_max_bytes}" >&2
  exit 1
fi

event_value() {
  local key=$1
  awk -v key="${key}" '$1 == key { print $2; found = 1 } END { if (!found) exit 1 }' \
    "${cgroup}/memory.events"
}

print_sample() {
  printf 'MEASURED cgroup sample epoch=%s\n' "$(date +%s)"
  for metric in memory.current memory.peak memory.max memory.swap.max memory.events; do
    printf '%s:\n' "${metric}"
    cat "${cgroup}/${metric}"
  done
}

stop_sampler() {
  if [[ -n "${sampler}" ]]; then
    kill "${sampler}" 2>/dev/null || true
    wait "${sampler}" 2>/dev/null || true
    sampler=""
  fi
}

# Invoked indirectly by the EXIT trap below.
# shellcheck disable=SC2329
cleanup() {
  primary_status=$?
  trap - EXIT
  set +e
  stop_sampler
  exit "${primary_status}"
}
trap cleanup EXIT

max_before="$(event_value max)"
oom_before="$(event_value oom)"
oom_kill_before="$(event_value oom_kill)"
oom_group_kill_before="$(event_value oom_group_kill)"

printf 'MEASURED cgroup memory ceiling bytes=%s swap_max_bytes=%s\n' \
  "${actual_memory_max_bytes}" "${actual_swap_max_bytes}"
(
  while true; do
    print_sample
    sleep "${sample_seconds}"
  done
) &
sampler=$!

set +e
/usr/bin/time -v \
  /usr/bin/timeout --signal=TERM --kill-after="${kill_after_seconds}s" "${timeout_seconds}s" \
  "$@"
status=$?
set -e

stop_sampler
print_sample
printf 'memory.stat selected:\n'
awk '$1 ~ /^(anon|file|kernel|kernel_stack|pagetables|sock)$/' "${cgroup}/memory.stat"

max_after="$(event_value max)"
oom_after="$(event_value oom)"
oom_kill_after="$(event_value oom_kill)"
oom_group_kill_after="$(event_value oom_group_kill)"
if (( max_after > max_before || oom_after > oom_before || \
      oom_kill_after > oom_kill_before || oom_group_kill_after > oom_group_kill_before )); then
  echo "stress test reached its cgroup memory boundary" >&2
  if (( status == 0 )); then
    status=137
  fi
fi

exit "${status}"
