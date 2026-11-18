#!/bin/bash
# Shared boot for scripts/aws/harness.py and the matchmaking EC2 provider. The harness uses its
# installed commands over SSH as `sudo -n om-harness <command>`; hosted machines have no SSH key.
#
# BOOT (cloud-init runs it as root with no arguments), in order, each step appending
# `<step> <seconds since kernel start>` to $STATE/phases, a failing one appending `failed <step>`:
#   guard     schedule this machine's own power-off at CAP_MIN minutes. The harness launches every
#             machine with shutdown behaviour `terminate`, so the power-off deletes the machine and
#             its volume whatever happens to the operator.
#   self      install this file as /usr/local/bin/om-harness, settings included.
#   key       the operator's per-run public key for `ubuntu`, the results channel. After `self`, so
#             a machine the operator can reach always answers `state`.
#   packages  (client only) libasound2t64: the unchanged client links libasound.so.2;
#             the dedicated server is built without audio and needs no package install.
#   updates   (hosted only) mask and verify automatic updater services before play.
#   downloaded, verified, unpacked
#             the release archive by explicit tag, checked against the checksum line the operator
#             read from the release's own `.sha256`, unpacked into /opt/overmatch.
#   started   (server) the server under systemd, unit `overmatch-server`.
#   ready     the machine has nothing left to do at boot.
#
# COMMANDS (operator, over SSH):
#   state                 the phases file; the server also its unit state and listening line.
#   host                  CPU count and model.
#   uptime-echo           one CLOCK_BOOTTIME reading per stdin line: the operator's clock offset.
#   start-client <k> <server ip:port> <ticks>
#                         client k as transient unit `om-client-<k>`, playing CLIENT_SCRIPT.
#   client-state <k>      its unit state and the kernel time of its first `input slot` line.
#   sample <unit>...      start the 1 Hz sampler (unit `om-sampler`) of those units and the host.
#   samples               the sampler's lines.
#   journal <unit>        that unit's journal, kernel-time stamped.
# HOSTED COMMANDS (privileged systemd hook):
#   hosted-stop           bounded push-journal, then request power-off even on capture failure.
#   push-journal          redact and upload one capped journal using the root-only report header.
# @settings
set -Eeuo pipefail

STATE=/var/lib/om-harness
INSTALL=/opt/overmatch
# Resolution: storage/pipe byte cap; at most cap + 1 newline-sized records need reading.
JOURNAL_MAX_BYTES=1048576
# Resolution: kill the entire capture/redaction/upload process group, independently of I/O.
JOURNAL_CAPTURE_S=12
# Resolution: curl connection and total deadlines, nested inside the capture kill.
JOURNAL_CONNECT_S=3
JOURNAL_UPLOAD_S=8
# Resolution: end-reply receipt, process exit and Worker alarm/shutdown scheduling jitter.
JOURNAL_GRACE_MARGIN_S=10
# DERIVED Resolution: the Worker waits for capture (upload included) plus reply/exit scheduling.
# shellcheck disable=SC2034 # Consumed by embed-host.mjs as the Worker grace.
JOURNAL_GRACE_S=$((JOURNAL_CAPTURE_S + JOURNAL_GRACE_MARGIN_S))
# Resolution: service process teardown and dispatch of the privileged post-stop hook.
JOURNAL_STOP_MARGIN_S=5
# DERIVED Resolution: systemd permits capture (upload included) plus local teardown/dispatch.
JOURNAL_STOP_S=$((JOURNAL_CAPTURE_S + JOURNAL_STOP_MARGIN_S))
JOURNAL_MAX_LINES=$((JOURNAL_MAX_BYTES + 1))

# The scripted client's session (src/net/harness.rs `Script`): hull parked, never firing, turret
# sweeping at CLIENT_SCRIPT's taste so every tick carries changing aim, as a scanning player does.
CLIENT_SCRIPT='(drive: Idle, aim: Sweep(radius_m: 800, rate_rad_s: 1.28), ticks: %s)'

