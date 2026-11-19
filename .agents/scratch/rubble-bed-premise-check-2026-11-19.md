# Rubble bed premise check: what the tank should touch

Citations are `lane/rubble-graded` unless marked, read with `git show`. Nothing was built or run.

**Decision.** The premise is half sound. Fill as a shared height map that the belts ride is sound. Fill as a rigid static collider for the stops and hull is not, and no law that edits heights around the solve can repair it. Continue the bed with one named contact: **sampled elastic-plastic fill support** for every vehicle part, with no vehicle-to-fill solver contact at all. A lab-only experiment of about 30 lines can kill the idea first.

## 1. Where the force comes from

| Path | Fill treated as | Evidence |
|---|---|---|
| Belt columns | Height samples under a spring-damper; no strength term, cannot spike | `src/rubble_bed.rs:342-360` (`Topped`), `src/track/forces.rs:861-873` |
| Compression stop | Rigid static heightfield | stop on `Layer::Vehicle` against all layers, `src/track/bump_stop.rs:225`; tile is `RigidBody::Static` + `FieldSurface`, `src/rubble_bed.rs:575-577` |
| Hull and turret proxies | Rigid static heightfield | `src/tank/template.rs:1050`; its comment at `:1052` assumes "the proxies sit clear of the ground" |
| Soft run | Never meets fill | meets `Layer::Debris` only, `src/track/soft_run.rs:89-91` |
| Side plates | Never meet terrain | `src/track/bump_stop.rs:177-187` |

- **The belt channel is calm on fill.** At the peak ticks it supplies 6.7 to 13.2 m/s² while the solver supplies 54 to 180 (MEASURED, arms report and belt diagnosis).
- **The front of the track is rigid from first touch.** End wheels have no travel (`src/track/rig_geom.rs:614-618`), so the sprocket wrap meets a mound with no spring in front of it.
- **The stop's friction on fill is the ground's grip**, with belt speed as surface velocity (`src/track/stop_contacts.rs:224-239`, `:393-397`).
- **The solver has no strength term.** The normal impulse is clamped only at zero (`vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/contact/normal_part.rs:179`).

**Is "rigid field plus after-the-fact lowering" the root of the jolts? Yes.**

- **Give is computed from the impulse already paid.** `depth_m: (force - capacity) * dt * dt * mobility` (`src/rubble_bed/bearing.rs:584`) reads `ContactTransfers` of the finished step (`:362-364`). A 6 MN impact buys at least 2.6 cm afterwards (DERIVED, 6e6 / 4096 / 57,000).
- **The traces agree.** At plain tick 811 the surface went 0.779 to 0.725 m after contact (MEASURED).
- **Editing heights before the solve fails too.** A rigid static surface answers with zero or with whatever stops the body, so no height edit can express an intermediate force. The plough's pre-physics set (`src/rubble_bed.rs:625-631`) left a part-lowered ramp: 10.973 N·s from the law against about 360,064 N·s from the solver (MEASURED).
- **The over-resistance is about two orders of magnitude (DERIVED).**
  - Honest resistance to a 0.3 × 3 m face at 12 m/s is about 6.2 kN passive plus ρv²A.
  - With ρ = 1845 × 0.60 = 1,107 kg/m³, ρv²A is about 143 kN, or 2.6 m/s² on 57 t.
  - The solver delivered 23 MN, about 404 m/s².
- **Bottoming is geometric at lab speed (DERIVED).**
  - Ride frequency 1.2 Hz and 57 t give 3.24 MN/m (`assets/tiger_1/tiger_1.tank.ron:5`, `:164`).
  - From rest to the 0.200 m stop (`:171`) the springs absorb about 65 kJ net, a vertical closing speed of about 1.5 m/s, undamped.
  - A 38° repose face at 12 m/s demands 9.4 m/s. Anything steeper than about 7° bottoms.
  - So on a fast pass the stop and hull law is the ride.

**The hangs have a second root.** Capacity is a level-footing form over the masked footprint and grows roughly as width squared (options note P5). A belly on a crest reads 4.76 MN against a 559 kN load and never gives (MEASURED). That is a law-geometry fault, not a timing fault.

## 2. Representations

