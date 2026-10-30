#!/usr/bin/env bash
# edgegap-register-2vcpu.sh <version> — register <version>-2vcpu: the same image and tag Edgegap holds
# for <version>, 2048 vCPU units, max_duration 120 (a profiling version's backstop), through
# scripts/ci/edgegap-version.sh. The token reaches that script only through its environment.
set +x
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd -P)"
v="$1"
line="$("$here/edgegap-read.sh" "$v")"
tag="$(jq -r '.docker_tag' <<<"$line")"
img="$(jq -r '.docker_image' <<<"$line")"
[[ -n "$tag" && "$tag" != null && "$img" == vikng-dev/overmatch-server ]] || { echo "no image for $v: $line" >&2; exit 1; }
EDGEGAP_TOKEN="$(<"$HOME/.config/overmatch/edgegap.token")" EDGEGAP_REQ_CPU=2048 EDGEGAP_MAX_DURATION=120 \
  /Users/Yan/github/vikng-dev/overmatch/.claude/worktrees/bay-3/scripts/ci/edgegap-version.sh \
  "$v-2vcpu" "ghcr.io/$img" "$tag"
"$here/edgegap-read.sh" "$v" "$v-2vcpu"
