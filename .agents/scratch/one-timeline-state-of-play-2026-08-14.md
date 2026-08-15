# One-timeline pivot — state of play, 2026-08-14

## RELEASED (2026-08-15): v0.4.0-alpha.1 — PR #64 merged to main (c0e597e), tag cut

The alpha IS the endpoint config, levers now opt-out. On top of the overnight arc, the
burst-proof wave (3cc7030..6a9b80e) landed after the morning field sessions exposed a periodic
60 ms/0.52 s two-sided link burst the ping EWMA reads as 3 ms: (1) NetEQ-form arrival-delay
quantile estimator drives ALL margins + the delay law (measured: 44 ms clean / 89.8 ms burst,
q50−min 2.9 vs Qp−min 67.3 on the synthetic burst link); (2) hold-at-horizon + derived blend
window (no mid-gap snap-back, no step closes); (3) own-fire loss-proofing — belt-delta fallback
+ change-tick refusal heal (zero skipped shots under burst; the morning's swallowed shot class
is dead). SLO contracts: p = 1 − 60/(64×3600) (one late frame/hour), T = 2 s spike window.
Merge-risk watch items: replicon default_write equality-skip (bracket test is the tripwire);
one warmup SyncEvent when the estimator first steps min_delay (FRONTIER steady= is the gauge).

## Alpha fix wave (2026-08-15, post-alpha field verdict: "feels perfect") — main 056a5b7

Yan's alpha drive surfaced 5 regressions; all traced to two roots and retired in one client-only
wave (wire pins byte-identical, droplet untouched):

- **Fused delay law** (`a1f5752`): min_delay = rtt/2 + max(Qp−min − g*, 0) + one interval — the
  buffer pays only the tail beyond the extrapolation horizon g* (imported from `extrapolate`,
  single source). Slew ≤0.5 tick/frame (half controller deadband), arming on 2 s + 128 samples.
  Certified vs old law on the synthetic burst link: settled 36–45 ms vs 92–105 ms (burst),
  ~21 ms vs ~40 ms (clean); extrapolator exercises (143 gaps/120 s, blends sub-mm), settled
  sync events 0. Retired regression: standing delay.
- **Presentation clock conformance** (`ba61be1`): one `present_shot` seam (flash+audio+tracer) at
  release, aged on the cursor (tracer spawn distance 0 m); `TrackDrive` interpolated
  (`track_drive_lerp`) so tracks read the cursor clock, never arrival-fresh packets; belt-delta
  fallback through the seam. Retired regressions: no muzzle flash, tracks leading hull, tracers
  starting downrange, misfires (exactly-once certified under 10% loss).
- **Estimator gate** (`4508dbc`): samples recorded only while `Playing` ∧
  `IsSynced<InterpolationTimeline>` — loading-phase CPU stalls (Qp ~380 ms) never enter, killing
  the arm/evict target swings (one resync ~1 min into every session). Both swings dead.
- **Jitter HUD** (`5aada8e`): debug-card row = the law's buffered excess
  `spread − g*` floored at 0; amber ≥1 tick, red >g*; `--` until armed. The player-facing
  instability disclosure (clean link reads +0).

Field verdict on the full wave (Yan, live droplet, Wi-Fi burst link): "it feels perfect."

NEXT: wave PR to main → demolition branch rebases onto main and lands (PROTOCOL_REV 27) →
0.4.0 proper.

Working synthesis for the `exp/unpredicted-drive` arc. Truth for landed code is the commits;
this documents the fork, the rulings, and the decision procedure.

## Landed on the branch (all pushed; checkpoint branch `checkpoint/recoil-response-replay` at 3bca010)

