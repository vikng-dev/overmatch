# Belt contact: after the refutation

**Decision: build the round-3 capped row with one change. The third step restarts from the zero-impulse base (this point's cached impulse dropped), not from the warm base.** That is about 8 changed lines on the discarded patch and it closes the refutation by proof, not by tuning. Nothing here was built or run; everything is DERIVED from source unless marked.

## 1. The refutation is real, and it is one fact seen from two sides

- **The descent claim is true but measured against the wrong reference.** A bare-inertia step never increases the row's energy relative to where the step starts. In the discarded patch it starts at `self.previous`, which already contains the cached impulse (`ContactMetric::minimum` in `refuted-round3.patch`; the fork applies it to both bodies in `contact/mod.rs` `warm_start` before the hook runs).
- **Warm start is not passive on its own.** Re-applying a cached impulse to a system at rest injects energy, and the row is what must remove it. Composed passivity therefore needs "no worse than zero friction impulse", not "no worse than the warm point".
- **The witness is fair.** From rest with a passive drivetrain, zero impulse is the unique minimum, so the witness demands exactness in that case. The builder's scalar example checks: 100 -> 3.06 -> 0.31, then the fallback restores 3.06.
- **The MEASURED 871 of 1881 N s residue** is consistent with a bare-inertia undershoot of roughly the bounded capacities times m/(m+B). I did not reproduce the figure exactly.
- **So "an upper bound on mobility cannot give composed passivity" is correct for that base, and false for the zero base.** A bare step from zero satisfies energy(end) <= energy(zero). At rest the slip at zero is zero, so the step is exactly zero.

**Would round 1 pass the same witness?** Yes by derivation; it needs a run to confirm.
- The witness has no parked or held brake (`park: false, mean_hold: false`), so the response is a convex potential's gradient and the line-searched Newton converges to zero.
- The f32 body and f64 owner updates cancel exactly, because the same f32 delta is applied both ways (`surface_motion.rs` `solve`, lane lines 711-719).
- The run needs only a test-only `kinetic_energy` accessor, about 12 lines; `DriveTick::contact_only` already has the witness's signature on the lane.

## 2. The options

Two findings from source decide most of this.

- **The convergence panic lives in the block, not in the cached path.** `Search::trial` computes speeds, then `check_face` clamps an interior impulse accepted up to bound·(1+1e-11), then `snap` zeroes the slip (`block.rs` lane lines 255-287, 640-700). The warm-seeded full solve has the same 1e-11 jump as `FaceResponse::apply`. Deleting the cached path does not remove it.
- **An exact solve does not exist once the block owns the regime.** With static above kinetic, re-decided from totals, there is a band where the held answer exceeds static capacity and the sliding answer falls below it. No self-consistent point exists there, so any iteration needs a terminal non-exact rule. Only round 1's latch avoids that, and the latch is the order dependence and the duplicated law.

| | (i) capped + exact third step | (ii) drop warm start for belt rows | (iii) round 1 + P2 fixes | (iv) capped, third step from zero base |
|---|---|---|---|---|
| No energy from rest, warm start included | yes if it terminates | yes, trivially | yes (derived, not run) | yes, proven |
| Brake within capacity holds, any substep | yes | only by relaxation near capacity | yes | yes, one visit when warm |
| Beyond capacity, exact impulse | yes outside the band | yes outside the band | yes, by latch | yes outside the band |
| Order decides whether a brake holds | no, but needs a band rule | no | yes, latch remains | no |
| Panic on valid input | yes until a new stop rule is added | no | yes until repaired | no |
| Block solves per point visit | 1 typical, unbounded tail | 1 typical | 4 or more, allocating | 1 typical, 4 worst |
| Lines against the lane | patch + about 65 | patch + about 10, fork grows | about +150 net, more for the latch | patch + about 35 |
| `avian_ext` independent of `track` | yes, via trait | yes | only after moving the adapter | yes, via trait |

- **(i)** needs the potential back on the trait, a line search, and a stop rule tied to objective roundoff to remove the panic. It still needs a terminal rule for the band, so it strictly contains (iv). Rejected.
- **(ii)** needs the fork to skip the tangent warm impulse for hooked rows, since `warm_start` applies it before dispatch. It discards warm information on every visit to fix a 1-in-20,000 case. Its creep cost is the cleared-friction slab regime, which the lane already passes at 1/3/6/12 substeps (MEASURED by the builder); its cost in play is unmeasured. Rejected as dominated by (iv), which drops the cached impulse only when the row cannot certify it.
- **(iii)** keeps every structural P2 source: tolerance convergence on a discontinuous response, the latch, and per-point allocation. Each repair adds code. Rejected.
- **(iv)** has two exits. "Face unchanged after the push" is exact, because the response is affine on a face and `same_face` compares the assignment and the regime flags. Otherwise it takes the bare step from zero. Both satisfy energy(end) <= energy(zero) for a fixed regime.

**Known cost of (iv):**
- When the third step fires on a warm point, that point gets about 14% of its needed impulse for one pass (W·I ≈ 0.166, recalled from the premise check).
- In the band every visit lands there, so braking force sits below the kinetic rating while hull slip is roughly 0.015 to 0.026 m/s per point (DERIVED from the lens figures: 437 N s static, 1.2 factor, 4750 kg, 3.5e-5 /kg). That is a few substeps at the end of a skid.

## 3. Build outline

1. **Re-apply `refuted-round3.patch` to the lane, minus its scratch note:** +1275/−1153 (MEASURED by `git apply --numstat`). This brings the trait, `src/track/surface_motion.rs` (344 lines, about 130 of them test and fixture), the block-owned regime, `same_face`, and the `contact_only` fix.
2. **Change the third step in `ContactMetric::minimum`:** push the owners back to zero total for this point, evaluate slip there, and take the bare-inertia disk step from zero. About +5/−3 lines.
3. **Rewrite the comment and the `track-contact.md` paragraph** to state the two exits, the zero-base rule and the band. About +10/−8 lines.
4. **Bump `PROTOCOL_REV`** if the builder's rule requires it for a sim change; the refutation left it at 107.

**Witnesses:**
- `bounded_drivetrain_warm_start_cannot_create_energy_from_rest`, kept as written. Its named mutant is "restore the warm base", which fails at mass 100000 (MEASURED).
- One new witness, about 30 lines: from a moving start on the same grid, energy after the visit is no more than energy after a frictionless visit. The round-3 gap was that rest-only coverage on a free surface hid the composition.
- Every existing witness: free-surface energy, both-body momentum, exact sliding impulse, slab hold cleared and warm at 1/3/6/12 substeps, readback bit-check, `final_contact_totals_redetermine_the_brake_regime`, the Driver ledger, and both creep probes.

**Cheapest refuting check:** steps 1 and 2 in a bay, then run the new witness, the warm slab witness and the exact-sliding witness, and count third-step firings on one driving-and-steering scorecard run. The recommendation is refuted if any of these happens:
- any witness cell leaves energy above roundoff;
- warm slab creep exceeds round 1's 0.001 m/s;
- the third step fires on more than 1 in 1,000 point visits while driving. That bounds traction loss under 0.1% (DERIVED: at most 86% of one point for one pass).

The formulation is settled enough to build. The one open measurement is that firing rate under steering load.

## 4. Size, plainly

- **The lane today:** +3080/−743 over 23 files, net +2337 (MEASURED). Round 0 alone is +1446/−544, net +902 (MEASURED).
- **Under this recommendation:** net about +2490 against `main` (DERIVED: 2337 + 122 from the patch + about 35). The patch is nearly neutral in total because its 344-line adapter file and 170 lines of scorecard offset the deletions.
- **Where it shrinks:** `src/avian_ext/surface_motion.rs` falls from 1097 to about 780 lines (DERIVED).
- **Is there a materially smaller correct fix? No.** The creep law is a mass ratio, so the row must see the drivetrain's held mobility. That needs the owner's response inside the row, and a release mid-step needs the second step. Round 0 alone is the 4 to 15 cm/s creep.
- **What can be trimmed is scope, not formulation:** the two `#[ignore]` observation probes with no named runner (lens N5), and the scorecard additions (299 lines changed on the lane, 170 added by the patch).

## 5. What I could not verify

- Nothing was run, including round 1 against the new witness and the zero-base step against the warm slab.
- `row-creep.log` and `row-cost-final.log`, cited by the refutation, are not in `/Users/Yan/github/vikng-dev/overmatch/.agents/scratch/rubble-explore/belt-main/`. The 1.5 to 4.2 microsecond and 3.0 + 3.3 ms figures are the builder's, unchecked.
- Cost against `main`, in release, on two cores. The parallel-work P2 stays open under every option; 6.3 ms per tick for 8 tanks is large against a tick and is the next thing to price.
- How the builder counted "more than two iterations", so the third-step rate may differ from 1 in 20,000 to 60,000. The rate under steering load is unknown.
- Passivity across a static-to-kinetic break. Bare inertia is not an upper bound on mobility at the moment a loaded static brake releases, because the held load is freed (extra belt speed up to about 0.015 m/s, DERIVED). The proof covers a fixed regime and the at-rest witness; round 1 shares this limit.
- The lens's ping-pong panic input was derived, never reproduced.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/.agents/scratch/rubble-explore/belt-main/refuted-round3.patch
- /Users/Yan/github/vikng-dev/overmatch/src/avian_ext/surface_motion.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/surface_motion.rs (created by the patch)
- /Users/Yan/github/vikng-dev/overmatch/src/track/block.rs
- /Users/Yan/github/vikng-dev/overmatch/.agents/docs/design/track-model/track-contact.md
