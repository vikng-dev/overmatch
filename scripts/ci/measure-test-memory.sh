#!/usr/bin/env bash

set -euo pipefail

if (( $# == 0 )); then
  echo "usage: $0 TEST_BINARY [TEST_ARGUMENT ...]" >&2
  exit 2
fi

memory_max_bytes="${MEMORY_MAX_BYTES:-}"
image="${STRESS_IMAGE:-overmatch-shot-stress-runtime:ci}"
workspace="${GITHUB_WORKSPACE:-$(pwd)}"
test_binary="$(realpath "$1")"
shift

if [[ -z "${memory_max_bytes}" ]]; then
  echo "MEMORY_MAX_BYTES is required for the stress test" >&2
  exit 1
fi
if [[ ! "${memory_max_bytes}" =~ ^[1-9][0-9]*$ ]]; then
  echo "MEMORY_MAX_BYTES must be a positive byte count" >&2
  exit 1
fi
if [[ ! -x "${test_binary}" ]]; then
  echo "test binary is not executable: ${test_binary}" >&2
  exit 1
fi

workspace="$(realpath "${workspace}")"
case "${test_binary}" in
  "${workspace}"/*) ;;
  *)
    echo "test binary must be inside the mounted workspace: ${test_binary}" >&2
    exit 1
    ;;
esac

if ! docker image inspect "${image}" >/dev/null 2>&1; then
  echo "stress runtime image is unavailable: ${image}" >&2
  exit 1
fi

container_binary="/workspace/${test_binary#"${workspace}"/}"
name="overmatch-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-0}-$$"
container_id=""

# Invoked indirectly by the EXIT trap below.
# shellcheck disable=SC2329
cleanup() {
  primary_status=$?
  trap - EXIT
  set +e
  cleanup_status=0

  if [[ -n "${container_id}" ]]; then
    if ! docker rm --force "${container_id}" >/dev/null 2>&1; then
      echo "failed to remove stress container ${container_id}" >&2
      cleanup_status=1
    fi
  fi

  if (( primary_status != 0 )); then
    exit "${primary_status}"
  fi
  exit "${cleanup_status}"
}
trap cleanup EXIT

container_id="$(
  docker create \
    --name "${name}" \
    --init \
    --memory "${memory_max_bytes}" \
    --memory-swap "${memory_max_bytes}" \
    --pids-limit 2048 \
    --mount "type=bind,src=${workspace},dst=/workspace,readonly" \
    --workdir /workspace \
    --env "EXPECTED_MEMORY_MAX_BYTES=${memory_max_bytes}" \
    --env "SAMPLE_SECONDS=${SAMPLE_SECONDS:-5}" \
    --env "TEST_TIMEOUT_SECONDS=${TEST_TIMEOUT_SECONDS:-300}" \
    --env "TEST_KILL_AFTER_SECONDS=${TEST_KILL_AFTER_SECONDS:-10}" \
    "${image}" \
    "${container_binary}" "$@"
)"

configured_memory="$(docker inspect --format '{{.HostConfig.Memory}}' "${container_id}")"
configured_memory_swap="$(docker inspect --format '{{.HostConfig.MemorySwap}}' "${container_id}")"
if [[ "${configured_memory}" != "${memory_max_bytes}" ]]; then
  echo "Docker did not apply the requested memory limit: ${configured_memory}" >&2
  exit 1
fi
if [[ "${configured_memory_swap}" != "${memory_max_bytes}" ]]; then
  echo "Docker did not apply the requested no-swap limit: ${configured_memory_swap}" >&2
  exit 1
fi
printf 'MEASURED docker memory ceiling bytes=%s swap_total_bytes=%s\n' \
  "${configured_memory}" "${configured_memory_swap}"

set +e
docker start --attach "${container_id}"
attach_status=$?
set -e

container_status="$(docker inspect --format '{{.State.ExitCode}}' "${container_id}")"
state_status="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
oom_killed="$(docker inspect --format '{{.State.OOMKilled}}' "${container_id}")"
state_error="$(docker inspect --format '{{.State.Error}}' "${container_id}")"
printf 'MEASURED docker state status=%s exit_code=%s oom_killed=%s attach_status=%s error=%q\n' \
  "${state_status}" "${container_status}" "${oom_killed}" "${attach_status}" "${state_error}"

if [[ "${state_status}" != exited ]]; then
  echo "stress container did not reach an exited state" >&2
  exit 1
fi
if (( attach_status != 0 && container_status == 0 )); then
  echo "docker start --attach failed before producing a failing container status" >&2
  exit "${attach_status}"
fi
if [[ "${oom_killed}" == true && "${container_status}" == 0 ]]; then
  echo "Docker reported an OOM kill despite a zero container exit status" >&2
  exit 137
fi
exit "${container_status}"
