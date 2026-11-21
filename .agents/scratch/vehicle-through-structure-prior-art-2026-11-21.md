# Prior art: a heavy vehicle through a destructible structure (2026-11-21)

Subject: game physics only. How shipped games and physics middleware let a fast heavy rigid body
pass through a structure of bonded pieces without the solver stopping or bouncing it, and without
one contact collapsing the structure. Read-only research; nothing here is measured on our build
unless it says MEASURED. Labels: CONFIRMED (read at the owning source this session), SECONDARY
(press or third-party summary), UNCONFIRMED (not reached at a primary source), DERIVED (my
inference from confirmed facts).

## Summary (one page)

1. **The industry's answer to "the vehicle is stopped" is a bound on what a breakable piece may
   push back with, not a better prediction.** Four independent sources ship or document the same
   law: a breakable body's contact impulse is capped at its break strength inside the solve, so
   the striker keeps the remainder. Smash Hit's engine (Gustafsson 2014), NVIDIA Blast's
   `Kinematics Max Contact Impulse` ("set it somewhere around Hardness * Health"), Havok's
   break-off-parts utility (`m_maxImpulse` per piece) and PhysX `PxContactSet::setMaxImpulse`.
   Stock engines that decide the break after the solve (Bullet's fracture demo, Box2D's
   `Breakable` sample, Chaos collision strain) all show the stop-then-split artefact and patch it
   by restoring or carrying the pre-impact velocity.
2. **"Break before contact" alone has a known hole**: whatever the prediction misses meets the
   solver as rigid, unbounded static matter. Our own hook keeps every point with
   `penetration >= 0` (`src/construction/breakage/sweep_intake.rs:112`), and Avian's normal solve
   has no upper clamp (`normal_part.rs:179`, lower clamp at zero only) plus an overlap push-out
   of up to 4 m/s (`plugin.rs:595`). DERIVED: a hull that starts a tick overlapping unfreed
   standing cells is stopped by the velocity constraint and pushed back by the bias. That matches
   the shape of the first measurement (stopped, then −1.2 m/s); it is a hypothesis to measure,
   not a finding.
3. **Structures survive one hit in shipped systems by separating two loads**: impact damage is
   local (radius or graph-distance bounded, capped per event) and the support check runs on
   weight. Blast states the split outright: stress is "most common[ly] just applying gravity
   force on a static construction", and passing impact impulses into the stress graph is an
   alternative to impact damage, "not recommended to use them both". Our weights-only support
   check is the documented mainstream form.
4. **Avian 0.8.0 (our fork) has**: contact removal and edit hooks, pair filtering, dominance,
   layers, sensors, swept CCD, opt-in speculative CCD, shape casts; and the fork's own compliant
   spring law per contact point. **It does not have** a per-contact maximum impulse or a per-pair
   mass scale. The cap law needs either a one-line fork hook beside the existing compliant hook
   or a game-side approximation.
5. **Recommendation**: complete the sweep with (c) the reaction bound, stated as a law with no new
   number (a standing cell returns at most the hold the sweep already computes), and (b) the
   same-tick consequence (a cell whose bound binds has failed, and its neighbours are judged
   again in the tick, bounded rounds). Decline one-way/dominance for the vehicle, velocity
   restore, non-colliding debris as a rule, and tuned softness on fresh debris.

---

## 1. The standard patterns

### (a) Break before contact (predictive sweep, trace, CCD-time fracture)

