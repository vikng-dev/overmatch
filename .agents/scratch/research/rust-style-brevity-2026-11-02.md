- **Adopt a brevity principle and lens:** fewer independently maintained statements of meaning, with line count as evidence.
- Google’s [Rust guide](https://google.github.io/styleguide/rustguide.html) is public; its initial export is **MEASURED from commit metadata: 2026-10-21**. Adoption remains codebase-specific.
- Much of the useful advice already exists in Overmatch’s doctrine or code-quality brief. The main addition is consistent review coverage.
- **MEASURED:** `src/` contains **260,631 Rust lines**; approximately **198,963 code, 48,544 comment, 13,124 blank**.
- **DERIVED:** the ten sample sites below could save **69–75 lines**, including helper overhead.
- **DERIVED planning range:** roughly **600–2,800 lines** across the Rust tree; **low confidence**, not a target.
- Preserve invariants, rationale, rejected alternatives, measured bounds, diagnostic outcomes and protective types. Read-only research completed; no edits, builds, tests or git changes.

## Sources and rules

Subject throughout: Overmatch’s code, including the **game’s damage model; game state only**.

Google’s canonical URL is **https://google.github.io/styleguide/rustguide.html**. The [initial export commit](https://github.com/google/styleguide/commit/3cf4f72013826804b7442896ce84d3777cf89ae0) added `rustguide.md`; its committer timestamp is **MEASURED metadata: 2026-10-21 16:41:53 UTC**. The live document is published guidance, with adoption and strictness determined by individual codebases. It prioritizes readability and maintainability, without prescribing a minimum line count. [Google guide](https://google.github.io/styleguide/rustguide.html)

The Rust [Style Guide](https://doc.rust-lang.org/style-guide/) defines rustfmt’s default style. Its substantive expression advice is separated below from formatting. The [API Guidelines](https://rust-lang.github.io/api-guidelines/) chiefly concern library interfaces; applying every recommendation to this unpublished application would add machinery.

Local references: [AGENTS.md](/Users/Yan/github/vikng-dev/overmatch/AGENTS.md), [lens README](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/README.md), [error-handling](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/error-handling.md), [code-branching](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/code-branching.md), [base-truths](/Users/Yan/github/vikng-dev/overmatch/.agents/lenses/base-truths.md), and the additional [code-quality brief](/Users/Yan/github/vikng-dev/overmatch/.agents/docs/code-quality-standard.md), abbreviated **Brief**.

| Rule | Source | Covered / new / conflict | Our rule or proposed class |
|---|---|---|---|
| Application simplicity; traits only when needed; derive equivalent implementations | [Google](https://google.github.io/styleguide/rustguide.html) | Covered / new lens | Brief §A6; **B2 shallow abstraction**, **B4 standard expression** |
| Prefer borrowing; flexible references; caller controls copying | [API flexibility](https://rust-lang.github.io/api-guidelines/flexibility.html) | Covered / new lens | Brief §A2/A4; **B3 needless intermediate**. Preserve scratch buffers and justified ownership. |
| Expose useful intermediate results to avoid repeated computation | [API flexibility](https://rust-lang.github.io/api-guidelines/flexibility.html#c-intermediate) | New lens | **B1 duplicated knowledge/work**; expose only results an actual caller needs. |
| Prefer expression-valued `if`/`match` over deferred assignment | [Rust style advice](https://doc.rust-lang.org/style-guide/advice.html) | New lens | **B3/B4** |
| Meaningful conversion, getter and iterator names; conventional casing; avoid misspelled keywords | [API naming](https://rust-lang.github.io/api-guidelines/naming.html), [Rust style advice](https://doc.rust-lang.org/style-guide/advice.html) | New base convention | Name meaning and ownership clearly; follow established Rust conventions when touching interfaces. |
| Standard conversion/collection traits instead of bespoke equivalents | [API interoperability](https://rust-lang.github.io/api-guidelines/interoperability.html) | Covered / new lens | Brief §A1; **B4**. Implement `From`/`TryFrom`, rather than direct `Into`/`TryInto`. |
| Newtypes distinguish semantics; enums express choices; flags represent sets | [API type safety](https://rust-lang.github.io/api-guidelines/type-safety.html) | Covered | AGENTS “unrepresentable”, “One number, one job”; Brief §A2b. Caller count does not invalidate these types. |
| Builders for complicated construction | [API type safety](https://rust-lang.github.io/api-guidelines/type-safety.html#c-builder) | New lens | **B5 test ceremony**, only when net setup and reader effort shrink. |
| Pattern matching instead of check-then-unwrap; contextual `expect` and assertions | [Google](https://google.github.io/styleguide/rustguide.html) | Covered | Parse once at the edge; error-handling’s bug outcome; tests name their invariant. |
| Typed meaningful errors, documented failure and safety contracts | [API interoperability](https://rust-lang.github.io/api-guidelines/interoperability.html#c-good-err), [documentation](https://rust-lang.github.io/api-guidelines/documentation.html#c-failure) | Covered | Earliest deliberate outcome; visible outside-input handling; current invariant comments. |
| Static validation first | [API dependability](https://rust-lang.github.io/api-guidelines/dependability.html#c-validate) | Covered | Error-handling: unrepresentable before runtime checking. |
| Debug-only validation or unchecked sibling functions | [API dependability](https://rust-lang.github.io/api-guidelines/dependability.html#c-validate) | **Conflict** for required invariants | Every-build outcomes and ADR-0011 win. No release-only disappearance of a required check. |
| Prefer short clear combinators; retain clearer loops | [Google](https://google.github.io/styleguide/rustguide.html) | Covered / qualified | Brief §A6; **B4** requires preserved outcomes. Google’s dropped-error examples are unsuitable here. |
| Eager common traits, serialization, broad generic interfaces | [API interoperability](https://rust-lang.github.io/api-guidelines/interoperability.html), [flexibility](https://rust-lang.github.io/api-guidelines/flexibility.html) | **Conflict when blanket** | Brief §D6 and present users win; no speculative application interfaces or features. |
| Clear receivers become methods; unsurprising operators and `Deref`; return values over output parameters | [API predictability](https://rust-lang.github.io/api-guidelines/predictability.html) | New base convention | Apply where ambiguity exists. Intentional mutation, reusable buffers and ECS system parameters remain valid. |
| Remove redundant generic struct bounds; keep representation private where it protects invariants | [API future-proofing](https://rust-lang.github.io/api-guidelines/future-proofing.html) | New lens / covered | **B3**; unrepresentable states. Passive data structs need no accessor scaffolding. |
| Useful documentation and examples; familiar, composable macro syntax | [API documentation](https://rust-lang.github.io/api-guidelines/documentation.html), [macros](https://rust-lang.github.io/api-guidelines/macros.html) | Covered / qualified | One narrative owner; **B6 prose duplication**. No trivial examples or macro language merely to shrink source. |
| Explicit fallible close; nonfailing destruction | [API dependability](https://rust-lang.github.io/api-guidelines/dependability.html#c-dtor-fail) | New clarification when relevant | Handle close errors visibly; the source’s permission to ignore teardown errors loses to local handling doctrine. |
| Small crates, flat layout, leaf-only implementations | [Google](https://google.github.io/styleguide/rustguide.html) | Scoped / conflict if generalized | Chromium build choices do not override deep cohesive modules; Brief §B1 wins. |
| Import scoping and qualification | [Google](https://google.github.io/styleguide/rustguide.html) | New convention, low priority | No import-only sweep. The guide permits dependency preludes, accommodating Bevy. |
| Responsible unsafe/`const`, no hidden foreign items or pre-main execution; selective `must_use` | [Google](https://google.github.io/styleguide/rustguide.html) | New base candidates | Adopt when a concrete contract benefits; no blanket expansion. Platform-specific concurrency prescriptions do not transfer. |
| `correctness`, `suspicious`, `style`, `complexity`, `perf`; `all` aggregates these | [Clippy catalogue](https://doc.rust-lang.org/clippy/) | Already gated | Manifest/CI policy, verified from source; no fresh claim that the working tree passes. |
| `pedantic`, `restriction`, `nursery`; also `cargo` and `deprecated` | [Clippy groups](https://doc.rust-lang.org/clippy/lints.html) | Selective instruments | Evaluate individual lints. Reject panic-family restrictions; preserve explained Bevy `type_complexity` allowance. |

Publication metadata, licensing and release packaging advice are outside expression efficiency. Google’s codebase-specific test frameworks likewise offer no replacement for Overmatch’s invariant-based tests.

## Draft lens: brevity

**Principle:** `AGENTS.md`, “Express each fact economically.” Reduce independently maintained knowledge and unnecessary ceremony while preserving the information needed to understand and safely modify the code.

### Finding classes and fixes

| Class | Finding | Fix |
|---|---|---|
| **B1 — Knowledge stated twice** | Sites encode one fact and must change together; redundant state or repeated computation lacks justification. | Name the shared fact and both sites. Give it one owner; return already-computed information where useful. Similar shape alone is insufficient. |
| **B2 — Shallow abstraction** | A wrapper/type/helper merely forwards, hides nothing and forces another lookup. One caller is a search clue. | Apply the deletion test. Inline/remove it only if complexity disappears and no invariant, ownership distinction or real seam disappears. |
| **B3 — Needless intermediate** | A binding, conversion, allocation or bound adds no meaning, reuse or required lifetime behavior. | Use the value directly or retain its existing representation. Preserve evaluation order, borrow/drop timing and useful names for units or derivations. |
| **B4 — Handwritten standard expression** | A match/loop manually implements a readily understood standard operation. | Use a verified equivalent operation or expression-valued branch. Keep a loop when the chain requires more explanation. Count helper/import overhead. |
| **B5 — Test ceremony** | Repeated setup obscures the varying input and asserted invariant. | Use a local fixture constructor, builder or named case table. Keep every distinct assertion, case diagnostic and mutant-catching invariant. |
| **B6 — Prose duplication** | A comment restates syntax, narrates edit history, or repeats a design narrative already owned elsewhere. | Remove the redundant statement or replace the narrative with a link. Preserve current contracts locally; deliberately relocate unique material before removing it. |
| **B7 — Dead implementation** | No live caller, registration, reflection, script, feature or platform contract uses the item. | Delete it and its exclusively dependent scaffolding. Check all relevant configurations; hand kept alternatives to code-branching. |
| **B8 — Fragmented concept** | Understanding one change requires traversing forwarding modules with no useful independent interface. | Consolidate the concept and reduce exposed surface. File count and file length alone establish nothing. |

### Not a finding

- Stated invariants, units, ordering constraints, rationale, rejected alternatives, counterexamples, measured bounds and provenance labels.
- A protective newtype, meaningful intermediate name, cohesive large file, reusable scratch buffer or justified cache.
- Similar code expressing different knowledge across sim/view or authority/presentation.
- Explicit failure handling. Never shorten through unseen `.ok()`, fallback `unwrap_or`, ignored arms or dropping `let … else`.
- A tripwire whose apparent tautology deliberately detects dependency changes.
- A line-count reduction produced by denser formatting or hidden macro expansion.

### Search instruments and reporting

Use `rg` for repeated setup/calls, forwarding bodies, `clone`, `collect`, conversions, dead-code allowances and historical comment markers. Search callers, scheduling/observer registrations, scripts, platform arms and feature users before classifying a site.

Clippy names such as `derivable_impls`, `useless_conversion`, `manual_map`, `redundant_closure` and `needless_collect` help organize searches; catalogue hits are not findings. Preserve temporary lifetimes when removing bindings. [Clippy lint index](https://rust-lang.github.io/rust-clippy/master/index.html)

Follow the existing lens report format. A brevity finding normally has **P3** severity and includes the class, affected reader, precise replacement, protected information and **DERIVED net lines saved**. Report semantic risks under the relevant existing lens. Keep `src/tests/`, `src/net/tests/`, vendor patches and wire-surface contracts under the Brief’s protections.

## Measurements and samples

**MEASURED inventory:** `rg --files src` found **306 files**, including **302 Rust files**. Text files total **262,656 lines**, excluding the binary KTX2 fixture. Rust totals **260,631**.

Classification used read-only `awk` over `rg --files`: blank lines separately; comment-only `//` and block-comment lines; remaining lines as code. This is approximate: inline comments remain code, and comment-shaped text inside string literals can be misclassified.

| Rust category | MEASURED classified lines | DERIVED share |
|---|---:|---:|
| Code | 198,963 | 76.34% |
| Comment | 48,544 | 18.63% |
| Blank | 13,124 | 5.04% |

**DERIVED comment/code ratio:** `48,544 ÷ 198,963 = 0.244`. Comment volume establishes no deletion opportunity.

The largest source modules, treating each `.rs` file—including test modules—as the measurement unit:

| Path under `src/` | MEASURED total | Approx. code | Approx. comment | DERIVED comment/code |
|---|---:|---:|---:|---:|
| `buildings/fracture.rs` | 8,334 | 6,564 | 1,483 | 22.6% |
| `ballistics.rs` | 7,950 | 6,069 | 1,500 | 24.7% |
| `damage_cam/pen_cam/stage.rs` | 4,845 | 4,067 | 623 | 15.3% |
| `ballistics/walk/tests.rs` | 4,466 | 3,527 | 713 | 20.2% |
| `settings.rs` | 4,376 | 2,473 | 1,675 | 67.7% |
| `net/client.rs` | 4,036 | 3,105 | 702 | 22.6% |
| `track/transmission/tests.rs` | 3,929 | 3,107 | 654 | 21.0% |
| `ballistics/walk.rs` | 3,702 | 2,484 | 1,042 | 41.9% |
| `buildings/breakage/tests.rs` | 3,620 | 3,215 | 312 | 9.7% |
| `headless_test.rs` | 3,506 | 2,932 | 429 | 14.6% |

These are proposed, unimplemented changes. Savings include added helper code and retained explanations.

| Site | Class and concrete opportunity | DERIVED lines saved |
|---|---|---:|
| [settings.rs:2754](/Users/Yan/github/vikng-dev/overmatch/src/settings.rs:2754) | **B5:** share reconciler setup repeated at the other three sites. Existing setup **38** lines → documented helper **15** + calls **4**. Preserve each caller’s update/spawn sequence. | 19 |
| [cost.rs:103](/Users/Yan/github/vikng-dev/overmatch/src/cost.rs:103) | **B1:** insert `CostRole(role)` inside successful installation; remove duplicated caller guards/role literals. Update `shot_trace`’s contract reference. | 6 |
| [settings/probe.rs:201](/Users/Yan/github/vikng-dev/overmatch/src/settings/probe.rs:201) | **B5:** table backend capability cases; retain backend explanations and named failure diagnostics. Existing **36** lines → projected **18–24**. | 12–18 |
| [tank_surface.rs:1139](/Users/Yan/github/vikng-dev/overmatch/src/tank_surface.rs:1139) | **B2:** delete `shipped()`’s pure forwarding alias; its three callers use `SurfaceData::shipped()` directly. | 4 |
| [flow.rs:181](/Users/Yan/github/vikng-dev/overmatch/src/flow.rs:181) | **B5:** within each of the two flow tests, table the changing boolean and expected state. Keep both tests and invariant docs. | 10 |
| [settings/store.rs:740](/Users/Yan/github/vikng-dev/overmatch/src/settings/store.rs:740) | **B5:** reuse the identical parsed snapshot already constructed above; preserve every missing-field assertion. | 3 |
| [settings/store.rs:451](/Users/Yan/github/vikng-dev/overmatch/src/settings/store.rs:451) | **B5:** table Linux XDG inputs and expected paths; retain precedence and empty-value explanations. | ~7 |
| [bake.rs:701](/Users/Yan/github/vikng-dev/overmatch/src/bake.rs:701) | **B4:** replace mutable key plus zip loop with array `map`; retain exact negative-zero normalization. [`array::map`](https://doc.rust-lang.org/std/primitive.array.html#method.map) supplies the operation. | 3 |
| [bitprobe.rs:688](/Users/Yan/github/vikng-dev/overmatch/src/bitprobe.rs:688) | **B1:** fold words through existing `fingerprint::fnv1a_64`, preserving little-endian bytes and seed; remove the repeated hash arithmetic. | 3 |
| [track/view.rs:49](/Users/Yan/github/vikng-dev/overmatch/src/track/view.rs:49) | **B6:** state template loading/hiding and shared scope directly; remove historical incident narration. | 2 |

**DERIVED extrapolation:** these sites total **69–75 lines** in host files totaling **MEASURED 14,250 lines**. Applying that density unchanged gives:

`(69–75) ÷ 14,250 × 260,631 = approximately 1,262–1,372 lines`.

A deliberately broad **DERIVED sensitivity range**, applying **0.5–2 times** that density, is approximately **600–2,800 lines**. These multipliers are assumptions. Selection was purposive, host files were not exhaustively reviewed, and formatting was not run: **low confidence**, with no statistical upper bound. No sufficiently supported dead-code or fragmented-module deletion was established.

## Recommended text to land

Add to `AGENTS.md` Working discipline:

> **Express each fact economically.** Prefer the clearest complete expression with fewer independently maintained statements. Shared knowledge has one owner; an abstraction earns its interface by hiding complexity or enforcing a contract. Use standard operations when their semantics fit. Count helpers, callers and documentation when judging a reduction. Preserve stated invariants, rationale, rejected alternatives, measured bounds and provenance. Line counts are instruments, never targets; a shorter expression that hides an outcome is a bug.

Add the drafted `brevity` lens and its README row. Add this review convention:

> Follow Rust’s established naming and conversion conventions when changing an interface. Add traits and generality for present users. Every brevity finding names the information preserved and the net reduction after replacement overhead.

Keep current lint policy. Reconcile the Brief with the new lens: retain its protections, distinguish comment content from volume, and avoid a separate duplicated brevity checklist.