uptime_s() {
  local up _
  read -r up _ </proc/uptime
  printf '%s' "$up"
}

mark() { printf '%s %s\n' "$1" "$(uptime_s)" >>"$STATE/phases"; }

boot_failed() {
  trap - ERR
  mark "failed $step" || echo 'boot phase recording failed' >&2
  if [[ "$ROLE" == hosted ]]; then
    # Unit creation is the journal fact; stop its hook synchronously before fallback power-off.
    local journal=false
    [[ ! -f /etc/systemd/system/overmatch-server.service ]] || journal=true
    curl --silent --show-error --fail --max-time 10 \
      --header @/etc/overmatch-report-header --header 'Content-Type: application/json' \
      --data "{\"reason\":\"boot-$step\",\"journalAvailable\":$journal}" \
      "$HOSTED_API/v1/servers/$HOSTED_SERVER_ID/end" >/dev/null 2>&1 || echo 'boot report failed' >&2
    if [[ -f /etc/systemd/system/overmatch-server.service ]]; then
      systemctl stop overmatch-server || echo 'failed unit stop reported failure' >&2
    fi
    systemctl --no-block poweroff
  fi
  exit 1
}

boot() {
  step=guard
  trap boot_failed ERR
  install -d -m 755 "$STATE"
  mark userdata
  if [[ "$ROLE" == hosted ]]; then
    # Root-only files; systemd reads the environment before dropping to the service user.
    umask 077
    printf 'Authorization: Bearer %s\n' "$HOSTED_REPORT_SECRET" >/etc/overmatch-report-header
    printf '%s' "$HOSTED_ENV_B64" | base64 --decode >/etc/overmatch-host.env
  fi
  shutdown -h "+$CAP_MIN"
  mark guard

  step=self
  if [[ "$ROLE" == hosted ]]; then
    install -m 700 "$0" /usr/local/bin/om-harness
  else
    install -m 755 "$0" /usr/local/bin/om-harness
  fi
  mark self

  if [[ "$ROLE" != hosted ]]; then
    step=key
    install -d -m 700 -o ubuntu -g ubuntu /home/ubuntu/.ssh
    printf '%s\n' "$PUBKEY" >>/home/ubuntu/.ssh/authorized_keys
    chown ubuntu:ubuntu /home/ubuntu/.ssh/authorized_keys
    chmod 600 /home/ubuntu/.ssh/authorized_keys
    mark key
  fi

  if [[ "$ROLE" == client ]]; then
    step=packages
    if ! dpkg-query -W -f '${Status}' libasound2t64 2>/dev/null | grep -q 'install ok installed'; then
      apt-get -o DPkg::Lock::Timeout=300 update -q
      DEBIAN_FRONTEND=noninteractive apt-get -o DPkg::Lock::Timeout=300 install -y -q \
        --no-install-recommends libasound2t64
    fi
    mark packages
  fi

  if [[ "$ROLE" == hosted ]]; then
    step=updates
    # Ubuntu's apt timers can run at startup. Mask before starting play; absent units are harmless.
    # The image's snap packaging varies, so cover both timer and daemon refresh paths defensively.
    local updaters=(apt-daily.timer apt-daily.service apt-daily-upgrade.timer
      apt-daily-upgrade.service unattended-upgrades.service snapd.service snapd.socket
      snapd.snap-repair.timer snapd.snap-repair.service)
    systemctl mask --now "${updaters[@]}" || echo 'updater stop reported failure; verifying masks and active state' >&2
    local updater status load active
    for updater in "${updaters[@]}"; do
      # is-enabled returns nonzero for a masked unit, including a mask for an absent package.
      status="$(systemctl is-enabled "$updater" 2>/dev/null)" || true
      [[ "$status" == masked ]]
      # show succeeds for absent units (LoadState=not-found). A query failure enters boot_failed.
      status="$(systemctl show --property=LoadState --property=ActiveState "$updater")"
      load='' active=''
      while IFS='=' read -r key value; do
        case "$key" in LoadState) load="$value" ;; ActiveState) active="$value" ;; esac
      done <<<"$status"
      [[ -n "$load" ]]
      if [[ "$load" != not-found ]]; then
        [[ "$active" == inactive || "$active" == failed ]]
      fi
    done
    mark updates
  fi

  step=download
  local archive="$STATE/server.tar.gz"
  curl -fsSL --retry 3 -o "$archive" "$ARCHIVE_URL"
  mark downloaded

  step=verify
  # The release edge checks the checksum's asset name; verification uses the fixed local name.
  printf '%s  %s\n' "${ARCHIVE_SHA256%% *}" "$archive" | sha256sum --check --strict --quiet -
  mark verified

  step=unpack
  install -d -m 755 "$INSTALL"
  tar -xzf "$archive" --strip-components=1 -C "$INSTALL"
  # Delete rule: spell out the path so the delete cannot follow a changed variable.
  rm /var/lib/om-harness/server.tar.gz
  mark unpacked

  if [[ "$ROLE" == server || "$ROLE" == hosted ]]; then
    local environment stop_post
    step=user
    useradd --system --no-create-home --shell /usr/sbin/nologin overmatch
    if [[ "$ROLE" == hosted ]]; then
      environment='EnvironmentFile=/etc/overmatch-host.env'
      stop_post="ExecStopPost=+/usr/local/bin/om-harness hosted-stop
