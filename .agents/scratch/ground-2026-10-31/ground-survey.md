# How tanks and loose bodies meet the ground: give and friction

Subject: the game's physics. The launched turret is a consequence in the game's damage model (an
ammo cook-off, game state only); nothing here concerns anything but game state.

Read-only survey, 2026-10-31, at `ddecd291` on `lane/atomic-crossing`. Avian paths are under
`vendor/avian3d-0.7.0-child-pose/src/` (the published 0.7.0 crate plus two change-tick patches that
do not touch materials or the solver, `OVERMATCH_PATCH.md`); parry paths under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/parry3d-0.27.0/src/`. Labels: MEASURED (read
off a file or asset by me), DERIVED (computed from cited inputs), ASSUMED, CLAIMED (someone's
statement, not checked), CITED (a value stated in source or a primary document).

## TLDR

1. The turret that never stops and the hull that slid off the rubble have one cause: every tank collision box is made frictionless on purpose (the track model owns all grip), and that zero beats every other material, including rubble the tracks cannot feel and a turret that is no longer on tracks.
2. The reverse "throw" most likely starts at the hull's blunt rear bottom edge, about 0.2 m above the ground, which meets the terrain as a rigid contact once the soft track suspension gives way. The nose has a 31° skid plate and the tail a near-vertical one, and the heightfield runs with its contact-normal fix for triangle edges switched off.
3. The ground already gives where the tracks touch it: the belt contact is a spring and damper with about 0.17 m of sag. What does not give is the hull's backstop contact. Avian 0.7 has no per-collider softness, only global solver settings, so a softer hull–ground contact means our own law or a global tweak.
4. Avian supports friction per material: a coefficient per collider, a combine rule per pair, and contact hooks for a full pair table. The cheap fixes are a "Max" friction rule on rubble and building parts, and removing the turret's zero friction when it launches.
5. Permanent ground deformation is possible but extra-large: a server-side rut and crater layer, a rebuilt collider, replication and a late-join snapshot. PRODUCT.md leaves it as an open fork, and none of the three symptoms needs it.

## 1. Our code today

### Materials per body

| Body | Collider(s) | Friction | Restitution | Where |
|---|---|---|---|---|
| Tank hull | `Hull_Collider` convex hull(s), child of the root, `Layer::Vehicle` vs ALL | `Friction::ZERO.with_combine_rule(CoefficientCombine::Min)`, CITED | none set → `DefaultRestitution` 0, Average | `src/tank/spawn.rs` `spawn_proxy_parts` (line 859); declared `colliders: ["Hull_Collider", "Turret_Collider"]` in `assets/tiger_1/tiger_1.tank.ron:330` |
| Turret, attached | `Turret_Collider`, same function | ZERO, Min | 0 | same |
| Turret, launched | the same `Turret_Collider`, re-rooted onto the freed turret body | **still ZERO, Min**: `launch_turrets_on_cookoff` inserts body type, mass, inertia, velocity, nothing about material | 0 | `src/damage.rs:611-683` |
| Tank armour and witness volumes | trimeshes | n/a: filter `LayerMask::NONE`, they contact nothing | n/a | `src/tank/spawn.rs:771-778` |
| Heightfield terrain | one `Collider::heightfield`, `RigidBody::Static`, `Layer::Terrain` | none → `DefaultFriction` 0.5, Average | 0 | `src/world.rs:589-595`, `src/terrain_grid.rs:872-884` |
| Authored blocks (test course, graybox buildings) | unit cuboids | none → 0.5 | 0 | `src/world.rs:809-826` |
| Building elements and standing chunks | convex prisms, `[Terrain, Armor]` vs ALL | none → 0.5 | 0 | `src/buildings.rs:155-176` |
| Loose wall pieces (`WallPiece` + `DebrisPiece`) | convex hulls, `[Debris, Armor]` vs `[Terrain, Vehicle, Debris]` | none → 0.5 | 0 | `src/buildings/breakage.rs:1076-1139` |
| Trees, scatter trunks | static | none → 0.5 | 0 | `src/trees.rs:865`, `src/scatter.rs:384,414` |

Nothing in `src/` sets `Restitution`, `DefaultFriction`, `DefaultRestitution`, `LinearDamping` or
`AngularDamping` (MEASURED: grep, test modules aside).

How Avian resolves a pair (CITED, `collision/narrow_phase/system_param.rs:604-629`): friction is the
collider's `Friction`, else its body's, else `DefaultFriction`; restitution is the collider's, else
`DefaultRestitution` (the body is **not** consulted for restitution). The two values combine under
the higher-priority rule, `Max > Multiply > Min > GeometricMean > Average`
(`dynamics/rigid_body/physics_material.rs:5-8,204-215`). Only `dynamic_coefficient` reaches the
solver (`system_param.rs:618`); the static coefficient is stored and never read.

So, DERIVED from the above: hull or turret against anything = `Min(0, x)` = **0**, whatever `x` is,
unless the other side uses `Max` or `Multiply` (none does). Everything else against everything else
= 0.5 friction, 0 restitution.

Why the zero exists (CLAIMED in the track-model log, consistent with code): the colliders "were
silently doing surface physics"; at a trench wall the collider's 0.5 friction dragged the hull down
while the belt ground up, the "wall-climb lock", fixed with the Min-rule zero so "the belt owns ALL
tangential physics" (`.agents/docs/design/track-model/HQ.md:557-566`; restated in
`architecture.md:62`).

### Solver configuration

Neither composition overrides any solver resource: `offline_physics()` is `PhysicsPlugins::default()`
(`src/lib.rs:983-985`), and the networked `physics_plugins()` only disables Avian's transform and
interpolation plugins (`src/net/physics.rs:31-36`). MEASURED: no `SubstepCount`, `SolverConfig`,
`NarrowPhaseConfig`, `SpeculativeMargin`, `CollisionMargin` or `SweptCcd` anywhere in `src/`.

| Setting | Value | Source |
|---|---|---|
| Physics step | 1/64 s | CITED: bevy_time 0.19.1 `DEFAULT_TIMESTEP` 15 625 µs (`fixed.rs:76`); net `tick_duration` (`src/net/client.rs:501`); Avian runs in `FixedPostUpdate` (`schedule/mod.rs:54`) |
| Substeps | 6 → h = 1/384 s | CITED default (`dynamics/solver/schedule.rs:187-191`); h DERIVED |
| `contact_damping_ratio` | 10 | CITED default (`dynamics/solver/plugin.rs:291-300`) |
| `contact_frequency_factor` | 1.5 | CITED, same |
| Contact frequency, dynamic–dynamic | 48 Hz | DERIVED: `1.5 × min(1/(2·dt), 0.25/h)` = 1.5 × min(32, 96) (`plugin.rs:326-350`) |
| Contact frequency, against static/kinematic | 96 Hz | DERIVED: 2 × the above (`plugin.rs:346-348`); tier chosen by relative dominance (`dynamics/solver/contact/mod.rs:150-154`) |
| `max_overlap_solve_speed` | 4 m/s | CITED default; the push-apart bias is `(bias_coef × separation).max(−4)` (`dynamics/solver/contact/normal_part.rs:147`) |
| Push-apart rate against static ground | 28 s⁻¹ × overlap, so the 4 m/s cap is hit at 0.14 m overlap | DERIVED: `ω/(2ζ + ωh)` with ω = 2π·96 (`dynamics/solver/softness_parameters/mod.rs:62-80`) |
| Relax pass | each substep re-solves without bias, so push-apart velocity is meant to be taken back out | CITED (`plugin.rs:56-62,131-136`) |
| `restitution_threshold` | 1 m/s; `restitution_iterations` 1 | CITED default |
| Speculative contacts | unbounded margin; contact tolerance 5 mm; margin from **linear** velocity only (dt × relative linear speed; angular velocity not included) | CITED (`collision/narrow_phase/mod.rs:249-256`; `system_param.rs:664-685`) |
| Contact matching / warm start | skipped when a pair has more than 4 manifolds | CITED (`system_param.rs:789`); a hull belly on the heightfield gets one manifold per touched triangle |
| Heightfield internal-edge fix | **off**: `Collider::heightfield` calls `SharedShape::heightfield`, which builds `HeightField::new` with empty flags | CITED (avian `collision/collider/parry/mod.rs:1128-1141`; parry `shape/heightfield3.rs:84-86`); `triangle_normal_constraints` returns `None` unless `FIX_INTERNAL_EDGES` is set (`heightfield3.rs:410-416`), and the convex-vs-heightfield manifold reads it (`query/contact_manifolds/contact_manifolds_heightfield_shape.rs:158`) |

For scale, DERIVED: against the ground, a 57 t body under a 96 Hz, ζ = 10 contact sees an
equivalent stiffness of order m·ω² ≈ 2×10¹⁰ N/m, four orders above the belt suspension's
≈ 3.2 MN/m (§4). Against terrain, the hull collider is effectively rigid.

### The tank's own ground contact (not Avian)

The belt force model is the tank's real ground contact: a penalty spring along each station's
inward normal, minus a normal-velocity damper, with a 20 mm engage ramp, probing the analytic
oracle, not Avian colliders (`src/track/forces.rs:11-26,768-781`). The stiffness is not authored:
`k = W / A₀`, the weight over the rest penetration area, so the full droop travel is the static sag;
damping is `2ζ√(k·m)` spread over the contact length (`src/track/envelope.rs:1-14,170-176`). The
probes saturate at `probe_reach` (0.5 m) plus local travel (`src/track/oracle.rs:30-33`;
`src/track/sim.rs:564`). Forces are computed once per tick and held for the whole step, all six
substeps (CITED: avian `dynamics/rigid_body/forces/mod.rs:98`; `src/track/sim.rs:870`).

## 2. Diagnosis of each symptom (no runs)

### 2a. Reversing downhill, the hull's bottom rear catches a bump and the tank is thrown up

The geometry, MEASURED from `assets/tiger_1/tiger_1.sim.glb` (hull frame, front is −z, the sprocket
end):

- `Hull_Collider` belly at y ≈ 0.288 m, matching the authored `Hull_Bottom` plate (0.281 m). The
  belly is 2.07 m wide (x ±1.034) and runs z −2.33 → +2.66.
- Its **tail is blunt**: the rear face rises from the belly edge at z 2.66 to z 2.77 over 0.82 m, so
  it is 7.7° off vertical (DERIVED). Its **nose is a skid**: the lower front face climbs 0.49 m over
  0.82 m, so it sits 31° off horizontal (DERIVED).
- The idler (unsprung, travel 0) has its centre at z 2.44, y 0.56, radius 0.34; the road-wheel
  centres sit at y 0.509 with a tyre radius of 0.379.
- Ground clearance under the belly at rest ≈ **0.2 m** (DERIVED: 0.288 − (0.509 − 0.379 −
  ~0.05 m ASSUMED plate)). The track-prints research ASSUMED 0.4–0.5 m
  (`.agents/scratch/impact-feedback-2026-10-28/track-prints-approaches.md`, §6); this model sits
  lower than that.

Ranked causes:

1. **Rigid, frictionless hull contact at the blunt rear edge, after the soft belt hands over (most
   likely).** The belt is a 1.2 Hz ride (CITED `tiger_1.tank.ron:216`), so its total vertical
   stiffness is m(2π·1.2)² ≈ 3.24 MN/m with a static sag of ≈ 0.17 m (DERIVED). At the idler the
   travel is zero, but the law keeps the same per-metre spring, so a bump under the rear run
   intrudes deep before it carries the rear's share of the weight (DERIVED from `forces.rs:768-781`
   and `envelope.rs:170-176`). Once the bump has risen ~0.2 m into the belt, the belly's rear edge
   meets it as a static-tier Avian contact, effectively rigid (§1). If it arrives through the
   speculative branch (gap > 0), it is solved fully rigid, with no softness and no speed cap
   (CITED `normal_part.rs:129-131`). Reversing, that edge leads, with a near-vertical face behind
   it, at up to 2.56 m/s on the flat in top reverse gear (DERIVED from 9.2 km/h, CITED
   `tiger_1.tank.ron:127`), and faster downhill.
   - An impulse that stops the tail at a point ~0.6–0.8 m below the centre of mass (CoM node at
     y 0.881, MEASURED) rotates the hull nose-up over its tail: the tank "trips". This is DERIVED
     from the lever arm alone; the drive force keeps pressing the tail into the bump.
   - Going forward, the 31° nose skid rides over the same bump, which matches "in reverse"
     (DERIVED from the geometry).
   - **Aggravator: uncorrected heightfield edge normals.** With `FIX_INTERNAL_EDGES` off (§1), a
     hull edge crossing a triangle edge of the 1.465 m grid (CITED `src/terrain_grid.rs:158`) can
     take that triangle's edge-derived normal, which may be steep or nearly horizontal: the classic
     "catch on a flat seam". Two sources describe the effect: parry's own flag doc
     (`heightfield3.rs:47-53`) and Jolt's architecture doc, which says a box sliding over an
     internal edge can "stop or jump up" (§5). Whether it happens here is CLAIMED, not measured.
2. **Overlap pushed out at up to 4 m/s.** If the tail does sink into the ground, the soft step
   pushes it back out at up to 4 m/s: a 4 m/s vertical launch rises 0.82 m (DERIVED, v²/2g). The
   relax pass is designed to remove that velocity again, so this ranks second. Deep overlap is
   plausible here because the speculative margin uses linear velocity only (`system_param.rs:684`),
   while the tail of a pitching hull moves faster than the centre of mass (2.7 m lever, MEASURED),
   and because pairs with more than 4 manifolds lose warm starting (`system_param.rs:789`).
3. **Belt forces (unlikely).** The support spring is soft and depth-bounded by the probe reach. The
   damper is ≈ 301 kN·s/m in total (DERIVED: 2·0.35·√(3.24×10⁶·57 000)), integrated explicitly at
   64 Hz with c·dt/m ≈ 0.08 (DERIVED), well inside stability. Both apply only over the length of
   belt in contact.
4. **Restitution: ruled out.** It is 0 on every collider (§1).

Not the fix: friction. Adding friction to the hull–terrain contact would make the tail catch harder
and would bring back the documented wall-climb lock (§1).

### 2b. The hull box caught on a tall rubble pile and slid off as if on ice

1. **The hull–rubble contact is exactly frictionless (certain as a mechanism).** Pieces carry the
   default 0.5 Average; the hull's 0 Min wins, so the combined friction is 0 (§1).
2. **The tracks cannot feel rubble or building parts at all.** The oracle is the heightfield plus
   the `TerrainMap` blocks (`src/track/terrain.rs:34-55`). Structures and their pieces are "never a
   block" (`src/scatter.rs:24-32`; playtest fork F9), and pieces sit on `Layer::Debris`, which "no
   ground query reads" (`src/lib.rs:666-669`). On a pile, then, the only support is the
   frictionless hull contact, and gravity's downslope component slides the hull off with nothing to
   resist it. The belts pass through the pile.

Note: fork F10's text in `.agents/scratch/playtest-forks/README.md` describes a freeze-to-static rule
and a 256-piece cap. The code says rest is Avian's island sleep and that the cap was deleted
(`src/debris.rs:8-34`; commit `98f3c294`). The code wins; the fork text is stale.

### 2c. A launched turret keeps sliding almost indefinitely

1. **Its only contacting collider is still the frictionless proxy (certain).** It is launched at
   14 m/s up and 3 m/s forward (CITED `src/damage.rs:677`), keeps `Friction::ZERO` with the Min
   rule (§1), and no body carries any damping. On level ground nothing removes its horizontal speed.
   With an ASSUMED μ = 0.5 it would stop from 3 m/s in ≈ 0.9 m (DERIVED, v²/2μg).
2. It also never reaches Avian's sleep thresholds (0.15 m/s, CITED in `src/debris.rs:10-11`) while
   it slides.

### Hidden repros a later lane should run (headless, no window, `scripts/serial-cargo`)

- **2a, reverse throw.** Write an `#[ignore]` test beside the driving-feel fixtures in
  `src/headless_test.rs`. It uses `booted_sim_on(Some(grid))` (line 272) with a synthetic
  `HeightGrid::new` (`src/terrain_grid.rs:303`), whose spacing must be 1.465 m to match the shipped
  map. The grid is a 12° slope that flattens out, with one ridge of samples raised 0.25 m about
  4 m past the slope's foot. Spawn the tank on the slope facing uphill and give it full reverse via
  `drive_tick(app, tank, -1.0, 0.0)` (line 738) for 8 s.
  - Log per tick: the root's position, its vertical velocity, its pitch and pitch rate. For every
    `ContactPair` touching `Hull_Collider`, log each manifold's normal, its deepest penetration and
    its normal impulse. Log the support load at the stations around the idler, from the side
    reports.
  - Variants: **A** baseline; **B** heightfield built with
    `SharedShape::heightfield_with_flags(.., HeightFieldFlags::FIX_INTERNAL_EDGES)`; **C**
    `SolverConfig { max_overlap_solve_speed: 0.5, .. }`; **D** the ridge moved to straddle a cell
    diagonal rather than a grid line; **E** no ridge, as the control.
  - Read the result this way: a single-tick hull impulse with a near-horizontal normal at the rear
    edge, which B removes, confirms cause 1 together with its aggravator. A pre-event penetration
    above 0.1 m whose launch C damps confirms cause 2. A support-load spike with no hull impulse
    points at cause 3.
