# Online profiling readiness: release → Edgegap → hidden client → metrics (audit 2026-10-30)

Read-only audit of `main` @ `ff4a3cd`. Nothing was edited, built, pushed or deployed. The Edgegap
calls were three GETs, and the token was read only into a 0600 header file. Number labels:
MEASURED (observed by me today, or by a named past run), DERIVED (arithmetic on labelled inputs),
CLAIMED (vendor docs), ASSUMED (not checked).

## TLDR

- **The release path is ready.** The job needs no human, has no approval gate, and main is green.
  Its one manual input is the version bump: `Cargo.toml` still says `0.6.0`, the version already
  released.
  - Edgegap holds the new server version about 16 min after the tag push. MEASURED on v0.6.0.
  - Publish follows 40–68 min after the push. MEASURED over 8 tag runs.
- **A direct Edgegap deploy works and is the only hands-off route.**
  - The token is valid: 3 GETs returned 200, and 0 deployments are live. MEASURED today.
  - The scripted client cannot go through the Worker. Simulate mode forces a direct link with the
    all-zero dev key (`src/net/client.rs:285-287,615`).
  - So the server must be deployed **without** the hosted env vars.
- **Client metrics are mostly ready.** `SPIKE_FRAME_COST`, the drawn counter and the offscreen mode
  all mount in the online client. The hidden scripted mode exists.
  - **MISSING:** no perf runner has an online condition. `frame-run.zsh:143` hardcodes `--offline`.
- **BLOCKER: the remote server's metrics.** Its tick cost goes only to a *file*
  (`SPIKE_COST_TRACE`), and Edgegap gives no file access, only stdout logs while the deployment
  runs.
  - A server stdout stats line must land **before the tag**, because the server image is built
    from the tag.
- **Cost guard: MISSING.** A non-hosted server never exits on its own, and the Worker's sweep ignores
  it. Edgegap ends it only at `max_duration` 1440 min, a worst case of about $1.66 (DERIVED).
  - The run script must stop the deployment itself, with a watchdog.

---

## 1. The release path

| # | Item | Status | Evidence |
|---|---|---|---|
| 1.1 | Pipeline steps | READY | `.github/workflows/release.yml`. Jobs: `create-release` (draft, :84), `build-client` win+linux (:213), `build-server` (:379), `smoke-linux` / `smoke-windows` (:430, :501), `register-edgegap` (:568, runs `scripts/ci/edgegap-version.sh`), `upload-*` (:340, :600), and `publish-release` (:790, needs registration + both smokes + uploads). `field-casts` is dispatched and gates nothing (:191). macOS is on demand only (:43-54, :636). |
| 1.2 | Version-bump rule | READY (manual input) | The tag must equal `v<Cargo.toml version>`, and the `Cargo.lock` entry for `overmatch` must match (release.yml:97-107). The tag commit must be an ancestor of `origin/main` (:169-180). `Cargo.toml:3` is `version = "0.6.0"`, which is the **already-released** version. So a bump commit (`0.6.1`, or a digit per ADR-0009) plus a lock update is required. |
| 1.3 | Is a release actually needed? | Yes | `PROTOCOL_REV` was 31 at v0.6.0 (`git show v0.6.0:src/net/protocol.rs:143`) and is 37 on main (`src/net/protocol.rs:197`). `materials.ron`, which is folded into the handshake, also changed since. So no client from main can join the live `0.6.0` server. Dispatching `edgegap-version.yml` at 0.6.0 instead would re-point the **published** version's servers (edgegap-version.yml:12-16). Do not do that. |
| 1.4 | Last release | — | `v0.6.0`, Latest, 2026-10-12 22:13Z. main is **390 commits** past it (MEASURED, `git log v0.6.0..origin/main`). |
| 1.5 | Wall-clock time | READY | Tag runs took 40.4, 40.8, 47.2, 52.4, 56.0, 57.5, 64.5 and 68.5 min (MEASURED, `gh run list`). v0.6.0 by job (MEASURED): `build-server` 15.5 min; `register-edgegap` done **+16.4 min** after the push; the Windows client is the long pole at 46.0 min; publish at +47 min. Build-only dispatch runs took 49.5 and 52.5 min. `edgegap-version.yml` dispatch took 13.7 min. |
| 1.6 | main's CI | READY | The last `CI` run on main is green: `22507da`, 2026-10-29, 7.0 min (MEASURED). Later commits are scratch-only and covered by `paths-ignore` (ci.yml:15). Assets and Backend are green at `ff4a3cd`. Older Assets reds (10-27/28) are superseded by greens. |
| 1.7 | release.yml exercised since v0.6.0 | RISK | Its last run was 2026-10-12, 390 commits ago. Nothing has proven the Windows build, the smokes or the asset-certificate step on today's main. A build-only dispatch (`gh workflow run release.yml --ref main`, ~50 min MEASURED) is a free pre-flight. |
| 1.8 | Human gates | READY | No `environment:` key in release.yml, and the repo has no environments or rulesets (MEASURED, `gh api`). Branch protection lists `fmt`, `check` and `assets`, but a direct push to main is accepted (memory `release-authority-standing`). SSH push works non-interactively (MEASURED, `git ls-remote`). Commits and tags are SSH-signed with a key file, and HEAD and v0.6.0 are both signed. The `pre-push` hook only runs `git lfs pre-push` (scripts/hooks/pre-push). |
| 1.9 | Authority | AMBIGUOUS | AGENTS.md:30 says "pushing, tagging and releasing … still need explicit approval". Memory `release-authority-standing` (2026-08-17/18) grants standing release authority, digit included. Get Yan's one-line go for tonight's tag before the night. |

