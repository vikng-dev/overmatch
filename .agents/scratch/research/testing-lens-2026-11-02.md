## TLDR

- Add a `testing` lens with **coupled assertion** and **substituted composition** as its finding classes.
- A useful test distinguishes a contract-breaking change from a permitted implementation or data change.
- MEASURED: **2,134 `#[test]` attributes**, **124,106 test-only lines**, and **178 test `App` constructor sites** under `src/`.
- DERIVED: test-only lines occupy **47.51%** of Rust source lines: `124,106 / 261,240`. Neither size nor test count measures value.
- Minimal Bevy apps are appropriate for narrow system tests. Composition claims must exercise the production installer responsible for the claimed behavior.
- Keep asset-specific pass conditions in **base-truths**, setup ceremony in **brevity**, and redundant-test policy in quality-standard A5.
- Preserve explicit tripwires. Mutation testing remains an on-demand diagnostic, never a gate.
- This investigation was read-only. References to the game’s damage model concern game state only; crew are HP pools at stations.

## Rules table

| Rule | Source | Covered / new / conflict | Our rule or proposed class |
|---|---|---|---|
| Arrange, exercise real code, assert intended results. | [Rust Book: writing tests](https://doc.rust-lang.org/book/ch11-01-writing-tests.html) | Covered | Name the invariant and its consumer, as AGENTS already requires. |
| Rust supports private unit tests; integration tests use public interfaces. | [Rust Book: organization](https://doc.rust-lang.org/book/ch11-03-test-organization.html) | Conflict with blanket “public APIs only” | Test the useful interface of the owning module. Direct tests of private mathematical laws remain legitimate. |
| `rustc --test` enables `cfg(test)` and supplies libtest; Cargo selects test targets. Tests within an executable normally run concurrently. | [rustc testing](https://doc.rust-lang.org/rustc/tests/index.html), [Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html), [Book: execution](https://doc.rust-lang.org/book/ch11-02-running-tests.html) | Covered mechanics; context risk | Harness success does not establish shipped composition. Isolate shared environment, files and clocks. |
| Brittle tests fail after changes outside their intended contract. Prefer meaningful APIs, observable state and focused behavior. | [Software Engineering at Google, testing overview and unit testing](https://abseil.io/resources/swe-book/html/ch11.html), [chapter 12](https://abseil.io/resources/swe-book/html/ch12.html) | New application | **Coupled assertion:** unrelated tests pin another module’s permitted choices. |
| A transliteration of implementation can detect edits without detecting defects. | [Google Testing Blog: change-detector tests](https://testing.googleblog.com/2015/01/testing-on-toilet-change-detector-tests.html) | New application | Require an independently justified expectation; do not copy the implementation’s answer. |
| Prefer state over incidental interactions; use DAMP rather than maximizing DRY. | [Google chapter 12](https://abseil.io/resources/swe-book/html/ch12.html) | Covered / boundary clarification | Brevity owns obscuring setup. Explicit examples can repeat; copied policy knowledge cannot. Ordering itself remains observable behavior when consumers depend on it. |
| Simplified dependencies lose fidelity; broader tests cover interactions they omit. | [Google test doubles](https://abseil.io/resources/swe-book/html/ch13.html), [larger testing](https://abseil.io/resources/swe-book/html/ch14.html) | New application | **Substituted composition:** a test supplies the wiring it claims production provides. |
| Fragility includes interface, behavior, data and context sensitivity. | [Meszaros, *xUnit Test Patterns*, primary book text mirror](https://studylib.net/doc/28145404/xunit-test-patterns) | Covered / new | Interface sensitivity is not automatically wrong. Unrelated behavioral/data coupling maps to coupled assertion; uncontrolled context threatens repeatability. |
| Obscure tests hide cause and effect; test logic in production can make tested behavior differ from shipped behavior. | [Meszaros book text](https://studylib.net/doc/28145404/xunit-test-patterns), [author’s Obscure Test page](http://xunitpatterns.com/Obscure%20Test.html), [Test Logic in Production](http://xunitpatterns.com/Test%20Logic%20in%20Production.html) | Covered | Brevity owns ceremony; code-branching owns retained alternatives. Do not introduce another class here. |
| Good tests balance isolation, determinism, speed, readability, behavioral focus, structure insensitivity, specificity and predictive value. | [Beck’s desiderata](https://testdesiderata.com/), [Desirable Unit Tests](https://newsletter.kentbeck.com/p/desirable-unit-tests) | Covered principle; new lens | A failure should identify a meaningful broken contract; a pass supports only the exercised scope. |
| Redundancy means identical failure conditions, type-guaranteed assertions or superseded coverage—not similar setup. | `.agents/docs/code-quality-standard.md:245` | Covered | Retain A5; require the distinct mutant each test catches before deleting either. |
| Asset-specific gates, labelled inline `Fixture` data, test ceremony and tripwires already have owners. | `AGENTS.md`; base-truths; brevity; quality-standard B4 | Covered | Do not duplicate these classes in testing. |
| Mutation testing is on demand, never a gate. | `.agents/scratch/research/lint-gates-brief.md:10`; `.github/workflows/mutants.yml:3`; quality-standard:84 | Covered | No mutation-score target or new mandatory workflow. |

The Meszaros author site was unavailable during retrieval; the cited mirror contains his book text. It distinguishes interface changes, changed behavior affecting dependent tests, fixture-data changes, and environmental/context changes.

Tool choice follows the risk:

- **proptest** generates inputs against a stated property and shrinks failures. It can expose unanticipated boundary combinations; it cannot supply a missing property. Failure persistence replays seeds, with qualifications when strategies change. For Overmatch, I recommend generic bounds, geometry and state-transition properties. [Official introduction](https://proptest-rs.github.io/proptest/), [failure persistence](https://proptest-rs.github.io/proptest/proptest/failure-persistence.html)
- **insta** detects changes against reviewed output references and supports structured snapshots/redaction. I recommend focused stable readouts or compiler output, where reviewers can judge the reference. Whole-world snapshots risk preserving incidental structure; acceptance alone does not establish correctness. [insta](https://insta.rs/), [Beck on snapshots](https://newsletter.kentbeck.com/p/snapshot-testing)
- **cargo-mutants** changes implementation and checks whether tests notice. Missed mutants reveal questions about omitted cases or weak assertions, but may be equivalent or irrelevant; they do not automatically require another test. [Introduction](https://mutants.rs/), [interpreting results](https://mutants.rs/using-results.html)

The transferable principles are contract ownership, independent expectations, controlled context and appropriate integration fidelity. I would **not** adopt service-specific test proportions, mock architecture or single-threaded-test classifications as simulation rules. Google explicitly distinguishes test scope from execution resources. [Chapters 11](https://abseil.io/resources/swe-book/html/ch11.html) and [14](https://abseil.io/resources/swe-book/html/ch14.html)

## Draft lens

```markdown
# Lens: testing

Subject: the evidence tests provide about the game's behavior and composition.
The game's damage model is game state only; crew are HP pools at stations.

Principle: AGENTS.md, "A test is written under scrutiny". A test names the
invariant a plausible mutant would break and who relies on it. Its expectations
and harness must distinguish that defect from a permitted change elsewhere.

## Finding classes

- **Coupled assertion**: a test pins another module's taste, fixture, timing or
  representation beyond the invariant it names. A permitted change there makes
  this test fail without breaking its consumer's contract.
  Fix: assert the owned relationship or outcome. Where authored parameters are
  relevant, read them from their owner and derive the expectation independently;
  otherwise supply explicit local fixture data through the real interface.
  Do not obtain the expected answer by calling the implementation under test.

- **Substituted composition**: a test claims production registration, ordering,
  gating or integration but recreates that mechanism in its harness. Removing
  the production guarantee leaves the test's replacement intact, or the harness
  omits a required reader and cannot observe its failure.
  Fix: mount the production plugin or installation seam owning the guarantee.
  Control outside inputs and add probes around it. Exercise the schedules,
  transitions and event routing on which the claim depends.

## Not a finding

- A minimal App or direct system invocation proving a deliberately narrow claim.
- Exact values independently justified by the tested law or declared contract.
- Explicit fixture data asserting a class invariant.
- A tripwire naming the dependency contract or workaround it guards.
- An intentionally different composition probing an upstream defect.
- Similar tests with distinct failure conditions.

Asset-specific pass conditions belong to base-truths. Repeated setup obscuring
the varying input belongs to brevity's test ceremony. Redundancy follows
code-quality-standard A5; dead alternatives belong to code-branching.

## Fix

Name a plausible production defect this test must detect and a permitted change
it must tolerate. Repair the assertion or harness accordingly. Delete a test
only after showing that its evidence is already supplied elsewhere.
```

## Measurements and samples

**MEASURED, current filesystem:** a textual census found **302 Rust files**, **261,240 physical lines**, **2,134 `#[test]` attributes across 237 files**, and **124,106 test-only lines**. The census masked comments/literals, matched `cfg(test)` items and recursively followed their external modules, including **55 entirely test-only files**. Test lines include fixture code, comments and blanks within those regions; preceding module documentation is excluded.

**DERIVED:** `124,106 ÷ 261,240 × 100 = 47.51%`. This is source allocation, not executed coverage. The quality standard’s older MEASURED count is historical.

### Coupled assertion: MEASURED ten concrete sites

The input data owns the relevant taste values at [drive.ron:3](/Users/Yan/github/vikng-dev/overmatch/assets/input/drive.ron:3). Values below are MEASURED source literals, not recommended settings.

| Site | Coupling beyond the named invariant | Fix |
|---|---|---|
| [killcam/tests.rs:776](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/game/killcam/tests.rs:776) | Device ownership requires live input; asserts full axes after a frame. | Assert live/nonneutral intent, then neutralization and restored ownership. |
| [command.rs:1065](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1065) | Tap persistence/sign test copies notch values. | Derive stepped expectations from authored step, with saturation considered. |
| [command.rs:1092](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1092) | Hold/release test pins intermediate and restarted notch magnitudes. | Assert derived steps, full hold and neutral release. |
| [command.rs:1155](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1155) | Reversal test pins pre-hold notch. | Read current intent; independently check neutral reversal behavior. |
| [command.rs:1168](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1168) | Subframe tap test copies step and uses a fixed repetition count. | Derive step and saturation-driving repetitions. |
| [command.rs:1414](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1414) | Tick neutralization requires preserving the gathered command; pins its magnitude. | Capture gathered intent and compare later ticks with that input. |
| [drive_hud.rs:506](/Users/Yan/github/vikng-dev/overmatch/src/drive_hud.rs:506) | HUD position test copies keyboard notch values. | Compare with actual lever state; retain independent mapping/bound checks. |
| [death_flow_input.rs:416](/Users/Yan/github/vikng-dev/overmatch/src/net/death_flow_input.rs:416) | New-life transport test copies first-notch magnitude. | Compare filed/restored/received input with the gathered value. |
| [death_flow_input.rs:471](/Users/Yan/github/vikng-dev/overmatch/src/net/death_flow_input.rs:471) | Per-tick transport test copies negative notch magnitude. | Preserve the sampled frame command across transport assertions. |
| [track/drive.rs:130](/Users/Yan/github/vikng-dev/overmatch/src/track/drive.rs:130) | Slew/no-overshoot test pins completion to a taste-dependent tick count. | Derive travel bounds from rate and fixture timestep. |

Labelling shipped taste values `Fixture` does not transfer their ownership. Also, reading parameters is insufficient if setup still assumes a particular step permits several unsaturated taps.

### Bevy mechanics and composition evidence

Verified against **Bevy v0.19.0**; cited line spans locate the relevant source:

| Mechanism | Verified pattern and implication |
|---|---|
| Minimal system app | Bevy’s own [tests/how_to_test_systems.rs, lines 45–180](https://github.com/bevyengine/bevy/blob/v0.19.0/tests/how_to_test_systems.rs#L45) supplies resources, mounts selected systems, updates and inspects results. Valid narrow scope. |
| Production-plugin integration | [tests/how_to_test_apps.rs, lines 1–144](https://github.com/bevyengine/bevy/blob/v0.19.0/tests/how_to_test_apps.rs#L1) mounts `game_plugin` in its tests, with a minimal engine shell, fake window and injected input. This avoids copying game registration. |
| `run_system_once` | [system/system.rs, lines 250–400](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_ecs/src/system/system.rs#L250) recreates the system each call; `Local` and change-detection history do not persist. It cannot prove persistent scheduling behavior. |
| `App::update` | [bevy_app/app.rs, lines 100–180](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_app/src/app.rs#L100) runs app schedules. Use it when startup, transitions or system interaction matter. |
| Time | [bevy_time/lib.rs, lines 95–130 and 240–300](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_time/src/lib.rs#L95) provides manual duration/instant and `FixedTimesteps`; own tests control time. Account for initialization before interpreting frame counts. |
| Input | The [app test](https://github.com/bevyengine/bevy/blob/v0.19.0/tests/how_to_test_apps.rs#L50) injects `ButtonInput` and clears transient state. Actual [keyboard processing, lines 150–210](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_input/src/keyboard.rs#L150) clears transients while consuming messages. Choose the input edge the claim covers. |
| Observers/states | Exercise [observer registration and triggers](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_ecs/src/observer/mod.rs#L1), including deferred effects; mount [StatesPlugin and transition machinery](https://github.com/bevyengine/bevy/blob/v0.19.0/crates/bevy_state/src/app.rs#L330) for transition claims. |

**MEASURED:** **178 test `App` constructor sites**, rather than runtime instances: **185 total minus seven non-test sites**. `SubApp` is excluded. Constructor harness strategies:

| Mounting strategy | MEASURED sites |
|---|---:|
| Bare app with direct resources/systems/observers/installers | 75 |
| Selected bespoke plugins | 45 |
| `MinimalPlugins` plus selected plugins | 48 |
| Delegated minimal physics floor | 2 |
| Shared GPU-less default plugins | 7 |
| Customized default plugin group | 1 |

Callers can extend these floors. Therefore constructor counts do not count distinct final plugin compositions.

### Substituted composition: MEASURED ten inspected sites

These are **samples and controls**, not ten alleged defects. Static evidence establishes the overlay defect; the reported UI trigger is now repaired.

| Site | What its harness proves |
|---|---|
| [overlay.rs:746](/Users/Yan/github/vikng-dev/overmatch/src/overlay.rs:746) | **Finding:** recreates the ordering chain at line 750 that production installs at line 330. Removing production ordering leaves the replacement intact. |
| [net/ui_ownership.rs:193](/Users/Yan/github/vikng-dev/overmatch/src/net/ui_ownership.rs:193) | Manually selected UI composition; now includes command input. Valid ownership audit, insufficient evidence of complete client wiring. |
| [client/hosted_e2e.rs:83](/Users/Yan/github/vikng-dev/overmatch/src/net/client/hosted_e2e.rs:83) | Selected network/sim/UI composition; duplicated installation policy merits maintenance scrutiny, not an automatic finding. |
| [pen_cam/game.rs:620](/Users/Yan/github/vikng-dev/overmatch/src/damage_cam/pen_cam/game.rs:620) | Selected plugins and production view wiring; ignored system errors require checking that claimed readers actually execute. |
| [command.rs:1392](/Users/Yan/github/vikng-dev/overmatch/src/command.rs:1392) | **Control:** minimal shell mounts production command plugins for the tick interaction. |
| [net/physics.rs:52](/Users/Yan/github/vikng-dev/overmatch/src/net/physics.rs:52) | **Control:** inspects ordering from production physics installers. |
| [net/test_harness.rs:42](/Users/Yan/github/vikng-dev/overmatch/src/net/test_harness.rs:42) | **Control:** explicit minimal asset/state floor; callers choose default or production network physics. |
| [headless_test.rs:167](/Users/Yan/github/vikng-dev/overmatch/src/headless_test.rs:167) | **Control:** GPU-less shell; boot helper mounts `SimPlugin` and spawn plugin with declared physics differences. |
| [camera.rs:634](/Users/Yan/github/vikng-dev/overmatch/src/camera.rs:634) | **Control:** mounts production pivot-tracking installer for a scoped schedule handoff. |
| [avian_body_query_gap.rs:42](/Users/Yan/github/vikng-dev/overmatch/src/tests/avian_body_query_gap.rs:42) | **Control:** deliberately altered upstream composition testing a documented workaround. |

I recommend these two classes only. This investigation did not establish enough redundant or context-sensitive defects to justify additional classes.

## Recommended AGENTS.md wording

Replace the existing test sentence with:

> **A test is written under scrutiny: it names the invariant a plausible mutant would break and who relies on it.** Assert that contract, without pinning another module’s taste values or incidental representation beyond what the contract requires. Relevant authored parameters come from their owner; expectations follow independently from the invariant, never by calling the implementation for its answer. A test claiming composed behavior mounts the production plugin or installation seam that owns it, including the scheduling, gating and event routing the claim depends on. A minimal app is appropriate for a narrower system invariant. Control time and outside inputs, and make fixture assumptions explicit. Keep distinct evidence; test count is not a metric. Mutation testing remains on demand, never a gate. See `.agents/lenses/testing.md`.

Add `testing` to the lens index. Leave base-truths’ asset-specific gate, brevity’s test ceremony and the existing tripwire protections with their current owners.