- **2b and 2c, friction.** Write an Avian-only unit test in the style of `src/debris.rs`'s tests
  (`MinimalPlugins` + `PhysicsPlugins`, 64 Hz):
  - A dynamic body whose child collider carries `Friction::ZERO.with_combine_rule(Min)` rests on a
    static 15° slab with default friction. Expect it to slide more than 1 m in 2 s. Then give the
    slab `Friction::new(0.6).with_combine_rule(Max)` and expect it to hold.
  - The same body, launched horizontally at 3 m/s onto flat static ground, travels its whole
    3 m/s × t. With its `Friction` removed it stops within about 1 m.

## 3. What Avian 0.7 offers

- **Per-collider friction and restitution.** `Friction { dynamic_coefficient, static_coefficient,
  combine_rule }` and `Restitution { coefficient, combine_rule }` are components. Each falls back
  to a global `DefaultFriction` or `DefaultRestitution` resource (defaults: friction 0.5 Average,
  restitution 0 Average). Friction falls back collider → body → default; restitution falls back
  collider → default (`physics_material.rs:40-60,137-214,305-361`; `system_param.rs:604-629`).
  Static friction is accepted but ignored (§1).
- **Pair combining:** `CoefficientCombine` `Average | GeometricMean | Min | Multiply | Max`, with
  the higher-priority rule winning (`physics_material.rs:5-38`). That is enough for "this material
  overrides" (Max) and "this material zeroes" (Min) without a table.
