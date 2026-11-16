# Game-owned warm-start predecessor ranking

Found in Overmatch's server physics, a multiplayer tank video game. Local record; not filed.
This is a game policy, not a current upstream proposal. The correctness patch is
[UNKNOWN feature identity](avian-unknown-contact-feature-identity.md).

`src/avian_ext/contact_match.rs` ranks admissible predecessors across all old manifolds by the
sum of squared anchor distances, including known feature IDs. The fork's `ContactMatcher` hook
keeps that choice game-side; its default retains upstream's manifold order, first identity match
and early exit. No iterator API is needed.

The witness `tests::avian_warm_start::nearest_warm_start_is_selected_across_old_manifolds`
builds two overlapping spheres (radii 1, centres 1.5 apart). Parry produces one contact with
`face(0)` on both sides, both known. The test supplies two old one-point manifolds with those same
IDs: a coincident point carrying normal impulse 11 and tangent (7, 13), and a point offset by
0.05 on both anchors carrying 5 and (2, 3). These are Fixture values. The new contact must retain
the coincident impulses in either old-manifold order. Upstream's repeated first-match calls let
the later offset manifold overwrite them; flattening and taking the first known identity instead
fails in the reversed order. The deleted seam at `21a66be7:src/avian_ext/contact_match.rs`
explicitly ranks known identities too. The witness requires that policy, so it stays game-side.

The engine correctness draft compiles independently against the published crate; this record
contains no patch to apply or compile.
