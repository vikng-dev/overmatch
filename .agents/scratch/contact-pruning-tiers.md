# Contact pruning tiers

Subject: the multiplayer tank game's server contact physics; game state only.

## Implementation

The prescribed helper and two-tier construction replace the previous patch on `77c88dbd`.
Penetrating (bearing) points first; speculative (separated) points fill free slots by the same
selection. Homogeneous construction preserves upstream order and under-capacity results.
Mixed tiers use selection-order picks and input-order completion. No lone-point fallback,
projected-anchor deduplication, new geometry heuristic or allocation was added.

The three selection loops retain upstream comments and assertions, with only eligibility filters
and the normal parameter changing their logic. Rustfmt wraps the moved distance expression.
The mixed branch returns before the homogeneous helper call and unchanged upstream construction.
The local draft includes the pruning-only diff against the cached published archive, omitting
fork-only patch comments.

## Fork inventory

MEASURED using `diff -ru` on `src/` against the cached published avian3d 0.8.0 crate,
SHA-256 `ca7479ccec5f1440675946bada8c805f2b6d514af78e1d2b0937ca53b1e2c543`, counting
added plus removed lines and fork-only file lengths: **1,071 total**, **993** in upstream files,
**78** fork-only. The contact-types file has **161**, including **156** pruning lines and
**5** pair-state lines. Earlier inventory: **968 / 890 / 78**, contact types **58**, pruning **53**.
The loops' move and indentation count as removed and added source. Before restoring the
construction block to upstream indentation: **1,082 / 1,004 / 78**, contact types **172**,
pruning **167** (MEASURED).

## Validation

R1–R7 cover collinear, square/edge-midpoint and pentagon families. R6 compares selection-order
oracle picks followed by remaining separated indices in input order, truncated to the free slots.
This includes collinear mixed manifolds whose oracle supplies only two picks for three free slots.
The all-bearing collinear/pentagon under-capacity witness guards upstream compatibility.

The initial implementation passed all eight pruning guards before mutation. Each mutant was
applied alone, run through
`scripts/serial-cargo test --offline --lib --features dev_ui,bitprobe tests::avian_contact_pruning`,
observed failing with test exit 101, then reverted before the next mutation (MEASURED):

| Mutant | Failed relations |
|---|---|
| (a) Unsigned depth weights | R2, R6, R7 |
| (b) Separated fill by input order | R6 |
| (c) Bearing fill by input order | R5 |
| (d) Drop all separated points, homogeneous and mixed | R2, R4, R6 |
| (e) Fill homogeneous manifolds to four | R1 and `upstream_can_keep_fewer_than_four_bearing_points` |

The mixed-tier-only version of (d) also failed R4 and R6 before the global version was checked.
Every mutation was reverted after its test process ended. The restored source matches the draft's
proven function. Final commit gates passed after the global mutation was reverted:

- `cargo fmt --all --check`.
- `scripts/serial-cargo clippy --offline --locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`.
- `scripts/serial-cargo test --offline --lib --features dev_ui,bitprobe` with each filter:
  `tests::avian_contact_pruning` (8 passed), `tests::avian_warm_start` (3 passed), and
  `headless_test::track_scorecard` (16 passed, 7 on-demand probes ignored).
- One `CARGO_NET_OFFLINE=true scripts/track-scorecard` capture: passed every law check.

No push, external repository action, timing measurement or remaining implementation work.

## Follow-up verification

MEASURED: after restoring collinear generation, all eight pruning guards and 16 scorecard tests
passed (seven on-demand probes ignored). Mutants (b) and (e) were each applied alone and returned
101: (b) failed R6; (e) failed R1 and the upstream under-capacity witness. Each was reverted after
its test process ended. On the restored source, `cargo fmt --all --check`, CI clippy
(`--locked --workspace --all-targets --features dev_ui,bitprobe -- -D warnings`), and both
targeted test modules passed again through the build queue with `CARGO_NET_OFFLINE=true`.

The regenerated draft applies cleanly to pristine published 0.8.0. Its pruning function matches
the fork after omitting fork-only patch comments; the upstream construction block matches
pristine verbatim. The cached pristine `src/` was verified byte-for-byte against the published
archive before counting fork lines.

## Initial standalone draft compile proof

Extracted the cached published `.crate` into the new empty `tier-selection-_b0boc89` directory
under the requested `scratchpad/draft-check-prune/`. Read the diff directly from the Markdown
draft's fenced block and applied it with `patch --batch -p1`: clean application; the resulting
pruning function was byte-for-byte equal to the then-vendored function. No other fork patch was
applied.

Crate root:
`/private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/draft-check-prune/tier-selection-_b0boc89/avian3d-0.8.0`.

A direct `cargo check --lib --no-default-features --features 3d,parry-f32` through the queue
could not resolve the published dev-dependency `bevy_mod_debugdump ^0.17` offline; only older
versions are cached. The published manifest and lockfile were left unchanged. A sibling
`check-harness` depends on this extracted crate by path, with `default-features = false` and
features `3d,parry-f32`. As a dependency, Avian's unused dev-dependencies are not resolved.
Its only source line is `pub use avian3d;`.

Command (run from this bay, through the machine queue):

```sh
scripts/serial-cargo check --offline --lib --manifest-path \
  /private/tmp/claude-502/-Users-Yan-github-vikng-dev-overmatch/13e20afd-7b0e-4ad2-9b6e-b78f39af0d6c/scratchpad/draft-check-prune/tier-selection-_b0boc89/check-harness/Cargo.toml
```

MEASURED: `cargo check` **passed** for both the patched published Avian library and the harness.
No published source or manifest change beyond the draft patch was needed.

## Scorecard observations

MEASURED by one fresh complete capture; every law check passed. No timing measurements or acceptance targets.
Earlier columns are retained from [the preceding report](contact-fixes-rereview.md); its baseline
counted all solver pairs/manifolds rather than bearing pairs/manifolds. Each new value is an
observation for judgement, not a test target.

| Observation | Before | Earlier after | d71637c3 | Re-review | Tier selection |
|---|---:|---:|---:|---:|---:|
| Full-width wall normal, kN | 280.792 | 367.760 | 372.579 | 372.417 | 372.796 |
| Wall vertical friction, kN | −32.650 | −82.154 | −89.929 | -89.639 | -90.290 |
| Left/right wall normal, kN | 197.664 / 197.852 | 193.694 / 195.217 | 193.694 / 195.240 | 193.694 / 195.240 | 193.694 / 195.217 |
| Layer support / weight | 1.000010 | 0.999992 | 1.000004 | 1.000005 | 0.999983 |
| Layer reaction ratio | 0.999955 | 0.999991 | 0.999973 | 0.999974 | 0.999975 |
| Layer vertical jitter, mm RMS | 28.922 | 28.928 | 28.854 | 28.870 | 28.867 |
| Layer peak piece speed, m/s | 0.23249 | 0.20167 | 0.15328 | 0.15328 | 0.15328 |
| Layer bearing pairs/manifolds per tick | — | 15.4766 | 15.4141 | 15.4453 | 15.4883 |
| Layer bearing points per tick | — | 60.2813 | 60.0117 | 60.1992 | 60.3242 |
| Layer total solver points per tick | 99.8320 | — | 106.6055 | 106.3398 | 106.5469 |
| Highest step climbed, m | 0.6 | 0.6 | 0.6 | 0.6 | 0.6 |
| 0.6 m step peak pitch, degrees | 14.834 | 14.936 | 15.196 | 15.094 | 15.072 |
| Pivot yaw, rad/s | −0.128220 | −0.128220 | −0.128220 | -0.128220 | -0.128220 |
