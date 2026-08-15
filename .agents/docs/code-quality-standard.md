# Code quality standard — the simplifier's brief

**Audience:** an agent asked to simplify, clean up, tidy or "improve quality" in `overmatch`.
**Scope:** `src/`, `tests/`, build scripts. NOT `vendor/`.
**Measured:** 2026-07-29, branch `feat/authoritative-facts`.

This file is a *brief*, not the doctrine. The doctrine already exists (§0). This adds only the
three things the doctrine does not state: **what to hunt** (§A), **what not to touch** (§B), and
**how to work** (§C). §D records where published literature and this house disagree.

Read §B before §A. The guardrails are the expensive half: a missed hunt target costs nothing, a
violated guardrail costs a deploy cycle or a silent desync.

---

## 0. What already exists — read, do not restate

| Source | Owns |
|---|---|
| `.agents/AGENTS.md` | comment discipline; prose-is-a-claim; MEASURED/DERIVED labels; the one-place narrative rule; sim-view split; Bevy-knowledge-is-stale; UI font coverage |
| `.agents/skills/codebase-design/SKILL.md` | the vocabulary — **module, interface, implementation, depth, seam, adapter, leverage, locality**; the deletion test; "one adapter = hypothetical seam, two = real" |
| `.agents/skills/codebase-design/DEEPENING.md` | dependency categories; replace-don't-layer testing |
| `.agents/skills/improve-codebase-architecture/SKILL.md` | the *discovery* flow for architectural friction; ADR-conflict etiquette |
| `.agents/skills/writing-great-skills/SKILL.md` | information hierarchy; progressive disclosure; no-op pruning |
| `tests/fn_length.rs` | the 300-line **function** ceiling, and the reasoning for function-not-file |
| `tests/doc_citations.rs` | the doc-rot gate — comments may not cite dead paths or line numbers |
| `.agents/docs/adr/` | settled decisions. Do not re-litigate; see §C.7 |
| `.agents/GLOSSARY.md` | domain names. Use these, never invented synonyms |

Use the codebase-design vocabulary **exactly**. Do not drift into "component", "service", "API",
"boundary", "layer" — `improve-codebase-architecture` asks for the same, and it matters more here
than usual because "component" already means an ECS component in this repo.

**What this brief adds to that set.** `improve-codebase-architecture` finds *deepening*
opportunities — restructuring that ends in a design conversation with the owner. This brief covers
*local* simplification: work that ends in a small green commit with no design conversation at all.
If a candidate needs a design conversation, it is not this brief's work. Hand it to
`improve-codebase-architecture` and move on.

---

## 1. What the toolchain already enforces — do not "find" it

Establish this before hunting, or you will file findings the compiler already rejects.

`scripts/hooks/pre-push` and CI both run (VERIFIED, read from the hook):

```
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

`Cargo.toml` has **no `[lints]` table**. So clippy runs stock: categories `correctness` (deny),
`suspicious`, `style`, `complexity`, `perf` (warn) — all promoted to hard errors by `-D warnings`.
`pedantic`, `nursery`, `restriction` and `cargo` are allow-by-default and are **not** enabled.
(VERIFIED against the clippy lint index — §E.)

Two binding consequences:

1. **Everything in those five categories is already gone from `src/`.** `redundant_clone`,
   `needless_collect`, `manual_map`, `useless_vec`, `needless_range_loop`, `clone_on_copy`,
   `large_enum_variant` — the whole stock list is clean on a green tree. A "finding" naming one of
   these is either wrong, or sits somewhere a human deliberately `#[allow]`ed. **Hunt what a
   linter cannot see.** That is the entire value you add over `cargo clippy`.
2. **Do not enable `pedantic` or `nursery` as part of a simplification pass.** That is a policy
   change with a huge diff and it is not yours to make. If you think a specific lint earns its
   place, name the lint, count the sites it fires on, and stop. *(That ordering — propose, never
   enable — is house rule §C.7. The specific opinion that pedantic-wholesale is wrong for this
   repo is **my preference**, flagged as such.)*

