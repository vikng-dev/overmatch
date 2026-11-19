# Heavy vehicle on weak debris: strength-limited contact

All citations are `lane/rubble-graded` (`git show lane/rubble-graded:<path>`). Decision up front: build strength-limited contact as a **pre-solve yield** (the sweep extended to loose pieces, reusing the crumble law), not as a solver impulse clamp at crushing strength. Add two smaller laws beside it: the belt's hard-contact friction coupled to the drivetrain inside the tick, and a capacity cap on rigid contacts with the static bed. Nothing was built or run; every DERIVED number below is my arithmetic from data and the report.

## 1. What pushes on the hull

- **Stop** (`src/track/bump_stop.rs:3-12`, spawn near `:213-246`): one convex prism per side on `Layer::Vehicle`, the belt's outline at full bump travel (`bump_stop: 0.200` m, `assets/tiger_1/tiger_1.tank.ron:171`). It is a plain rigid Avian collider carrying a `BeltFace`, so its surface moves at belt speed.
- **Side plates**: thin slices on `Layer::TrackSide`; only lateral contacts survive the hook (`src/track/stop_contacts.rs:76-82, 205-214`).
- **Body proxies**: `Hull_Collider` and `Turret_Collider`, substance RHA (`tiger_1.tank.ron:~317`). There is no separate belly collider; the lab's "body contact proxy" label is any other `Layer::Vehicle` collider (`src/rubble_lab/jolts.rs:126-130`).
- **Soft run** (`src/track/soft_run.rs:1-36`): the belt against loose pieces, through the compliant-contact seam (`src/avian_ext/compliance.rs`). Terrain and standing matter reach the belt through the sampled support columns (`src/track/forces.rs`).

| Path | Bounded by the struck matter's strength? |
|---|---|
| Sampled support columns, soft run | No strength term, but a calibrated spring-damper (`SupportLaw`), so it cannot spike. This is the wheel oscillation the owner likes. |
| Sweep on **standing** matter (`src/construction/sweep.rs`, `breakage/sweep_intake.rs`) | Yes. Pre-solve minimum cut over joint capacities against Hertz peak plus drive (`.agents/docs/design/construction-destruction.md` §"Bodies in contact", from line 349). |
| Crumble (`src/debris/crumble.rs:1-20`) | Yes, but after the solve. It reads the step's solver impulses, splits by joint capacity, and spends at most `½ΣJv` on fracture work. A hull counts as `Contactor::RIGID` (`src/contact_law.rs:52-57`). |
| Stops, plates, proxies against **loose pieces** | No. Plain rigid contact with Coulomb friction. |
| Stops, proxies against the **static bed** (`FieldSurface`) | No. Rigid; the bearing law lowers fill only afterwards (`src/rubble_bed/bearing.rs:550-566`). |

## 2. Why nothing relieved the loose assembly first

