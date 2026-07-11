# Parry filing — ready for Yan's review (2026-07-11)

Everything verified on the PR base. Branch `fix/shape-cast-stagnation-refine` = upstream
`master` (v0.29.0, 8436f7c) + the three cherry-picked commits, pushed to
`vikng-dev/parry` (public fork). All suites green there (parry3d 63u+22i, parry2d, both f64
crates, fmt); RED (89eb49a) fails on master with the same numbers as at the 0.27.0 tag.
Commits carry your authorship.

File as ONE PR (dimforge takes direct PRs — #414 was one). To open after you approve the text:

```
cd ~/.codex/upstream-determinism-wave-a/parry
gh pr create --repo dimforge/parry --base master \
  --head vikng-dev:fix/shape-cast-stagnation-refine \
  --title "fix(gjk): shape-cast TOI error scales with target shape extent" \
  --body-file <this file's PR BODY section, extracted>
```

---

## PR BODY (draft — edit voice as you like)

### What

`cast_shapes` / `minkowski_ray_cast` returns a time of impact that comes back SHORT by an
amount that scales with the target shape's extent. Casting a ball (r = 0.5166) straight down
at a cuboid, measured max TOI distance error:

| cuboid half-extent | before | after (this PR) |
|---|---|---|
| 5 m | 0.246 mm | 0.113 mm |
| 50 m | 3.64 mm | 0.0023 mm |
| 500 m | **139.45 mm** | 0.00024 mm |

The witness data (`point1`/`normal1`) is exact even when the TOI is wrong.

### Reproduction

```
git checkout 89eb49a   # test commit only, no fix
cargo test -p parry3d --test lib shape_cast_toi_accuracy
#   FAILS — prints the error table above (139 mm at 500 m half-extent)
git checkout fix/shape-cast-stagnation-refine
cargo test -p parry3d --test lib shape_cast_toi_accuracy   # passes
```

### Mechanism

At first I suspected the relative convergence tolerance (gjk.rs:775-786), but it never fires for
this case — instrumented over the test's 2,400 casts: 0 hits.
What actually happens: `minkowski_ray_cast` advances a lower TOI bound `ltoi` and tracks an upper bound.
With large support coordinates orthogonal to the cast direction, float cancellation keeps the upper bound from decreasing;
the "upper bounds inconsistencies" last-chance exit fires (gjk.rs:712-715) and returns the
current `ltoi` as the hit (gjk.rs:721) — still short by the stagnated gap. All 2,278 erroneous
TOIs in the instrumented run exited through this path.

### Fix

On the last-chance path, refine `ltoi` from the simplex witnesses already in hand before
accepting the impact: project the witness separation along the cast direction
(`(witness1 - witness2 - ray.origin) · dir`). The witnesses stay precise exactly where the
upper bound stagnates, so this recovers the accuracy the bound lost. Witness and normal
generation are untouched — the returned contact data is bit-identical to before; only the TOI
scalar changes. Added cost is confined to the rare stagnation exit (≤ dim+1 weighted witness
accumulations via the existing `result()`).

Two intentional behavior changes to be aware of:

1. The refined TOI is no longer a certified lower bound: measured overshoot up to +0.113 mm at
   5 m half-extent (curvature converts lateral witness deviation into forward error ≈ r·θ²/2),
   where the old exit was strictly short. Three orders of magnitude below the old error, but
   callers relying on "TOI never long" should know.
2. A stagnation hit whose refined TOI exceeds `max_time_of_impact` now returns `None`, where
   the old code returned a wrongly-short hit inside the limit.

### Real-world impact

Found in a 64 Hz networked tank sim: wheel sphere-casts against a 1000 m ground cuboid came
back short by p50 33 mm / p99 134 mm / max 200 mm over 19,828 samples; through a 551 kN/m
suspension spring that was 10-40 kN of per-tick force noise — a sustained at-rest hull limit
cycle (~12 mm heave) and a standing client/server divergence amplifier. #414 hit the same exit
independently from a Rapier `DynamicShapeCastVehicleController` (wheels vs a 10,000-unit
ground backstop, TOI ~10× short) — this PR is the same diagnosis with a regression test and
the refine kept inside the existing exit. Possibly related: #180's pose-knife-edge `None`
returns come from this same function's exit structure, though this PR does not address that
case.

### Tests

`shape_cast_toi_accuracy_does_not_scale_with_shape_extent` (tests/geometry/time_of_impact3.rs)
pins max error ≤ 0.2 mm across half-extents 5/50/500 m and exact witnesses. Full workspace
`--tests` suites pass for parry3d, parry2d, parry3d-f64, parry2d-f64; `cargo fmt --check`
clean.

---

## Notes for you (not in the PR)

- #414 (nickyvanurk, 2026-04-22) was self-closed after 8 minutes with zero comments — no
  maintainer signal exists either way. Citing it strengthens the case (independent repro from
  Rapier's own vehicle controller) and is honest about prior art.
- The one-sidedness disclosure (change 1) is load-bearing for OUR repo too: when we retire
  `sphere_cast_ground_contact`, drop its "TOI is provably never long" comment.
- If a maintainer asks for the instrumentation evidence (exit-path counts), it's in
  `.agents/scratch/wave-a-ab-records/review-parry.md`.
- Optional demo video (your call): the at-rest limit cycle vs calm — we measured the in-game
  numbers (ab-parry.md) but the visible heave needs an UNPATCHED build with the workaround
  gated off; the bay `wave-a/parry-alone` branch minus the override + SPIKE_RAW_TOI=1 would
  show it. Nice-to-have, not required — the two-command repro is the maintainer-grade proof.

---

## FILED 2026-07-11

- Issue: https://github.com/dimforge/parry/issues/429
- PR: https://github.com/dimforge/parry/pull/430 (head vikng-dev:fix/shape-cast-stagnation-refine, base master)
- Discord follow-up posted by Yan (dimforge server).