TimeoutStopSec=${JOURNAL_STOP_S}s"
    else
      environment="Environment=OVERMATCH_BOT=$BOTS OVERMATCH_SERVER_STATS_S=$STATS_S"
      stop_post=''
    fi
    step=start
    cat >/etc/systemd/system/overmatch-server.service <<UNIT
[Unit]
Description=Overmatch dedicated server
Wants=network-online.target
After=network-online.target

[Service]
User=overmatch
NoNewPrivileges=yes
WorkingDirectory=$INSTALL
$environment
Environment=NO_COLOR=1
ExecStart=$INSTALL/overmatch-server
LogRateLimitIntervalSec=0
$stop_post
UNIT
    systemctl daemon-reload
    systemctl start overmatch-server
    mark started
  fi
  mark ready
}

# One privileged post-stop command owns capture then power-off. An asynchronous power-off in an
# earlier ExecStopPost could cancel the upload. The outer timeout kills journalctl/redaction/curl
# together; neither a failed upload nor a stuck journal reader can hold the machine alive.
cmd_hosted_stop() {
  if ! timeout --signal=KILL "$JOURNAL_CAPTURE_S" /usr/local/bin/om-harness push-journal; then
    echo 'journal push failed; powering off' >&2
  fi
  systemctl --no-block poweroff
}

