#!/usr/bin/env bash
# edgegap-version.sh — make Edgegap app `Overmatch` hold a version named VERSION that runs IMAGE:TAG.
#
#     usage: EDGEGAP_TOKEN=… scripts/ci/edgegap-version.sh <version> <image> <tag>
#     e.g.   scripts/ci/edgegap-version.sh 0.5.4 ghcr.io/vikng-dev/overmatch-server 0.5.4-26d7c80
#
# The version NAME is the client's `CARGO_PKG_VERSION`: the Worker deploys `version_name` = the
# version a joining client reports (backend/README.md, Deploy step 7). Every field below is
# rewritten on each run, so a re-run converges: absent → POST create; present → PATCH.
# `restart_policy` is create-only in Edgegap's v1 schema (`AppVersionUpdatePayload` has no such
# field), so an existing version is read back and refused unless it already says `never`.
#
# NO ENV VARS ARE BAKED: the Worker passes every variable of backend/contract/server-env.json per
# deployment, the two secrets hidden. `envs: []` on PATCH also clears any set by hand.
#
# Env:
#   EDGEGAP_TOKEN    API token, with or without the `token ` prefix. Sent only from a 0600 header
#                    file, never on a command line or in the log.
#   EDGEGAP_API      default https://api.edgegap.com
#   EDGEGAP_APP      default Overmatch (the Worker's `EDGEGAP_APP`)
#   EDGEGAP_DRY_RUN  when set, print the create and update bodies and exit; no token, no network.
set -euo pipefail

version="${1:?version}"
image="${2:?image, e.g. ghcr.io/vikng-dev/overmatch-server}"
tag="${3:?tag}"
api="${EDGEGAP_API:-https://api.edgegap.com}"
app="${EDGEGAP_APP:-Overmatch}"

# `game` is the Worker's `EDGEGAP_PORT_NAME`; 5888 is the server's fixed listen port. `to_check`
# off is the setting the risk test connected through (risk-test.md).
#
# max_duration 1440 min — Edgegap ends every deployment this long after its start, occupied or
# not. Behind the Worker's per-minute orphan sweep it is the last defence, for when the Worker is
# down; it also cuts a server still occupied at 24 h (its players Join again into a fresh one; there
# is no graceful recycle before the cap). A leak costs at most 1440 × $0.00115/vCPU-min ≈ $1.66 per
# vCPU (DERIVED, list price).
#
# time_to_deploy 60 s = the Worker's START_DEADLINE_S: past it the Worker stops the deployment
# anyway, so Edgegap marking it failed at the same moment changes nothing but its own report.
#
# 1 vCPU / 1 GB: MEASURED ~55 MiB resident with 2 clients (risk-test.md); 10 players unmeasured.
body="$(jq -n \
  --arg image "${image#*/}" --arg registry "${image%%/*}" --arg tag "${tag}" '{
    is_active: true,
    docker_repository: $registry,
    docker_image: $image,
    docker_tag: $tag,
    req_cpu: 1024,
    req_memory: 1024,
    max_duration: 1440,
    time_to_deploy: 60,
    verify_image: true,
    ports: [{port: 5888, protocol: "UDP", name: "game", to_check: false}],
    envs: []
  }')"
create="$(jq --arg name "${version}" '{name: $name, restart_policy: "never"} + .' <<<"${body}")"

if [[ -n "${EDGEGAP_DRY_RUN:-}" ]]; then
  jq -n --argjson create "${create}" --argjson update "${body}" '{create: $create, update: $update}'
  exit 0
fi

: "${EDGEGAP_TOKEN:?EDGEGAP_TOKEN is not set}"
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT
headers="${work}/headers"
install -m 600 /dev/null "${headers}"
token="${EDGEGAP_TOKEN#token }"
printf 'authorization: token %s\ncontent-type: application/json\naccept: application/json\n' "${token}" > "${headers}"

# One call: prints the HTTP status; the response body lands in ${work}/out.
call() {
  local args=(-sS --max-time 60 -o "${work}/out" -w '%{http_code}' -H @"${headers}" -X "$1" "${api}$2")
  if [[ $# -ge 3 ]]; then args+=(--data-binary "$3"); fi
  curl "${args[@]}"
}
fail() {
  echo "::error::$1 (HTTP $2)"
  cat "${work}/out" >&2 || true
  exit 1
}
path="/v1/app/${app}/version/${version}"

status="$(call GET "${path}")"
case "${status}" in
  404)
    status="$(call POST "/v1/app/${app}/version" "${create}")"
    [[ "${status}" == 200 ]] || fail "creating Edgegap version ${app}/${version} failed" "${status}"
    echo "created Edgegap version ${app}/${version} → ${image}:${tag}"
    ;;
  200)
    policy="$(jq -r '.restart_policy // empty' "${work}/out")"
    if [[ "${policy}" != never ]]; then
      echo "::error::Edgegap version ${app}/${version} has restart_policy '${policy:-unset}', which PATCH cannot change; delete it in the dashboard (no live deployments) and re-run"
      exit 1
    fi
    status="$(call PATCH "${path}" "${body}")"
    [[ "${status}" == 200 ]] || fail "updating Edgegap version ${app}/${version} failed" "${status}"
    echo "updated Edgegap version ${app}/${version} → ${image}:${tag}"
    ;;
  *) fail "reading Edgegap version ${app}/${version} failed" "${status}" ;;
esac

# Read back what Edgegap now holds rather than trusting the write's answer.
status="$(call GET "${path}")"
[[ "${status}" == 200 ]] || fail "reading back Edgegap version ${app}/${version} failed" "${status}"
jq -e --arg tag "${tag}" '.docker_tag == $tag and .restart_policy == "never" and .is_active' \
  "${work}/out" >/dev/null || fail "Edgegap version ${app}/${version} does not hold ${tag} with restart_policy never" "${status}"
echo "Edgegap version ${app}/${version} runs ${image}:${tag}"
