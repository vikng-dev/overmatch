# Avian patch review verdict — SOUND-WITH-CORRECTIONS, one BLOCKING (reviewed 2026-07-11)

Repo clean at fix/solver-constraint-order @ cd6bdaf.

## BLOCKING: 2D determinism hash constant miscalibrated (would fail upstream CI as submitted)

src/tests/determinism_2d.rs:63 changes 0x34e9643f → 0x3126af7d, but upstream CI
(.github/workflows/ci.yml:104) runs `--no-default-features --features ...avian2d/f64,parry-f64,
xpbd_joints,enhanced-determinism,...`. Measured (ARM machine validated as faithful control —
RED + exact CI features PASSES the old 0x34e9643f):
- HEAD + exact CI feature set: FAILS — "Expected transform hash 0x3126af7d, found 0x4fa858dc".
  Reproduced twice.
- HEAD + default f32 + enhanced-determinism: 0x3126af7d passes → Codex derived from f32 default.
- Hash also feature-sensitive (dropping xpbd_joints at RED → 0xe7570d4). Only the exact CI set
  is authoritative.
Fix before filing: re-derive under CI features (our measurement: 0x4fa858dc) and confirm from
the PR's own CI run. The hash CHANGE itself is expected and honest (patch changes canonical
results globally) — only the value is wrong.

## Traps

(a) Threading PASS — solve loops untouched (plugin.rs:528-557/619-647/717-745/797-830);
parallel-safety invariant re-established for canonical order via separate canonical_body_set
coloring (constraint_graph.rs:263-310).
(b) Geometry-derived PASS — key = lexicographic-min world contact point then world normal,
total_cmp (plugin.rs:24-49); no entity ids. RESIDUE for PR text: stable sort over
entity-ordered handles → exact bit-equal key ties resolve to entity order (coincident-geometry
manifolds could still diverge cross-World; degenerate).
(c) Staleness/warm-start PASS — dirty flag on every graph mutation (add_manifold :224,
pop_manifold :369, clear :392), consumed at prepare top (plugin.rs:409); despawn → pop → dirty.
Warm-start keyed by contact_id+manifold_index (plugin.rs:806-808), immune to reorder. CAVEATS:
(1) ContactManifoldSortKey::new .expect panics on zero-point manifold (plugin.rs:31-36) where
old code tolerated empty (plugin.rs:511) — likely unreachable, review liability; (2) dirty flag
is GLOBAL — busy scenes rebuild ~every step; the +0.37% perf claim is NOT verified (no bench in
diff), nor are 6.8 ns / 1.47 ms numbers.
(d) = the blocking correction above.

## Minimality PASS

4 files, +328/−34, no churn. Semver note for upstream: GraphColor/ConstraintGraph gain private
fields (breaks downstream struct-literal construction).

## Tests (exact)

- HEAD named test ok (0.19s). Full -p avian3d: 68 passed / 1 failed =
  trimesh_builder rasterizes_compound, last-mantissa-bit diffs, CONFIRMED failing at base
  v0.7.0 tag → pre-existing; Codex accurate.
- RED bc7165d verbatim: "rigid body states diverged at tick 1; manifold counts: server=2,
  client=2", angular velocity dominant (−0.5246,−1.3324,0.8212 vs 1.6117,−0.1795,−3.5390) —
  genuine cross-world multi-manifold divergence.
- Test non-vacuous: two Worlds, different spawn histories (16 spawn/despawn + swapped spawn
  order), assert_ne! on entity indices, ≥2 manifolds both bodies ≥100/180 ticks, full-state
  equality every tick. Pedantic: PartialEq float equality, not strictly bit-identical
  (−0.0==0.0 passes) — practically equivalent.

## Correction to OUR report framing

Mechanism confirmed, but RED divergence magnitude (order-1 Δav at tick 1) shows the driver is
sequential-impulse (Gauss-Seidel) SOLVE-ORDER dependence — reordering changes which impulses
see already-updated velocities — not merely float-sum non-associativity. Same root cause,
stronger effect than the report's phrasing implies. Update report §Mechanism framing.

## Not re-verified (machine constraints)

New 3D test under CI f64 matrix; doc-tests; all Codex perf numbers.
