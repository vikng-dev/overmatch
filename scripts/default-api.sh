# default-api.sh — sourced, never run: exports OVERMATCH_DEFAULT_API, the matchmaking Worker URL a
# distributed client bakes in through `option_env!("OVERMATCH_DEFAULT_API")` (net::client).
#
#     source scripts/default-api.sh
#
# THE ONE SOURCE of the production Worker URL: release.yml and every local distribution script
# (`build-linux.sh`, `package-macos.sh`) source this file. A value already in the environment wins,
# so a build can point at another Worker; an empty one takes the default. A client built without
# it falls back to `127.0.0.1:5888` and finds nobody, so a blank result is refused. Not a secret:
# the Worker's public hostname.
: "${OVERMATCH_DEFAULT_API:=https://overmatch-backend.vikng.workers.dev}"
if [[ -z "${OVERMATCH_DEFAULT_API// /}" ]]; then
  echo "default-api.sh: OVERMATCH_DEFAULT_API is blank — the client would ship with nowhere to connect" >&2
  return 1
fi
export OVERMATCH_DEFAULT_API