## 2. Launching a server on Edgegap

| # | Item | Status | Evidence |
|---|---|---|---|
| 2.1 | Worker source | READY | It is in this repo, under `backend/`. Routing is in `src/index.ts`, the registry in `src/registry.ts`, and the only Edgegap code in `src/edgegap.ts`. Deployed by hand with wrangler (`backend/README.md:137-151`). URL: `https://overmatch-backend.vikng.workers.dev` (`scripts/default-api.sh:11`). |
| 2.2 | How the Worker starts a deployment | — | `POST /v1/deploy` with `app_name`, `version_name` = the client's version, `ip_list`, a city `filters` entry, `tags:[om-<32hex>]`, `env_vars` and `webhook_url` (`backend/src/edgegap.ts:132-145`). The env carries the hosted contract, including a hidden netcode key and report secret (`backend/src/registry.ts:471-481`). A Worker-started server therefore holds a **random key**, which a scripted client (zero key) cannot join. |
| 2.3 | Orchestrator deploys directly with `~/.config/overmatch/edgegap.token` | READY | MEASURED today: `GET /v1/apps` returned 200 (`Overmatch`, active). `GET /v1/app/Overmatch/versions` returned 200: `risk-3f9b93d` (max 30 min, 2 GB), `0.5.4` (max 360), and `0.6.0` (tag `0.6.0-7cf9974`, 1 vCPU / 1 GB, max_duration 1440, restart never). `GET /v1/deployments` returned 200, total_count 0. The same token is the Worker's and CI's (memory `credentials-local`). It deployed in the 2026-10-12 risk test (MEASURED then). Write rights are DERIVED from that. Not re-tested: I made no writes. |
| 2.4 | Address, port | READY | `GET /v1/status/{id}`: `running`, `public_ip`, `ports.game.external` (`edgegap.ts:215-235`). READY after 3.3 s warm and 7–12.5 s cold (MEASURED, risk-test.md:22-24). Placement with a city filter is deterministic. Singapore was READY in 4.6 s (MEASURED, decisions.md:100-105). A city with no capacity errors out within about 1.5 s (Frankfurt, MEASURED). |
| 2.5 | Logs | READY, with limits | `GET /v1/deployment/{id}/container-logs` returns stdout and stderr **while running** (MEASURED, risk-test.md:77). They are **deleted when the deployment stops** unless Endpoint Storage is configured (CLAIMED: docs.edgegap.com/docs/deployment; OpenAPI `deployment-logs-get`). It is not configured. Size or line cap: unknown (ASSUMED risk). There is no exec or file-pull API (MEASURED: grep of the v1/v2 OpenAPI). A file written in the container is **unreachable**. |
| 2.6 | Metrics | PARTIAL | `GET /v1/metrics/deployment/{id}?raw=true` (with `start_time`, `end_time`, `steps`). Memory and cumulative tx/rx come back at 1-min samples. The CPU series came back **empty** (MEASURED, risk-test.md:94-111). Pay-as-you-go "unlocks 1 second intervals" (CLAIMED, docs). The account has a card (decisions.md:75). |
| 2.7 | Stop | READY | `DELETE /v1/stop/{id}` sends SIGTERM (CLAIMED, docs). It is idempotent in the Worker (`edgegap.ts:177-185`). Verify with `GET /v1/deployments` → `total_count 0` (MEASURED pattern, risk-test.md:79). There is also a bulk stop: `/v1/deployments/bulk-stop` (OpenAPI). |
| 2.8 | Does it end on its own? | **NO (cost hazard)** | Without `OVERMATCH_SERVER_ID` the server is non-hosted: no heartbeats, no empty-exit (`src/net/hosting.rs:5-6,105-109`). Edgegap ends it at the version's `max_duration` of **1440 min** (edgegap-version.sh:51; MEASURED on `0.6.0` today), or at the 24 h cloud cap (CLAIMED). The Worker's per-minute sweep stops only deployments tagged `om-<32hex>` (`backend/src/sweep.ts:34,66`), so a direct deploy is never swept. No per-deployment duration override exists in `/v1/deploy` (MEASURED, OpenAPI). Self-stop exists as `ARBITRIUM_DELETE_URL/TOKEN`, injected into the container (CLAIMED), but the server does not use it. |
| 2.9 | Cost | — | $0.00115/vCPU·min = **$0.069/vCPU·h**, egress $0.10/GB, a $1 minimum in any month with a deployment (CLAIMED, edgegap.com/pricing.md, reviewed 2026-10-26, fetched today). The registered versions are 1 vCPU, so **$0.069/h compute** (DERIVED). Egress is about 27 KB/s per client (MEASURED, 2 clients, risk-test.md:105), about 0.1 GB/h per client, about $0.01/h (DERIVED). A 45-min session with 4 clients costs about **$0.08** (DERIVED). A server left running to the cap costs about **$1.66** (DERIVED). |