- **Contact softness: global only, two tiers.** `SolverConfig` holds `contact_damping_ratio`,
  `contact_frequency_factor`, `max_overlap_solve_speed`, `warm_start_coefficient`,
  `restitution_threshold` and `restitution_iterations` (`plugin.rs:206-300`). These feed
  `ContactSoftnessCoefficients { dynamic, non_dynamic }`, where non-dynamic is always twice the
  dynamic frequency and "not intended to be modified manually" (`plugin.rs:302-350`, with the
  in-source TODO "Perhaps the non-dynamic softness should be configurable separately"). There is
  **no per-collider or per-pair softness**: `ContactConstraint::generate` picks one of the two
  tiers from relative dominance (`contact/mod.rs:150-154`). Other global levers: `SubstepCount`;
  `NarrowPhaseConfig` (speculative margin, contact tolerance, contact matching). Per-entity levers:
  `SpeculativeMargin`, `SweptCcd`, `CollisionMargin` (artificial thickness, not compliance;
  `system_param.rs:631-644`).
- **Per-pair friction per contact: collision hooks.** `CollisionHooks::modify_contacts(&self,
  &mut ContactPair, &mut Commands) -> bool` runs in the narrow phase before constraints are built
  (`collision/hooks.rs:147-190`; called at `system_param.rs:774-781`).
  - It may write each `ContactManifold`'s `friction`, `restitution` and `tangent_velocity`, and
    its points (`collision/contact_types/mod.rs:342-378`). The module doc names "Non-uniform
    friction and restitution" as a use (`hooks.rs:10-14`).
  - Mounting: `PhysicsPlugins::default().with_collision_hooks::<H>()` (`src/lib.rs` in avian,
    `:701-709`). The result is a `PluginGroup`, so the networked `.build().disable::<..>()` chain
    still works (`lib.rs:830-852`).
  - Conditions: one hook set per app; the hook is called only if one entity of the pair has
    `ActiveCollisionHooks::MODIFY_CONTACTS` and one body is neither static nor sleeping; the hook's
    system param is read-only and may run in parallel.
  - It cannot change softness: a manifold carries no softness field.