---

## A. What to hunt, ranked by value

Ranked by (damage removed) × (confidence it is really a defect). Work down, not across.

### A1. Hand-rolled implementations of something a dependency already provides

Highest value because the replacement is **deletion**; the risk is bounded by an existing test; and
the win compounds at the next engine bump — hand-rolled code must be ported by hand, a library call
need not be.

**The confirmed instance, and it is the owner's own example.** MEASURED 2026-07-29: `src/` contains
**zero** references to `bevy_ui_widgets`, `bevy_core_widgets`, `CoreButton`/`CoreSlider`/
`CoreCheckbox`/`CoreRadio`, **and zero uses of `Interaction`** — Bevy's own UI interaction
component. The settings UI instead hand-rolls its pointer handling:

- `src/settings/ui.rs:934` and `:999` read `window.physical_cursor_position()` directly;
- `src/settings/ui.rs:970-971` and `:1531` document a hand-written slider `track_fraction`,
  including a hand-managed physical-vs-logical pixel conversion the comments call out as a trap.

That is a slider drag interaction implemented from raw cursor coordinates in a ~1 900-line file.
Note this **contradicts a widely-repeated claim that "game UI stays bevy_ui + core_widgets"** — that
was the intent; the tree does not currently reflect it. Flagged, not acted on (§C.7): whether to
adopt the widget crate is an owner decision, because it is an interaction-behaviour change, not a
simplification.

Where else to look, in order:

- **glam / `bevy_math`** — `project_onto`, `reject_from`, `slerp`, `Isometry3d`, `Dir3`,
  `EaseFunction` and the `Curve` traits, `f32::rem_euclid` for angle wrap, `f32::clamp`,
  `StableInterpolate`. Hand-rolled versions are common in physics-heavy code, and this repo is
  ~15 k lines of it under `src/track/` alone.
- **`std`** — `slice::windows` / `chunks` / `iter::zip` for manual index arithmetic;
  `min_by_key`, `partition_point`, `total_cmp`, `Option::zip`, `let ... else`.
- **Bevy ECS facilities** — a hand-maintained "did this change" flag where `Changed<T>` or
  `Ref<T>::is_changed()` exists; a hand-rolled countdown where `Timer` exists; a manual
  parent/child index where a relationship exists.

**The bar for filing one.** Name the exact replacement API **and the version you verified it in**.
"Bevy probably has something for this" is not a finding — delete it from your report.

**The counter-rule.** Some hand-rolled code exists *because* the library version was wrong here,
and this repo writes that down. `Cargo.toml` carries a multi-paragraph comment explaining that
`objc-sys` is a dependency *specifically to avoid* a hand-written `extern "C"` block whose ABI
would be silently wrong on one architecture. If a hand-rolled thing has a comment explaining why,
that comment is the answer. Believe it or measure it; do not delete it.

### A2. State that could be derived

Second because it removes a *class* of bug — two facts disagreeing — not just lines.

Signals, strongest first:

- **A field kept in sync by hand in more than one system.** The hand-sync is the tell. One writer
  is a cache; two writers is a bug waiting.
- **A struct field computable from its siblings**, with no measured perf reason to cache.
- **Parallel collections indexed in lockstep** — `Vec<A>` + `Vec<B>` where index *i* means the same
  entity in both. A `Vec<(A, B)>` or a component makes the invariant unfalsifiable.
- **`Local<T>` shadowing world state.** Distinguish carefully: a `Local<Vec<_>>` used as a
  *scratch buffer* is the correct house pattern for avoiding per-tick allocation and must not be
  "simplified" away. A `Local` holding a *copy of something the World already knows* is the defect.
- **Dirty flags** where change detection exists — but see the caveat below.

**Caveat on dirty flags (VERIFIED, Bevy 0.19).** Mutably dereferencing a `Mut`/`ResMut` flags the
value as changed **regardless of whether the value actually changed**; `set_if_neq` exists
precisely to avoid that. So a hand-rolled dirty flag may have been written by someone who hit this
exact behaviour. Before calling one redundant, check whether `set_if_neq` would serve — and check
whether the author already tried it.