## 3. The client in online mode

| # | Item | Status | Evidence |
|---|---|---|---|
| 3.1 | Address levers | READY | Resolution order: `--local` → `OVERMATCH_SERVER=<host>[:5888]` → runtime `OVERMATCH_API` (Worker) → baked `OVERMATCH_DEFAULT_API` (`option_env!`, client.rs:48) → `127.0.0.1:5888` (`choose_mode`, client.rs:76-100). A local build without `OVERMATCH_DEFAULT_API` set has no Worker baked in. **Simulate mode ignores the Worker** and always connects directly (client.rs:278-287). A direct link uses the all-zero key (client.rs:615); a non-hosted server holds the same key (server.rs:155). |
| 3.2 | Hidden, scripted, rendering online client | READY | `SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1` gives the full presentation stack (`NetClientPlugin`, client.rs:526) in a **hidden** window: `visible: !hidden`, `CaptureWindowPinned`, and `revoke_macos_activation` (client.rs:378-388, 452, 483, 495). The script ends the client cleanly after `SPIKE_SIM_TICKS`. Workloads: idle, forward, reverse, combat, aim-sweep and fire levers (`src/net/harness.rs:6-46`). Audio: set `OVERMATCH_MUTE=1` (`src/sfx/mod.rs:61`), or a hidden client plays sound out loud at night. |
| 3.3 | `SPIKE_FRAME_COST` + drawn counter | READY | `frame_cost::client_plugin` is mounted in the net client (client.rs:541). It registers `count_drawn` in the RenderApp (frame_cost.rs:228-238). In GPU-less simulate mode it mounts but has no RenderApp, so it gives no evidence there. |
| 3.4 | Offscreen (`OFFSCREEN=1` → `SPIKE_FRAME_COST_OFFSCREEN=1`) | READY by reading; **UNTESTED online** | The plugin itself hides the window (`dev_capture::hide_window`) and retargets cameras to a 2560x1440 image; nothing is parked or raised (frame_cost.rs:239-256; dev_capture.rs:160-173). It works on any windowed root, so it should work online. No online offscreen run is on record. The only online frame-cost runner (`run-pencam-cost.sh:197-201`) runs its measured client **visible**, against a **local** server. |
| 3.5 | Other client recorders online | READY | `SPIKE_COST_TRACE` gives the client's fixed-tick cost, which is still UNMEASURED online (performance-target-research.md:264). `SPIKE_RENDER_COST` gives per-pass cost. `SPIKE_TRACE` gives frame and tick rows with link `rtt` and jitter (client.rs:534-541; trace.rs:274-335). RTT also appears in `net: interpolation min_delay … (rtt X ms)` log lines (risk-test.md:88). |
| 3.6 | Client bandwidth | READY (method) | Per process, both directions, through `nettop` socket counters (MEASURED method, risk-test.md:101-105). No in-process counter. |
| 3.7 | Perf runners: online condition | **MISSING** | `frame-run.zsh:143` launches `"$CLIENT" --offline`, and every frame runner sources it. `run-fire-capture.sh:12` is offline by design. The pencam MP condition is visible and local. |
| 3.8 | Fingerprint match | READY | The handshake tag is `PROTOCOL_FINGERPRINT`, which folds the pinned wire hashes, the avian and lightyear versions, `PROTOCOL_REV` and `CARGO_PKG_VERSION` (protocol.rs:201-208). It is then folded with the map content digest and `materials.ron` (protocol.rs:230-246; map.rs:446-466). No git SHA, profile or feature enters it, and `src/net` has no `cfg(feature)` (grep). So a local `--release` or `--profile profiling` build of **the tagged commit** connects, provided its `assets/maps/**` and `assets/trees/*.ron` are byte-identical: LFS hydrated, no CRLF. A build from main past the tag still connects if only scripts changed. Any client at `0.6.0` is refused. |
| 3.9 | Which client binary | DECISION | The release ships no macOS client on the tag path (release.yml:43-54). Build locally in a bay: `scripts/serial-cargo build --locked --release --bin overmatch --bin overmatch-server`, with the shipped feature set and no `dev_ui`, as the perf RUNBOOK precedent does (`.agents/scratch/perf-2026-10-29/RUNBOOK.md:19-25`). This deviates from AGENTS.md:30's `dev_ui,bitprobe`. `dev_ui` mounts an egui panel (lib.rs:943-944), so it is not the shipped client. A cold release build takes 601 s (MEASURED, Cargo.toml:232) or 9 m 36 s (MEASURED, risk-test.md:140). |

