# hsim session → wave-A session (2026-07-10)

From the hsim-divergence session (task #24, branch `hsim-divergence-decode`, rebased onto post-collapse main). Fold into your
report edits as you see fit, then delete this file.

## What you get from us

- **Per-field hsim decode + window attribution** landed on `hsim-divergence-decode`
  (@ f516fb6, rebased onto d006b4b): `hdrv`/`hsrv`/`hrld`/`hrec`/`hanc` sub-hashes, `SPIKE_TRACE_SIM_FIELDS=1` for
  raw carried values (`simf`), and `analyze.py` MISMATCH WINDOWS (per-window field tally +
  magnitudes + opens@first-shared vs mid-run + replay-row counts). This is the tooling your
  A/B wants. Full findings: design doc §9 on that branch.
- Caveat for A/Bs: the decode CHANGES the `hsim` construction, so client and server binaries
  must be the same build — a mixed pair reads as 100% `hsim` mismatch with clean physics (we
  hit this live when the main checkout's target/ was rebuilt under us mid-run-set).

## What lands in YOUR court (class 3 — the discriminating workload you asked for)

A mid-run window carrying pos+rot+lv+av that opens at a perturbation event and persists to
trace end, below rollback thresholds (MEASURED, 80/10, `SPIKE_PERTURB=0`, short course):

- Seeds at the FIRE tick (rebaseline main@2a482c6: short @493, short3 @497 — initial |Δp|
  0.230 mm in both, same seed twice) or at the connect-replay tail (our d7 @295 ~1.7 mm,
  f1 @358). Incidence 2/3 rebaseline shorts, 3/12 our valid decoded runs (d7, f1 connect-seeded; g3 fire-seeded with reload=recoil=0 — the seed is the shell-spawn/impulse perturbation itself, not the fixed recoil ambiguity).
- The seed tick shows an ALL-16-wheel anchor discriminant flip (one end's brush anchors all
  release/re-grip, the other's don't) — smells like your contact-restore/BVH class (#5)
  and/or constraint-order (#2) at island-change events (shell spawn at fire; replayed contact
  discovery at connect).
- Once seeded it can contract dynamically (d7: 1.7 mm → 78 µm) then RE-AMPLIFY at contact
  events (d7 late course: |Δlv| 6.27 m/s, |Δp| 64 mm, still no rollback triggered).
- reload/recoil/drive stay 0 throughout; servo/anchor divergence is derived from the pose
  offset (aim targets are pose-dependent, anchors are world points) — don't chase them.

Suggested discriminator: your avian constraint-order fork A/B on the short course, N≥6 runs,
counting class-3 incidence pre/post. Our traces: `<our scratchpad>/decoded/` (d7, f1 are the
class-3 specimens with decode; the rebaseline short/short3 are yours).

## FYI

- Fire recoil windows (33-tick `hrec`-only) were OURS: fire↔apply_recoil Bevy order ambiguity,
  fixed on the branch (f516fb6). Post-fix they're gone; don't count them in your A/Bs if you
  fork from the branch — and DO expect them if you A/B on main-era builds.
- §7 connect hang: reproduced 3/10 at 80/10 (not lat0-specific). Client goes silent right
  after the connect ROLLBACK-SNAP log line, process alive. Budget retry loops in scripted runs.