Avian offers `filter_pairs` and `modify_contacts` (`vendor/.../collision/hooks.rs:164`, `:187`) and one global softness (`vendor/.../dynamics/solver/plugin.rs:593-595`). The fork's compliant arm is a linear spring-damper per manifold with no ceiling (`normal_part.rs:137-151`, `src/avian_ext/compliance.rs:147-158`). Avian can express a spring on a manifold, not plasticity.

| | (a) sampled elastic-plastic | (b) Law C cap in solver | (c) firm layer + soft layer | (d) predictive sweep |
|---|---|---|---|---|
| Stone lift | Unchanged: a 0.5 m piece stays a body on the soft run; relief is felt by belt columns | Unchanged | Unchanged | Unchanged |
| Wheel oscillation | Unchanged: belt columns untouched | Unchanged | Unchanged | Unchanged |
| High-centring | Fixed by form: stress is local to each sample, so a crest yields until the belts carry | Not fixed: the cap is the same patch capacity | Not fixed | Reduced in the plough run, with new faults |
| Launch on fill | Bounded by yield pressure plus ρv² | Bounded, if per-point area and deep normals behave | Moves the rigid ramp down, does not remove it | Any geometric residue costs a full rigid impulse |
| Launch on pieces and walls | Untouched; needs Law A | Untouched | Untouched | Untouched |
| Determinism | Server force pass on the committed surface; flow committed in body order (`bearing.rs:361`) | Server solver | Server | Server; order-sensitive cuts |
| Server cost | Removes vehicle-heightfield narrow-phase pairs; adds roughly 100 to 150 height lookups per tank over a tile (DERIVED, 0.5 m lattice on a 3.5 × 6.3 m underside plus nose) | About 6 fork lines, same pairs | Second surface per tile | Sweep and quadrature per body |

- **(a) is the one to build.** The cap is in the game's own force code, so plasticity is one `min`. The bearing law stays, as the flow rule for the pressure the contact already limited in the step.
- **(a) makes bookkeeping errors harmless.** Fill left inside the hull costs a bounded force and a visual overlap under the hull, not 10⁵ N·s.
- **(a) is stable (DERIVED).** Explicit forces need total stiffness under 4m/dt² = 934 MN/m.
- **(a) feels heavy for a physical reason.** A plastic set does not spring back, so a landing is a thud over tens of ticks, not a one-tick reversal.
- **(b) is second.** One clamp in the compliant arm is small, but the solver does not know a point's patch area. Normals still come from convex-against-heightfield manifolds at depth, which I do not trust.
- **(c) should be emergent.** In (a) the firm floor is the base terrain plus hardening as fill crushes. A separate rigid layer is rejected.
- **(d) is rejected.** The plough builder's own 1/e residual and creep barrier are symptoms of asking geometry to carry a force law.

## 3. Is the comparison fair?

The verdict (the bed drives worse) is fair. The obstacle is not equal.

**The bed's own faults:**
- Stop or proxy against field is the dominant contact in 8 of 9 corrected first passes (MEASURED).
- The long hangs: 11.9 s graded, 18.3 s coarse (MEASURED).
- The crossing takes 3.98 s against 1.13 to 1.17 s (MEASURED).

**Today's defects that the lab exposed:**
- The conveyor billed a tick late (from `ebbd3eea`, before the bed). The graded-lane tables predate that fix, so their longitudinal peaks include it.
- The friction launch off a loose wall assembly: 168.97 on the bed, 117.87 today (MEASURED).
- The ram stimulus: 94.2 up and 554.9 longitudinal, identical in all arms, against a standing wall (MEASURED).
- Oblique normals on flat ground, which supply 95.5 of 124.4 m/s² in today's game at tick 845 (MEASURED there).
  - Lead, not proven: the stop keeps the raw wrap vertices (`rig_geom.rs:619-626`, `:921-947` drop only exact collinears). The soft run had to drop sliver faces for that reason (`soft_run.rs:232-235`).

**Lab artefacts:**
- The prescribed 12 m/s entry exceeds the tank's measured 10.63 m/s top speed.
- One thin-walled house converts wholly to fill (options note P1). The bed arms climb a 0.8 to 1.0 m repose mound that today's game deletes.
- Today's own first-pass peak moved 32.49 to 57.09 between instrumented and filmed runs (MEASURED), so single-run gaps under about 40% are noise.
- Reversal counts have unequal windows. Per second they are 3.66 fixed against 1.79 to 3.72 today (DERIVED there), which overlap.