## 4. The server's instrumentation

| # | Item | Status | Evidence |
|---|---|---|---|
| 4.1 | Tick cost | EXISTS, but **file-only** → **BLOCKER remotely** | With `SPIKE_COST_TRACE=<path>` the server writes per-tick JSONL rows `{us, mus (projectile march), mc, np, nt, ne}` after a 384-tick warmup (`src/cost.rs:42-129`; mounted at server.rs:76). The path is role-qualified, so stdout cannot be named (`trace.rs:113-127`: `/dev/stdout` becomes `/dev/stdout.server.jsonl`). No file API exists (2.5). |
| 4.2 | Per-system time | MISSING | No tracy or span output in release builds. Only the whole tick and the march share are timed (cost.rs). Out of tonight's scope. |
| 4.3 | Bandwidth | MISSING server-side | No byte counter. lightyear's `metrics` feature (aggregate gauges such as `transport/recv_bytes`) is off (Cargo.toml:50-57). Per-link payloads are reachable through `Link.send/recv.iter()` (lightyear_link 0.30.1 lib.rs:241-290). Substitutes: client `nettop` per client (3.6), and Edgegap cumulative tx/rx (2.6). |
| 4.4 | Entity and tank counts | File-only | `nt`, `np` and `ne` are in the cost rows (cost.rs:113-128). |
| 4.5 | CPU | MISSING | No in-process reading. Edgegap's CPU series was empty (2.6). The local method was `ps -o time` (server-frame-rate.md:26-27). |
| 4.6 | Output | stdout = event logs only | Bevy `LogPlugin` default `info`; the image sets no `RUST_LOG` (`.github/actions/server-image/Dockerfile`). **Flood hazard:** `log_tank_commands` logs at `info!` on **every fixed tick** for every client whose throttle is non-zero or which is firing (server.rs:1038-1047, mounted at :127). That is 64 lines/s per driving client. It pollutes tick cost and fills container logs. |

## 5. Load

