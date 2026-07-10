# Handoff — the aim-intention transition regression cluster

2026-07-10, end of the correctness/feel session. For a DEDICATED debugging session with Yan
available for windowed testing. Delete when consumed. Board task: #27.

## The one-line problem

The committed-aim unification (73fce71 + entry fix d608490, both on main) still mis-lays the gun
around mode transitions and first optic input. The class is representation conversion: third
person commits a RESOLVED WORLD POINT, the optic aims a DIRECTION; every place the code converts
between them is a suspect.

## Live symptoms (Yan, windowed, main @ d608490 — his words paraphrased, verbatim answers below)

1. **Gunner → third person:** the white (screen-center) and amber (committed-aim) dots end up
   ABOVE the green (bore) dot; the gun then "immediately corrects and aims at the new target,
   which is slightly higher." Happens with RMB held AND up. Happens regardless of dialed range.
2. **Third person → gunner (the d608490 fix holds for entry):** "the aim looks correct until I
   move the mouse — at which point the amber dot SNAPS MUCH HIGHER and the gun pitches upwards."
3. Earlier in the day (FIXED by d608490, context): entry itself used to re-encode the committed
   floor point as a 10 km far point — measured gun rise +2.58° @ 50 m, +9.05° @ 15 m, → 0 at the
   horizon (mount parallax: hull-frame origin ≈ ground level, Main_Gun_Pitch mount at
   (0, 2.2171, −1.100) hull-local, from tiger_1.glb).

**Discriminators already collected (do not re-ask):** both RMB states affected; the gun
self-corrects to the (wrong, higher) target immediately; range-dial-INDEPENDENT; entry is stable
until first mouse motion.

## What is MEASURED vs what is HYPOTHESIS — keep this line sharp