**The sharp counter-rule.** In a server-replicated sim, "derived" and "stored" are not
interchangeable. A value stored in a replicated component is *overwritten by the authority's
stream*; the same value recomputed from inputs is *recomputed*. That is a different program.
ADR-0016 (`replicate-causes-derive-consequences`) settles which side things sit on. **Before
converting stored → derived anywhere under `src/net/` or `src/track/`, check whether the
component is replicated. If it is, this is a design change, not a simplification — stop and
report.**

### A3. Duplicated knowledge

Not duplicated *text* — duplicated **knowledge**: two places that must change together, with
nothing enforcing it.

The suspicion map for this repo, by size:

- **client/server parallel rules** — `src/net/client.rs` (2 756 lines) and `src/net/server.rs`
  (1 243). The house pattern is that a shared rule lives in a shared module and both sides call it.
  A rule literally spelled twice is the defect.
- **sim/view forks** — `src/track/{sim,view,wrap,link_view}.rs`. A view de-fork already happened
  here, so residue is plausible but novelty is not. Note the sim/view split is **load-bearing**
  (ADR-0014): apparent duplication across that seam is often two genuinely different computations
  wearing similar names.
- **dev sandbox vs game path** — `src/sandbox.rs` (1 212) and `src/track_sandbox/` (2 121 in
  `mod.rs` alone). A sandbox that re-implements game logic instead of calling it is a real defect,
  because it can drift and then **lie to you in a playtest** — which is the whole point of the
  sandbox.
- **VFX scaffolding** — `src/vfx/{impact,trail,muzzle}.rs` (1 334 / 1 145 / 985). Spawn, lifetime
  and despawn plumbing written three ways is the classic shape.
- **RON/spec parsing** — `src/spec.rs`, `src/track/rig_geom.rs`, `src/track/marker_model.rs`.

**The bar.** Show both sites and state *the single fact* they both encode, in one sentence. If you
cannot write that sentence, they are **not** duplicates — they are two computations that resemble
each other, and merging them creates a false abstraction, which is worse than the duplication.
Prefer two clear copies over one parameterised version with a `mode: bool` argument (Rust API
Guidelines **C-CUSTOM-TYPE**: arguments convey meaning through types, not `bool`).

### A4. Needless allocation and cloning in per-tick paths

Bounded but real. **Only counts in `Update` / `FixedUpdate` / physics schedules.** Allocation in
setup, spawn, asset-load or UI-construction paths is not a finding; say so and move on.

- `format!` / `String` rebuilt every frame for text that changes rarely. The fix is a change-gated
  rebuild, not a cheaper `format!`.
- A `Vec` allocated inside a per-tick system where the `Local<Vec<_>>` scratch pattern applies.
  Find an existing correct use in the tree and match it rather than inventing a shape.
- `.clone()` of a large struct or `Vec` per tick where a borrow or `std::mem::take` works.
- `.collect()` into a temporary that is iterated exactly once.

Remember §1: stock clippy already caught the mechanical instances. What remains is the semantic
kind — a clone that is cheap per call but sits inside a **194-element track-link loop**. So: **say
which loop, and say what its N is.** A per-tick allocation with N = 1 is not worth a commit.

### A5. Redundant tests

Only *after* you can state what each test would catch that the other would not.

- Two tests whose **failure conditions** are identical — not merely whose setup looks alike.
- Tests asserting a property the type system already guarantees.
- Tests stranded by a past deepening. `DEEPENING.md` states this rule directly: old unit tests on
  the shallow modules become waste once tests exist at the deepened interface. Those are deletable.

MEASURED: `src/` carries **675** `#[test]` functions across **79** files with `#[cfg(test)]`;
`tests/` carries **36**. Redundancy would accumulate in the in-`src` suite —
`src/track/transmission/tests.rs` alone is 4 153 lines, and `src/headless_test.rs` is 4 631.

**Never touch the eleven files in `tests/`.** See §B4. Several look pointless by design.

