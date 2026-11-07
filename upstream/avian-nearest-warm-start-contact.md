# Warm-start matching chooses the first admissible point, then lets later manifolds overwrite it

Subject: the video game's server contact solver and environment rubble; game state only.

**Target:** avian3d 0.7.0 and current upstream `main` (source inspection).
**Status:** **DRAFT, not filed**. Checked read-only on 2026-11-08; unfixed on `main` at
"Add missing `Without<ColliderDisabled>` in `solve_continuous` (#1101)".

## Mechanism

`ContactManifold::match_contacts` copies the first admissible predecessor's normal and tangent
warm-start impulses and breaks. After correcting the separate
[unknown-ID admissibility defect](avian-unknown-contact-feature-identity.md), several predecessors
can still lie inside the distance threshold. Array order then selects the cache, even if a later
point has exactly coincident anchors.

`NarrowPhase::update` calls the matcher separately for each old manifold. Each call can overwrite
the preceding call's choice without comparing distances. Choosing the nearest point inside each
manifold therefore still leaves the pair-wide result dependent on manifold order.

Sources: published `avian3d-0.7.0/src/collision/contact_types/mod.rs`,
`ContactManifold::match_contacts`, and `src/collision/narrow_phase/system_param.rs`,
`NarrowPhase::update`, read from the Cargo registry;
[upstream matcher](https://github.com/avianphysics/avian/blob/main/src/collision/contact_types/mod.rs#L435)
and [upstream caller](https://github.com/avianphysics/avian/blob/main/src/collision/narrow_phase/system_param.rs#L894).
The shipped selection is at
`vendor/avian3d-0.7.0-child-pose/src/collision/contact_types/mod.rs:437`, with its pair-wide iterator
at `vendor/avian3d-0.7.0-child-pose/src/collision/narrow_phase/system_param.rs:859`.

## Evidence

Our commit **"physics: match unknown contact features through their anchors (#235)"** fixes
admissibility but retains first-match selection and the per-old-manifold calls. Its successor,
**"physics: match warm starts to the nearest admissible contact (#235)"**, extends
`src/contact_transfer.rs:888`, `unknown_contact_features_match_by_position`:

- Fixture inputs: predecessors at the origin and 0.05 m along x, a 0.1 m radius, cached normal
  impulses of 5 and 11 N s, and a new point coincident with the latter; all feature IDs unknown.
- **MEASURED in the successor commit:** first-match copies 5 N s rather than the coincident
  point's 11 N s even after the admissibility correction. Nearest selection makes it pass.
- **DERIVED from the caller:** put the coincident predecessor in an earlier old manifold and the
  offset predecessor in a later one. The later call replaces 11 N s with 5 N s. This is a source
  counterexample; the existing direct witness does not exercise multiple manifolds.

**MEASURED in the successor commit:** its final 60 s wave ends at 62 awake / 514 asleep, against
the first corrected-sweep wave's 81 awake / 460 asleep. These are observations of different
emergent populations, not an isolated A/B of nearest selection, a sleep requirement or evidence
of a CPU speedup. No sleep rule changed; complete settling remains unproven.

## Upstream status and filing boundary

The inspected `main` still breaks on the first match and calls once per old manifold. Read-only
GitHub issue/PR searches for `warm`, `UNKNOWN`, `match_contacts`, `contact matching`, `nearest`
and `anchor` found no matching report. [Issue #1098](https://github.com/avianphysics/avian/issues/1098)
reports generated sphere/trimesh geometry, and
[issue #734](https://github.com/avianphysics/avian/issues/734) proposes cache-free solving;
neither reports this selection error. [PR #480](https://github.com/avianphysics/avian/pull/480)
discusses contact orientation and [PR #385](https://github.com/avianphysics/avian/pull/385)
introduces the solver/matching machinery. Their discussion does not identify nearest selection
or cross-manifold overwrite. No matching report found is a scoped search result, not proof of absence.

File this separately from unknown identity: admissibility and ranking have different invariants
and independently failing fixtures. The nearest-point witness is evaluated after fixing
admissibility; this report does not attribute that prerequisite's failure to ranking.

## Suggested upstream fix

Choose a predecessor once across every old manifold of the contact pair. Among admissible
candidates, minimize the sum of squared anchor distances over the accepted direct/reversed
orientations, then copy both cached impulse fields once. Preserve known-feature matching and
the unknown-feature distance gate. With no predecessor, keep the new contact's empty cache.

The shipped implementation accepts a cloneable iterator and flattens old point iterators without
allocating a point buffer. **DERIVED work:** O(current points × prior points), unchanged in
worst-case order, with a full candidate scan instead of first-match early exit. Runtime cost is
unmeasured here. For upstream acceptance, add permutations of predecessor and manifold order
with a uniquely nearest point. The existing fixture asserts the normal field in a single
manifold; tangent copying and pair-wide selection need explicit coverage.

## Our shipped workaround and its removal condition

The successor commit's vendored matcher and narrow-phase caller implement the pair-wide nearest
selection above. Remove those changes when an upstream replacement passes the nearby-point
witness and a positive cross-manifold order witness, while retaining the unknown-ID correction.
The current single-manifold witness alone cannot authorize retiring the caller patch. Other
patches still require the vendored crate.

## What fixing this unlocks for us

Retire the local iterator API change, nearest-selection implementation and narrow-phase caller
change. Rubble contacts can use upstream warm starts without our correspondence patch. This
does not justify changing sleep thresholds or promise that every pile settles.
