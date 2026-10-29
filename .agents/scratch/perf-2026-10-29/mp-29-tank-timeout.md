# MP: the client's netcode connection times out at 29 tanks

Commit 83fee31 (0.7.0-alpha.1), Edgegap Singapore, `OVERMATCH_BOT=28`. Labels: MEASURED, DERIVED,
ASSUMED, CLAIMED.

## Verdict (confidence: high)

The cause is not the network, the server, the bandwidth or the join snapshot. **The client freezes
its main thread for about 0.16 s per replicated tank** when it attaches local sim bodies, all in one
frame. At a 29-tank join that is **one 4.6–4.9 s frame** (MEASURED). The client sends nothing during
that frame, so **the server's netcode times the client out 3.0 s into it** (MEASURED; the connect
token's 3 s timeout) and drops the session without a disconnect packet. The client's own
`ConnectionTimedOut` follows as soon as its clock catches up. The client then reconnects, receives
all 29 tanks again and freezes again. That loop gives 11 connects per ~90 s.

**97 % of the freeze is `firecontrol::RangeTable::for_weapon`** (MEASURED, sampling profile). This
brute-force ballistic sweep runs again for every weapon of every tank at every spawn, but its output
depends only on the round's `(muzzle_speed, drag_k)`. Collider construction is about 3 % (MEASURED).

**Threshold.** The stall is about 0.16 s × the tanks in view at join, and the session drops once it
passes 3 s. That puts the limit at about 19 tanks on this M4 (DERIVED: 19 × 0.16 = 3.04 s). On
loopback (MEASURED), 17 tanks hold and 21 tanks loop. A slower client CPU lowers the threshold in
proportion: the EPYC server spends about 320 ms per tank spawn (MEASURED), and at that speed the
limit would be about 9 tanks (DERIVED).

**Recommended fix:** memoize the range table per `(muzzle_speed, drag_k)` bit pattern. The output is
bit-identical, and the change is roughly 20–40 lines in `src/firecontrol.rs`.

## Evidence chain

### 1. The client builds every replica's sim body in one system, in one frame

- `src/net/client.rs:656` registers `rig::attach_replicated_rig` in `Update`.
- `src/net/rig.rs:44-57` loops over every replicated tank root that has no `RigidBody`. For each one
  it logs `replicated tank gets local sim body` (`:45`) and calls `attach_replicated_tank_body`
  (`:46`) synchronously.
- `src/tank/spawn.rs:303` → `assemble_tank_body` (`:966`) → `insert_weapons` (`:976`) →
  `RangeTable::for_weapon` (`:599`), once per weapon. The Tiger has 3 weapons (MEASURED: the 3
  entries in the server's SIM-EVIDENCE `weapon_gate` lists).
- `src/firecontrol.rs:57-81` sweeps the launch angle from 0 to 45° in 0.0002 rad steps
  (`:59-60`; up to 3,927 angles per weapon, DERIVED). Each angle is a full trajectory at dt = 5 ms
  (`:58`) through `simulate_range` (`:128`) and `ballistics::freeflight_step`
  (`src/ballistics.rs:172`). The sweep stops when range stops rising or reaches 5 km.
  - It is a pure function of `(speed, drag_k)`.
  - Its consumers only read it: `aim.rs:429`, `sight.rs:392`, `sight/reticle.rs:403,628` and
    `camera.rs:564` all take `Query<&RangeTable>`.
- **Profile.** macOS `sample`, 3 s, taken during the first attach burst of the bay-3 debug client
  (it has symbols; dependencies are built at opt-level 3). All MEASURED:
  - 2,264 main-thread samples in `attach_replicated_rig`;
  - 2,190 of them (96.7 %) in `RangeTable::for_weapon`;
  - 68 (3.0 %) in `Collider::trimesh_with_config` (`spawn.rs:784`, the parry BVH build).

  The profile is from the debug build. That the release build has the same hot spot is ASSUMED: its
  per-tank cost is 160 ms against the debug build's 173 ms (both MEASURED).
- **Per-tank cost**, from the gaps between consecutive `gets local sim body` lines (all MEASURED):
  - Edgegap passes: median 165 ms and 167 ms.
  - Loopback, release GPU-less client: median 160 ms.
- **Stall frames.** Each connect produces one frame of 4,554–4,934 ms
  (`online-1/bots28/bots28.client.jsonl`, rows 238, 404, 608 …) and 4,905–4,924 ms in online-2
  (MEASURED). At 11 tanks the join frame was 1,763–1,867 ms and the client held (MEASURED).

### 2. A client silent for 3 s is timed out by the server

- **Where the 3 s comes from.** `src/net/client.rs:144` sets `client_timeout_secs: 3`. That value is
  written into the manual connect token (`lightyear_netcode-0.30.1/src/auth.rs:56-78`). The server
  adopts it per client (`server.rs:786`) and enforces it in `check_for_timeouts`
  (`server.rs:845-861`). A timeout sends no disconnect packet; the server only removes the
  connection and logs `Disconnection from netcode client … Despawning entity`
  (`server_plugin.rs:391`).
- **Edgegap, online-1** (MEASURED):
  - client's first attach line: 20:34:50.543;
  - server's `Disconnection`: 20:34:53.702, which is 3.16 s later. This crosses two clocks, so it
    includes an unknown clock offset plus one-way latency;
  - client's stall ends at about 20:34:55.10.
- **Loopback** (MEASURED): `DEBUG lightyear_netcode::server: server timed out client` comes +2.99 to
  +3.03 s after the first attach line, in every cycle (8 of 8 in b28, 9 of 9 in b28w).
- **The server keeps ticking throughout.** On Edgegap, `tick_hz` is 59–67 in every stats period of
  the first cycle; on loopback it is about 64 with 0 overruns (both MEASURED).

### 3. The client's `ConnectionTimedOut` is an echo of the server's drop

- **The packets arrive; nothing is lost on the network.** Packets the server sent during the first
  3 s of the stall are delivered and processed after the stall (MEASURED):
  - Edgegap cycle 2: `sync_margin` logs `arrival(… qp-min=3109.4ms n=95)` at 20:35:02.237, 47 ms
    before the client's timeout;
  - loopback: `n=192`, with `qp-min` 2,973 ms.
- **The client's netcode clock is `Time<Real>.delta`** (`client_plugin.rs:149,159`), and
  `client.rs:589` advances it before receiving. In a windowed client, `Time<Real>` takes the render
  world's instant (bevy_time-0.19.1 `src/lib.rs:157-174`; bevy_render-0.19.1
  `src/lib.rs:425,483-486`), and that instant lags by one frame.
  - MEASURED in the hidden-window loopback run: row 137 is the stall (4,713 ms). Row 138's `t`
    advances only 6.5 ms. The 4.76 s jump lands in row 139.
  - So the first frame after the stall drains the queued packets under the stale clock
    (`client.rs:453`).
  - The next frame jumps about 4.7 s with nothing new to receive, because the server has already
    dropped the session. That trips `last_receive_time + 3 < time` (`client.rs:459-460,492-493`).
  - Result: `ConnectionTimedOut` about 0.1 s after the stall ends (MEASURED: timeout at +4.77 to
    +4.88 s after stall start; stall end at +4.68 to +4.76 s).