### A6. Wrong abstraction level

Lowest rank: most subjective, most likely to become churn.

Genuine instances: a function taking six parameters that are always the same six fields of one
struct; a `pub` item with exactly one caller inside its own module (shrinking that interface is
depth, and it is the one abstraction move safe to make unilaterally); an `impl Trait` return
hiding a type the caller must then name anyway; a combinator chain
(`.and_then().map().unwrap_or_else()`) three or more links long that a `match` or `let ... else`
states plainly.

That last one is a readability trap, not a correctness one. It is **my preference** that a
three-link `Option`/`Result` chain crossing a line break is worse than the `match`. Treat it as a
suggestion, never as a mass rewrite: **at most one combinator rewrite per commit, in code you are
already touching for another reason.** A diff that only re-spells combinators is churn (§C.5).

---

## B. What NOT to touch — the guardrails

Each of these has been misapplied before. Getting one wrong does damage.

### B1. Big files are not a defect — deep modules are house doctrine

`src/track/transmission.rs` is ~3 065 lines with 33 public items and is **deliberately one file**.
`src/ballistics.rs` is 4 720. `src/net/protocol.rs` is 3 227. **None of these is a finding.**

`tests/fn_length.rs`'s module doc settles it in the repo's own words: the gate is on **function**
length (300 lines) precisely *because* a file gate would fight the deep-module doctrine, and a
2026-07-28 vendor survey found no model vendor publishes a source-file line limit — every published
number governs files that are *always in context*, which source files are not. The doc says
splitting `transmission.rs` "would scatter the drivetrain and manufacture exactly the
bouncing-between-small-modules friction `improve-codebase-architecture` says to hunt for."

**"Split this large file" is a wrong answer in this repo.** If a file feels unnavigable, the
finding is about its module doc or its internal ordering, not its size.

Corollary: a long *function* is fair game up to the gate — and the gate's `ALLOWED` list is short
by design ("meant to stay SHORT and to shrink"; "I did not want to refactor it is not a reason").
**Do not add rows to it.** If your simplification would need a new exception row, it is not one.

### B2. Comment volume is not the problem — and the numbers are counter-intuitive

MEASURED 2026-07-29 over `src/` (87 768 lines):

| kind | lines | share |
|---|---:|---:|
| `//!` module docs | 2 134 | 2.4 % |
| `///` item docs | 12 283 | 14.0 % |
| `//` narration | 5 686 | 6.5 % |
| **all comments** | **20 153** | **23.0 %** |

**Reconciling the two figures in circulation:** the "6.9 % and falling" number is the **`//`
narration row**, not the total. Both are right; they measure different things. A simplifier that
sees 23 % and reasons "a quarter of this file is comments, cut it" will be destroying the **`///`
item docs — which are the module interface**. In codebase-design terms the doc comment *is* part of
the interface: it carries the invariants, units and ordering constraints that `SKILL.md` defines as
belonging to the interface. Deleting it shrinks nothing and removes the thing that made the module
deep.

A restatement detector over 83 k lines found **3** candidates, all legitimate. The base rate of
useless comments here is approximately zero. **Do not open a commit whose purpose is removing
comments.**

The rule that *does* apply (`AGENTS.md`) is orthogonal to volume: **a comment states the current
invariant, never the edit history.** `(Codex round 5)`, `(review round, FIX 2)`,
`X was DELETED 2026-07-27` — drop the label, keep the sentence. Rationale, rejected alternatives,
counterexamples, measured bounds and named constraints all stay. If you are deleting a whole
paragraph, you are doing the wrong thing.

### B3. The wire surface is fingerprinted

`src/net/protocol.rs` holds `PROTOCOL_REV`, `WIRE_SURFACE`, a pinned
`WIRE_SURFACE_HASH` (`0xf321_3c48_61b3_bfea`), and `PROTOCOL_FINGERPRINT` derived from them.
ADR-0018 owns this.

