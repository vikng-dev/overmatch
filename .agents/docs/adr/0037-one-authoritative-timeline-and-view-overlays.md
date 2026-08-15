# 0037 — One authoritative timeline, and view overlays

Status: ACCEPTED 2026-08-15. Ratified by feel on the `exp/unpredicted-drive` arc and by
measurement (`.agents/scratch/one-timeline-state-of-play-2026-08-14.md`, the decision record;
`.agents/scratch/error-smoothing-legacy-hunt-2026-08-14.md`, the dissolution inventory).
Shipped as declared `PROTOCOL_REV = 27`.

Supersedes, with the survivals named in §5:
[[0015-divergence-doctrine]] (the removable-scaffolding layer only),
[[0017-mutual-contact-resolves-on-the-authority]],
[[0027-element-grip-netcode]],
[[0029-weapon-gate-is-tick-correlated-authority-state]] (the owner-prediction half),
[[0030-servo-pose-is-owner-reconciled]],
[[0032-unpredictable-authoritative-facts-adopt-unconditionally]].

## Ruling

Every hull — the client's own included — renders from the interpolated server stream, at a
delay derived from the link (`min_delay = rtt/2 + send_interval_ratio × tick`, zero tuned
constants). The client predicts nothing and reconciles nothing: there is one timeline of
authoritative state, and everything the player must feel before that stream can show it is a
**view overlay** — presentation computed beside the stream, never a second simulation corrected
against the first.

The compensable class is epistemic, not architectural: only what is self-caused, locally
computable, and finite may present ahead of the stream (fire intent, own recoil response, own
turret). No engineering effort grows that class; a foreign fact is presentable no earlier than
its arrival.

Per-channel clock assignment replaces the architecture binary. Each perceptual channel is
assigned to {click, arrival, cursor}; extrapolation applies only to continuous channels
(inertia makes the near future computable), discrete events fuse to the motion cursor. The full
map lives in the state-of-play doc.

## Why

- Prediction existed to hide own-input latency. Measured, the derived margin laws recovered most
  of what it bought: click→bang 209.5 → 159.9 ms at 80 ms/10 ms jitter, 156.2 → 132.2 ms at
  40 ms/3 ms — with zero late inputs and zero interpolation starvation. The 3-tick input delay
  contributes zero wall latency (subtracted from the objective, re-added to the tick).
- The correction machinery was the dominant bug class. Every threshold-gated snap-vs-ease
  decision in the codebase lived in the dissolving set (hunt inventory: 12 of 22 mechanisms);
  nothing that survives makes one.
- The recoil microsim proves the overlay discipline works at fidelity: the kicked/unkicked pair
  shares the real `belt_tick`, residual 0.11 mm flat, 12.08 mm mid-bounce — a real trajectory
  difference, not error. That 12.08 mm bar also derives the extrapolation horizon
  (`sqrt(2·ε_vis/a_max)` = 52.3 ms).
- Remote fire fused to the cursor moved bang-before-motion from −3.52 ticks to +0.09: discrete
  facts never needed a buffer, they needed coherence with the motion cursor.

## What dissolved (Wave C, this branch)

~17.6 kLOC deleted across four commits, wire REV 26 → 27:

- `render_error` (the 8-dial correction smoother), `adoption` (ADR-0032's staging/ordering
  stack), the rollback `watchdog`, the element-grip netcode (anchors, checkpoints, resync —
  ADR-0027's machinery, ~3.9 kLOC), and the arrival/hull-shock/lead-zero rollback fixtures.
- `HullShock` + `ShockCause` (REV 22/24/25) and the `victim` impact fields (REV 23): the shove
  is ordinary stream content now; the spark arrives at RTT/2, the shove at RTT/2 + D, so the
  ordering problem ADR-0032 existed to police is structurally impossible — inverted from the
  predicted world where the shove was systematically faster.
- Every `.predict()` registration, `PredictionTarget`, rollback condition/comparator,
  correction fn, `local_rollback` registration, the promotion/`DisableRollback` dance, and the
  shipping rollback/correction policies. The client mounts no `PredictionManager`.
- Trace rollback attribution (trigger slots, `rollback` rows, `rp`/`rb`/`rbt` fields) and the
  state hash's `shk` stream.
- The servo ULP comparison bands died with the comparators. One line so this is not
  rediscovered as "one timeline fixed our floats": the bands were a symptom sensor for real
  cross-machine float divergence, which remains real and undetected — the determinism state
  hash still consumes raw bits, and the glam/parry landmine is the same class.

No replica/presentation system split was needed: `drive_tracks` self-gates on
`RigidBody::Dynamic` and every client tank is `Static`, so the hull sim never runs on a
replica; the sim modules stay compiled for the server, single-player, and the client-side
presentation consumers (recoil microsim, belt view).

## Surviving levers (all threshold-clean by design, or named as debt)

- `interp_delay` — the derived delay law; no smoothing, no hysteresis.
- `sync_margin` — derived margins, both wire directions; `OVERMATCH_INPUT_MARGIN_MS` /
  `OVERMATCH_INTERP_MARGIN_MS` pin them for experiments.
- `fire_presentation` — client owns fire start/stop and cadence; the replicated `WeaponGate`
  is a legality report consumed as a belt-delta ledger, threshold-free.
- `recoil_overlay` — the microsim-subtract own-recoil response (fork 1's candidate A).
- `cursor_queue` — discrete facts present at the cursor crossing of their authority tick.
- `extrapolate` — ε-bounded continuous gap-filler behind `OVERMATCH_EXTRAPOLATE` (fork 2's
  candidate); its impulse-coincidence instrument now covers Fire/Damage classes only (the
  Shock source left with the wire fact).
- `hit_feel` at arrival (under review, fork 2), the FRONTIER/INPUT-ARRIVAL instruments,
  the track-view snap detector (its render_error upper bracket is unpinned — the remaining
  discontinuity sources are respawn, terrain revision, and lightyear's freeze-then-step clamp),
  servo view interpolation and the `RemoteServos` integrator, lightyear's clock steering, and
  the wire-quantization guards (`HIT_EPS_HP`).
- Named tuned-constant debt, complete: `PHASE_HEAL_OMEGA` (derived replacement sketched in
  `recoil_overlay`) and the `SNAP_TRANSLATION`/`SNAP_AXIS` pair.

**Amended 2026-08-15.** The recoil-overlay/hull-overlay lane above was closed by ruling: the
fused own-fire echo is permanent, and `recoil_overlay`, `CameraKick`, and the cursor-hit-feel
fork were deleted in demolition pass 2 — along with every mode lever (`OVERMATCH_FUSED_FIRE`,
`OVERMATCH_EXTRAPOLATE`, `OVERMATCH_CURSOR_HIT_FEEL`) and the INPUT-ARRIVAL instrument
(FRONTIER remains). The one-timeline half of this ADR stands unchanged.

## What survives of the superseded ADRs

- **0015**: the permanent layer is untouched law — continuous force laws, the solo-divergence
  model, the shape-cast tiling guard. Only the removable netcode-scaffolding layer (its reason
  to distinguish divergence from misprediction) dissolves: there is no misprediction.
- **0017**: subsumed. Contact still resolves on the authority; "non-owned tanks stay
  interpolated" generalized to all tanks.
- **0027**: the disclosure boundary survives — the private element field never leaves the
  server, now trivially (clients simulate no elements). The convergence machinery dissolves.
- **0029**: the wire shape survives verbatim — one atomic `WeaponGate`, absolute `ready_tick`,
  arrival never writes live sim. The owner-prediction/rollback-restore half dissolves; the
  client reads the gate as a legality report.
- **0030**: the atomic `TankServos` component and its determinism-trace role survive; owner
  reconciliation dissolves. The own turret is click-immediate client view state; remotes drive
  from replicated `ServoAngles`.
- **0032**: dissolves wholly, mechanism and need.

## The two open forks (deliberately not decided here)

1. **Own-fire presentation — A (replay) vs B (fuse)**. A is the landed microsim-subtract
   (instant, ~900 LOC + a standing fidelity contract); B presents the whole shot at the cursor
   crossing (zero machinery, cost = the click→bang floor, ~145 ms at 75 ms ping). Taste call at
   the feel gate, order-counterbalanced with washout, at real margins. B winning ratifies
   delay-as-anticipation and kills A's machinery plus the foreign-impulse-early door.
2. **Cursor position — padded tail-quantile vs arrival edge + ε-bounded extrapolation**. Data
   call: the kill test and the impulse∧gap coincidence instrument decide. Certifying the edge
   kills the padding AND dissolves the getting-hit-at-arrival exception (arrival ≈ cursor);
   failing keeps the padded cursor with the honest freeze, and hit feel stays a live question.

The track-snap detector rewrite is fork-2-dependent (clamp vs extrapolation seam) and was
deliberately excluded from Wave C.
