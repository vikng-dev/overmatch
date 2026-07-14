# Upstream report candidates — index

One file per independently-filable report, from the 2026-07-06/07 MP jitter campaign +
architecture-review session. Each file is self-contained: mechanism, evidence (vendored
file:line + our commits + measurements), suggested upstream fix, our shipped workaround and its
removal condition, and **what fixing it would let us clean up, optimize or explore** (the
`What fixing this unlocks for us` section — added 2026-07-12; three of the ten honestly unlock
*nothing* for us and say so). File them upstream in any order; each carries enough to stand alone.
Severity is OUR impact, not upstream's triage.

| # | File | Target | Severity (us) | Our workaround | Unlocks for us |
|---|------|--------|---------------|----------------|----------------|
| 1 | [lightyear-check-starvation.md](lightyear-check-starvation.md) | lightyear 0.28 | CRITICAL — silent 35–50 m desync at LAN latency | `net/watchdog.rs` (8ae795c) | Delete `net/watchdog.rs` (346 lines) + the `pub(crate)` threshold coupling. **Input delay stops being a correctness knob** → the 0-tick / adaptive-delay experiments |
| 2 | [lightyear-avian-blanket-apply-pos-to-transform.md](lightyear-avian-blanket-apply-pos-to-transform.md) | lightyear_avian3d 0.28 | CRITICAL — collider attachments ratchet off the rig | `AuthoredLocalTransform` observers (33cc4e4) | Delete the shield (marker + 2 observers + `authored_attachment`) and the standing "every child collider must carry the marker" rule; one of the two conditions for tightening the rollback bars |
| 3 | [parry-gjk-cast-relative-tolerance.md](parry-gjk-cast-relative-tolerance.md) | parry3d 0.27 | HIGH — 200 mm cast noise vs large colliders | witness reconstruction (f4a24c2), test tripwire | Delete the reconstruction + `SPHERE_CAST_TOI_SLACK` + `tests/spherecast_scale.rs`; retires ADR-0015's "tile static colliders to ≤10 m" map rule; **future** shape-cast consumers (track model, armor probes) stop inheriting the defect |
| 4 | [lightyear-dual-marker-tiebreak.md](lightyear-dual-marker-tiebreak.md) | lightyear 0.28 | HIGH — Position rollback checks silently dead | ~~`demote_predicted_interpolated`~~ **deleted at f2c9510** (root cause was our missing `ReplicationSender`) | Nothing to delete — **diagnostic only**: a `warn!` on the marker combination turns a multi-second silent-desync hunt into a log line |
| 5 | [lightyear-avian-restore-assumes-enlarged-aabb.md](lightyear-avian-restore-assumes-enlarged-aabb.md) | lightyear_avian3d 0.28 | MEDIUM — restore comment/logic false for child colliders | none needed post-#2 fix; document | **Nothing for us** — filed for the ecosystem (goes live only if a child collider's pose can genuinely diverge during prediction) |
| 6 | [lightyear-confirmedhistory-seeding.md](lightyear-confirmedhistory-seeding.md) | lightyear 0.28 | MEDIUM — local_rollback components restored to add-time defaults | `strip_confirmed_history` (net/protocol.rs) | Delete `strip_confirmed_history` + the standing rule that every new `local_rollback` component must be registered there. **Not** the reason `NetBelts` pins (checked — see the file) |
| 7 | [lightyear-sealed-correction-policy.md](lightyear-sealed-correction-policy.md) | lightyear 0.28 | LOW — API gap, no builder for CorrectionPolicy | render-error layer bypasses (597ec21) | **Nothing for us** — `net/render_error.rs` is permanent architecture (ADR-0014/0015), not scaffolding. Filed as an API suggestion |
| 8 | [avian-solver-constraint-order.md](avian-solver-constraint-order.md) | avian3d 0.7 | LOW impact / **HIGHEST strategic** — the last same-machine non-determinism; linchpin for deterministic MP | none (bounded/self-healing, absorbed by server auth) | Nothing to delete. **The enabler for predict-everyone** (ADR-0017 names this file as its revisit condition) → tank-tank collision felt locally; tightens the rollback bars; makes deep replays cheap |
| 9 | [lightyear-avian-childof-not-replicated-transform-mode.md](lightyear-avian-childof-not-replicated-transform-mode.md) | lightyear_avian 0.28 | NONE for us (we use Position mode) — upstream's own test fails at the 0.28.0 tag | none needed | **Nothing for us** — filed for the ecosystem (a failing test already in their tree; the cheapest issue here to act on) |
| 10 | [lightyear-absent-anchor-input-freeze.md](lightyear-absent-anchor-input-freeze.md) | lightyear 0.28 | CRITICAL — server fires unauthored rounds after release (ammo + damage, nothing to roll back) | `fixed_input_delay(3)` + `TankCommand.for_tick` (REV 5) | Retire the delay pin + the anti-adaptive tripwire. `for_tick` **stays** (the invariant is ours) but its wire cost (+20 B/msg aiming, +140 B/msg idle, measured) becomes optional rather than load-bearing |
| 11 | [lightyear-native-input-encoder-inverted-range-oom.md](lightyear-native-input-encoder-inverted-range-oom.md) | lightyear_inputs_native 0.28 | CRITICAL before containment — inverted client tick range allocates until OOM | pre-`PrepareInputMessage` buffer guard + fixed shipping delay | Delete the guard, metrics, malformed-composition fallback and focused tests. The Docker fan-out lane stays; adaptive delay remains blocked on #1 and #10 |