| # | Item | Status | Evidence |
|---|---|---|---|
| 5.1 | Extra tanks without clients | PARTIAL: 1 bot | `OVERMATCH_BOT` (set, even empty) spawns **one** ownerless circling bot that fires each reload (server.rs:733-757, 1024-1035). `CombatantIds.bot` is a single `Option` (server.rs:180, 201). `OVERMATCH_PROBE_TANKS` is **offline only** (`src/tank/scenario.rs:463,531`). No N-bot lever exists. |
| 5.2 | Several clients from this Mac | READY | GPU-less scripted clients: `SPIKE_SIMULATE_INPUT=1` without `SPIKE_SIM_WINDOWED` (client.rs:392-405; archive-smoke.sh:17-19,108-112). Client ids are random u64s (client.rs:312-322). Spawn lanes are monotonic and non-overlapping (server.rs:226-228). 5/5 headless clients connected to Edgegap from this Mac (MEASURED, risk-test.md:14-17). Per-client CPU on the M4 is unmeasured (ASSUMED non-trivial: a 2 ms run loop, client.rs:395). Load clients on the same machine contaminate the measured client's frame times. |
| 5.3 | Player cap | — | Non-hosted (tonight): lightyear's default, **256** (`lightyear_netcode-0.30.1/src/server.rs:32`; server.rs:156). Hosted: `OVERMATCH_CAPACITY`, default 10 (hosting.rs:127); Worker `CAPACITY` 10 (wrangler.jsonc:41). |

## 6. Hands-off hazards

| # | Hazard | Status | Evidence / mitigation |
|---|---|---|---|
| 6.1 | Login, 2FA or dialog in the release path | None found | `gh` is authed through the keyring with `repo` scope. SSH and signing are non-interactive (1.8). |
| 6.2 | Manual approval in a workflow | None | No environments (1.8). Only the authority question in 1.9 remains. |
| 6.3 | Visible window | Avoidable | Use the hidden mode in 3.2 plus `OFFSCREEN`. **Never** reuse `run-pencam-cost.sh mp-*` (`SPIKE_SIM_VISIBLE=1`, :199). Without `OFFSCREEN`, frame-cost mode's `raise_when_occluded` asks for focus (frame_cost.rs:391-414). Always pair `SPIKE_FRAME_COST` with `SPIKE_FRAME_COST_OFFSCREEN=1`. |
| 6.4 | macOS firewall prompt | UNVERIFIED | The application firewall is **on** with stealth mode (MEASURED, `socketfilterfw`). No `overmatch` entry is in its app list. A fresh unsigned client receiving UDP *might* prompt. The risk test ran local clients against Edgegap without reporting one. Prove it by day with a hidden run before the night. |
| 6.5 | Cold Metal shader cache | Known | The first run after a rebuild stalls past keepalive, and each reconnect burns a lane (memory `capture-harness-hardening`). Do one discarded offscreen warm-up run after the build. |
| 6.6 | Cost runaway | **MISSING guard** | See 2.8. Required: a `trap` that stops the deployment on any exit, a background watchdog, and a final `total_count 0` check. Optional backstop: a `<ver>-profile` Edgegap version with `max_duration` 120. That needs `edgegap-version.sh` to stop hardcoding 1440 (:51). |
| 6.7 | Handshake refusing an untagged build | Low | See 3.8. Build the client from the tag commit. Confirm `client: connected` and `— input slot` in its log. |
| 6.8 | Logs lost at stop | Known | Pull container logs and metrics **before** `DELETE /v1/stop` (2.5). |
| 6.9 | Placement or capacity | Known | A city can have no capacity (Frankfurt, MEASURED). Fall back through a list, or read `/v1/locations?app=&version=`, which is capacity-aware (risk-test.md:131-133). |
| 6.10 | Measured client knocked out | Possible | Bots fire each reload (server.rs:1033). A scripted client knocked out mid-run turns its frames into the death screen (run-pencam-cost.sh:24-30). Place lanes away from the bot circle, or disable bot fire. |
| 6.11 | Machine | — | `caffeinate -dims`. No compile during measurement (memory `machine-time-night-runs`). Two bays are free now: bay-2 and bay-3 (MEASURED, `scripts/bays/status`). |
| 6.12 | Doc drift (not blocking) | — | DEPLOY.md:29-34 omits `maps/*/prototypes.ron` and `trees/*.ron`. The real allowlist ships them (`.github/actions/build-server/action.yml:110-113`), and `content_digest` reads them (map.rs:451-453). |

