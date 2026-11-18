#!/bin/bash
# @settings
set -Eeuo pipefail

# Boot/command invariants: .agents/scratch/aws-journal-slice-4.md
STATE=/var/lib/om-harness
INSTALL=/opt/overmatch
# Resolution: storage/pipe byte cap; at most cap + 1 newline-sized records need reading.
JOURNAL_MAX_BYTES=1048576
# Resolution: whole process-group kill, nested curl deadlines, and reply/exit scheduling margin.
JOURNAL_CAPTURE_S=12
JOURNAL_CONNECT_S=3
JOURNAL_UPLOAD_S=8
JOURNAL_GRACE_MARGIN_S=10
# Resolution: conservative capture + upload + margin, shared by the Worker grace and service stop.
JOURNAL_STOP_S=$((JOURNAL_CAPTURE_S + JOURNAL_UPLOAD_S + JOURNAL_GRACE_MARGIN_S))
JOURNAL_MAX_LINES=$((JOURNAL_MAX_BYTES + 1))

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
    local updaters=(apt-daily.timer apt-daily.service apt-daily-upgrade.timer
      apt-daily-upgrade.service unattended-upgrades.service snapd.service snapd.socket
      snapd.snap-repair.timer snapd.snap-repair.service)
    systemctl mask --now "${updaters[@]}" || echo 'updater stop reported failure; verifying masks and active state' >&2
    local updater status load active
    for updater in "${updaters[@]}"; do
      status="$(systemctl is-enabled "$updater" 2>/dev/null)" || true
      [[ "$status" == masked ]]
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
  printf '%s  %s\n' "${ARCHIVE_SHA256%% *}" "$archive" | sha256sum --check --strict --quiet -
  mark verified

  step=unpack
  install -d -m 755 "$INSTALL"
  tar -xzf "$archive" --strip-components=1 -C "$INSTALL"
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

cmd_hosted_stop() {
  if ! timeout --signal=KILL "$JOURNAL_CAPTURE_S" /usr/local/bin/om-harness push-journal; then
    echo 'journal push failed; powering off' >&2
  fi
  systemctl --no-block poweroff
}

cmd_push_journal() {
  umask 077
  journalctl -b -n "$JOURNAL_MAX_LINES" -u overmatch-server -o short-monotonic --no-pager -q |
    tail -c "$((JOURNAL_MAX_BYTES + 1))" |
    python3 -c '
import base64, pathlib, re, sys
cap = int(sys.argv[1])
raw = sys.stdin.buffer.read()
truncated = len(raw) > cap
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
text = re.sub(r"(\bOVERMATCH_[A-Z_]+[ \t]*=)[^\n]*", r"\1[credential]", text, flags=re.I)
text = re.sub(r"(\b(?:ticket|secret|token|password|authorization|private[_ -]?key|netcode[_ -]?key|env_vars|environment)\b[\x22\x27]?[ \t]*[:=][ \t]*(?:Bearer[ \t]+)?)(?:\[[^\[\]\n]*\]|\{[^{}\n]*\}|\x22[^\x22\n]*\x22|\x27[^\x27\n]*\x27|[^\s,;]+)", r"\1[credential]", text, flags=re.I)
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
text = re.sub(r"(?:[0-9]{1,3}\.){3}[0-9]{1,3}", "[ip]", text)
encoded = text.encode("utf-8")
truncated = truncated or len(encoded) > cap
pathlib.Path("/run/overmatch-journal").write_bytes(encoded[-cap:].decode("utf-8", errors="ignore").encode("utf-8"))
pathlib.Path("/run/overmatch-journal-header").write_text("X-Journal-Truncated: " + str(int(truncated)) + "\n")
' "$JOURNAL_MAX_BYTES"
  curl --silent --show-error --fail --connect-timeout "$JOURNAL_CONNECT_S" --max-time "$JOURNAL_UPLOAD_S" \
    --header @/etc/overmatch-report-header --header @/run/overmatch-journal-header \
    --header 'Content-Type: text/plain; charset=utf-8' --data-binary @/run/overmatch-journal \
    "$HOSTED_API/v1/servers/$HOSTED_SERVER_ID/journal" >/dev/null 2>&1
}

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