**Evidence note on `InputDelayConfig::balanced()`:** it is implicated in **three** CRITICAL failure paths
(#1, #10 and #11). Its prediction-margin and fabricated-input failures are measured in #1/#10. Its role
in exposing #11 is an inference from the source path and the zero-clear fixed-delay A/B recorded in #11.
We have pinned it. Reconsider adaptive delay only when all three are resolved upstream.

## Cross-report unlocks — the things blocked on MORE THAN ONE fix

- **Adaptive input delay (`balanced()`) — needs #1, #10 AND #11.** A delay that tracks the link would give
  near-0 input latency on a good connection and safety on a bad one. It is unsafe today for three
  independent failure paths: a *varying* delay corrupts the input stream (#10 — `Δend_tick != 1` strands stale
  PRESSED inputs on the server and fabricates `SameAsPrecedent` ones on both ends), can invert the native
  encoder range after resync (#11 — inferred trigger for an unbounded client allocation), and a delay that grows
  into the link's natural lead walks the prediction margin to zero, where state rollback is silently dead
  (#1). Any one alone is a shipping defect. This is the headline cross-report item.
- **0-tick input delay — mostly #1, softly #8, and NOT what you would guess.** The intuition that 0-tick
  squeezes the prediction margin is **backwards**: `sync_objective` subtracts input delay from the
  prediction lead one-for-one (`lightyear_sync` timeline/input.rs:285-310), so 0-tick *maximises* the
  margin — it is #1's own falsifier. `no_input_delay()` is also constant, so #10's wobble seeds never fire
  and the anti-adaptive tripwire does not block it. What 0-tick really costs: the deepest prediction window
  (→ deep rollbacks through chaotic contact, measured 5.6× median / 43× p90 correction amplification — the
  bill #8 pays down) and zero jitter cushion, so a late input becomes a **dropped trigger pull** (we fail
  closed on `for_tick`) instead of an unauthored round. An experiment to run (`SPIKE_INPUT_DELAY_TICKS=0`
  already exists as the lever), not a free win. What #1 changes is that input delay becomes a *feel* knob
  we may tune by playtest instead of a correctness knob we must pin.
- **Predict-everyone (and with it, tank-tank collision a player can feel) — needs #8, plus work that is
  not upstream's.** ADR-0017 names `avian-solver-constraint-order.md` as its explicit revisit condition.
  But determinism is necessary, not sufficient: the bot has no `ControlledBy` and no client authoring its
  input (so this is really predict-every-*player*, a mixed mode), and reliable remote fire needs input-side
  rollback, which would break the tick-agnosticism of the crew/belt appliers. Note also what it does *not*
  buy: `ServoAngles` and `FireEvent` **survive** predict-everyone (ADR-0016 and ADR-0021 both retracted the
  earlier "deletable" claim), so there are no wire savings — the payoff is gameplay and the collapse of the
  divergence error class.
- **Tightening the rollback bars (`ROLLBACK_POSITION_M` / `ROLLBACK_ROTATION_RAD`, 5× coarser than the
  1 cm / 0.01 rad reference) — needs #2 AND #8.** ADR-0015 calls them "a ratchet, not a setting" and names
  exactly those two conditions ("contact-restore fix, upstream constraint ordering"). #2 is banked; #8 is
  the open half.

Housekeeping: when an upstream fix ships, the matching workaround's removal condition is stated
in each file (and #3 has an automatic tripwire: `tests/spherecast_scale.rs` FAILS when parry
fixes the tolerance, flagging the workaround for retirement).