Renaming, reordering, merging or retyping anything that rides the wire changes the handshake and
requires a deliberate re-pin plus a `PROTOCOL_REV` bump. **This is never a side effect of a
simplification.** Treat every type named in `WIRE_SURFACE`, and every field reachable from one, as
frozen for this brief's purposes. "Tidying" a replicated struct's field order is a protocol change
wearing a refactor's clothes.

### B4. `tests/` is eleven gates and tripwires — touch none of them

Several look redundant or tautological *on purpose*. Deleting one removes an alarm, not a test.

| file | what it is |
|---|---|
| `fn_length.rs` | function-length gate |
| `doc_citations.rs` | doc-rot gate |
| `ui_ascii.rs` | font-coverage gate (rendered strings only) |
| `net_boundary.rs` | nothing outside `net` may name the netcode layer |
| `gpu_layout.rs` | Rust ↔ shader struct layout contract |
| `determinism_deps.rs` | **tripwire** — fails if a dep bump splits `glam` and silently drops the `scalar-math` pin |
| `net_input_buffer_wrap.rs` | **tripwire** — pins lightyear 0.28's non-saturating `Tick` subtraction behind the connect-hang guard |
| `net_interp_delay.rs` | **tripwire** — pins the `send_interval = 0` interpolation-delay collapse |
| `bevy_shadow_view_render_layers.rs` | **tripwire** — pins the vendored `bevy_pbr` shadow-layer patch (upstream #24797, lands 0.19.1) |
| `bevy_ktx2_uastc_fallback.rs` | **tripwire** — pins the KTX2/UASTC fallback slice panic |
| `net_fire_release.rs` | mechanism proof for the MG fire-release leak |

A tripwire's job is to **fail on a dependency bump**, telling you a local workaround is now
retirable — or still needed. `determinism_deps.rs` is the clearest case: it asserts something
currently true and boring, and the day it stops being boring, the sim desyncs across platforms
**with no compile error at all**. It looks like a test of nothing. It is a test of everything.

Each tripwire also names the local workaround it guards, so deleting one silently orphans a guard
in `src/`. If a tripwire looks wrong, report it (§C.7); never edit it.

### B5. `vendor/` is off limits

Three patched crates — `bevy_reflect`, `bevy_pbr`, `bevy_light`, all 0.19.0 — wired through
`[patch.crates-io]`. Every deviation from pristine upstream is marked `// OVERMATCH PATCH:` (11
such marks in `bevy_pbr` alone) so the diff is self-describing, and each crate carries an
`OVERMATCH_PATCH.md`. **Do not format, lint, tidy, de-duplicate or comment-prune anything under
`vendor/`.** Its entire value is that it diffs cleanly against upstream; any cosmetic change
destroys exactly that.

### B6. MEASURED / DERIVED labels are load-bearing

MEASURED 2026-07-29: `src/` carries **117** `MEASURED` and **155** `DERIVED` markers. `AGENTS.md`
requires them — *"Label every number MEASURED or DERIVED"* — because a much-quoted `~125 m` figure
once turned out to be derived, never measured, and 2.5× too large.

These are not noise. Do not strip them, do not consolidate them into one note at the top of a file,
and **never move a number to a place its label does not follow.** If a simplification would
separate a constant from its provenance label, do it differently or not at all.

### B7. Determinism is NOT a refactor brake — do not over-constrain

Here because it has been misapplied in the **cautious** direction.

Client and server ship together; the handshake is version-exact, so a mismatched peer is *refused*,
not desynced. `PROTOCOL_FINGERPRINT` hashes the **wire surface**, not the sim math. Therefore:

**Math and float refactors in the sim are allowed.** Reassociating an expression, changing an
iteration order, replacing a hand-rolled dot product — none of these is forbidden by determinism.
**Do not decline a legitimate simplification citing "determinism".**

Two narrow rules survive:

1. **Cross-platform** scalar-math determinism still binds (ADR-0028) — that is what the `glam`
   `scalar-math` pin and `determinism_deps.rs` protect. Do not introduce a SIMD math path.
2. If a refactor **moves a measured result**, report the new number. Never silently re-pin a
   constant to whatever the code now produces. Silent re-pinning is the failure mode.

### B8. Settled decisions

If a simplification contradicts an ADR, it is not a simplification — it is a proposal to
revisit an ADR, which per `improve-codebase-architecture` is raised explicitly and not performed.
Check the index before touching `src/net/`, `src/track/`, `src/ballistics.rs` or the render-layer
policy; those are the most heavily adjudicated areas.

### B9. Rendered UI strings

Any string literal reaching a Bevy `Text` must stay within Barlow Condensed coverage: printable
ASCII plus `… — – ° × ± ≤`. `tests/ui_ascii.rs` enforces it per-file. Comments, logs, panics and
asserts are **unrestricted**, and the house style uses em dashes heavily there — do not "normalise"
punctuation in them. The dev sandboxes use Bevy's default font; their labels must stay ASCII.

---

## C. How to work

### C1. One concern per commit, named in the subject line

A commit that swaps a hand-rolled widget for the library one does *that*. It does not also rename a
variable it passed on the way. Reviewability is the constraint being optimised — not commit count.

### C2. Never mix a behaviour change into a simplification

The most important rule in §C. If the diff changes what the program does, it is not a
simplification and must not be described as one. A simplification's diff should be justifiable as
"same behaviour, less to know", and a reviewer should be able to check that claim **without running
anything**.

If you find a bug while simplifying: **stop, report it, do not fix it in the same commit.** Fixing
it silently inside a cleanup is how behaviour changes ship unreviewed.

Adopting a library to replace hand-rolled code is the common case of this rule, and it is worth
naming explicitly. Swapping in an upstream widget, scheduler, or state machine changes interaction
semantics, focus, timing or feel even when the types line up. **That is a dedicated task with an
owner decision, not a simplification.** Surface it as a scoped proposal — what exists hand-rolled,
what upstream replaces it, what measurably changes for the player, migration order, risk — and let
the owner schedule it. Do not perform it inside this stream. One such proposal is open and
unscheduled: `design/bevy-ui-widgets-migration-proposal.md` (the settings page's hand-rolled
interaction semantics vs Bevy's headless widgets). Do not apply it, and do not re-file it.

### C2b. New patterns are welcome — silent ones are not

Introducing a new pattern is a legitimate and often valuable outcome. It is **not** forbidden.

What makes it different from a local simplification is blast radius: a pattern is copied. Humans and
agents both reproduce whatever they find, so a pattern introduced in one module becomes a codebase
convention by imitation, without anyone deciding it should be. That is the cost to weigh — not the
pattern itself.

So: **consider patterns freely, apply them visibly.** A pattern-level change gets its own commit,
its own justification, and an explicit note in the report saying it is a pattern and naming where
else it would apply if adopted. If it is genuinely better, say so and argue for it. What is
prohibited is a pattern arriving as a side effect of a cleanup commit, where nobody chose it.

### C3. Leave the suite green at every commit

`cargo test --locked` and `cargo clippy --locked --all-targets -- -D warnings` pass at *each*
commit, not merely at the end of the branch. If a commit needs a follow-up to be green, it is one
commit, not two.

### C4. Commit to the working branch; never push

Small commits on the branch you were given. No pushes, no tags, no PRs, no `git checkout`,
`restore`, `stash` or `clean` — other agents may be working in the same tree.

### C5. Distinguishing simplification from churn

A change is a **simplification** if you can name the thing a future reader **no longer has to
know**. That is the codebase-design test — depth is leverage at the interface, measured in what the
caller must learn — and it is the only test that reliably separates the two.

It is **churn** if the honest justification is "this is how I would have written it":

- Re-spelling working code in a preferred idiom, no interface change → churn.
- Extracting a helper called once, used nowhere else → churn. The deletion test says so: delete the
  helper and nothing reappears anywhere.
- Splitting a function that is under the gate and has no independent reader → churn.
- Renaming to a synonym → churn; worse than churn if the old name is in `GLOSSARY.md`.
- Adding an abstraction for one caller → churn. *One adapter means a hypothetical seam.*

Apply the **deletion test** before filing anything: imagine the code you are about to add is
deleted. If complexity reappears across N call sites, it earns its keep. If it merely moves, it
does not.

### C6. Verify every engine API against the pinned version before writing it

`AGENTS.md`: treat Bevy/Avian API knowledge as deprecated. Pins are **Bevy 0.19**, **avian3d 0.7**,
**lightyear 0.28**, **glam 0.32**, **wgpu 29**, **bevy_egui 0.41**. Check `docs.rs/bevy/0.19.0/…`,
`docs.rs/avian3d/0.7.0/…`, or the `v0.19.0` / `v0.7.0` git tags. This has repeatedly caught real
renames (`Trigger`→`On`, buffered events→observers, `Camera` moving to `bevy::camera`,
`SceneRoot`→`WorldAssetRoot`). A simplification proposed from memory of a different Bevy version
either does not compile — the good case — or compiles and means something else.

### C7. When it is a judgement call, STOP and report

Do not guess. Report it and move to the next item. Automatically a judgement call, never a
simplification:

- anything touching a type in `WIRE_SURFACE`, or a replicated component
- anything that would add a row to `fn_length.rs`'s `ALLOWED`
- anything contradicting an ADR
- converting stored state to derived inside `src/net/` or `src/track/`
- any change to a file in `tests/`, including "fixing" a tripwire that looks tautological
- re-pinning a MEASURED constant
- deleting a comment longer than a couple of lines
- adopting a new dependency, or enabling a lint category

"I found six things, did four, and here are the two I did not touch and why" is a better outcome
than six commits, two of which need reverting.

### C8. Treat the repo's own prose as a claim

`AGENTS.md` is explicit, and it applies to *you* reading a comment that says "this is fine": the
docs here are unusually detailed, which makes them unusually persuasive when stale. Where comment
and code disagree, **the code wins and the comment is the bug** — and the fix is a one-line comment
correction, reported as such, not a code change to match the comment.

### C9. Report negative results

If a hunt category turns up nothing, say so. "Checked `src/vfx/` for duplicated spawn scaffolding;
the three files already share `…` and the remainder is genuinely per-effect" is a useful sentence —
it stops the next pass re-walking the same ground. Padding a report with speculative findings to
look thorough is the failure mode this replaces.

---

## D. Where the literature and this house disagree

Stated plainly, with which wins here.

**1. Ousterhout on comments — agrees, and is the house's source.** *A Philosophy of Software
Design* argues comments capture information the designer had that is not in the code, and names
"comments are a code smell" as a red herring. That is §B2 exactly. No conflict; the house took the
position early.

**2. Ousterhout on module size — the house extends him.** He argues for deep classes and against
classitis but is not specific about line counts. This repo goes further: a gate on **function**
length and an explicit refusal to gate **file** length. **House wins**, and `tests/fn_length.rs`
gives the reasoning — the actual defects here were function-scale (1 122 and 1 080 lines at nesting
depth 6 and 8), invisible to any file rule. Where a general principle and a local measurement
disagree in this repo, the local measurement wins.

**3. Ousterhout's "depth = implementation lines ÷ interface lines" — explicitly rejected.**
`codebase-design/SKILL.md` names this in its *Rejected framings*: the ratio rewards padding the
implementation. The house uses **depth-as-leverage**. **House wins.** Do not use the ratio, even
informally, to argue a module is deep.

**4. "Prefer many small functions" (Clean Code; most linter defaults) — rejected here.** The house
position is that bouncing between small modules to understand one concept is itself the friction to
hunt — `improve-codebase-architecture` says so in as many words. **House wins**, with the 300-line
function gate as the negotiated ceiling, *not* an aspiration to beat.

**5. DRY as usually taught — heavily qualified.** The sim/view split (ADR-0014) and the
client/server split deliberately keep code that *looks* alike but encodes different knowledge.
Merging on textual similarity is a live risk here. **House wins:** duplication of *knowledge* is
the defect; duplication of *shape* frequently is not.

**6. Rust API Guidelines — adopted only where they bear on judging an abstraction.** C-NEWTYPE
(newtypes give static distinctions) and C-CUSTOM-TYPE (meaning through types, not `bool`) are the
two that matter, for §A3 and §A6. But the guidelines are written for **published crates**: C-SERDE,
C-COMMON-TRAITS and the "examples on every item" documentation bar target public APIs and are
**not** obligations for this binary. Do not file "missing `Display` impl" as a finding.

**7. Clippy `pedantic` as a quality bar — rejected as a bulk action.** See §1. *(My preference,
flagged.)*

---

## E. Sources

**VERIFIED** = fetched/read during this pass. **INFERRED** = established knowledge or repo
inference, not re-fetched.

**Repo evidence — VERIFIED**, all read directly: `.agents/AGENTS.md`;
`.agents/skills/codebase-design/{SKILL,DEEPENING}.md`;
`.agents/skills/improve-codebase-architecture/SKILL.md`;
`.agents/skills/writing-great-skills/SKILL.md`; `.agents/skills/ask-matt/SKILL.md`;
`tests/fn_length.rs`; `tests/doc_citations.rs`; the header of every other file in `tests/`;
`Cargo.toml`; `.cargo/config.toml`; `scripts/hooks/pre-push`; `src/net/protocol.rs` (fingerprint
constants); `src/overlay.rs` and `src/settings/ui.rs` (cursor handling); `vendor/*/OVERMATCH_PATCH.md`.
All line counts, comment counts, `#[test]` counts, `MEASURED`/`DERIVED` counts, the
`bevy_ui_widgets`/`Interaction` absence, and `PROTOCOL_REV = 22` were **measured on 2026-07-29
against this branch**.

**External — VERIFIED:**

- Clippy lint categories and default levels — <https://rust-lang.github.io/rust-clippy/master/index.html>.
  Confirms correctness (deny) / suspicious / style / complexity / perf (warn) ship enabled, and
  pedantic / nursery / restriction / cargo are allow-by-default. This is what §1 rests on.
- Bevy 0.19 change detection — <https://docs.rs/bevy/0.19.0/bevy/ecs/change_detection/index.html>
  and <https://docs.rs/bevy/0.19.0/bevy/ecs/change_detection/trait.DetectChangesMut.html>.
  Confirms **for the pinned version** that mutable deref flags a change regardless of whether the
  value changed, and that `set_if_neq` exists to avoid it. Underpins the §A2 caveat.
- Rust API Guidelines checklist — <https://rust-lang.github.io/api-guidelines/checklist.html>.
  C-NEWTYPE and C-CUSTOM-TYPE as cited in §A3, §A6 and §D6.

**External — INFERRED** (not fetched this pass; stable, and already restated inside the repo):

- Ousterhout, *A Philosophy of Software Design* — deep modules, information hiding, complexity as
  dependencies + obscurity, comments-as-red-herring. Credible here because house doctrine is built
  on it: `codebase-design/SKILL.md` cites him by name, including to *reject* his depth-ratio.
  <https://web.stanford.edu/~ouster/cgi-bin/aposd.php>
- Michael Feathers, *Working Effectively with Legacy Code* — the **seam** definition the house
  vocabulary uses verbatim, and attributes to him in `SKILL.md`.
- Matt Pocock — `.agents/skills/` is substantially his skill system (`setup-matt-pocock-skills`,
  `ask-matt`, `grill-with-docs`, `to-prd`, `to-issues`). Credible here **by adoption**: the repo
  already runs his flows, so his framing is the house's framing.

**Budget:** 4 web fetches of the ~10 allowed, spent where being wrong was most likely and most
costly — clippy's default categories (which determines what is *already* enforced, §1) and Bevy
0.19 change-detection semantics (the API most relevant to §A2, on a fast-moving version) — plus the
API-guidelines checklist. Ousterhout and Feathers were not re-fetched because the repo already
restates their positions, and **the repo's restatement is what binds here anyway**.