cmd_push_journal() {
  umask 077
  # Bound records and bytes before Python; raw bytes stay in the pipe, redacted output in tmpfs.
  journalctl -b -n "$JOURNAL_MAX_LINES" -u overmatch-server -o short-monotonic --no-pager -q |
    tail -c "$((JOURNAL_MAX_BYTES + 1))" |
    python3 -c '
import base64, pathlib, re, sys
cap = int(sys.argv[1])
raw = sys.stdin.buffer.read()
truncated = len(raw) > cap
# A cut line may have lost its credential label. Discard it before pattern matching.
if truncated:
    raw = raw.split(b"\n", 1)[1] if b"\n" in raw else b""
text = raw.decode("utf-8", errors="ignore")
credential = pathlib.Path("/etc/overmatch-report-header").read_text().strip().removeprefix("Authorization: Bearer ")
forms = [credential, base64.b64encode(credential.encode()).decode()]
if re.fullmatch(r"[0-9a-f]{64}", credential, re.I):
    hexes = [credential.lower(), credential.upper()]
    forms = [p + h for h in hexes for p in ("0x", "0X")] + forms + hexes
    forms += [base64.b64encode(h.encode()).decode() for h in hexes]
    forms += [base64.b64encode(bytes.fromhex(credential)).decode()]
for form in forms:
    text = text.replace(form, "[credential]")
# Multiline values and split Debug arrays are unmasked: no src/net or vendored netcode emitter.
# Live report/netcode secrets are caught by their hex/array shapes.
text = re.sub(r"(\bOVERMATCH_(?:[A-Z_]+[ \t]*=|(?=[A-Z_]*(?:SECRET|KEY|TOKEN|TICKET))[A-Z_]+[ \t]*:))[^\n]*", r"\1[credential]", text, flags=re.I)
text = re.sub(r"((?<![A-Za-z0-9])(?:ticket|secret|tokens?|password|authorization|private[_ -]?key|netcode[_ -]?key|env_vars|environment)\b[\x22\x27]?[ \t]*[:=][ \t]*(?:(?:Bearer|Basic)[ \t]+)?)(?:\[[^\[\]\n]*\]|\{[^{}\n]*\}|\x22(?:\\[^\n]|[^\x22\\\n])*\x22|\x27(?:\\[^\n]|[^\x27\\\n])*\x27|[^\s,;]+)", r"\1[credential]", text, flags=re.I)
text = re.sub(r"https?://\S+", "[url]", text, flags=re.I)
text = re.sub(r"(?<![0-9a-f])(?:0x)?[0-9a-f]{64,}(?![0-9a-f])", "[credential]", text, flags=re.I)
text = re.sub(r"(?<![A-Za-z0-9_-])[A-Za-z0-9_-]+\.[0-9a-f]{32}\b", "[ticket]", text, flags=re.I)
# NetcodeConfig Debug exposes byte arrays; no server path emits its key as base64.
text = re.sub(r"\[(?:[ \t]*[0-9]{1,3}[ \t]*,){31}[ \t]*[0-9]{1,3}[ \t]*\]", "[credential]", text)
def ipv6(match):
    token = match[0]
    colon = token.find(":")
    if colon < 0: return token
    prefix = token[:colon + 1] if colon > 0 and re.search(r"[g-z_]", token[:colon], re.I) else ""
    candidate = token[len(prefix):]
    return prefix + "[ip]" if re.fullmatch(r"[0-9a-f:.]+(?:%[a-z0-9]+)?", candidate, re.I) and re.search(r"[0-9a-f]", candidate, re.I) and ("::" in candidate or candidate.count(":") >= 3) else token
text = re.sub(r"[a-z0-9_:.%]+", ipv6, text, flags=re.I)
text = re.sub(r"(?<![a-z0-9_-])ip-(?:[0-9]{1,3}-){3}[0-9]{1,3}(?![a-z0-9_-])", "[ip]", text, flags=re.I | re.ASCII)
text = re.sub(r"(?:[0-9]{1,3}\.){3}[0-9]{1,3}", "[ip]", text)
encoded = text.encode("utf-8")
truncated = truncated or len(encoded) > cap
pathlib.Path("/run/overmatch-journal").write_bytes(encoded[-cap:].decode("utf-8", errors="ignore").encode("utf-8"))
pathlib.Path("/run/overmatch-journal-header").write_text("X-Journal-Truncated: " + str(int(truncated)) + "\n")
' "$JOURNAL_MAX_BYTES"
  # One attempt inside the whole-capture kill; upload failure never postpones power-off.
  curl --silent --show-error --fail --connect-timeout "$JOURNAL_CONNECT_S" --max-time "$JOURNAL_UPLOAD_S" \
    --header @/etc/overmatch-report-header --header @/run/overmatch-journal-header \
    --header 'Content-Type: text/plain; charset=utf-8' --data-binary @/run/overmatch-journal \
    "$HOSTED_API/v1/servers/$HOSTED_SERVER_ID/journal" >/dev/null 2>&1
}

