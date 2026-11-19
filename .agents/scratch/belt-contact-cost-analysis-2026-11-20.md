# Belt contact cost analysis

Line references are to `lane/belt-finite-inertia` at `bfc1ff58` unless marked `main:`. I built and ran nothing. Every number is either DERIVED from source or taken from the builder's MEASURED table (`track-contact.md:97-102`).

Belt-point visits per tick are DERIVED from that table (summed visits divided by 128 ticks): slab 85 / 680 and debris 257 / 2054, for 1 / 8 tanks.

## 1. Where the cost goes

**Per belt point, per pass (biased and relax).** The common path is `solve` in `avian_ext/surface_motion.rs:267-326`.
- **Metric rebuild:** every visit rebuilds the 2×2 rigid tangent metric, 6 cross products and the gradient (`:290-311`). That is about 100 flops, and the result does not change within a step.
- **Conversions:** about 15-20 f32↔f64 conversions per visit. At roughly one cycle each they cost nothing worth cutting.
- **`minimum` (`:146-184`):** one `evaluate` (`:114-144`: one downcast and 3 dyn calls), one `disk_minimum`, and one `exchange_pair`, which calls `Coordinates::push`.
- **`disk_minimum` (`:190-262`):** always runs an eigendecomposition (atan2, sin_cos, hypot). When the point is on the friction-disk boundary it also runs 64 bisection steps (`:251-260`).
- **`push` (`track/surface_motion.rs:190-222`):** one full block solve (`:211`) plus `same_face`.
- **Inside that block solve:**
  - `MechanicalRows::solve` clones the speed Vec (`transmission.rs:1669`).
  - `solve_limited` allocates start, free, bounds and dynamic (`block.rs:396-406`), clones the seed (`:333`) and allocates a state Vec (`:458`).
  - One `trial` (`:543-663`) then does: `face_system` (`:95-130`, 3 plus size Vecs), impulses, two velocity Vecs, an elimination (`:671`, `:718`), multipliers, `physical_equalities`, one Gram matrix plus elimination per snapped row (`:619-644`), `snap` (`:786-827`), and the Solution Vecs (`:653-661`).
  - DERIVED: about 20-40 heap allocations and 2-4 small matrices per solve, around roughly 200 flops of real arithmetic. A driving Tiger has 2-3 rows: clutch and steering; stop rows appear only when parked, braking or turning on the inner side (`transmission.rs:1716-1782`).
- **When the face changes:** `response` (`block.rs:142-187`) adds about 28 allocations and 3 eliminations.
- **Not on the hot path:** no `Arc`, no `clone_model`. The solver writes `SolverBody` in place (`solver_body/plugin.rs:236-326`).
- **Increment over main (DERIVED from MEASURED):** the lane adds 0.60-0.82 µs per visit per pass, across both passes and both supports. That fits one solve dominated by allocator traffic. Allocator cost itself is unmeasured.

**Per point, per substep, outside "contact ms".**
- During WarmStart, the hook pushes each point's cached impulse (`avian_ext/surface_motion.rs:284-289`; fork `contact/mod.rs:297-306`; coefficient is 1.0, `plugin.rs:596`). That is one more full solve per point per substep.
- The owner's totals climb from zero one point at a time, so its face walks from the unloaded state to the loaded one, rebuilding the response along the way.
- Avian books this time in `diagnostics.warm_start`. The probe sums only `solve_constraints` and `relax_velocities` (`track_scorecard.rs:3639-3640`), so this cost lands in "app ms".

**Per tank, per substep.**
- **prepare (`sim.rs:1270-1343`, parallel by tank):** `prepare_rows` allocates about 12-15 Vecs. Then `Coordinates::drivetrain` (`track/surface_motion.rs:72-84`):
  - solves at zero contact totals with a fresh `WarmStart::default()` (`:73`), which is a cold search starting from all-interior;
  - builds a response for that unloaded face, which the warm start then discards;
  - allocates 3 boxes.
- **The tick's warm start is written but never read.** `finish` remembers it (`:105-109`, `sim.rs:1467`), but the live path never reads it back. Main's per-substep `advance` did read it (`main:sim.rs:1276-1291`).
- **Serial drop:** the previous scratch is freed inside the serial install loop (`sim.rs:1338-1344`).
- **collect (`sim.rs:1353-1390`):** the same finishing work main did.
- **Per tick:** `prepare_belt_surfaces` (`stop_contacts.rs:268-288`) is lighter than main's per-tick map plus per-substep update (`main:stop_contacts.rs:266-392`). It is not a source of the gap.

**The roughly 2 ms outside the pass (debris, 8 tanks).**
- Outside-pass gaps (app delta minus contact delta, DERIVED from MEASURED): 0.336 / 1.277 / 0.459 / 2.011 ms.
- A two-term fit (DERIVED) gives:
  - about 0.53 µs per visit, which matches one warm-start solve per visit;
  - about 0.3 ms per tick for a lone tank, and about 0.11 ms per tank with eight tanks on two workers, which matches the parallel per-owner prepare work (cold seed, unloaded face, allocations).