## Blockers

1. **The remote server's metrics are not retrievable.** The tick cost goes to a file, Edgegap has
   no file API, and nothing useful reaches stdout (4.1, 2.5). The fix is server code, so it must be
   on main **before the tag**.
2. **No hands-off deploy/stop tooling, no cost guard.** A direct deploy is required (2.2, 3.1). A
   non-hosted deployment runs up to 24 h unless stopped (2.8).
3. **No online perf runner.** Every frame runner launches `--offline` (3.7).

Everything else is a manual step or a hazard with a known mitigation.

## Minimal build list (each one agent session)

**B1 — Server stats on stdout (server, MUST merge before the tag).**
- New `src/net/server_stats.rs`, mounted from `server::run` and armed by an env lever (e.g.
  `OVERMATCH_SERVER_STATS_S=<period>`).
- Every period it logs one line on its own log target: `server_stats {json}`.
- The JSON carries:
  - fixed ticks run, and tick wall µs p50/p95/max. Bracket the tick with `FixedFirst`/`FixedLast`
    as in `cost.rs`.
  - frame-loop wall p50/max and the count of frames longer than `FRAME`.
  - connected clients, tanks, projectiles and entities.
  - process CPU seconds and RSS: Linux `/proc/self/stat` and `/proc/self/statm`, null elsewhere.
  - optionally, per-link payload bytes in and out, through `Link.send.iter()` before
    `LinkSystems::Send` and `Link.recv.iter()` after Receive. The builder verifies which layer the
    server-side `Link` holds in lightyear 0.30.1.
- Demote `log_tank_commands` (server.rs:1038) to `debug!`.
- No wire change, so no `PROTOCOL_REV` bump.
- Test: the period aggregator. Percentiles and reset per period, named for the mutant they catch.
- Verify locally against a release server and a GPU-less client.

**B2 — N server bots (server, merge before the tag; RECOMMENDED, not blocking).**
- `OVERMATCH_BOT=<n>`. An empty or present value keeps today's 1.
- `CombatantIds.bot` becomes per-bot. Bots spawn at distinct lane points, each respawns on its own,
  plus an optional `OVERMATCH_BOT_FIRE=0`.
- This is the only way to put 10–30 tanks on the server without 10–30 clients.
- It must stay generic (ADR-0034).
- Without it, load is 1 bot plus K headless clients from this Mac (5.2).

**B3 — Edgegap session script (ops; any time).**
- `scripts/edgegap/session.sh`. The token is read from the file into a 0600 header file, never on
  argv or in a log, the way `scripts/ci/edgegap-version.sh:65-70` does it.
- `deploy <version> [--city C …]` posts `/v1/deploy`:
  - no `OVERMATCH_SERVER_ID`;
  - tag `profile-<stamp>`, never `om-`;
  - `env_vars` from args (B1 lever, `OVERMATCH_BOT`, `RUST_LOG`);
  - city fallback on ERROR.
- `wait` prints `ip:port`. `logs` saves `.logs`. `metrics` saves the `?raw=true` JSON.
- `stop` calls DELETE and then confirms the deployment is gone.
- `watchdog <minutes>` is a detached stopper.
- Also: `EDGEGAP_MAX_DURATION` (default 1440) in `edgegap-version.sh`, for an optional
  `<ver>-profile` backstop version.
- Day test: one real deploy of version `0.6.0` (the image exists) with no client: deploy → wait →
  logs → metrics → stop → `total_count 0`. Cost about $0.01 (DERIVED).

**B4 — Online frame-cost runner (perf; any time).**
- `scripts/perf/run-online-cost.sh`, reusing `frame-run.zsh`'s gates. Make the hardcoded
  `--offline` (:143) a runner-supplied launch spec.
- The client runs with:
  - `SPIKE_SIMULATE_INPUT=1 SPIKE_SIM_WINDOWED=1 SPIKE_FRAME_COST_OFFSCREEN=1`;
  - `OVERMATCH_SERVER=$TARGET OVERMATCH_MUTE=1 OVERMATCH_CONFIG_DIR=<cfg>`;
  - `SPIKE_SIM_TICKS` greater than (startup grace + warmup + duration) × 64;
  - optional `SPIKE_COST_TRACE` and `SPIKE_TRACE`.
