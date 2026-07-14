#!/usr/bin/env bash

set -euo pipefail

if (( $# == 0 )); then
  echo "usage: $0 TEST_BINARY [TEST_ARGUMENT ...]" >&2
  exit 2
fi

sample_seconds="${SAMPLE_SECONDS:-5}"
memory_max_bytes="${MEMORY_MAX_BYTES:-}"
cleanup_poll_attempts="${CLEANUP_POLL_ATTEMPTS:-100}"
cleanup_poll_seconds="${CLEANUP_POLL_SECONDS:-0.05}"
relative_parent="$(awk -F: '$1 == "0" { print $3 }' /proc/self/cgroup 2>/dev/null || true)"
parent="/sys/fs/cgroup${relative_parent}"
name="overmatch-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$"
cgroup="${parent}/${name}"
sampler=""
cgroup_created=false

print_sample() {
  printf 'MEASURED cgroup sample epoch=%s\n' "$(date +%s)"
  for metric in memory.current memory.peak memory.max memory.events; do
    if [[ -r "${cgroup}/${metric}" ]]; then
      printf '%s:\n' "${metric}"
      cat "${cgroup}/${metric}"
    fi
  done
  free --bytes
}

# Invoked indirectly by the EXIT trap below.
# shellcheck disable=SC2329
cleanup() {
  primary_status=$?
  trap - EXIT
  set +e
  cleanup_status=0

  if [[ -n "${sampler}" ]]; then
    kill "${sampler}" 2>/dev/null || true
    wait "${sampler}" 2>/dev/null || true
  fi

  if [[ "${cgroup_created}" == true && -d "${cgroup}" ]]; then
    if ! printf '1\n' | sudo tee "${cgroup}/cgroup.kill" >/dev/null; then
      echo "failed to kill the measured cgroup" >&2
      cleanup_status=1
    fi

    if [[ -r "${cgroup}/cgroup.events" ]]; then
      populated=1
      for (( attempt = 0; attempt < cleanup_poll_attempts; attempt++ )); do
        populated="$(awk '$1 == "populated" { print $2 }' "${cgroup}/cgroup.events")"
        [[ "${populated}" == 0 ]] && break
        sleep "${cleanup_poll_seconds}"
      done
      if [[ "${populated}" != 0 ]]; then
        echo "measured cgroup remained populated after cleanup" >&2
        cleanup_status=1
      fi
    else
      echo "measured cgroup has no readable population state" >&2
      cleanup_status=1
    fi

    removed=false
    for (( attempt = 0; attempt < cleanup_poll_attempts; attempt++ )); do
      if sudo rmdir "${cgroup}" 2>/dev/null; then
        removed=true
        break
      fi
      sleep "${cleanup_poll_seconds}"
    done
    if [[ "${removed}" != true ]]; then
      echo "failed to remove the measured cgroup" >&2
      cleanup_status=1
    fi
  fi

  if (( primary_status != 0 )); then
    exit "${primary_status}"
  fi
  exit "${cleanup_status}"
}
trap cleanup EXIT

if [[ -z "${relative_parent}" || ! -d "${parent}" ]]; then
  echo "cgroup v2 is unavailable; refusing to run the unbounded stress test" >&2
  exit 1
fi
if [[ -z "${memory_max_bytes}" ]]; then
  echo "MEMORY_MAX_BYTES is required for the stress test" >&2
  exit 1
fi
if ! sudo mkdir "${cgroup}"; then
  echo "cannot create a measured child cgroup; refusing to run the stress test" >&2
  exit 1
fi
cgroup_created=true

for control in \
  cgroup.events cgroup.kill cgroup.procs \
  memory.current memory.events memory.max memory.oom.group memory.peak memory.swap.max; do
  if ! sudo test -e "${cgroup}/${control}"; then
    echo "required cgroup-v2 control is unavailable: ${control}" >&2
    exit 1
  fi
done

printf '%s\n' "${memory_max_bytes}" | sudo tee "${cgroup}/memory.max" >/dev/null
printf '0\n' | sudo tee "${cgroup}/memory.swap.max" >/dev/null
printf '1\n' | sudo tee "${cgroup}/memory.oom.group" >/dev/null
if [[ "$(<"${cgroup}/memory.max")" != "${memory_max_bytes}" ]]; then
  echo "the cgroup memory ceiling did not take effect" >&2
  exit 1
fi
printf 'MEASURED cgroup memory ceiling bytes=%s\n' "${memory_max_bytes}"

(
  while [[ -d "${cgroup}" ]]; do
    print_sample
    sleep "${sample_seconds}"
  done
) &
sampler=$!

set +e
sudo bash -euo pipefail -c '
  cgroup=$1
  uid=$2
  gid=$3
  shift 3
  printf "%s\n" "${BASHPID}" > "${cgroup}/cgroup.procs"
  grep -qx "${BASHPID}" "${cgroup}/cgroup.procs"
  exec /usr/bin/setpriv --reuid "${uid}" --regid "${gid}" --init-groups /usr/bin/time -v "$@"
' _ "${cgroup}" "$(id -u)" "$(id -g)" "$@"
status=$?
set -e

kill "${sampler}" 2>/dev/null || true
wait "${sampler}" 2>/dev/null || true
sampler=""
print_sample
exit "${status}"