- On debris with 8 tanks that is about 1.1 ms of warm-start solves plus about 0.9 ms of owner preparation.

**Confounder.** Both builds used `bitprobe` (`track-contact.md:86-95`). On main, the bitprobe-only `seed_conveyor_impulses` and `record_conveyor_work` walk every constraint once per tank per substep (`main:sim.rs:1316-1404`). The lane deleted them. So the MEASURED gap understates the gap a production server would see, by an unmeasured amount.

## 2. What the law requires vs what the implementation adds

**Required by the law:**
- Each owner's points are solved in sequence. Graph colouring already serialises a hull's rows.
- Each visit must see the owner's current speed and held mobility. That needs a face check per push, which is O(rows), and a re-solve only when the face changes.
- The third step, about 1 in 100,000 visits.
- Biased-pass belt friction (`track-contact.md:45-47`); relax-only friction fails the holding-travel witness (`:83`).
- One exact owner solve per substep for the drivetrain report. Main also did one.

**Added by the implementation:**
- A full re-solve every time one scalar total changes, although the response is affine on a face. The lane's own `same_face` argument already relies on that.
- WarmStart solves whose results nothing reads.
- The unloaded, cold-seeded solve in prepare.
- 20-40 allocations per solve.
- Rebuilding the step-constant metric twice per substep.
- 64 bisection steps where a safeguarded Newton step converges in a handful.
- Serial dispatch of independent owners. Each colour holds at most one row per hull, so hull-carrying colours likely stay under Avian's 64-row parallel floor (`plugin.rs:791,887`; `utils.rs:148`). That floor was priced for rows costing about 0.1 µs. Bringing row cost back down restores its premise, so no fork change is needed.

## 3. Cuts, in order of saving per line changed

**1. Defer the warm-start push.** About 30 lines. Risk: low.
- **Change:**
  - WarmStart only adds to the owner's totals and marks it stale (a new trait method, `avian_ext/surface_motion.rs:14-20`).
  - The first Solve-stage visit settles it with one solve, seeded by the tick's `WarmStart` (pass it into `drivetrain`, `track/surface_motion.rs:72-84`).
  - prepare stops solving at zero totals.
  - `finish` re-solves once from the final totals.
- **Why physics is unchanged:** nothing reads the owner between warm pushes (`:284-289`). Totals are summed in the same order. Speeds are unique for given totals (`block.rs:5-8`). Only a degenerate optimal face can differ, and there the lane's answer already depends on the seed.
- **Saving (DERIVED, debris 8):** about 1.1 ms of warm-start solves plus most of the 0.9 ms owner term.
- **Note:** the settle runs in the serial colour pass. That is one bounded solve per owner per substep, and it is serial because the warm totals only exist after Avian's warm start.

**2. Allocation-free block solve.** About 60-100 mechanical lines. Risk: very low.
- **Change:** use `SmallVec` (already a dependency, `Cargo.toml:32`) for the vectors and matrices in `block.rs`, `Row.jacobian`, `Solution`, `FaceResponse` and the WarmStart layout. Use a stack copy instead of the clone in `MechanicalRows::solve`.
- The inline capacity is a `Resolution` number (a cost choice). Constructions with more rows spill to the heap and stay correct, so this is not a per-tank limit.
- **Why physics is unchanged:** results are bit-identical, because the arithmetic order does not change.
- **Saving (DERIVED, unmeasured):** up to about 1.5-1.8 ms on debris 8, if allocation is most of a roughly 0.6 µs solve (about 200 flops suggests it is). It also speeds main's `advance` and the resting trials.

**3. Affine face update.** About 80-110 lines, including the witness. Risk: medium, because it adds a second path computing the same quantity.
- **Change:**
  - `response` keeps the per-column impulse and multiplier sensitivities it already computes (`block.rs:159`).
  - `push` predicts speeds, impulses and multipliers, then runs the existing `check_face` and break test (`:466-478`) on the prediction.
  - It falls back to `solve_limited` only when the face changes.
- **Why the result is unchanged:**
  - Face decisions are identical, because the same tests run on quantities that are exactly affine on a face.
  - Speeds match to f64 roundoff; held coordinates stay exact because the response zeroes them (`:174-180`).
  - The final state is exact via `finish`.
- **Exception:** a static row that broke during this interval may hold again under static bounds, and the current face cannot certify that. Those cases take the full path; they are rare (the end of a skid).
- **Saving (DERIVED):** about 0.4-0.8 ms if done after cut 2, about 2.4 ms if done without it.
- **Witness:** prediction equals re-solve across the class drivetrains and random push sequences.

