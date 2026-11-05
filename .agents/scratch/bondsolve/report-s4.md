# Bondsolve, session 4

Subject: the video game's environment destruction model, game state only. The test flight
fixtures concern the game's damage model; crew remain HP pools at stations.

## Result and decision

**The placed-house spawn bar is still not met.** The solver defect behind the explosive
cascade is fixed. The larger houses still have converged initial nail-edge overloads under
the authored data. Roof, map, scales, strengths and settled state are unchanged.

[The gravity diagnosis](gravity-s4.md) owns the per-house tables, all initial failure classes,
demand/capacity and positions, the scale derivation, judgement residuals, contact trace and
owner options. This handoff records the implementation and verification boundary.

## Solver and observation

`Assessment::failures` refuses an unfinished field. Cohesive failure requires projected
equilibrium; contact separation still accepts a certified mechanism. The 32-round runtime
budget, material values and load law are unchanged. A topology change retains the previous
displacement as a guess, then recomputes its reactions before admitting any strength verdict.

The analytical stale-warm hanging-chain witness was red on the original law and green on
the convergence gate. It verifies both the unfinished stage's preserved cohesion and eventual
convergence below the independently calculated total-weight/area limit. Observational helpers
keep constrained degrees out of residuals, print admitted traction uses and follow actual
projected contact resultants. The census's unfinished count reads the actual game state,
including the case that convergence finishes on its last observed tick.

MEASURED first stages: **39 houses clean; 35 larger houses crack 2–15 joints; 257 total**.
All initial failures are `nailed` board-edge shear on the western roof, against the attachment's
Unknown 3,240 Pa lateral analogue. Unit peak use is 0.992994; largest-scale peak is 1.627850.
The initial residuals are below 1e-8. The unguarded law had 523 additional nonempty stages
judged while Iterating, at residuals 0.134912–2.154933402e142.

MEASURED with the gate: no additional cracks or departures through 128 law ticks. All 635 trees
stay intact; 35 houses still redistribute. The largest house later reaches two genuine,
converged stages: 13 more cracks at numerical tick 342 and 8 at 912, with zero departures.
It remains unfinished at tick 2047. Closed cracked seams retain contact and connectivity.
No price or real-time convergence promise is inferred from these counts.

The owner can select unit-scale placement, the observed standing range, or authored size
variants with correct framing and stock dimensions. DERIVED: unit-scale placement would need
one admitted house equilibrium instead of the current 75 distinct scales. The previously
reported missing roof framing, automatic board-edge nailing, Unknown fixing resistance and
scalar timber response still limit physical-soundness claims. No number was increased to pass.

## Caller corrections

The broad library run completed with **2,382 passes, seven failures, 40 ignored**. All seven
failures have concrete fixture causes; none is a reason to restore the old failure law:

- Four populated-map headless witnesses and both real-flight witnesses started their async
  watchdog before synchronous construction admission. Captured failures name Loading, including
  an already Loaded spec. The watchdog now starts after the first update's synchronous Startup;
  its 60-second async limit and frozen simulation clock remain. Playing, delayed-IO, real-flight,
  collider coverage and pristine common-spawn assertions remain intact.
- The real-UDP reset fixture used `Crack` to mean removing every incident support. A closed crack
  now retains pressure/friction contact. Its explicit fixture cause is `Separate`, and it copies
  separation state with crack/fate state when publishing mock departures. Current-world carrier,
  reconnect, reconstruction and ledger-prefix assertions remain.

The pinned Bevy 0.19.1 `App::update` and `Main::run_main` sources confirm that synchronous
Startup runs on the first update. No Avian API or wire type is changed. The pins audit's
class-b contracts stay; the reset fixture now supplies the cause its own support-loss premise
requires rather than preserving the retired meaning of a crack.

## Dedicated-server proof

The dedicated build was started first. Its initial incremental archive failed to link;
rebuilding with incremental compilation disabled succeeded, as did the build carrying the
convergence gate. The preliminary launch was stopped before observation; it is not a pass.

MEASURED final launch, unattended, headless, muted, loopback and zero bots, under the machine
lock: **nine stats lines and 257 fixed ticks across the completed ten-second observation**.
Every window contains fixed ticks. The final counters are **257 cracked joints, zero separated
joints, zero departed chunks**. The process keeps ticking and is terminated after observation.
This proves continued operation, not the unchanged placed-house spawn bar or a tick-price gate.
Counts are in `target/bondsolve-s4-boot/counts.json`; no timing prices, ram runs or captures were
made. The solver's production code is unchanged by the later test-harness corrections.

## Gates

Before caller corrections, **265 targeted construction/contact/wall/stats/source tests passed**,
four ignored, and the extended census completed. The first clippy pass rejected the enlarged
observation function's size; its residual/use/departure calculations now have shared test-only
helpers, without an allow attribute. Corrected formatting, all-target/workspace CI clippy and
**49 source-guard/regression tests passed**. All seven previously failing caller tests passed
in the restarted full run. The **final corrected library run passed 2,389 tests, with zero
failures and 40 ignored** on the final Rust source. Formatting and CI clippy passed on that
same source. The log is `target/bondsolve-s4-final-gates.log`.

## Remaining work

- Owner decision on roof construction and placement scale; the larger houses still crack at spawn.
- Unfinished redistribution under the bounded runtime budget; later stability is not certified.
- Directional timber, axial fixing compliance and matched lateral connection facts remain open.
- Pricing and play review belong to the orchestrator. The b14 serial-split finding is already
  addressed by session 2: production contact groups are split inside owner workers, with ordered
  collection; the old `split` adapter is test-only. No extra parallel path was added.

Session start: 2026-11-05 16:15:41 local. Work and runs stayed in the leased bay; no push,
external issue comment, forbidden document, screen capture or visible binary.

Session close: 2026-11-05 17:41 local, within the 90-minute limit. Both the full library
verification and the ten-second server proof completed. No lane process remains running.