- **The sweep targets standing matter only.** Its owners are `Without<Piece>` (`sweep_intake.rs:55, ~247`). A piece can be a sweep source (`:349-364`) but is never a target, so "the contact was with a stop rather than the swept volume" is not the cause.
- **Crumble is post-solve.** It runs in `LandingSet`, reads `ContactTransfers` of a finished step (`crumble.rs` `crumble_landings`, plugin at the file's end), and skips a group that already holds a `PendingSplit`. The solver had already paid the impulse.
- **A pusher reaches matter behind a piece one step late.** `pushes()` (`sweep_intake.rs:~300-330`) estimates the press from the last step's contacts, and the design doc names it a StandIn with a one-step lag. A newly loose piece has no last step.
- **The solver has no strength term.** The normal impulse is clamped only at zero from below (`vendor/avian3d-0.8.0-overmatch/src/dynamics/solver/contact/normal_part.rs:125-187`).

The assembly was backed by something, which I could not identify (DERIVED):
- Friction is clamped at `μ·N` (`tangent_part.rs:157-245`). With pair μ = √(0.6·0.65) = 0.6245, the corrected run's +169,195 N·s tangential needs a normal impulse of at least 270,900 N·s, a mean of 17.3 MN over one 1/64 s tick.
- Bringing a free 3,000 kg body to 12 m/s takes 36,000 N·s, about 13% of that. The rest passed through the piece into a backing.

## 3. Friction and restitution

- **Friction** is one coefficient per substance, combined by geometric mean (`src/substances.rs:52-58, 68-77`). All three values are labelled `Unknown`, no source read:
  - TrackSteel 0.6 (stops, plates, runs): `assets/materials/materials.ron:276`.
  - RHA 0.5 (proxies): `:152`.
  - Brick 0.65: `:624`.
  - Pairs (DERIVED): stop on brick 0.6245, proxy on brick 0.570.
- **On a `FieldSurface`** the hook overwrites manifold friction with the ground's grip (`stop_contacts.rs:218-236`).
- **Restitution**: no game code sets it, so Avian's default 0.0 applies (`vendor/.../physics_material.rs:324-331`). The bounces are not restitution.

**The coefficient is doing two jobs on a stop.** It is the sliding resistance of steel on brick, and, through `tangent_velocity`, it is the track's traction against any face.
- The belt speed is written into the constraint every substep from the drivetrain's own advance (`src/track/sim.rs:1293-1312`, `stop_contacts.rs:~393`).
- The friction the solver delivers returns to the transmission only on the next driving tick (`stop_contacts.rs:1-2`).
- So within a tick the stop face is a conveyor of unlimited momentum. DERIVED from fixture data:
  - One belt is 97 × 30 kg = 2,910 kg, holding 34,900 N·s and 210 kJ at 12 m/s.
  - The tick delivered 169,195 N·s along it and raised the hull's vertical kinetic energy by 339 kJ.

## 4. Masses and strengths (data)

| Quantity | Value | Source |
|---|---|---|
| Brick unit crushing strength | 36.5 MPa | `materials.ron` Brick `bulk.blast.compressive_strength`, Measured |
| Joint tensile strength | 0.69 MPa | same row, `tensile` |
| Joint shear cohesion | 0.165 MPa | `shear_cohesion` |
| Fracture energy, mode I / II | 10 / 28.4 J/m² | `fracture_energy`, `shear_fracture_energy` |
| Brick density | 1,845 kg/m³ | `density` |
| Design chunk | 0.014625 m³, about 0.245 m | `assets/ballistics/physical_laws.ron` `coarse_grain_m` label |
| Wall thickness, house_1 | 0.25 m | element hulls x 1.75 to 2.0, `house_1.construction.ron` |
| Smallest server body | 0.5 m cube root of volume | `server_rubble_min_size_m`, Taste |
| Fixture hull mass | 57,000 kg | `tiger_1.tank.ron:5`, example only |

DERIVED per 0.25 m joint face (0.0625 m²):
- Tension 43 kN, shear cohesion 10 kN, crushing 2.28 MN.
- Crack work 0.6 J (tension) or 1.8 J (shear).
- A chunk is about 27 kg; the 3,000 kg assembly is about 1.63 m³, roughly 110 chunks.

Loose brickwork therefore resists a hull by its inertia and its joints (tens of kN, about a joule each), and by crushing only when confined.

## 5. Design

### Judging strength-limited contact honestly

The principle is right. As a per-contact impulse clamp at `σ_c × patch area` it does not work (DERIVED, patch area assumed):
- A stop on a 0.2 m² patch is allowed 7.3 MN, so friction is still allowed 4.6 MN, about 80 m/s² on the fixture. Brick is strong in compression; the clamp binds far too high.
- A clamp leaves the hull overlapping matter that nothing has removed.

What yields in truth is the **assembly**: its joints, or the confined chunk. The game already has that law twice, as the sweep's minimum cut and as crumble's field solve. The defect is that for loose pieces it runs after the solver.

### Law A: matter in a body's swept path is judged before the solve, loose or standing

- **Form**: extend the sweep's targets to `Piece` bodies. Cast in the piece's frame using relative rigid motion.
- **Demand**: the sweep's own, `P(m_eff, v_n) + drive` (`contact_law.rs:76-84`), with `m_eff` the two-body reduced mass.
- **Verdict**: `crumble()` unchanged (`crumble.rs` `crumble`), with the predicted touch at the witness chunk and energy `½ m_red v_n²`.
- **Routing**: `route` as today. Fragments under 0.5 m become dust and `Shed`, which the bed arms deposit; larger fragments stay bodies via `PendingSplit`.
- **Confined chunks**: a chunk occupied by the hull and pressed against non-moving matter reads bulk capacity `σ_c·A`, exactly as a ground endpoint does in the cut.
- **Exchange**: the source exchanges momentum once with what it freed, as the sweep does now.
- **Inputs**: all in data. Nothing per tank, no object kind; pieces striking pieces are judged the same way.
- **Belts stay out of the demand**, as they are today, so wheel travel is untouched.
- **Hooks**: none needed. `CollisionHooks::modify_contacts` (`vendor/.../collision/hooks.rs:187`) can edit friction, restitution, tangent velocity and points, but not impulse limits, and the game already uses its one hook set (`stop_contacts.rs:163-165`).

### Law B: a track gives no more than its drive holds

- Feed each substep's hard-contact friction increment along belt travel into that owner's next substep drivetrain advance. The advance already runs per substep (`sim.rs:~1255-1288`).
- Delete the next-tick `HardReaction` path so one path remains.
- The lag falls from one tick to one substep. Belt speed collapses against a face it cannot move, the conveyor stalls, and friction then opposes the hull's rise.
- μ returns to one job. No fork change.
- Honest bound (DERIVED): one belt's full momentum is still 0.6 m/s of hull heave if dumped in one tick, so B alone does not fix the launch. It bounds the cases A cannot touch.

### Law C: the bed's reaction is its bearing capacity in the same tick

- The bed already states perfectly plastic give, `dz = (V − Q)dt²/m_eff` (`bearing.rs:563-566`), but the rigid contact lets `V` be whatever the impact needs.
- Add `limit` (N) to `ContactCompliance` and one clamp in the fork's existing compliant arm. The fork's "Compliant contact" hook is 25 lines today (`OVERMATCH_PATCH.md:590-624`, MEASURED there).
- A bed writer sets the law for stop and proxy manifolds on a `FieldSurface`, with `limit` from that patch's `Shape::capacity`.
- This is a divergence at an existing seam, not a new one.
- I rejected emulating the cap with the existing spring (large `rest`, small stiffness), because it would push across a gap.

### The three measured cases

| Case | Effect |
|---|---|
| Friction launch off the loose assembly | A shatters the struck chunks before the solve. Each struck chunk can drag its neighbours with about 0.1 to 0.2 MN (DERIVED). With a normal of 0.5 MN, friction lifts at about 5.5 m/s² instead of 169. B caps what a face that does not break can return. |
| Bed facets and relief | C binds. MEASURED 104.97 m/s² is 5.98 MN mean (DERIVED). A stop-sized patch of loose fill carries tens of kN (recalled bearing factor, not computed from `Shape::capacity`). Stops plough until belts and broad patches carry the hull. The hangs (4.76 MN belly capacity against 559 kN load, MEASURED) are a different defect and C does not fix them. |
| Landings | C softens landings on the bed. Landings on rigid ground, and the oblique flat-ground normals, stay as they are; that open fault is separate. |

### What must survive

- **A 50 cm stone lifts the tracks.** A single 0.5 m block has no joints. Half the fixture's weight on 0.05 m² is 5.6 MPa, under 36.5 MPa, so it holds and the belt rides it through the soft run.
- **Wheel travel.** MEASURED 0.075 to 0.099 m RMS and about 0.35 m peak to peak across arms; A and C do not touch the belt channels.
- **Real obstacles.** A thick standing wall, a permanent core and the ground stay rigid. The cut holds, and B lets the tracks stall or climb against them believably.
- **Believable range.** Stopped or launched by a loose panel is absurd. Passing a thick wall or core without cost is absurd. Between them: slowing, a modest nose rise, bricks thrown.

### Determinism and cost

- Server only. Splits travel by the existing `Record`, `PendingSplit` and `Shed`; commits stay in entity order; clients predict nothing.
- A adds piece targets to casts near awake movers, plus one crumble solve per struck piece per tick, owner-parallel.
- A cheap precheck skips a piece when its demand share is under its weakest joint.
- Hosted two-core cost is unmeasured.

### Rejected alternatives

- Clamping hull acceleration or raising hull damping: an outcome rule with no law, and it would mute real obstacles.
- Debris massless to the hull: removes the stone lift and the lean on a heap.
- A clamp at crushing strength alone: shown too weak above.
- Lower friction on steep faces, or a per-object rule: a rule for one kind of thing.
- Removing surface velocity from stops: the track would no longer climb.

## 6. Build outline (line counts DERIVED from analogous code)

1. **Law A**, about 260 source and 150 test lines.
   - About 120 for piece targets and relative motion in the intake.
   - About 50 for the predicted touch.
   - About 40 for the exchange.
   - About 50 net to share the commit half of `crumble_landings` (about 135 lines today).
2. **Law B**, about 70 new lines, about 60 removed with the next-tick path, about 80 test lines. The analogues are `by_owner` and `reactions`, about 110 lines.
3. **Law C**, about 6 fork lines, about 20 in `compliance.rs`, about 60 for the bed writer, about 60 test lines.

Order: A, then B, then C; each is independent.

### Cheapest experiment

The launch reproduces exactly (MEASURED twice, tick 683), so one run per arm is enough: `scripts/rubble-lab run burst <out> --arm plain`.

- **E0, no build**: from the existing `jolts.json` at tick 683, list every contact of body `3955v0` and the hull's horizontal velocity change. This settles what backed the assembly.
- **E1, one lab line**: return zero belt speed for stop faces. This splits conveyor friction from plain sliding and sizes Law B.
- **E2, about 40 lab lines**: part every joint of any piece inside the hull's swept bounds and route it. This is Law A's upper bound.

The lane deletes both arms when it reports. Compare before and after:

| Number | Today's value (MEASURED) |
|---|---|
| Peak upward acceleration, plain burst | 168.97 m/s² |
| Peak upward acceleration, plain passes | 100.34 m/s² |
| Hull vertical speed across tick 683 | 0.936 to 3.576 m/s |
| Tangential / normal upward impulse | +169,195 / −9,048 N·s |
| Hull vertical reversals on the fast pass | 45 on the bed, 4 in today's game |
| Hang, plain burst | 1.469 s |
| Wheel RMS / peak to peak, plain burst | 0.075 / 0.356 m (should hold) |
| Sweep and crumble stage cost | not yet measured |

## 7. Not verified

- What backed the assembly at tick 683, and whether the tangential sum covers that one contact only.
- The substep count in play; I found no game-side setting and did not read Avian's default.
- Whether `LawSet` runs before or after the physics step within the tick.
- The shoe width and the true patch area; 0.2 m² is assumed.
- The reflected drivetrain inertia, and the stability of substep feedback in Law B.
- The small-patch bed capacity; it rests on a recalled bearing factor.
- The Hertz peak for a 3,000 kg piece at 12 m/s, about 20 MN, used a contact modulus I assumed from the row's Young's modulus and Poisson ratio 0.25.
- Whether a piece's `Geometry` can be cast in a moving frame without new baking.

### Critical Files for Implementation
- /Users/Yan/github/vikng-dev/overmatch/src/construction/breakage/sweep_intake.rs
- /Users/Yan/github/vikng-dev/overmatch/src/debris/crumble.rs
- /Users/Yan/github/vikng-dev/overmatch/src/track/stop_contacts.rs
- /Users/Yan/github/vikng-dev/overmatch/src/avian_ext/compliance.rs
- /Users/Yan/github/vikng-dev/overmatch/src/rubble_bed/bearing.rs
