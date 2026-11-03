Cause: the belt reader admits a freed tree's compound collider, then panics because it only extracts boxes and convex polyhedra (`src/track/rubble.rs:252`, `Surface::triangles`).
Subject: Overmatch's video-game environment physics and dedicated-server stability; game state only.

The producer/reader contract breaks at detachment: `trees::motion::launch_range` (`src/trees/motion.rs:113`) wraps each convex segment in a translated single-child compound; `spawn_range` installs that collider on the freed carrier. `RubbleQuery::supports` admits it by solidity/layers; `RubbleBodies::gather` sends it to the narrower extractor. `Surface::support` also requires a convex support map, so fixing triangle extraction alone leaves another panic.

MEASURED: untouched `v0.7.0-alpha.10` and untouched main at **perf: refuse incomplete collapse benchmark observations** both exited with status 101 on the same full-throttle drive. This reproduces a local server exit consistent with the reported reconnect; the lost hosted log cannot prove that deployment's cause.
MEASURED: last authoritative speeds were 8.42548 m/s (tag), 8.29809 m/s (main); last tank-centre distances to the tree were 3.31984 m and 3.32486 m. Both backtraces name `Surface::triangles -> RubbleBodies::gather -> apply_track_forces`.

The course targets the nearest tree to the first authored spawn point, instance `fir_a_0020`, with a DERIVED 70 m straight run-up along the spawn heading. Its spawn pose is DERIVED from the shipped heightmap and the tank settles before release; the production fill-up and real UDP admission/input pipeline are retained. Firing is disabled. Both clients use the complete presentation stack in a hidden window.
MEASURED: the shipped map has 635 trees, all resolving to the same tree kind. Broader tree/speed/angle/building/host-limit variations were unnecessary because the first tag drive reproduced; no hosted deployment was used.

MEASURED: a temporary regression witness frees the shared tree fixture through production tree-motion systems, then gives its real colliders to the production belt query. It fails with the identical panic without networking, a tank, gravity, motion or a collision; the test itself takes 0.22 s. Its collider census is three single-child compounds plus the three sensor hulls; the belt admits the former and excludes the latter.
MEASURED: a temporary probe on main named the failing collider just before the same panic: shape `Compound`, owner `TreePiece { tree: 19, cut: 1 }` (`evidence/main-identity.txt`).

Earlier tests miss the join: `track::rubble::tests::a_trunk_segment_carries_the_belt_and_is_loaded_by_it` constructs a standing convex segment, not a freed collider. `trees::motion::tests` exercises production detachment and solver contacts without installing the running-gear reader. The witness preserves that missing producer/consumer seam for the eventual fix.

Smallest generic options (no fix retained):
- Recommended: at the belt snapshot boundary, unwrap single-child compound transforms into the convex leaf's world pose once; use that same normalized shape/pose for triangles, support points and face casts, retaining the original collider/body identity, friction and mass. Keep true multipart compounds subject to an explicit supported-shape contract; do not replace them with a convex hull across gaps.
- Alternatively, keep local collider poses separate from shapes in the shared freed-piece launch representation, spawning convex child colliders directly. This makes the existing belt contract hold but touches launch mass/frame construction and more callers.
Neither option changes tank/tree-specific behavior. The owner should choose before implementation; the joined regression and the original network drive should gate that change.

Evidence: `evidence/nearest.ron`, `evidence/reproduce.py`, `evidence/reproduce.md`, both panic/backtrace extracts, `evidence/witness.patch`, `evidence/identity-probe.patch` and `evidence/witness-red.txt`. Raw complete process logs and authoritative traces remain in `target/diag-tree-crash/` in this bay. No gameplay fix is committed; temporary source changes stayed out of this report's commit. The lane's run was ended by the provider's automated filter before it finished, so this report was completed from its draft by the orchestrator.

Read-only panic reporting: install a hosted-process panic hook that captures payload, location and a backtrace on the panicking thread, chains the normal stderr hook, and makes a bounded authenticated report before unwinding/exiting. Extend the existing end-report contract with a separate bounded private crash-text field, persisting it before `Registry::finish`/`stopWithEvidence`; the existing sanitized end reason cannot preserve panic text. This covers Rust panics, not abrupt process kills or an unavailable network.


Orchestrator's note, for the owner's ruling: `buildings::breakage` builds a freed chunk with `chunk_collider(kind, c, scale, centroid)`, one convex shape baked about the centroid, with no compound. Building a freed tree segment the same way in `launch_range` is the second option at its smallest, and it needs no change to the belt reader.