| Commit | What |
|---|---|
| 852c5e2 | `OVERMATCH_UNPREDICTED_DRIVE` server flag: own tank interpolated, not predicted |
| 2d1ec32 | client env lever |
| 484d68d | derived interp delay (Law C): min_delay = rtt/2 + send_interval_ratio × tick, zero constants |
| 02caae6 | track view: presented_phase carries drawn phase at belt speed (also on main, PR #63) |
| b6555ca | recoil overlay v1 — sequential cubic (REJECTED by feel: two-phase kick) |
| df57d9c | fire-intent ledger: gate snapshot = legality report; cadence local; loopback gaps exactly cyclic |
| 3bca010 | recoil microsim = the real sim (shared `belt_tick`, kicked/unkicked pair); flat residual 0.11 mm, mid-bounce 12.08 mm |
| 2be721a | wave B: cursor_queue (fire announcements present at cursor crossing — remote bang-before-motion −3.52 ticks → +0.09) + `OVERMATCH_FUSED_FIRE=1` fused own fire (presents at +0.24 ticks from echo; refused fire never presents; mode A bit-identical unset) |
| d1fd35f | buffer-edge starvation instruments (always on: FRONTIER gap/coincidence/SyncEvent counters — the silent clamp is now loud) + `OVERMATCH_EXTRAPOLATE=1` ε-bounded gap-filler: post-interpolation overwrite system (lightyear untouched), horizon 52.3 ms = sqrt(2·ε_vis/a_max) from μg and the ratified 12.08 mm bar, projective blend over 1 tick, beyond-horizon = exact clamp. 10 mutation tests |
| 001d1dc | `OVERMATCH_CURSOR_HIT_FEEL=1`: own being-hit cue at the cursor crossing of its damage tick (outgoing hit marker stays at arrival deliberately). 3 mutation tests |
| effb701 | merge of the two lanes; full suite green on the combination; PUSHED — the morning test tip |
| 24e39d6 | derived sync-margin laws, both wire directions: uplink floor 0.5 t (mean content phase, structural) + deadband = measured jitter in ticks (rewritten per frame, arms when own hull rides the stream); downlink multiple 2 + 0.5 t floor (static, lightyear reads live jitter). Levers `OVERMATCH_INPUT_MARGIN_MS` / `OVERMATCH_INTERP_MARGIN_MS` pin the whole margin. Server-side INPUT-ARRIVAL instrument (late = strictly negative margin; previously silent hold-last now loud). Measured: click→bang 209.5→159.9 ms @80/10, 156.2→132.2 @40/3, zero late inputs, zero starvation. 6 mutants killed. ONE justified deploy (server sampler) — in flight. |

Demolition pass 2 (2026-08-15) removed every lever the rows above mention: fused own fire and
the extrapolation gap-filler are now the unconditional defaults, `OVERMATCH_CURSOR_HIT_FEEL`
and the recoil overlay are deleted, and the INPUT-ARRIVAL instrument is retired (FRONTIER
remains).

## Standing rulings (Yan)

- Client owns fire start/stop, looks instant; server owns legality. (Implemented: df57d9c.)
- Sequential two-phase kick rejected; response replay approved, then rebuilt faithful (3bca010), feels good.
- Camera never carries impulse feel. Recoil need NOT remain a physical chassis impulse — open to explore.
- Plan B (cursor fusion) accepted as FIRST candidate for the feel gate, judged after margins land.
- The delay-as-anticipation framing is admissible ("satisfying anticipation, i won't rule it out").
- No droplet deploys for client-only changes.

## The lean fork menu (collapsed 2026-08-15, Yan: "remove the no-brainers")

Two real decisions remain; everything else is dead, ruled, or data-gated:

1. **Own-fire presentation — A vs B** (taste; Yan at the feel gate, real margins, order-
   counterbalanced). B winning kills A's ~900 LOC + the foreign-impulse-early door and ratifies
   delay-as-anticipation.
2. **Cursor position — padded tail-quantile vs arrival edge + ε-bounded extrapolation** (data;
   the kill test + impulse∧gap coincidence instrument decide). Certifies → padding dies AND the
   getting-hit exception dissolves (arrival ≈ cursor); fails → padded cursor + honest freeze
   stay, and the hit-feel lever remains a live feel question.

Order: data gate first (sets the delay the feel gate runs at), feel gate second, wave C sized
by the outcomes. Removed as no-brainers/dead: N (spike), late-latch + FEC (frontier no-build),
redundancy (instrument-decided, events-only), quantile law + margins + instruments (strictly
better, building), adaptive input delay + masking + prediction (ruled).

## The fork (the only open design question)

Per-channel assignment of {rock, surge, muzzle VFX} to {click, arrival, cursor}. Everything else
is assigned and uncontested. Candidates:

- **A — replay** (landed, 3bca010): microsim + subtract. Instant, proven. Cost: ~900 LOC + standing
  fidelity contract (softenable: Smith-predictor band-limiting; demote to CI calibration under N).
- **B — fuse** (building, `OVERMATCH_FUSED_FIRE` lever): present the whole shot at cursor crossing.
  Zero machinery; cost = click→bang ≈ 200 ms today / ~165 ms after margin laws / floor ~145 ms @75 ms ping.
- **N — normalize: DEAD** (spike 2026-08-15, `n-spike-hull-rock-fit-2026-08-14.md`). Killed on all
  three axes: (1) spring fit residual 8.04 mm@3m flat = 73× A's floor, error proportional (~20% of
  peak), slope pose breaks the constants (ω −63%, ζ ×6; 57.7 mm@3m cross-applied), off-axis rock
  unreachable by a v₀ spring; (2) NO cell of the surge grid is invisible-late — even the 88's
  arrested transient is 11.7–36 mm, 183-class 109–681 mm w/ gas; (3) bore discrepancy 2.68 mrad
  peak = real ballistics change. Fallback per the design study: "A hardened".

## Key derived numbers