- **GPU-less client.** It has no render world, so `Time<Real>` is `Instant::now()`. The chain is the
  same, but the client's timeout lands 3.05 s after the stall ends (MEASURED, b28).
- That pipelined rendering causes the one-frame lag is ASSUMED (Bevy's default `multi_threaded`
  feature). It makes no difference to the outcome: the server's drop at +3.0 s ends the session
  either way.

### 4. The reconnect loop

About 1 s after the timeout the client reconnects (`reconnect attempt 1`). The join replicates all
29 tanks again, which triggers 29 new attaches and another freeze of about 4.6 s (MEASURED). Counts,
all MEASURED:

- Edgegap: 11 connects and 10 timeouts per pass, in both passes.
- Loopback b28: 8 connects and 8 timeouts in 75 s.

The low client network rates at 29 tanks follow from this (DERIVED): each ~8 s cycle is about 4.6 s
frozen plus about 1–3 s reconnecting.

## Candidates

| Candidate | Verdict | Evidence |
|---|---|---|
| Server main thread too busy to send keepalives | **Killed** | The loopback server ran at 59 % CPU, tick p50 3.8 ms, 0 overruns, and the loop still happened (MEASURED). The server is the side that does the timing out, because the client went silent. |
| Send-bandwidth cap or priority starving the link | **Killed** | Packets queued during the stall are received after it (n=95 on Edgegap, MEASURED). |
| Join snapshot or per-tick packet budget | **Killed as cause** | The snapshot is complete within 1 s: connected 20:34:49.720, own tank at 50.416, all 29 tanks attaching from 50.543 (MEASURED). |
| Client-side stall | **Confirmed** | §1. One 4.6–4.9 s frame per connect, made of 29 × ~160 ms attaches. |
| Netcode timeout value | **Sets the threshold; not the cause** | Both ends use 3 s, so any client frame over 3 s drops the session. |

## Local reproduction

**Setup**

- **Server:** bay-3 `target/debug/overmatch-server`, version 0.6.0, built from 1413ca1. Its `src/`
  and `assets/` match 83fee31; only the version bump differs (MEASURED with `git diff --stat`). No
  83fee31 server binary was on disk, and building was out of scope for this lane.
- **Client:** `perf-2026-10-29/bin/night-22507da/overmatch` (release). It is 0.6.0 with the same
  pinned wire hashes, so the handshake matches. The 83fee31 client's handshake tag differs from the
  0.6.0 server's because the crate version is part of it.
