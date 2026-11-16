# Manifold pruning lets separated points displace bearing points

Subject: rigid-body contact selection in a video game's server physics; game state only.

**Target:** published avian3d 0.8.0, `ContactManifold::prune_points` in
`src/collision/contact_types/mod.rs`. **Status:** LOCAL DRAFT, not filed; upstream head unchecked.
Source verified against the cached published crate, without network access.

## Mechanism

The first two selections multiply squared projected distance by squared penetration, floored at
1 mm squared. Squaring a negative penetration rewards separation: a point at -12 mm has
144 times the depth weight of a bearing point at +0.5 mm (DERIVED from the heuristic). Such
separated points are legitimate candidates admitted by tolerance or closing speed, but retaining them
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
arithmetic on these anchors). The game-side guard covers generated bearing/separated assignments,
with zero-depth, sub-floor and above-floor bearing points, varied separation and input orders.

## Proposed fix and local patch

Retain bearing points before separated points, using upstream's distance/area preference then
input order to fill four slots whenever separated candidates exist, clamping depth before squaring
so extra separation cannot improve selection, and leaving all-bearing selection unchanged.

The projection, first two distance/depth selections and two-sided area selection stay upstream's.
Construction visits those selected indices before remaining input indices, bearing candidates
first, stopping at four. This preserves current support and free-slot speculative stops without
adding a geometry heuristic or heap allocation. The unchanged floor prevents a zero distance or depth
factor from erasing the other factor's ranking. The normal solver turns separated points into
speculative stops, and our contact reducer reads their `normal_speed` after pruning.

An unconditional `min(n, 4)` result conflicts with exact all-bearing upstream selection: for five
collinear all-bearing points upstream returns two endpoints (DERIVED by its zero signed areas;
MEASURED by the direct guard). A convex pentagon with linearly varying positive depths can select
an edge first and return three points (also covered by the guard). This local patch
chooses exact all-bearing compatibility and guarantees full capacity when any separated candidate
exists; a requirement to fill every all-bearing manifold would also change upstream behavior.

Local guard: `src/tests/avian_contact_pruning.rs`, registered in the game's `src/tests.rs`.
The workspace excludes `vendor/`, so an in-crate dependency test would not run in the game's CI.
The guard checks generated mixes against bearing priority, capacity where compatible with
upstream, exact all-bearing upstream selection and separation monotonicity. It asserts contracts,
not historical physics outputs. The patch inventory is that crate's `OVERMATCH_PATCH.md`.

## What fixing this unlocks for us

The server's friction law can use the body's bearing support without manifold selection discarding
it in favour of separated candidates. The experiment lane MEASURED a full-width wall push at
0.83–0.99 of mu*(weight + downward wall friction) over five placements with unsigned depth;
the signed-depth experiment reached 0.99998–1.00005 at the upstream 0.02 m tolerance. Those are
observations, not acceptance values, and that experiment did not include the bearing-priority
retention proposed here. No tolerance adjustment, experiment switch or logger is shipped.

Retire this local pruning patch when an upstream release preserves the bearing-priority invariant
under the direct guard. Other fork patches remain independent.