## 4. What has earned its keep

- **Shared height map (shape, sight lines, cover, late join): yes**, in the coarse form. The plough tables give 598 to 1,498 B/s coarse against 3,870 to 10,574 plain, 6.3 to 7.1 times (MEASURED there). I did not find the 11 times figure in what I read.
- **Fill under the belts: yes.** It is the calm channel and keeps the wheel motion.
- **Fill as a rigid collider for vehicles: no.**
- **Grading: partly.** Two solids are the hardening variable a plastic contact needs. The 0.25 m graded authority arm had the worst hangs and has not earned a second copy.
- **Ploughing: no.** It was rejected by its builder, and (a) removes its job. Its directional transport may stay as the flow rule's deposit direction.

## 5. Recommendation: continue the bed with sampled elastic-plastic fill support

**Law.** Per sample of area A on a vehicle solid below the committed surface by e:
- `F = A · min(k·e + c·ė, p_y + ρ·v_n²)` along the surface normal, with friction μF opposing slip.
- `p_y` reads the node's coarse and fines state.
- After the step, bearing lowers saturated nodes by `e − p_y/k`, moves that volume and firms it.
- All constants are material rows, Unknown until sourced. Nothing is per tank.

**Build outline** (line counts DERIVED from the named analogues):
1. **Filter.** Tiles get `MODIFY_CONTACTS`; `WorldContacts` drops manifolds between a `Layer::Vehicle` collider and a bed tile. About 25 lines; analogue `stop_contacts.rs:224-239`, 16 lines.
2. **Sampler.** A lattice on the stop outline and proxy undersides, built at spawn, reading `BedSurface::height_at`. About 120 lines; analogue `bearing.rs:17-76` plus `:386-402`.
3. **Law and emission** through the existing `ForceApp` path. About 90 lines; analogues `forces.rs:861-873` and `:1365-1445`.
4. **Flow rule.** Bearing reads saturated samples in place of solver presses for vehicles, and the whole-patch capacity goes for vehicle patches. About 80 lines changed; analogue `bearing.rs:572-596`.
5. **Tests.** About 200 lines: bounded force at any closing speed, no rebound after a plastic set, a crest yields until the belts carry, conservation.

Total about 515 lines.

**Delete:**
- `src/rubble_bed/displacement.rs` (902) and `displacement_adapter.rs` (129), less its transport function.
- The lab's plain arm (1,184 across four files).
- The 0.25 m authority copies of bearing, deposit and flow, once the coarse arm owns the shared surface seam.

**Cheapest experiment, before any of the law.** Build step 1 only, in the lab composition, so fill pushes the tank through belt columns alone.
- Run `passes` on the coarse arm at 8 m/s and 12 m/s entry, three runs each, against today's game.
- Prediction (DERIVED): fill-sourced upward acceleration at most 3.24 MN/m × 0.373 m / 57 t − g = 11.4 m/s².
- Watch for belts punching through. Probes buried past the 0.5 m reach carry nothing (`forces.rs:291`, test at `:2314`), and the stop then lands rigidly on base ground. Step 3 exists to catch that case.
- If this arm is not calmer than today on upward peak, longitudinal peak, reversals per second and hang, drop the bed for driving and keep it as shape only.
- If it is calmer, steps 2 to 4 can only add force up to the cap.

**Still needed either way.** Law A from the weak-debris design: the piece and wall launches are not fill.

## Not verified

- Every constant in the law; I recalled soil-mechanics forms and read no source.
- Whether the belt columns alone are calmer than today's game. That is the experiment.
- The split of `displacement.rs` between sweep and transport; I read only its head.
- Parry's heightfield manifold behaviour at depth, and the sliver-face lead for oblique normals.
- That the track forces are explicit once per tick; I saw `apply_force_at_point` at `src/track/sim.rs:1746` but did not trace its schedule.
- Server cost of any option, and hosted two-core cost.
- How a rigid piece resting on fill behaves under a stop. It stays rigid on a rigid field and may need the options note's soft-bed contactor.

### Critical Files for Implementation
All at `lane/rubble-graded`:
- /Users/Yan/github/vikng-dev/overmatch/src/track/stop_contacts.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/forces.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_coarse/bearing.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/bump_stop.rs