MEASURED / established:
- The mount-parallax table above (d608490's report) — real, but it predicts a SMALL first-nudge
  drift (~2.6° @ 50 m, ~0 far). Yan reports "snaps MUCH higher" — the observed term is plausibly
  LARGER than mount parallax, and symptom 1 appears even for far (directional) commitments where
  parallax ≈ 0. Something else is in the sum.
- Conventions as documented in code: `CommittedAim` holds the RAW sight line (pre-superelevation);
  `drive_aim_servos` adds `lob(dialed range)` to the pitch servo target (both modes — it is
  mode-agnostic); the gunner camera looks along bore − θ; the optic's Bound 1/Bound 2 shift by θ
  (`sight_now = g_current − θ`). Whether the CODE actually honors these conventions on every path
  is exactly what's in question.
- The four `CommittedAim` invariants (doc block, src/aim.rs): recirculation (b206f34 — holding is
  an act), possession entity-key, single-writer (toggle frame ordered), zero-input identity
  (d608490). Any fix must preserve all four.

HYPOTHESES (ranked, none verified — verify by instrumentation, not reasoning):
1. **θ folded once too many (or once too few) in the point→direction conversion.**
   `from_hull_local_dir` decomposes the committed point to a bearing; the optic treats intent
   pitch as SIGHT-LINE pitch. If the decomposed pitch is then measured against / clamped toward
   `sight_now = g − θ` and re-published, θ can enter the published direction once more than the
   convention allows. Symptom's range-dial-independence cuts against a DIALED-range θ term —
   check what range the `RangeTable` lob actually uses at default dial, and whether `lob` of the
   DEFAULT range is already large enough to explain "much higher" (if θ(default) ≈ 0, this
   hypothesis weakens — say so and move on).
2. **The lob applied to a mismatched target distance.** `drive_aim_servos` lobs by the dialed
   range regardless of the committed point's actual distance. A near point lobbed for a far dial
   (or the 10 km directional point lobbed at all) produces a bore/reticle disagreement that
   RENDERS as dots-vs-green offsets. Range-independence of the symptom is a constraint here too.
3. **Residual mount parallax appearing on BOTH transitions** (the d608490 residual, but larger
   than predicted because the hull-frame origin↔mount geometry enters somewhere twice — e.g. the
   decomposition uses hull origin while `drive_aim_servos` converges per-servo from the mount).
4. **The three HUD markers disagree about frames**: amber in third person projects through the
   un-kicked camera Transform (b206f34-era fix), the gunner reticle projects the actual point
   (d608490), the green bore dot projects the barrel direction — check each marker's projection
   math against the same committed value before trusting any of them as evidence.

## Where the code is (all on main @ d608490)

- `src/aim.rs` — `CommittedAim` (+ the four-invariant doc), `commit_aim` (third-person commit +
  RMB hold), `drive_aim_servos` (mode-agnostic servo convergence + lob), `update_aim_indicator` /
  `update_bore_indicator` (amber/green dots).
- `src/sight.rs` — `drive_gunner_aim` (resume/seed, Bound 1 travel clamp, Bound 2 margin clamp,
  θ shifts, sensitivity), `resume_commit` (the zero-input-identity decision), `GunnerIntent`
  conversions (`local_dir` / `from_hull_local_dir`), `update_intent_reticle`, `toggle_sight`.
- `src/firecontrol.rs` — `Ranging`, `RangeTable`, `lob`, `adjust_range`.
- History that carries the invariants: b206f34, 73fce71, 1079859, d608490 — read the commit
  bodies; they are the doctrine.

## How to work it (hard-won process rules from this session)

1. **Instrument before theorizing.** The session that found the recirculation bug won by logging
   bit-exact values. Add a temporary env-gated debug line (or on-screen debug text) dumping, per
   frame in both modes: the committed hull-local point, its decomposed yaw/pitch, `t_current` /
   `g_current`, θ, the servo pitch target, and what each HUD marker is about to project. One
   windowed session with Yan reproducing while the numbers stream beats any amount of schedule
   reading. Strip the instrumentation before merge.
2. **Windowed repro is REQUIRED** — the headless harness writes `command.aim` directly and never
   exercises these paths. Yan is available to drive; `OVERMATCH_SERVER=157.245.48.161 cargo run`
   or a local server (see AGENTS/docs harness notes; avoid lat0).
3. **Premises must be marked.** This session lost an agent-day to an unverified premise stated as
   fact in a brief. Everything in the HYPOTHESES list above is unverified.
4. **The feel matrix includes the NEAR/FAR axis.** The unification originally shipped through
   review + feel checks because every check was at the horizon, where point ≡ direction. Any fix
   here gets verified at: near floor (~15 m, ~50 m), horizon, sky × both transitions × RMB
   held/up × default and dialed range.
5. **Consider whether the right fix is the deeper design** rather than a third patch: make the
   optic ALSO commit resolved points (raycast along the sight line to ground/target, far-point
   fallback) so both modes speak points and the direction/point dualism — this entire bug class —
   disappears. That connects naturally to real ranging (the dial). It changes optic feel
   semantics, so it's Yan's call to make with the numbers in front of him; the session should
   present both the surgical fix and this option with tradeoffs.
6. Workflow: implementers work in the bay worktree (`../overmatch-bay-1`, warm) or the main
   checkout if Yan isn't using it — never both writing one directory; foreground builds/gates
   only (background-and-wait kills agent sessions); commit early; the five gates + team-lead
   review before merge (this is the most regression-prone surface in the repo — three regressions
   in one day came from it).

## Verification gates (unchanged)

```
cargo clippy --all-targets --features net -- -D warnings
cargo clippy --all-targets --no-default-features -- -D warnings
cargo test --features net
cargo test --no-default-features
cargo build --features net --bin overmatch --bin overmatch-server
```

NAN-tripwire/panic greps zero on any harness run; `tests/ui_ascii.rs` and the sight/aim unit
tests (`zero_input_resume_is_identity`, `intent_dir_round_trips`, the clamp tests) must stay
green — extend them with whatever invariant the fix establishes.
