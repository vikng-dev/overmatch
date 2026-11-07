# Unknown contact features bypass warm-start distance matching

Subject: the video game's server contact solver and environment rubble; game state only.

**Target:** avian3d 0.7.0 and current upstream `main` (source inspection).
**Status:** **DRAFT, not filed**. Checked read-only on 2026-11-08; unfixed on `main` at
"Add missing `Without<ColliderDisabled>` in `solve_continuous` (#1101)".

## Mechanism

`ContactManifold::match_contacts` promises feature-ID matching with positional matching when IDs
are unknown. The published implementation accepts equality of `PackedFeatureId::UNKNOWN` as
identity, copies that predecessor's normal and tangent warm-start impulses, and breaks before
checking anchor distance. `ContactPoint::new` initializes both IDs to `UNKNOWN`, so these points
can all inherit the first predecessor's cache even when their anchors are far apart.

The positional branch also parses as `(unknown && direct_match) || reversed_match`; the reversed
orientation bypasses the unknown-feature condition and can match conflicting known IDs. Both
orientations need the same admissibility gate.

Sources: published `avian3d-0.7.0/src/collision/contact_types/mod.rs`,
`ContactManifold::match_contacts` and `ContactPoint::new`, read from the Cargo registry;
[upstream matcher](https://github.com/avianphysics/avian/blob/main/src/collision/contact_types/mod.rs#L429).
The shipped correction is at
`vendor/avian3d-0.7.0-child-pose/src/collision/contact_types/mod.rs:435` (`known_features` and
`same_features`) and its positional gate at that file's `:457`.

## Evidence

Our commit **"physics: match unknown contact features through their anchors (#235)"** adds the
class witness `src/tests/avian_warm_start.rs`, `unknown_contact_features_match_by_position`.
Fixture inputs: predecessor anchors at the origin and one metre along x, a matching radius of
0.1 m, cached normal impulses of 5 and 11 N s, and a new point coincident with the latter.
**MEASURED in that commit:** the published matcher copies 5 N s rather than the coincident
point's 11 N s; the corrected witness passes. No geometry generation or full app is needed.

**MEASURED in the same commit:** its 60 s rubble wave still ends at 130 awake / 425 asleep.
That is an observation, not a settling guarantee or an isolated measure of this patch's effect.
The current witness also covers a nearby-predecessor case, owned by the separate
[nearest-contact draft](avian-nearest-warm-start-contact.md).

## Upstream status and filing boundary

The inspected `main` still accepts equal unknown IDs and has the positional precedence error.
Read-only GitHub issue/PR searches for `warm`, `UNKNOWN`, `match_contacts`, `contact matching`,
`nearest` and `anchor` found no matching defect report. Related
[issue #1098](https://github.com/avianphysics/avian/issues/1098) concerns invalid sphere/trimesh
geometry; [issue #734](https://github.com/avianphysics/avian/issues/734) proposes cache-free solving.
Neither reports this matcher failure. [PR #480](https://github.com/avianphysics/avian/pull/480)
adds orientation handling, and [PR #385](https://github.com/avianphysics/avian/pull/385) introduces
the solver/matching machinery; their discussion does not identify this defect. Searches cannot
establish that no report exists under different wording.

This is independently filable from nearest selection: fixing admissibility makes the distant
fixture pass but still permits the wrong predecessor inside the radius. The other draft owns
ranking admissible candidates and selection across manifolds.

## Suggested upstream fix

Require known IDs before accepting either orientation as a feature identity. Otherwise use
anchor distance, with both orientations inside the unknown-feature condition. Copy both cached
impulse fields only after a candidate passes. Retain the distant-point witness; also cover
partially unknown IDs and conflicting known IDs at reversed nearby anchors. The existing witness
measures the unknown-ID bypass, not those additional cases or the tangent field separately.

## Our shipped workaround and its removal condition

The vendored matcher above implements these gates; **"physics: match warm starts to the nearest
admissible contact (#235)"** retains them while adding nearest selection. Remove this part of the
patch when the replacement upstream implementation rejects unknown identities and applies the
positional gate to both orientations, with positive correspondence tests passing. Keep the
nearest-selection patch until its own removal condition holds. Other patches still require the
vendored crate.

## What fixing this unlocks for us

Retire the local admissibility correction and use upstream's cache correspondence for rubble
contacts. No sleep rule or gameplay workaround needs removal, and this alone does not establish
that every rubble island settles.