- **Heightfield flags.** parry's `HeightFieldFlags::FIX_INTERNAL_EDGES` corrects normals at
  triangle edges. It is reached through `Collider::from(SharedShape::heightfield_with_flags(..))`
  (parry `shape/shared_shape.rs:682-691`), because Avian's `Collider::heightfield` exposes no
  flags. The pseudo-normals are computed per contact, with no precomputation
  (`heightfield3.rs:414-470`).
- **No in-place terrain edit.** parry 0.27's `HeightField` has no height setter (its `&mut self`
  methods are `set_cell_status`, `cells_statuses_mut`, `set_flags` and `set_scale`,
  `heightfield3.rs:516-551`). A height change means a new shape through `Collider::set_shape`
  (avian `collision/collider/parry/mod.rs:551`).

## 4. Our ground data

- **Ground-type table** (`assets/terrain/ground_types.ron`, `src/ground_types.rs`): five types,
  concrete, dirt, gravel, mud and sand. Each has a single `print` block (softness mm/kPa, depth cap,
  darkening, roughness gain), and every value is ASSUMED (the file says so).
  - The doc anticipates grip, rolling resistance and sinking as sibling blocks
    (`ground_types.rs:4-9`). None exists.
- **Which ground is where** is the map's painted RGB mask: four packs, with the base pack =
  `gravel` and the R/G/B layers = dirt, sand and mud (`src/terrain_blend.rs:63,91-110`).
  - It is VIEW ONLY: only the windowed composition loads the mask (`src/world.rs:597-662`), and
    the server never does. Its only consumers are the track-print shader and the drawn belt sink
    (`src/terrain_blend.rs:31-38`; `src/track_prints.rs:1-4`).