- Click→visible = uplink lead + downlink delay. @75/5 ms: lead 94.4 (rtt/2 + 2j + floors),
  downlink ~run on lightyear DEFAULT margins. Total ~202 ms; ~37 ms recoverable (≈20 up, ≈18 down)
  via derived margin laws; structural floor ~145 ms.
- 3-tick input delay contributes ZERO wall latency (subtracted from objective, re-added to tick).
  Adaptive input delay stays dead (fabricated ticks; would recover 0 ms).
- Recoil residuals: A flat 0.11 mm / mid-bounce 12.08 mm (real trajectory difference, not error).
- 88 surge ≈ 0.14 m/s → ~1 mm arrested; FV4005-class ≈ ~2 m/s → ~20–30 cm (estimate; spike measures).

## Research corpus (this scratch dir, 2026-08-14 unless noted)

- `input-lead-budget` — the latency budget + derived margin-law spec (uplink AND downlink).
- `latency-precedent-crossdomain` — microsim ≡ Smith predictor (band-limit fix); Spanner commit-wait
  economics (absorb Δ under lock-time choreography); recalibration psychophysics (A/B needs order
  counterbalance + washout); viewpoint-recoil (timewarp analog; conflicts with camera-kick ruling).
- `declarative-timeline-calculus` — Candidate N + ranked alternatives (type-system-as-discipline;
  linearized/speculative/protocol-level rejected with grounds); bonus: foreign impulses could
  present ~90 ms early via A's machinery (alive only if A survives).
- `error-smoothing-legacy-hunt` — 22 mechanisms, 12 dissolve (~15 kLOC class incl. 3.9 k grip
  checkpoints); render_error's 8 dials die with the module; surviving tuned constants ≈
  PHASE_HEAL_OMEGA + track snap bracket; two latent flags (servo ULP float divergence; track snap
  detector rewrite vs freeze-then-step clamp).
- `recoil-microsim/` — 3bca010 capture manifests (the certified bars).
- `burst-state-fire-stack-map`, `impulse-prediction-mixed-timeline`, `input-only-resim`,
  `interp-delay-derivation`, `unpredicted-driving-*` (2026-08-13/14) — the earlier arc.
- `n-spike-hull-rock-fit` — PENDING (spike in flight).

## Per-channel clock map (the multi-clock architecture, one table)

Structural law: extrapolation applies ONLY to continuous channels (inertia makes the near future
computable); discrete events are un-extrapolable — they need coherence (fusion to the motion
cursor), never a buffer. One cursor; the continuous group's physics decides how close to arrival
it rides; every discrete group fuses to it.

| Physics group | Nature | Clock | Lever / mechanism | Status |
|---|---|---|---|---|
| Own fire intent (cadence, gate, belt) | discrete, self-caused | click | fire-intent ledger (df57d9c) | ratified |
| Own recoil {rock, surge} | continuous consequence of a discrete self-event | click+replay (A) or cursor (B) | A: microsim subtract (3bca010) / B: `OVERMATCH_FUSED_FIRE` (2be721a) | FORK 1 — feel gate |
| Own muzzle VFX | discrete, self-caused | rides the recoil fork | same | FORK 1 |
| Hull motion, own + remote (drive, suspension) | continuous, heavy, traction-limited | cursor | interp stream; delay = derived law (484d68d + 24e39d6) | ratified |
| Remote fire (bang + flash) | discrete | cursor | cursor_queue (2be721a) | ratified |
| Getting hit (feel + VFX) | discrete, foreign | arrival (legacy) vs cursor | `OVERMATCH_CURSOR_HIT_FEEL` (building) | dissolves if FORK 2 → arrival edge |
| Servos (turret, self-heal) | continuous, self-caused | click-immediate | client servos | ratified |
| Cursor position itself | — | arrival + margin vs arrival edge + ε-extrapolation | `OVERMATCH_EXTRAPOLATE` + `OVERMATCH_INTERP_DELAY_MS` (building) | FORK 2 — data gate |

## Doctrine hygiene (Yan, 2026-08-15)

"No corrections, ever" is NOT a ruled doctrine — it is an emergent property of the designs built
so far, wrongly promoted to axiom in mid-session briefs. Actually ruled (on measurement): the
architecture-scale correction machinery (prediction/rollback/snap-ease). NOT ruled: micro-scale
bounded extrapolation through jitter gaps with sub-perceptual blend-back — never tried, never
measured; reopened as a first-class candidate in the adaptive-cursor frontier research. The
getting-hit-at-arrival exception is likewise under review (legacy of the predicted era, when
cursor-coherent hits were impossible). Perceptual masking (saccade tricks) stays closed — that one
IS ruled by the VFX-honesty doctrine.

## Feel-gate data points