# The kernel time of the first journal line of <unit> holding <text>; nothing when the query
# succeeds and no line holds it. A failed query fails the caller, its error on stderr.
first_line_s() {
  local journal line
  journal="$(journalctl -u "$1" -o short-monotonic --no-pager -q)" || return
  line="$(grep -m1 -F -- "$2" <<<"$journal")" || [[ $? == 1 ]] || return
  [[ -z "$line" ]] || sed -E 's/^\[ *([0-9.]+)\].*/\1/' <<<"$line"
}

cmd_state() {
  cat "$STATE/phases" 2>/dev/null || true
  if [[ "$ROLE" == server && -f /etc/systemd/system/overmatch-server.service ]]; then
    echo "unit $(systemctl is-active overmatch-server || true)"
    local at
    at="$(first_line_s overmatch-server 'server: starting, listening')"
    [[ -z "$at" ]] || echo "listening $at"
  fi
}

cmd_uptime_echo() {
  local _line
  while read -r _line; do
    uptime_s
    echo
  done
}

cmd_start_client() {
  local k="$1" server="$2" ticks="$3"
  # shellcheck disable=SC2059 # CLIENT_SCRIPT is the format.
  printf "$CLIENT_SCRIPT\n" "$ticks" >"$STATE/client-$k.ron"
  systemd-run --quiet --unit "om-client-$k" -p User=ubuntu -p WorkingDirectory="$INSTALL" \
    --setenv=OVERMATCH_SERVER="$server" --setenv=OVERMATCH_HARNESS="$STATE/client-$k.ron" \
    --setenv=NO_COLOR=1 "$INSTALL/overmatch"
}

cmd_client_state() {
  local unit="om-client-$1" at
  echo "unit $(systemctl is-active "$unit" || true)"
  at="$(first_line_s "$unit" 'input slot')"
  [[ -z "$at" ]] || echo "slot $at"
}

# One line per second: `<boot s> <unit>=<cpu usec>,<rss kB> ... cpu <the /proc/stat cpu fields>`.
# Builtins only inside the loop, so the sampler's own cost stays out of what it samples.
cmd_sample_loop() {
  local units=("$@") pids=() unit line key value cpu rss stat
  for unit in "${units[@]}"; do pids+=("$(systemctl show -p MainPID --value "$unit")"); done
  while :; do
    line="$(uptime_s)"
    for i in "${!units[@]}"; do
      cpu=- rss=-
      while read -r key value; do
        [[ "$key" == usage_usec ]] && cpu="$value" && break
      done <"/sys/fs/cgroup/system.slice/${units[$i]}.service/cpu.stat" || true
      while read -r key value _; do
        [[ "$key" == VmRSS: ]] && rss="$value" && break
      done <"/proc/${pids[$i]}/status" || true
      line+=" ${units[$i]}=$cpu,$rss"
    done
    read -r stat </proc/stat
    printf '%s %s\n' "$line" "$stat" >>"$STATE/samples"
    sleep 1
  done
}

cmd_sample() {
  systemd-run --quiet --unit om-sampler /usr/local/bin/om-harness sample-loop "$@"
}

main() {
  case "${1:-}" in
    "") boot ;;
    hosted-stop) cmd_hosted_stop ;;
    push-journal) cmd_push_journal ;;
    state) cmd_state ;;
    host) printf 'cpus %s\nmodel %s\n' "$(nproc)" "$(sed -n 's/^model name[[:space:]]*: //p' /proc/cpuinfo | head -1)" ;;
    uptime-echo) cmd_uptime_echo ;;
    start-client) shift; cmd_start_client "$@" ;;
    client-state) shift; cmd_client_state "$@" ;;
    sample) shift; cmd_sample "$@" ;;
    sample-loop) shift; cmd_sample_loop "$@" ;;
    samples) cat "$STATE/samples" 2>/dev/null || true ;;
    journal) journalctl -u "$2" -o short-monotonic --no-pager -q ;;
    *) echo "om-harness: unknown command '$1'" >&2; exit 2 ;;
  esac
}

main "$@"
