# Manifold pruning lets separated points displace bearing points

Subject: rigid-body contact selection in a video game's server physics; game state only.

**Target:** published avian3d 0.8.0, `ContactManifold::prune_points` in
`src/collision/contact_types/mod.rs`. **Status:** LOCAL DRAFT, not filed; upstream head unchecked.
Source verified against the cached published crate, without network access.

## Mechanism

The first two selections multiply squared projected distance by squared penetration, floored at
1 mm squared. Squaring a negative penetration rewards separation: a point at -12 mm has
144 times the depth weight of a bearing point at +0.5 mm (DERIVED from the heuristic). Such
separated points are legitimate candidates inside the contact-tolerance band, but retaining them
at the expense of bearing points removes current support from the solver manifold.

Clamping depth to zero before squaring removes that reward but does not guarantee bearing priority.
The floor ties shallow bearing and separated points; the final area selection ignores depth.

## Geometry-free reproduction

No collider geometry, world or physics schedule is needed. Fixture: six points on a 2 m square,
three at +0.5 mm penetration and three at -12 mm separation:

```rust
use avian3d::prelude::*;
use bevy::prelude::*;

let anchors = [
    (-1.0, -1.0), (1.0, -1.0), (1.0, 1.0),
    (-1.0, 1.0), (0.0, 1.0), (0.0, -1.0),
];
let mut manifold = ContactManifold::new(
    anchors.into_iter().enumerate().map(|(i, (x, z))| {
        let at = Vec3::new(x, 0.0, z);
        ContactPoint::new(at, at, at, if i < 3 { 0.0005 } else { -0.012 })
    }),
    Vec3::Y,
);
manifold.prune_points();
let bearing_kept = manifold.points.iter().filter(|p| p.penetration >= 0.0).count();
assert_eq!(bearing_kept, 3, "pruning must preserve the three bearing points");
```

The unsigned heuristic selects two bearing points here. Exchanging the bearing and separated
halves makes even the depth-clamped heuristic select only one (DERIVED by executing the selection
arithmetic on these anchors). The guard beside the vendored function covers every assignment on
this square, with zero-depth, sub-floor and above-floor bearing points.

## Proposed fix and local patch

When a manifold has bearing points, make those the eligible set. If at most four remain, keep
them all. Otherwise retain the existing farthest-from-centre, farthest-from-first and two-sided
area selection, including its polygon order. If every point is separated, retain speculative
support through that same selection, weighting signed depth clamped to zero before squaring.
The distance/depth floor is unchanged. This enforces the invariant independently of placement,
point order and shallow-depth ties; it introduces no vehicle or material rule.

Local guard: `collision::contact_types::tests::pruning_never_drops_a_bearing_point_for_a_separated_point`
in `vendor/avian3d-0.8.0-overmatch/src/collision/contact_types/mod.rs`.
The patch inventory is that crate's `OVERMATCH_PATCH.md`.

## What fixing this unlocks for us

The server's friction law can use the body's bearing support without manifold selection discarding
it in favour of separated candidates. The experiment lane MEASURED a full-width wall push at
0.83–0.99 of mu*(weight + downward wall friction) over five placements with unsigned depth;
the signed-depth experiment reached 0.99998–1.00005 at the upstream 0.02 m tolerance. Those are
observations, not acceptance values, and that experiment is not the stronger eligibility rule
proposed here. No tolerance adjustment, experiment switch or logger is shipped.

Retire this local pruning patch when an upstream release preserves the bearing-priority invariant
under the direct guard. Other fork patches remain independent.