- 2026-08-15, Yan, first taste of B at WORST-CASE conditions (pre-margin ~200 ms fuse, freshly
  adapted to A, real droplet RTT): "B is... not bad. at all." Not the verdict — the ruled gate is
  post-margins, order-counterbalanced — but B survived its most hostile audition.

## Overnight plan (ratified 2026-08-15, Yan asleep; test menu due in the morning)

Sequenced pipeline, all client-only (no deploys unless the late-input instrument needs the
server-side counter — that one deploy is pre-authorized):

1. Margin-laws implementer (in flight) → review, merge, push. Click→bang ~200 → ~165 ms.
2. Frontier report LANDED (`adaptive-cursor-frontier-2026-08-15.md`) → second implementer
   dispatched (parallel worktree, file-scoped away from the margins agent): (a) instruments —
   starvation counter at the silent freeze-then-step clamp, SyncEvent counter, impulse∧gap
   coincidence; (b) `OVERMATCH_EXTRAPOLATE=1` — ε-bounded kinematic gap-filler at the buffer
   edge, horizon g* = sqrt(2·ε_vis/a_max) with a_max = μg (sim constants) and ε_vis = the
   ratified 12.08 mm bar, projective-velocity blend-back ≤ 1 send interval, beyond-horizon =
   today's clamp; (c) `OVERMATCH_CURSOR_HIT_FEEL=1` — getting-hit through the cursor queue.
   No new delay-target code: arrival-pace positioning tests via existing
   `OVERMATCH_INTERP_DELAY_MS`. The NetEQ-style quantile law (frontier §1) sequences AFTER
   margins land — same files, subsumes them.
3. Merge order: margins → extrapolation worktree on top → tests → push.
4. Update this doc + write the per-channel clock map (group → clock → lever → rationale). DONE.
5. DONE 03:02 — `exp/wave-c-demolition` PUSHED (stacked off effb701, NOT deployed): 5 stages,
   54 files, +422/−17,661, suite green (947 lib + all integration). PROTOCOL_REV 26→27,
   fingerprint re-pinned; ADR-0037 "one authoritative timeline and view overlays" +
   supersession pointers on 0015/0017/0027/0029/0030/0032. Stage 4 (replica split) skipped
   with proof: `drive_tracks` self-gates on `RigidBody::Dynamic`, client tanks are Static —
   the split already exists. Inventory disagreement resolved toward the hunt: servo ULP bands
   DISSOLVED (my brief wrongly said survive); landmine line (band = symptom sensor, float
   divergence remains) carried in REV-27 doc + ADR-0037. Extrapolate's impulse ledger lost the
   Shock class with its writer (Fire/Damage remain). Lands only after the verdict drive.
   Session soft-stop 04:00 local (Yan). Wave C demolition rationale (Yan, 2026-08-15: the class is endorsed by the pivot
   itself, invariant across both remaining forks) — on stacked branch `exp/wave-c-demolition`
   off the extrapolation merge, so the morning test branch stays untouched. Lands only after
   the verdict drive. Excluded: track-snap detector rewrite (fork-2-dependent — clamp vs
   extrapolation seam). Includes: 11 prediction modules, render_error, rollback watchdog,
   HullShock/adoption, 3.9k grip checkpoints, `.predict()` prune + PROTOCOL_REV 26→27,
   successor ADR draft.

Structural ruling behind it: extrapolation applies ONLY to continuous channels (inertia makes
the near future computable); discrete events are un-extrapolable and never needed the buffer —
they need coherence, i.e. fusion to the motion cursor. One cursor, positioned as close to
arrival as physics-insurance allows; the continuous group defines how close; every discrete
group fuses to it.

## Morning test menu (EXECUTED — its verdicts became the pass-2 rulings)

The menu ran; B (fused own fire) and the extrapolation gap-filler won and were made the
unconditional defaults in demolition pass 2 (2026-08-15), which deleted the A/B levers
(`OVERMATCH_FUSED_FIRE`, `OVERMATCH_EXTRAPOLATE`, `OVERMATCH_CURSOR_HIT_FEEL`) and the
INPUT-ARRIVAL instrument. FRONTIER still prints every run. The only surviving pace levers are
`OVERMATCH_INTERP_DELAY_MS` / `OVERMATCH_INTERP_MARGIN_MS` (pin the derived margins for
experiments).

## Decision procedure

1. N spike (in flight): fit residuals + roster surge table + bore honesty.
2. Wave B lands → review/land → THEN margin-laws implementer (same files, sequential).
3. Feel gate: A vs B at real margins, order-alternated with washout. Spike numbers alongside.
4. Recoil ruling → verdict drive at real RTT → wave C demolition (~15 kLOC + protocol prune +
   successor ADR "one-authoritative-timeline-and-view-overlays" superseding the two-timeline docs).

Parked behind the verdict: destruction 1A/2A calls; foreign-impulse early presentation.