- **The track model's friction is its own and global:** `MU = 0.9`, "a property of the track–ground
  PAIR, destined for the terrain/ground-type mechanic" (CITED `src/track/sim.rs:55-59`).
  - Further CITED knobs: `GRIP_SHEAR_MODULUS_M = 0.075` ("When the ground-type mechanic lands,
    TERRAIN owns this dial", `src/track/forces.rs:41-52`) and `SLIP_SATURATION = 0.4`
    (`sim.rs:60`).
  - The oracle carries geometry only: heightfield samples plus block boxes
    (`src/track/terrain.rs:1-55`).
- **Hull, turret and pieces read none of it.** They meet the ground only through Avian materials,
  all defaults except the tank proxies' zero (§1). `assets/materials/materials.ron` (the armour and
  substance registry) has no friction field.
- The ballistic substance registry and the ground-type table are separate tables; neither feeds the
  Avian contact.

## 5. How shipped games and engines do it

Every URL and quote comes from a delegated web survey, kept in full at
`ground-web.md` (beside this file).
I spot-checked only its Avian and parry claims, against local source; they match §1 and §3.

**Soft contact.**
- **Box2D v3** is the model Avian's solver follows (Avian cites `box2d.org/posts/2024/02/solver2d`
  in `softness_parameters/mod.rs:65`).
  - Contacts are tuned in hertz with a damping ratio: `contactHertz` 30, `contactDampingRatio` 10,
    a push-out cap of 3 m/s, and static contacts at 2× the hertz (CITED, Box2D `src/types.c` and
    `physics_world.c`).
  - The header calls this a stabiliser for overlap recovery, not a material property.
  - Speculative contacts are solved rigid, with no cap (CITED, `contact_solver.c`).
