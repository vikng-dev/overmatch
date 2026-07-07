# Deploy — TEMPORARY test-bed VPS

> **This is throwaway infrastructure.** A single hand-provisioned DigitalOcean droplet running
> the dedicated server under systemd. It exists only to let a couple of friends join the same
> server over the internet and shoot at each other while we shake out connection/combat feel.
>
> **Migration target:** move to **Fly.io** (or **PlayFlow** for match-based, allocated servers)
> once the join-and-shoot loop feels right. When that happens, delete this file's runbook and
> replace it with the real deploy. Do not build tooling on top of this droplet — it's disposable.

## The droplet (as of 2026-07-07)

| | |
|---|---|
| Name | `overmatch-server-sgp` (DigitalOcean id `582714013`) |
| Region / size | `sgp1` (Singapore), Ubuntu 24.04, 1 GB RAM / 1 vCPU / 25 GB |
| **Public IP** | **`157.245.48.161`** |
| Client connects with | `OVERMATCH_SERVER=157.245.48.161` (port 5888 is the default; bare IP is fine) |
| SSH | `ssh -i ~/.ssh/do-vikng-dev root@157.245.48.161` |

There is a second, unrelated droplet on the account (`euphoria-paper-1`, `178.128.221.176`) —
not part of overmatch.

## The service

systemd unit `/etc/systemd/system/overmatch-server.service`, `enabled` + `Restart=on-failure`
(self-heals across crashes/reboots), listening **UDP `0.0.0.0:5888`**:

```ini
[Service]
Type=simple
WorkingDirectory=/opt/overmatch-server
Environment=BEVY_ASSET_ROOT=/opt/overmatch-server
Environment=SPIKE_PERTURB=0
ExecStart=/opt/overmatch-server/server
Restart=on-failure
RestartSec=3
```

Payload on the droplet: `/opt/overmatch-server/{server,assets/}`.

Common ops:

```bash
ssh -i ~/.ssh/do-vikng-dev root@157.245.48.161
systemctl status overmatch-server        # health
journalctl -u overmatch-server -f        # live logs (SIM-EVIDENCE heartbeat every ~2s)
systemctl restart overmatch-server       # after a redeploy
systemctl stop overmatch-server          # stop the meter when not testing
```

> The idle server still burns CPU and ~$6/mo. **Stop the service (or power off the droplet)
> when not actively playtesting.**

## Redeploy (new server build → droplet)

The server binary must be Linux x86_64. The dev machine is an ARM Mac, so we build it on
GitHub's `ubuntu-latest` runner (which *is* the deploy target — glibc 2.39, matched by the
droplet). See `.github/workflows/server-build.yml`.

```bash
# 1. Build on CI (workflow_dispatch — pre-alpha, no build-on-push)
gh workflow run "Server build"
gh run watch                                   # wait for green
gh run download <run-id> -n overmatch-server-x86_64-linux

# 2. Ship + swap
scp -i ~/.ssh/do-vikng-dev overmatch-server.tar.gz root@157.245.48.161:/opt/
ssh -i ~/.ssh/do-vikng-dev root@157.245.48.161 '
  cd /opt && tar xzf overmatch-server.tar.gz &&
  systemctl restart overmatch-server &&
  systemctl status overmatch-server --no-pager | head -5'
```

## Known-provisional bits

- **Dev auth token is hardcoded** (fine for a friends playtest; not for anything public).
- **`SPIKE_PERTURB=0`** is baked into the unit — the server runs the deterministic path.
- The full 160 MB `overmatch-server.tar.gz` is left in `/opt` after extraction; harmless, delete
  if disk gets tight.
