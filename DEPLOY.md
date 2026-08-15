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
ExecStart=/opt/overmatch-server/overmatch-server
Restart=on-failure
RestartSec=3
```

Payload on the droplet: `/opt/overmatch-server/{overmatch-server,assets/}` — and `assets/` there is
**only what the server opens**, an allowlist stated in `.github/actions/build-server/action.yml`
rather than the client's whole tree:

| Path | Opened by |
|---|---|
| `assets/maps/*/level.json`, `assets/maps/*/*.png` | `crate::map::parse`, the terrain height decode, and the handshake content digest. Every map: `$OVERMATCH_MAP` picks one at runtime, and the heightmap's file name is inside `level.json`. |
| `assets/<id>/<id>.sim.glb` | the bake's consumer contract, the `geometry_lod` trio fingerprint, and the track rig — the one geometry artifact the server reads (ADR-0035) |
| `assets/<id>/<id>.lod.json` | `geometry_lod::load_certificate` (panics without it) |
| `assets/<id>/<id>.tank.ron` | the spec sheet, through the `AssetServer` |

The 64 MB view glb, `shell/shell.glb`, the terrain KTX2, the vfx atlases, the shaders and the fonts
stay out: every one is loaded by a plugin the server does not mount, or behind the
`!windows.is_empty()` branch a window-less server never takes. The shell model in particular belongs
to `ballistics::view_plugin` — the server mounts `ballistics::sim_plugin`, whose projectiles carry no
scene root at all. `materials/materials.ron` is `include_str!`d into the binary.

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

## Deploy (release → droplet)

**Cutting a `vX.Y.Z` tag redeploys the droplet.** The `Release` workflow
(`.github/workflows/release.yml`) builds the Linux `overmatch-server` from the tag, `scp`s *that
exact artifact* to `/opt`, extracts it into `/opt/overmatch-server`, `systemctl restart
overmatch-server`, and verifies (`systemctl is-active` + echoes the deployed git SHA into the run
log via a baked `DEPLOYED_SHA` marker). The GitHub Release is created as a **draft** and is
un-drafted only after that deploy succeeds, so the latest visible release and the running droplet
are always the same build; a failed deploy leaves no new release visible.

- **Auth:** the workflow uses the `DROPLET_SSH_KEY` repo Actions secret (a dedicated ed25519
  key whose public half is in the droplet's `authorized_keys`). The droplet's host key is
  **pinned** in the workflow (no `StrictHostKeyChecking=no`); if you ever rebuild the droplet,
  update the pinned `known_hosts` line in `release.yml` to match its new host key.
- **Only tag pushes deploy.** Pushes to `main` run CI only — the droplet does not track `main`.
  `workflow_dispatch` is build-only smoke mode: it builds the full matrix off the dispatched ref
  and touches neither the droplet nor any release.
- **Serialized:** a `concurrency` group queues overlapping deploys (never cancels a run that may
  be mid-scp/restart on the droplet).
- **Heads-up:** the restart is automatic, so **tagging a release mid-playtest will bounce the live
  server** and drop connected friends for a few seconds. Hold the tag while a session is in
  progress.
- **The droplet runs the last release, not `main`.** A client built from a `main` that has moved
  past the tag will be refused at the handshake by the build fingerprint
  (`.agents/docs/adr/0018-wire-surface-fingerprinted-and-refused.md`). Playtest `main` against a
  local server, or cut a release.

## Redeploy (manual fallback — new server build → droplet)

The release deploy above is the normal path; this manual procedure stays as a fallback (e.g. to
put a non-release build on the droplet, or if the deploy job is unavailable).

The server binary must be Linux x86_64. The dev machine is an ARM Mac, so we build it on
GitHub's `ubuntu-latest` runner (which *is* the deploy target — glibc 2.39, matched by the
droplet). The `Release` workflow's `workflow_dispatch` path uploads the `overmatch-server` tarball
as a plain artifact (see `.github/workflows/release.yml`).

```bash
# 1. Build on CI (workflow_dispatch — builds, deploys nothing, publishes nothing)
gh workflow run "Release"
gh run watch                                   # wait for green
# --repo is required when downloading outside the checkout (e.g. into a scratch dir):
# gh infers the repo from the surrounding git tree, and a scratch dir has none.
gh run download <run-id> -n overmatch-server-x86_64-linux --repo vikng-dev/overmatch

# 2. Ship + swap
scp -i ~/.ssh/do-vikng-dev overmatch-server.tar.gz root@157.245.48.161:/opt/
ssh -i ~/.ssh/do-vikng-dev root@157.245.48.161 '
  cd /opt && tar xzf overmatch-server.tar.gz &&
  systemctl restart overmatch-server &&
  systemctl status overmatch-server --no-pager | head -5'
```

## Known-provisional bits

- **Dev auth token is hardcoded** (fine for a friends playtest; not for anything public).
- The `overmatch-server.tar.gz` is left in `/opt` after extraction; harmless, delete if disk gets
  tight. It carries the server's asset allowlist (~13 MB of assets), not the client's tree.