- **Who**: Epic documents it as the cheap path: "Trace and External Strain ... Inexpensive method
  of forcing a break", works "with no projectile at all", paired with `Apply Breaking Linear
  Velocity`; and "The best solution is to combine these two methods" (collision plus external
  strain). CONFIRMED,
  https://dev.epicgames.com/documentation/unreal-engine/chaos-destruction-optimization?application_version=5.6
  (sections "Trace and External Strain", "Collision & External Strain"). The same page shows the
  failure it answers: early shots rebound from the collection until strain and velocity are
  raised.
- **How it avoids the stop**: the collider is gone before the narrow phase sees it.
- **Cost**: a sweep per mover per tick; a law for what is freed.
- **Failure look**: anything the prediction misses is a rigid wall (our case); anything it
  over-predicts is a hole that opens ahead of the vehicle with no touch.
- **Determinism, server**: good. It is ordinary game code before the step.
- No shipped source was found that uses break-before-contact alone for vehicles. UNCONFIRMED for
  Teardown, Battlefield and The Finals: no primary source on their vehicle-versus-structure
  ordering was reached (Teardown's BSC 2025 solver talk says breakage is an outcome of the physics
  step, SECONDARY, transcript notes of https://youtu.be/Kvsvd67XUKw).

### (b) Break inside the solve, then re-solve or carry the momentum

- **Smash Hit engine** (Dennis Gustafsson, later Teardown): with a stock engine the break is
  decided from impulses after the step, which gives "the impression of unbreakable objects
  colliding and then artificially split up afterwards." His pipeline: collide; solve with impulse
  caps on breakables; if a contact hit its cap, break, collide the new pieces, re-solve, loop;
  then integrate. He "capped the number of solves per simulation step to three", and concludes
  destruction "ties very deeply into the simulation pipeline, so it's not ideal to plaster it on
  top of an existing physics engine." CONFIRMED,
  https://blog.voxagon.se/2014/05/13/cracking-destruction.html
- **Bullet fracture demo**: `solveConstraints` runs the solver once and then `fractureCallback()`;
  the author's own note: "todo: after fracture we should run the solver again for better realism
  ... save all velocities and if one or more objects fracture: 1) revert all velocties 2) apply
  impulses for the fracture bodies at the contact locations 3) and run the constaint solver
  again". Not implemented. CONFIRMED,
  https://github.com/bulletphysics/bullet3/blob/master/examples/FractureDemo/btFractureDynamicsWorld.cpp
  (`solveConstraints`).
- **Box2D `Breakable` sample** (Erin Catto): break flagged in `PostSolve` from the max normal
  impulse, applied next step, with the velocity cached before the step restored onto the pieces
  ("Cache velocities to improve movement on breakage"). CONFIRMED,
  https://github.com/erincatto/box2d/blob/v2.4.1/testbed/tests/breakable.cpp
- **Havok break-off parts**: the demo's callback receives a `ContactImpulseLimitBreachedEvent`,
  and re-applies `m_maxImpulse` along the normal to the new piece because "if the original
  unbroken body was fixed, the colliding impulse is lost". SECONDARY (demo code seen through a
  code-example mirror, https://www.itcodet.com/cpp/cpp-hkpphysicssystem-class-examples.html); the
  utility's header was not reached, UNCONFIRMED.
- **Academic**: "Energized Rigid Body Fracture" (Li, Andrews, Jones, Bargteil, I3D 2018) breaks a
  constraint "whenever the impulses at a previous time step exceed a user-defined threshold" and
  then adds velocity from stored constraint energy scaled by a user α: one tick late by design,
  and about the fragments' liveliness, not the striker. CONFIRMED,
  http://profs.etsmtl.ca/sandrews/pdf/rigidBodyFracture_I3D2018_preprint.pdf (section 3.5). No
  paper was found that states the striker-bounce problem and solves it by time-of-impact
  re-simulation; Müller, Chentanez and Kim 2013 could not be fetched. UNCONFIRMED.
- **How it avoids the stop**: the striker only ever pays the capped impulse; the re-solve lets
  the pieces behind be judged in the same step.
- **Cost**: up to N solves of the affected contacts per step; needs the solver's cooperation.
- **Failure look**: at the solve cap, the last unbroken layer is a wall for one tick.
- **Determinism, server**: deterministic if the loop bound and order are fixed.

### (c) Contact modification: cap the impulse a breakable may return

- **PhysX**: `PxContactSet::setMaxImpulse()` lets you "Limit the maximum impulse applied at each
  contact", listed for soft contact effects and for limiting the impulse from a kinematic
  collision; the stated cost is that it "can potentially lead to additional penetration and bodies
  passing through each-other." CONFIRMED,
  https://nvidia-omniverse.github.io/PhysX/physx/5.4.1/docs/AdvancedCollisionDetection.html
  (section "Contact Modification").
- **Blast (UE4 plugin)**: `Kinematics Max Contact Impulse`: "Max contact impulse on kinematic (say
  bound to world) actors", "set it somewhere around Hardness * Health", "Works only on kinematic
  actors". CONFIRMED,
  https://archive.docs.nvidia.com/gameworks/content/gameworkslibrary/blast/1.1/authoring_docs/BlastUe4_BlastSettings.html
  (Impact Damage Properties). The purpose sentence (a heavy projectile carries on instead of
  bouncing; too low a cap lets projectiles slip through an unfractured destructible) was seen only
  in a search summary of the same page: SECONDARY.
- **Smash Hit**: the same cap, used with (b). CONFIRMED (above).
- **How it avoids the stop**: the wall becomes a brake of known force. The vehicle loses at most
  `cap` per contact per step; it is stopped only if the summed caps over the distance exceed what
  it carries, which is the outcome a wall that strong should produce.
- **Cost**: needs a per-contact clamp in the solver. The vehicle may overlap matter that has not
  been removed yet (PhysX's stated caveat), so the removal must follow in the same or next tick.
- **Failure look**: cap too low reads as a ghost wall; cap with no removal reads as wading through
  solid matter.
- **Determinism, server**: good; one clamp, no ordering.

### (d) One-way interaction, mass or dominance scaling

- **Chaos**: "One-way objects do not further the destruction (no strain on collision)"; one-way
  bodies have no effect on two-way bodies; a per-component level picks where pieces become
  one-way. Epic frames it as a debris and performance feature. CONFIRMED (optimization page,
  section "One-Way Interaction"). Epic's tutorial calls it the "Debris System since Unreal 5.4".
  SECONDARY, https://dev.epicgames.com/community/learning/tutorials/KWeX/unreal-engine-a-tech-artist-s-playbook-for-chaos-performance
- **PhysX**: `setInvMassScale0/1`: "inverse mass/inertia scale of 0 results in the body behaving
  as if it has infinite mass", per pair, in the modify callback. CONFIRMED (same PhysX page).
- **Avian**: `Dominance` (section 4).
- **How it avoids the stop**: the vehicle is infinite mass to the debris; debris cannot push it.
- **Cost**: none.
- **Failure look**: the vehicle loses no speed to anything dominated; debris pinned between the
  vehicle and a static wall has no solution, so the solver either tunnels it or jams (DERIVED:
  an infinite-mass body against a static body through a finite one is an over-constrained chain).
- **Determinism, server**: good.

### (e) Kinematic or trigger-volume cutters

- **Smash Hit**: "objects always break where they get hit": a small volume around the impact is
  carved by five randomized planes and split. CONFIRMED (Gustafsson 2014). It is a carve on
  contact, not a carried volume.
- **Chaos fields / external strain** applied from gameplay volumes: CONFIRMED as a mechanism
  (destruction overview: connections break when "collision constraints or field evaluations apply
  an impulse on the rigid body that exceeds the connections limit",
  https://dev.epicgames.com/documentation/en-us/unreal-engine/destruction-overview). A vehicle
  that carries such a field as shipped practice is UNCONFIRMED (forum lore only).
- **How it avoids the stop**: everything in the volume is debris before contact; the criterion is
  geometric.
- **Cost**: trivial.
- **Failure look**: strength and speed do not matter: a crawl opens the same hole as a charge,
  and a bunker opens like a shed.
- Our sweep is this pattern with a law in place of the geometric test.

### (f) Soft or decaying contacts for fresh debris

- PhysX names `setMaxImpulse` for "soft" contact effects (above). Embark's 2026 dev diary adds a
  layer in which freshly detached pieces stay loosely tethered ("pieces hinge, swing, twist, and
  bend") rather than dropping at once. SECONDARY,
  https://www.trueachievements.com/news/the-finals-destruction-overhaul-prime-time (the diary
  video itself was not read).
- **How it avoids the stop**: fresh debris cannot deliver a rigid impulse while it is still
  inside the hull's path.
- **Cost, failure look**: a softness number and a decay time with no physical owner; visible
  interpenetration.

### (g) Debris that does not collide with the vehicle, or only for a time

- **Instruments of Destruction**: debris collides with the player vehicle, structures and ground,
  and not with blocks or other debris; debris has no HP and is removed by timer or a global cap.
  SECONDARY (search summary of the developer's Steam post,
  https://steamcommunity.com/games/1428100/announcements/detail/3088910094668504531; two direct
  fetches failed).
- **Chaos**: one-way debris plus `Remove on Sleep` / `Remove on Break` timers. CONFIRMED
  (optimization page, "Removal Settings").
- **Blast**: `Debris Depth`, `Debris Lifetime Min/Max`. CONFIRMED (Blast settings page).
- **How it avoids the stop**: the pinned-debris case cannot occur.
- **Failure look**: rubble the vehicle passes through; no rubble pile to climb.

## 2. What stops the vehicle after the first cells are freed, and the published remedies

| Cause | Source that names it | Published remedy |
|---|---|---|
| Break decided after the solve: the striker has already been resolved against the intact body | Gustafsson 2014; Bullet's todo; Box2D sample | cap in the solve and re-solve (Smash Hit); revert velocities and re-solve (Bullet, unimplemented); restore cached velocity (Box2D) |
| Unbroken matter returns unlimited impulse | Blast setting, PhysX `setMaxImpulse`, Havok `m_maxImpulse` | cap at break strength; the piece that hit its cap is the piece that breaks |
| Fixed parent swallows the impulse, so the new piece starts at rest in the striker's path | Havok demo comment ("the colliding impulse is lost") | apply the capped impulse to the new piece |
| Fresh debris between striker and standing structure | not stated directly by any primary source found; DERIVED from (d) and (g) existing | one-way debris (Chaos), debris filtered from blocks (Instruments of Destruction), re-solve so the layer behind is judged in the step (Smash Hit) |
| Limited break budget per step delays breaks | Chaos cvars `p.Chaos.Clustering.PerAdvanceBreaksAllowed`, `...RescheduleLimit` (SECONDARY, Epic tutorial) | raise the budget; Smash Hit accepts a cap of three solves |
| Penetration left by a capped or late break is pushed out | PhysX caveat on `setMaxImpulse`; Bullet demo enables split impulse because "deep penetrations could otherwise trigger fractures" (SECONDARY summary of the demo) | keep position correction out of the velocity (split impulse); remove the collider in the same step |

For our build, the code facts behind the third and last rows: a freed chunk's contact hull stands
`LOOSE_GAP_M` = 1e-3 m off the faces it shared (`src/construction/breakage.rs:2598`), and the hook
treats points inside Avian's contact tolerance as look-ahead for exactly that reason
(`src/construction/breakage/sweep_intake.rs:31-41`).

## 3. How structures avoid total collapse from one hit

- **Stress from weight, impact kept separate (Blast)**: `Calculate Stress`: "every frame stress
  will be calculated and overstressed bonds would be broken"; "The most common usage is just
  applying gravity force on a static construction"; `Apply Impact Impulses` passes impacts into
  the stress graph as an alternative to impact damage, "it is not recommended to use them both
  turned on." CONFIRMED (Blast settings page, Stress Properties). Newer Omniverse Blast makes all
  damage stress-based, in units of acceleration at the support chunk (SECONDARY, changelog summary,
  https://docs.omniverse.nvidia.com/kit/docs/omni.blast/latest/CHANGELOG.html).
- **Local damage region bounded by radius (Blast)**: "Damage = Impulse / Hardness. This damage is
  capped by the material's health." "The damage radius is given by MaxDamageRadius multiplied by
  the ratio of damage to material health"; outside it, damage reaches `R * DamageFalloffRadiusFactor`,
  "linearly attenuated down to zero at the outer radius"; `Max Damage Threshold` caps the health
  one event may take ("0.8 won't allow more then 80%"). CONFIRMED (same page). So the radius is
  chosen from the excess of the impulse over the material's strength, with an authored maximum and
  a linear falloff. The Blast artist tool's radial profile is full damage inside a minimum radius
  and linear to zero at a maximum; its "Impact Spread" variant bounds by support-graph distance
  instead of Euclidean radius (SECONDARY,
  https://docs.nvidia.com/gameworks/content/gameworkslibrary/blast/1.1/authoring_docs/BlastTool_DamageSettings.html).
- **Propagation factors (Chaos)**: `Break Damage Propagation` is "a factor of the remaining strain
  that propagates to the connected pieces" (a wall of 100 hit by 200 leaves 100 to carry on);
  `Shock Damage Propagation` is "a factor of the original impact strain propagated to connected
  pieces". One-way debris adds no strain. CONFIRMED (optimization page). Clusters: a connection
  graph of nearest neighbours, each connection "a rigid constraint within the cluster ... given
  initial strain values". CONFIRMED (destruction overview). Per-level damage thresholds, anchor
  fields and kinematic anchoring are real features but were not on the pages read: UNCONFIRMED
  here.
- **Carve only where hit (Smash Hit)**: a small volume at the impact point; connectivity is then
  recomputed statelessly by a touching test. CONFIRMED (Gustafsson 2014).
- **Threshold against resting load (Bullet)**: "some threshold otherwise resting contact would
  break objects after a while". CONFIRMED (fracture demo, `fractureCallback`).
- **Red Faction: Guerrilla**: the talk exists (Eric Arnold, "Living in a Stressful World: Real-time
  Stress Calculation for Destroyable Environments", GDC 2011,
  https://www.gdcvault.com/play/1014658/Living-in-a-Stressful-World); its content was not
  reachable. That its stress pass reads weight carried by the pieces beneath is SECONDARY
  (https://thisweekinbevy.com/issue/2024-07-15-component-hooks-quill-ui-demo-and-procedural-applications).
- **The Finals**: a "Strain" layer for building integrity and general physics for debris, all on
  the server, plus the 2026 tethered layer. SECONDARY (press reports of the dev diary; the
  "Meet The Makers: Movement" transcript for server authority).
- **Battlefield**: no primary source on the physics side was found (the public DICE talk is
  rendering: Kihl, SIGGRAPH 2010 destruction masking). UNCONFIRMED.

## 4. What the pinned engine supports (vendored Avian 0.8.0 fork, read at source)

Paths are under `/Users/Yan/github/vikng-dev/overmatch/vendor/avian3d-0.8.0-overmatch/src/`.

| Pattern | Native? | Where |
|---|---|---|
| Remove or edit contacts before the solve | yes | `CollisionHooks::modify_contacts`, `collision/hooks.rs:187`; called at `collision/narrow_phase/system_param.rs:928`. Returns `false` to drop the pair; points, normal, friction, restitution and `tangent_velocity` are editable (`collision/contact_types/mod.rs:356`, `:677`). "Impulses stored in `contacts` are from the previous physics tick" (`hooks.rs:181`). |
| Filter pairs in the broad phase | yes | `filter_pairs`, `hooks.rs:164`; opt in per collider with `ActiveCollisionHooks` (`hooks.rs:227`). |
| One hook set only | constraint | "Only one set of collision hooks can be defined per broad phase and narrow phase" (`hooks.rs:139`). Ours is `WorldContacts`, which chains the sweep's hook (`src/track/stop_contacts.rs:171-173`). |
| Per-contact maximum impulse | **no** | `ContactManifold` and `ContactPoint` have no such field; the solve clamps the accumulated impulse at zero from below only (`dynamics/solver/contact/normal_part.rs:179`). |
| Per-pair mass scale | **no** | not in `ContactPair` or `ContactConstraint`. |
| Dominance (body-wide one-way) | yes | `Dominance(i8)`, `dynamics/rigid_body/mod.rs:654`; the dominant body gets zero inverse mass and inertia in the constraint (`dynamics/solver/contact/mod.rs:145-162`). Body-wide, not per pair. |
| Layers, sensors, disabling | yes | `CollisionLayers` (`collision/collider/layers.rs:362`), `Sensor` (`collision/collider/mod.rs:484`), `ColliderDisabled` (`:449`), `RigidBodyDisabled` (`dynamics/rigid_body/mod.rs:381`). |
| Swept CCD | yes, automatic for fast bodies | `SweptCcd`, `dynamics/ccd/mod.rs:320`. It moves the body back to the time of impact and documents "time loss" (`ccd/mod.rs:101`): a second, separate way a fast hull is stopped at a wall. Whether a tank hull qualifies as fast (motion per step against half its minimum thickness) was not measured. |
| Speculative contacts | yes | default margin for all, velocity-expanded with `SpeculativeCcd` (`ccd/mod.rs:512`); solved as a hard stop at the gap (`normal_part.rs:142`). Our hook already removes these against unjudged matter (`src/construction/breakage/sweep_intake.rs:112`). |
| Overlap push-out speed | global | `SolverConfig::max_overlap_solve_speed`, default 4.0 m/s (`dynamics/solver/plugin.rs:549`, `:595`); applied as the bias at `normal_part.rs:167`. |
| Global contact softness | yes | `contact_damping_ratio`, `contact_frequency_factor` (`plugin.rs:526`, `:539`). Global, not per pair. |
| Per-point spring law | **fork** | `ContactNormalPart::{compliant, spring}` (`normal_part.rs:33-38`, `:137-150`), driven game-side by `/Users/Yan/github/vikng-dev/overmatch/src/avian_ext/compliance.rs`. A spring-damper, not a cap. |
| Predictive queries | yes | `SpatialQuery::cast_shape` (`spatial_query/system_param.rs:478`), `shape_intersections` (`:1245`). |

**Without patching the fork**: (a), (d) body-wide, (e), (g), contact removal, and a force-shaped
approximation of (c) and (f) through the existing compliant law. **Not without a patch**: a true
impulse cap (c), per-pair mass scaling, and a re-solve inside the step (b); (b) can be approximated
before the step by more sweep rounds, which the sweep already has (`DEPARTURE_ROUNDS`).

## 5. Recommendation

**Complete the sweep with the reaction bound, then its same-tick consequence.**

1. **Reaction bound (pattern c), as a law**: a standing cell is held to the structure by its
   joints, so the force it can return to any body along a direction is at most its hold along
   that direction, the directional minimum cut the sweep already computes. Per tick the contact
   impulse is bounded by `hold · dt`. This is Newton's third law plus the joint strength; it adds
   no number, and it has one owner (the sweep's hold). It turns every cell the prediction did not
   free from a rigid wall into a brake of known force, which is what Blast, Havok, PhysX and Smash
   Hit ship. The work the vehicle loses is `hold × depth`, the same quantity the sweep's demand
   test compares against, so the sweep becomes the predictive integral of the bound rather than a
   separate rule.
2. **A bound that binds is a failure (pattern b)**: a cell whose reaction reached its hold has
   failed by the same criterion the sweep uses, and is freed with the bounded impulse as its
   momentum (the Havok demo's rule: the impulse goes to the new piece, not into the fixed parent).
   The cells behind it are judged again in the tick, with a fixed round count (Smash Hit uses
   three). Freshly freed debris then cannot pin the hull: it transmits load to the cells behind,
   which are under the same bound.
3. **Engine form**: Avian has no impulse cap. The clean form is a narrow fork hook, an optional
   upper clamp next to the existing lower one at `normal_part.rs:179`, set game-side per manifold
   like the compliant law. It is a divergence, so it needs its seam file in `src/avian_ext/`;
   PhysX's `setMaxImpulse` is the upstream precedent to cite. The no-fork alternative is to drop
   the standing contact in the hook and apply the bounded reaction as part of the sweep's
   exchange; its defect is that a cell strong enough to hold must then be handed back to the
   solver by a second rule, which is two paths.
4. **Measure first**: split the 5.8 m/s one-tick change into its sources before building: solver
   normal impulse against unfreed cells, overlap push-out (bias up to 4 m/s), swept-CCD time
   loss, and the plastic exchange with freed cells. The bound fixes the first two; it does not fix
   an exchange that is itself too large.

**Decline, knowingly**:

- **Dominance or one-way interaction for the vehicle (d)**: the vehicle would lose no speed to a
  wall of any strength. It is a rule on a body type, and it removes the strength-versus-momentum
  outcome a player can perceive and act on.
- **Restoring the pre-impact velocity after a break (Box2D sample, Bullet's todo)**: it deletes
  the momentum the break cost. Our sweep's plastic exchange and the bound account for it.
- **Debris that does not collide with the vehicle (g) as a rule**: rubble is terrain here. The
  honest version is a resolution choice (fragments below what a player can perceive are shed as
  decoration, which the sweep already does), never a filter on the vehicle.
- **A geometric cutter volume (e)**: strength and speed stop mattering.
- **Tuned softness or decay on fresh debris (f)**: a constant standing in for the bound.
- **Raising the overlap push-out speed or softness globally** to hide the throw-back: it moves
  every other contact in the game.

## Gaps

Not reached at a primary source: Chaos engine source (strain evaluation order, per-level
thresholds, anchors), the Red Faction: Guerrilla talk's content, Havok's break-off-parts header,
Teardown's and The Finals' vehicle-versus-structure ordering, Battlefield's physics side, the
Instruments of Destruction post in full, and Müller et al. 2013. BeamNG and Besiege were not
researched (soft-body and joint-force models, a different problem).
