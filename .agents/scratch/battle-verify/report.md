# Battle verification

Subject: the game's Battle flow and damage-model knockout credit, game state only.

**The full-round playtest gate is not cleared.** The initial capture exited before results.
The mandatory lane limit is 90 minutes; the lane started at MEASURED 2026-11-03 16:23:44 +07.

The source step is committed as `net: verify every weapon's knockout credit in the game's damage model (#230)`.
It adds the three missing real-round regressions, passive spawn/marker tick logs, and the deciding
`ShotId` and authority tick on the score log. The attribution decision remains solely in
`damage_cam::recorder::attribute_kills`: its one `killer` raises `TankKnockout` and tags the
open pen-cam encounter. Automatic fire raises that fact before the absent-recording branch.

MEASURED checks: formatting and CI clippy passed; the targeted/cross-cutting run passed 147 tests,
with no failures and five ignored tool/measurement cases. It covered recording, score, player/bot
respawn, Battle HUD, headless checks, `doc_citations`, `ui_ascii`, `net_boundary`, and `bake`.
Both dev binaries rebuilt. See `checks.log`. The first new automatic-fire fixture failed
its knockout assertion: its crew box was hard matter. The final fixtures use the
shipped witness representation, verified against `tank::spawn`, and retain the assertions.

## Preserved attempt

MEASURED from `attempt-1/server.log`, `attempt-1/client.log`, and `attempt-1/run-result.json`:
one loopback server on Kalinovo (`map::MAP_ID`), `OVERMATCH_BOT=30`, one hidden unattended client,
unchanged shipped mode, real application clock, all under one machine lock. There were 31
combatants, including the client. The client self-exited successfully after the final capture;
the server was then deliberately terminated. No panic appears in either log during this window.
The last inspected still shows combat with 3:39 remaining, so results and the next fill-up were
not observed. The fixed capture stop was too early for this run's authority-clock progress.
This is an incomplete verification, not a demonstrated game crash.

MEASURED: 112 distinct scored combatant/life pairs, all 112 with shooter credit; 105 cook-offs
and seven crew-loss knockouts; 112 bot-respawn log entries. DERIVED from the score handler's
post-update log: those entries represent 112 loss increments. These are **not an independent
marker census**, and wall-clock respawn gaps cannot prove the authority-tick wait.

MEASURED diagnostics: 929 steady interpolation-resync warnings on the client; client ERROR lines
zero. Server ERROR lines: one headless render-app message, six input-correction messages. The
client survived this observed window. No frame or tick cost recorder was armed.

Every retained still was inspected:

- [Initial fill-up](attempt-1/cycle-00030.0000s.png): `BATTLE STARTS IN 0:31`.
- [Combat Tab board](attempt-1/cycle-00500.0000s.png): all 31 rows, clock `8:24`; every score and
  ordering agrees with the authority log before the image was saved.
- [Final captured combat](attempt-1/cycle-01020.0000s.png): clock `3:39`, client still in match.

## Acceptance disposition

| Issue | Ask | Disposition |
|---|---|---|
| #230 | Always-on attribution independent of pen-cam recording, with deciding weapon/reason/tick | DONE: existing `TankKnockout` path; complete authority score-log record now exposed |
| #230 | Public per-combatant tally | DONE: existing replicated `Scoreboard` |
| #230 | Per-team tally and member-sum test | MOOT: #220 chose free-for-all; no team fact exists |
| #230 | Pen cam shares the attribution | DONE: one `killer` in `attribute_kills`; no second attribution added |
| #230 | Real-round automatic-fire, once-only later cook-off, and respawn-credit tests | DONE: three new regressions passed |
| #230 | Independent live marker/credit/loss census and exact respawn waits | DONE for the bounded rebuilt run: see audit below |
| #243 | Fill-up clock and combat Tab board/clock | DONE in inspected hidden-window stills |
| #243 | Results table, winner/draw, next fill-up | LEFT: not reached in the preserved attempt |
| #243 | Read replicated score/phase only; bundled-font coverage | DONE by source inspection and existing HUD/guard tests |

#230 can close against its stated acceptance: the bounded 30-bot audit and all requested non-team tests are complete. #243 cannot close: results and the next fill-up remain unobserved. The separate full-round playtest bar remains open.

## Rebuilt bounded audit

MEASURED from the rebuilt run's `server.log`, `client.log`, `run-result.json`, and `audit.json`:
30 bots and one client, hidden/unattended/silent on Kalinovo, unchanged shipped mode, one machine
lock. Both processes remained alive until deliberate SIGTERM at the lane limit. The final cleanup
completed at MEASURED 17:54:15 +07 (the limit was checked at 90.26 elapsed minutes before cleanup).
No panic appears in either log; the client continued through six resync warnings. The logs and
captures show ongoing combat; there is no evidence of a frozen process in this observed window.

MEASURED: **61 independent marker transitions = 61 score/loss entries = 61 shooter credits**.
Every marker matches its score fact by Battle, combatant, life, tank entity, and authority tick;
no marker key repeats. All 61 completed combat respawns follow the corresponding prior-life
knockout. Their MEASURED waits range from 640 through 653 authority ticks; every one is at least
the DERIVED required 640 ticks (shipped wait divided by the server timestep, rounded up).
`audit.py` reproduces these checks from the preserved log and the data owners. These observations
apply only to this bounded run; they do not establish the missing full-round result.

Inspected retained rebuilt stills:

- [Fill-up](cycle-00030.0000s.png): `BATTLE STARTS IN 0:31`.
- [Scored combat Tab board](cycle-00150.0000s.png): clock `13:31`, all 31 rows visible.
- [Later combat Tab board](cycle-00300.0000s.png): clock `11:03`, combat remains active; the scoreboard is readable.

No results still exists. The first attempt's premature fixed capture stop and subsequent build
queue waits consumed the lane window. The checked source and evidence are committed; all game
processes started by this lane were terminated. No merge, push, issue comment, or release occurred.

## Prepared rerun

`run.py` now stops after the authority announces the next fill-up, rather than treating a fixed
app-second capture as the end of a Battle. `cycle.ron` samples through the later phase window;
its last cue and the outer watchdog only bound a failed run. The runner uses existing player
settings in `config/video.ron` (FIXTURE choice: cap 30, debug panel off) and existing `RUST_LOG`
to expose independent marker/spawn ticks. It changes no mode duration or simulation rule.

Run from this bay, with no other game run active:

```sh
export OVERMATCH_CARGO_LOCK_WAIT_S=10800
scripts/serial-cargo build --locked --features dev_ui,bitprobe --bin overmatch --bin overmatch-server
env OVERMATCH_UNATTENDED=1 OVERMATCH_WINDOW=hidden /usr/bin/lockf -k -t 10800 /tmp/overmatch-cargo.lock python3 .agents/scratch/battle-verify/run.py
```

Preserve the current logs before rerunning: the runner writes `server.log` and `client.log` in
this folder. The full-round result must be judged from actual authority phases and inspected
stills, never inferred from the successful binary build or the short fixture-mode tests.