**4. Faster `disk_minimum`.** About 25 lines. Risk: low to medium.
- **Change:** solve the interior case directly (2×2 Cramer) before any eigendecomposition. On the boundary, use a safeguarded Newton step on the secular equation, ending on the admissible side.
- **Physics:** the same minimiser to roundoff. The KKT witness (`:373-410`) covers it.
- **Saving (DERIVED):** about 40 ns per interior visit and about 0.25 µs per boundary visit. The share of boundary visits is unmeasured.

**5. Skip zero-budget points.** 3 lines. Risk: very low.
- **Change:** when the friction budget is 0 and the cached impulse is 0, `continue` before building the metric.
- **Physics:** identical. The disk returns zero and `push(0)` is already skipped (`track/surface_motion.rs:206-208`).
- **Saving:** about 50 ns per speculative belt visit, an unmeasured share of visits.

**Rejected:**
- Dropping biased-pass belt friction: the witnesses require it, and after cut 3 it costs about the same as a relax visit.
- Caching the rigid metric per step: about 0.1 ms, but it needs new storage in the fork, which is a divergence.
- Changing the fork's dispatch floor: the cuts restore its premise.
- Doing the settle inside the parallel prepare: it needs an owner-to-constraint index, about 50 lines, to save at most about 0.05 ms.

## 4. Believable target (DERIVED)

- **Hook cost per visit per pass:** after cuts 1, 2, 4 and 5, about 0.15-0.3 µs: about 100 flops of setup, the disk, and one allocation-free solve (or an affine check with cut 3).
- **Debris, 8 tanks:**

| | main (MEASURED) | lane today (MEASURED) | target (DERIVED) |
|---|---|---|---|
| contact ms per tick | 0.49 | 3.34 | 1.0-1.5 |
| app ms per tick | 2.42 | 7.28 | 3.0-3.6 |
| µs per point, biased/relax | 0.07/0.18 | 0.79/0.84 | about 0.25/0.35 |

  The target app time is +25-50% over main.
- **Slab, 8 tanks:** about 4.4-4.7 ms per tick, against main's 4.07.
- **The remaining gap is inherent to the law:** the biased-pass friction visit main never pays, the per-visit owner coupling, and one extra owner solve per substep. Avian's scalar friction clamp versus a coupled 2D disk adds a small remainder. Main's cost is not reachable without dropping held mobility.

## 5. Recommendation: tighten first, then land

Do cuts 1, 2 and 5 in this lane: about 100 lines, no physics change. Then land. Cuts 3 and 4 go in only if a re-measure of 8 tanks on debris still shows more than +1.5 ms.

**Why tighten first:**
- **Debt by the project's rule (AGENTS.md:42):** a warm start the lane writes every substep and never reads, and solves whose results nothing reads, are behaviour nobody can explain. Cut 1 removes both.
- **Headroom:** +4.86 ms is 31% of a 15.6 ms tick on two cores, from one subsystem, against a stated high priority on compute headroom.
- **Review cost:** the six-lens review reads this seam now. Fixing it in a follow-up lane means reviewing it twice, and every track lane in between prices its work against an inflated baseline.

**The case for landing now:**
- Main's defect (4-15 cm/s creep, energy injected by warm start) corrupts every track lane's physics today.
- 7.3 ms fits inside 15.6 ms, and there are no players.
- The cuts preserve behaviour and are separable, so a small follow-up lane would review quickly. A landing never waits.

**Fallback:** if cut 1 or cut 2 breaks any witness, land as is and file both cuts.

## 6. What I could not verify, and the one measurement to take

**Not verified:**
- Nothing was run.
- Allocations per solve and allocator cost (the Apple M4 dev host versus the hosted Linux server).
- The share of visits on the disk boundary.
- How often the face changes during warm start, and how many trials the cold search takes.
- The two-term fit, which rests on four cells.
- The size of the `bitprobe` confounder.
- Whether any hull on debris exceeds 23 manifolds and spills into the serial overflow colour (`constraint_graph.rs:55-59`).
- How much of each pass actually ran serially (`ContactVisits`, `solver_cost.rs:10-15`, records this).

**The one measurement:** a sampling profile of the debris, 8-tank cell of `constrained_belt_relax_cost_with_two_workers` under `--profile profiling` (`Cargo.toml:227-232`), with the same features on both branches, ideally without `bitprobe`. One run splits the time across `solve_limited`, the allocator, `disk_minimum`, the warm-start hook and `prepare_surface_speeds`, which settles the order of cuts 1-4.

A cheaper partial alternative: add `diagnostics.warm_start` and the existing `cost_probe` Meter to the probe, and wrap one `push` in `allocations_in` (`src/tests/alloc_count.rs:51-56`).

### Critical Files for Implementation
All paths as on `lane/belt-finite-inertia`:
- /Users/Yan/github/vikng-dev/overmatch/src/track/surface_motion.rs
- /Users/Yan/github/vikng-dev/overmatch/src/avian_ext/surface_motion.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/block.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/transmission/advance.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/sim.rs