- **PhysX 5** is the only engine found with real compliant contact: a negative restitution is read
  as a spring stiffness, `setDamping` sets the damper, and an acceleration-spring flag makes it
  mass-independent. Its own docs warn that it works best when contact points stay stable from frame
  to frame (CITED, PhysX 5.5.1 Rigid Body Dynamics docs).
- **Jolt and Bullet** have slop and Baumgarte or ERP/CFM. Jolt uses `mPenetrationSlop` 0.02 m and
  `mMaxPenetrationDistance` 0.2 m (CITED, `PhysicsSettings.h`). Bullet uses a split impulse that
  keeps depenetration out of momentum (CITED, `btContactSolverInfo.h`), plus a per-object
  contact-stiffness and damping setter (CITED, `btCollisionObject.h`).
- **Havok:** no primary documentation found.

**Material pairs.**
- PhysX, Unity and Unreal Chaos use the same average/min/multiply/max scheme as Avian, with the
  stronger rule winning (CITED, each engine's API docs). Box2D v3, Jolt and Bullet expose a global
  combine callback instead.
- True surface × tyre tables live in vehicle SDKs, not in the rigid-body contact:
  - PhysX Vehicle SDK `PxVehicleDrivableSurfaceToTireFrictionPairs`;
  - Unreal's per-physical-material tyre friction scales;
  - Jolt's per-wheel `SetCombineFriction` callback (all CITED).
- That is the role our belt model's `MU` plays (§4). A surface table for the belt is that layer,
  not Avian's.

**Hull snagging and launch.**
- The industry pattern: running gear is a ray or sweep query, and the chassis is "supported only by
  suspension forces". The PhysX vehicle guide's troubleshooting section asks exactly that.
  - Ray wheels "pop" up at curbs, so Unity asks for smooth ground collision geometry.
  - Jolt ships a `TrackedVehicleController` whose wheel testers are rays, spheres or cylinders (all
    CITED).
  - Our belt is that pattern, an analytic query, so the remaining issue is the chassis contact.
- **Ghost collisions:** Jolt's architecture doc says a body sliding over an internal edge "will
  cause the box to stop or jump up". Its fix is active-edge tracking in mesh and heightfield shapes
  (CITED). parry's `FIX_INTERNAL_EDGES` is the counterpart (§3).
- **Push-out caps** cover only overlap recovery: PhysX `maxDepenetrationVelocity` (Unity default
  10 m/s), Box2D 3 m/s, Avian 4 m/s (CITED). A contact caught speculatively, before it overlaps, is
  rigid in both Box2D and Avian (CITED source), so a cap cannot soften a corner meeting a bump at
  speed.
- **Tank games:** World of Tanks' physics test notes fixed vehicles getting "stuck on slight terrain
  bumps up to 60cm high". The posts state that physics runs on the server, but they do not say how
  the hull meets the ground (CITED). No primary source was found for War Thunder's, Armored
  Warfare's or SnowRunner's hull–ground contact.

**Permanent deformation.**
- **Spintires / MudRunner** (lead developer, Game Developer article):
  - A separate mud heightfield sits over the base terrain, in 16 m blocks, with a 128² texture only
    where there is mud (CITED).
  - The CPU builds the deformation from wheel and chassis positions, sizes, velocities and
    penetration. Physics and rendering use "completely different sets of data".
  - Only blocks near the player's truck are active; the model is "pure empirics".
  - The claim that multiplayer did not sync mud comes only from a fan-site repost: CLAIMED.
- **War Thunder "Ground Breaking"** (2021):
  - Tracks and dozer blades crush soil, and bombs of ≥ ~50 kg make craters (CITED).
  - Climbing out of a deep crater can stop a weak vehicle, so the craters are in the simulation.
  - The scope is limited on purpose ("It's not Minecraft"). No sync or cost numbers are published.
- **Battlefield 3** (GDC 2012) uses a crater depth map packed to 4 bits and RLE-compressed. The
  slides do not say how craters feed physics or how they are networked (CITED).
- **Teardown multiplayer** (2026, developer blog):
  - Voxel data is never sent. Destruction was rewritten in fixed-point integer maths and replicated
    as ordered commands.
  - It runs at about 1 Mbit per client (CITED), and late joiners replay the command stream.
  - This is the shape our §6 option b already takes.
- **BeamNG:** soft ground is a per-contact force law (friction, Stribeck, depth, fluid and shear
  terms, CITED ground-model docs), not permanent deformation. This is our belt model's shape, too.
- No GDC talk was found that gives the memory, CPU or network cost of deformable vehicle terrain.

**What this says for us.** No engine found ships "the ground gives a little" as a tunable on its
solver except PhysX 5's compliant materials. The shipped answer to vehicle–ground compliance is a
query-plus-force law for the running gear, which we already have. A rigid chassis collider then
stays as the backstop, given smooth contact geometry (internal-edge fixes) and, where it matters, a
chassis shape that does not catch.

## 6. Permanent deformation here

**What the design says today.**
- PRODUCT: "Terrain topology is simulation-static **today**". Track marks, scars and shallow craters
  are Surface evidence with no gameplay effect, and "Whether ground deformation stays cosmetic
  scarring or becomes authoritative heightmap change is an open fork, settled by dedicated research
  at the world-detail milestone" (`.agents/PRODUCT.md:50`).
- The glossary entry for Surface evidence says to avoid the phrase "terrain deformation"
  (`.agents/GLOSSARY.md:34-36`).
- The track prints are view only; no print crosses the wire (`src/track_prints.rs:1-4`).

**What the code assumes.**
- ONE-SURFACE: one immutable `Arc<[f32]>` grid is decoded once and read by the collider, the oracle,
  the render mesh, spawn placement and ballistics rays (`src/terrain_grid.rs:1-37,303-316`).
- The render mesh is render-world only because "the ground never changes after startup" (≈100 MiB
  saved, CLAIMED; `terrain_grid.rs:895-902`).
- `TrackField` rebuilds the whole field on a `TerrainMap` revision bump (`src/track/terrain.rs:17-23`).
- The heightfield collider has no in-place edit (§3).

**What deformable ground would require.** The prior research in
`.agents/scratch/impact-feedback-2026-10-28/track-prints-approaches.md` §6 is checked here against
code; its numbers are its own, DERIVED from ASSUMED inputs.
- **Sim.** A server-owned sparse layer of depressions over the immutable base: 8 m tiles at 10 cm,
  u8 depth, written in tick order by integer operations.
  - The oracle gains a third term, so `BlockField` needs interior mutability rather than a revision
    rebuild.
  - A write law: pressure × hardness per ground type, which first needs the server to decode the
    mask synchronously at spawn (ADR-0014) and the ground-type table to grow a sim block.
  - For craters and berms at a scale the hull feels, a new heightfield via `Collider::set_shape` per
    edit. Any edit also wakes every body sleeping on that ground (the wake Avian lacks;
    `src/debris.rs:23-27`).
  - ONE-SURFACE must be restated, because the oracle then reads a finer surface than the collider.
- **Wire.** Under ADR-0037 clients do not simulate, so they need the layer only to draw it and to
  conform their belts. Either they derive it from replicated poses (0 B/s, approximate), or the
  server replicates the operations (≈ 1 KB/s per tank, CLAIMED there).
  - Either way a late joiner needs a snapshot: 16 MB raw, 1.6–5.4 MB compressed (CLAIMED there).
  - The map content digest no longer covers the live ground, so a checkpoint and replay format
    must too.
- **Client view.** The render mesh becomes CPU-editable per tile (the 100 MiB question above), or
  is displaced in the shader from the layer. Ballistics' `HeightGrid::cast_ray`
  (`terrain_grid.rs:442`) and spawn placement must read the edited surface.
- **Cost.** Server CPU 5.6–13 % of a core at 30 tanks, about 16 MB of server RAM, and XL work
  (ASSUMED 6+ weeks; all CLAIMED there).
  - Its own conclusion: the payoff sits at crater and berm scale (cover, line of sight, getting
    stuck), not at print scale on firm ground.

Deformation answers none of the three symptoms. Mud sinkage, the one deformation effect a player
would feel at print scale, can come without deformation: a per-ground-type sinkage and
rolling-resistance law in the belt model.

## Recommended order of fixes (cheapest true fix first)

1. **Launched turret: give it friction when it launches.** In `launch_turrets_on_cookoff`, remove
   `Friction` from the turret's proxy colliders, so it falls back to the default 0.5, or set a
   generic constant. ~5 lines, server-only, no wire change (the pose already replicates through
   `LaunchedTurretPose`). The value is a law constant, not tank data (ADR-0034).
2. **Rubble and buildings: Max-rule friction.** Give `WallPiece` colliders (`breakage::spawn_piece`)
   and building elements (`buildings::element_bundle`) `Friction::new(μ).with_combine_rule(Max)`.
   Max outranks the hull's Min, so the hull grips rubble and walls while hull versus terrain stays
   frictionless.
   - μ is a generic constant now. Later it should come from the substance registry (a friction
     field per substance), which is data.
   - Watch for drag when a hull scrapes along a wall (F9).
   - Use a `CollisionHooks` pair table only when a third pairing need appears.
3. **Run the 2a repro**, then turn on **`FIX_INTERNAL_EDGES`** on the heightfield if B helps.
   - It is one constructor change (`terrain_grid::heightfield_collider`). The geometry is
     unchanged, so ONE-SURFACE holds; the normals change only on the server's contacts.
4. **Make the hull–ground hand-off give.** The structural fix for the reverse throw, chosen by the
   repro:
   - a. Preferred: an analytic belly bump stop in the belt force model. Probe the hull belly's
     corners and edges against the oracle with a progressive spring and damper several times the
     belt stiffness over a few centimetres. The ground then gives for the hull exactly as it does
     for the belt, deterministically and per tank from its own collider data. The Avian collider
     stays as the deep backstop and as the contact with everything that is not terrain.
   - b. Blunt and global: lower `SolverConfig::max_overlap_solve_speed`, or retune damping. It is
     cheap to A/B, but it softens every contact in the game.
   - Note also that the game has no under-track hard stop, while the sandbox does (`HardStop`,
     `src/track_sandbox/mod.rs:294-300,1482-1530`). The red-team follow-up "the hull backstop box
     (±1.0 m) does NOT laterally cover the track bands" is still open (`HQ.md:1269-1271`).
5. **Ground-type grip for the belt** (`MU` per ground type, plus the server's own mask decode). This
   is a feature, not a fix.
6. **Permanent deformation**: leave it to the PRODUCT fork at the world-detail milestone (§6).