- **Environment:** loopback only (`OVERMATCH_SERVER_BIND=127.0.0.1`), `OVERMATCH_SERVER_STATS_S=1`,
  `RUST_LOG=info,lightyear_netcode=debug`. Each run is one `lockf` around one driver script.
- **Screen and cleanup:** no window was shown and no dialog or crash report appeared. Every process
  was killed afterwards.

**Results** (all MEASURED)

| Run | Tanks | Client | Stall per join | Connects / client timeouts / server timeouts | Server cpu / tick p50 / overruns (median) |
|---|---|---|---|---|---|
| b16 | 17 | GPU-less | 2.72 s | 1 / 0 / 0 (60 s) | 55 % / 3.2 ms / 0 |
| b20 | 21 | GPU-less | 3.38 s | 8 / 7 / 7 (60 s) | 55 % / 3.1 ms / 0 |
| b28 | 29 | GPU-less | 4.64 s | 8 / 8 / 8 (75 s) | 59 % / 3.8 ms / 0 |
| b28w | 29 | Hidden window (the runner's OFFSCREEN env) | 4.68–4.76 s | 9 / 8 / 9 (60 s) | 62 % / 4.1 ms / 0 |

**Conclusions**

- It reproduces on loopback with an idle server, so latency, the network and server load are all
  ruled out.
- The threshold on this M4 lies between 17 and 21 tanks; the arithmetic puts it at 19 (DERIVED).
- I did not run a 10-bot case: Edgegap already shows 11 tanks hold (1 connect).

## Fix options, ranked

1. **Memoize `RangeTable::for_weapon` per `(muzzle_speed.to_bits(), drag_k.to_bits())`.**
   - *Where:* `src/firecontrol.rs`. Store `points` as `Arc<[(f32, f32)]>`, behind either a
     process-wide cache inside `for_weapon` or a `Resource` that `insert_weapons` reaches
     (`src/tank/spawn.rs:599`). An alternative is to build each table once per weapon spec when
     `TankContent` is built; that needs more plumbing for the same effect.
   - *Effect:* per-tank attach falls to about 5 ms (DERIVED: the 3 % that is not the table, of
     160 ms), so a 29-tank join stalls for about 0.15 s.
   - *Safety:* the output is bit-identical, so aim, determinism and the wire are unchanged.
   - *Server benefit:* the same cost disappears from every server spawn and respawn (MEASURED:
     320 ms per bot spawn on Edgegap; a 252 ms max tick after a respawn burst).
   - *Generic:* the key is round parameters, not a tank, so it survives tank #2 unchanged.
   - *Cost:* small, plus one test that a cached table equals a fresh build.
2. **Make the sweep itself cheaper.** For example, a coarse sweep plus bisection per range row, or
   stopping at the sight's dialable range instead of 5 km / 45°.
   - This also removes the cost of the first spawn of each round.
   - It changes the table's rows, so aim shifts slightly and needs a behaviour check.
   - It complements option 1 but is not needed for this bug.
3. **Cap attach work per frame** in `attach_replicated_rig` (`src/net/rig.rs:44`): a time or count
   budget, with the rest done next frame. The query already waits on `Without<RigidBody>`.
   - This insures against any future per-tank cost.
   - It does not reduce total work, and tanks appear over a few frames.
   - *Cost:* small.
4. **Raise the netcode timeout** (`src/net/client.rs:144`; the token carries it to both ends).
   - It is one constant, but the client still freezes for 4.6 s.
   - Real link loss would be detected later.
   - `src/net/client/match_exit.rs:40,50` derive their bounds from the 3 s value and would need
     re-deriving.
   - Mitigation only; not recommended on its own.
5. **Not recommended:** server tuning, and bandwidth or priority changes. Neither is involved here.

## Side observations

- **Mid-match hitches.** Every remote respawn costs every client about 0.16 s, and simultaneous
  respawns stack (MEASURED: post-warmup frames of 853 ms and 1,023 ms in the Edgegap 10-bot passes;
  a burst of 7 respawns, 1.13 s, in loopback b16). Fix 1 removes these too.
- **Render errors at disconnect.** The client logs bursts of
  `bevy_render::slab_allocator: Use-after-free …` at each disconnect, while the 29 tanks despawn.
  This is unrelated to the timeout and worth a separate look.
- **The runner's gate worked.** Its "connected exactly once" check flagged bots28 as INVALID.

## Artifacts

All under
the session scratchpad (not kept):

- `bin/repro.zsh`: the driver (server plus client, `WINDOWED=1` for the hidden window).
- `bin/profile.zsh`: the profiler driver.
- `bin/bursts.py`: attach-burst timing.
- `runs/{b16,b20,b28,b28w}/`: client and server logs. `b28w` also holds `client.client.jsonl`.
- `runs/prof/sample.txt`: the call tree.
