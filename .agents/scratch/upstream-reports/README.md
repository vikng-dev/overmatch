# Upstream report candidates — index

One file per independently-filable report, from the 2026-07-06/07 MP jitter campaign +
architecture-review session. Each file is self-contained: mechanism, evidence (vendored
file:line + our commits + measurements), suggested upstream fix, our shipped workaround and its
removal condition. File them upstream in any order; each carries enough to stand alone. Severity
is OUR impact, not upstream's triage.

| # | File | Target | Severity (us) | Our workaround |
|---|------|--------|---------------|----------------|
| 1 | [lightyear-check-starvation.md](lightyear-check-starvation.md) | lightyear 0.28 | CRITICAL — silent 35–50 m desync at LAN latency | `net/watchdog.rs` (8ae795c) |
| 2 | [lightyear-avian-blanket-apply-pos-to-transform.md](lightyear-avian-blanket-apply-pos-to-transform.md) | lightyear_avian3d 0.28 | CRITICAL — collider attachments ratchet off the rig | `AuthoredLocalTransform` observers (33cc4e4) |
| 3 | [parry-gjk-cast-relative-tolerance.md](parry-gjk-cast-relative-tolerance.md) | parry3d 0.27 | HIGH — 200 mm cast noise vs large colliders | witness reconstruction (f4a24c2), test tripwire |
| 4 | [lightyear-dual-marker-tiebreak.md](lightyear-dual-marker-tiebreak.md) | lightyear 0.28 | HIGH — Position rollback checks silently dead | `demote_predicted_interpolated` (d4f92d3) |
| 5 | [lightyear-avian-restore-assumes-enlarged-aabb.md](lightyear-avian-restore-assumes-enlarged-aabb.md) | lightyear_avian3d 0.28 | MEDIUM — restore comment/logic false for child colliders | none needed post-#2 fix; document |
| 6 | [lightyear-confirmedhistory-seeding.md](lightyear-confirmedhistory-seeding.md) | lightyear 0.28 | MEDIUM — local_rollback components restored to add-time defaults | `strip_confirmed_history` (net/protocol.rs) |
| 7 | [lightyear-sealed-correction-policy.md](lightyear-sealed-correction-policy.md) | lightyear 0.28 | LOW — API gap, no builder for CorrectionPolicy | render-error layer bypasses (597ec21) |
| 8 | [avian-solver-constraint-order.md](avian-solver-constraint-order.md) | avian3d 0.7 | LOW impact / **HIGHEST strategic** — the last same-machine non-determinism; linchpin for deterministic MP | none (bounded/self-healing, absorbed by server auth) |
| 9 | [lightyear-avian-childof-not-replicated-transform-mode.md](lightyear-avian-childof-not-replicated-transform-mode.md) | lightyear_avian 0.28 | NONE for us (we use Position mode) — upstream's own test fails at the 0.28.0 tag | none needed |

Housekeeping: when an upstream fix ships, the matching workaround's removal condition is stated
in each file (and #3 has an automatic tripwire: `tests/spherecast_scale.rs` FAILS when parry
fixes the tolerance, flagging the workaround for retirement).