- Extra gates: `client: connected` and `— input slot` in the client log.
- Sample `nettop` on the client pid at the window's edges.
- Optional K GPU-less load clients, launched after the measured client connects (connect order is
  lane order).
- Per condition, call B3's `logs`/`metrics` when `$DEPLOYMENT` is set.
- Day test: hidden, against a local release `overmatch-server` with `OVERMATCH_BOT`. This also
  answers 3.4 and 6.4.

**Not code (orchestrator, now):**
- Dispatch `gh workflow run release.yml --ref main` as a pre-flight. About 50 min (MEASURED); it
  touches neither Edgegap nor any Release (release.yml:51-54).
- Get Yan's go for tonight's tag (1.9).

## The run recipe (once B1, B3 and B4 have landed; B2 optional)

```sh
# 0. By day: B1-B4 merged, CI green, release.yml pre-flight green, B3 and B4 day tests passed.

# 1. Bump and tag (orchestrator, main checkout).
#    Cargo.toml version → 0.6.1; `cargo update -p overmatch --offline`; signed commit; push main;
#    then the tag SOLO (memory overmatch-release-pipeline: batched tag pushes suppress the trigger).
git push origin main && git tag -s v0.6.1 -m v0.6.1 && git push origin v0.6.1

# 2. In parallel: the profiling client from the tag (bay, about 10 min, MEASURED cold).
scripts/bays/lease online-profile        # → bay-N; check out the v0.6.1 commit in it
#    Check the map/tree assets are not LFS pointers (head -c 42 …), then:
scripts/serial-cargo build --locked --release --bin overmatch --bin overmatch-server
#    Copy target/release/{overmatch,overmatch-server} and `cp -cR assets` into
#    $BIN/root, write $BIN/BUILD.txt, then release the bay.
#    One discarded offscreen offline run warms the Metal shader cache (6.5).
#    A rehearsal: B4 against the local overmatch-server (hidden, about 3 min).

# 3. Wait for registration (about +16 min, MEASURED v0.6.0). Do not wait for publish.
gh run view <run-id> --json jobs --jq '.jobs[]|select(.name=="register-edgegap")|.conclusion'
#    → success; GET /v1/app/Overmatch/version/0.6.1 holds docker_tag 0.6.1-<sha7>

# 4. Deploy, arm the guards, run (inside caffeinate -dims).
D=$(scripts/edgegap/session.sh deploy 0.6.1 --city Singapore --city Tokyo \
      --env OVERMATCH_SERVER_STATS_S=1 --env OVERMATCH_BOT=8 --env RUST_LOG=info)
trap 'scripts/edgegap/session.sh logs $D out/; scripts/edgegap/session.sh stop $D' EXIT
scripts/edgegap/session.sh watchdog $D 45 &          # hard stop at 45 min whatever happens
TARGET=$(scripts/edgegap/session.sh wait $D)         # ip:port, READY in about 5-13 s (MEASURED)
DEPLOYMENT=$D TARGET=$TARGET OFFSCREEN=1 BIN=$BIN ASSET_ROOT=$BIN/root \
  caffeinate -dims scripts/perf/run-online-cost.sh out/ \
    idle-alone combat-alone combat-plus-4-headless   # e.g. 15 s warmup + 60 s each

# 5. Collect and stop. Logs and metrics are pulled BEFORE the stop: they are deleted at stop.
scripts/edgegap/session.sh logs $D out/ && scripts/edgegap/session.sh metrics $D out/
scripts/edgegap/session.sh stop $D                   # then GET /v1/deployments → total_count 0

# 6. Analyze.
uv run scripts/perf/analyze.py --warmup-s 15 out/*/*.client.jsonl   # client frame cost
grep -h '^.*server_stats ' out/*.server.log | …                     # server tick/CPU/bytes per s
#    The client's bytes per second come from nettop. Label every figure "Singapore, from Da Nang, night".
```

Expected night timeline (DERIVED from the MEASURED pieces above):
- tag push → Edgegap version ready: about 16 min. The local build and the rehearsal fit inside it.
- deploy → stop: about 15–25 min, for 3 conditions plus overhead.
- Total: about 35–45 min after the tag.
- Compute: about $0.03. The release publishes on its own at +40–68 min.
