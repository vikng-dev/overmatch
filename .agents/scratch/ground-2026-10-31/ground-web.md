# Vehicle–ground contact in shipped engines and games: primary-source survey (2026-10-31)

All sources are primary: engine source at a named tag or branch, official API docs, official dev blogs, or GDC slides. Numbers are marked **CITED** (from the source given) or **DERIVED** (my arithmetic from cited values). Wherever I found no primary source, I say so.

---

## 1. Soft (compliant) contact in rigid-body engines

### Box2D v3 "soft step". Avian follows the same model.
- **Algorithm.** Soft Step is "PGS_Soft converted to sub-stepping": soft constraints, sub-stepping, warm starting and relaxation. Soft constraints are "an evolution of Baumgarte stabilization", modelled as a mass-spring-damper and tuned in hertz plus a damping ratio. The post says "keep the Hertz at 30Hz or below for a 60Hz time step" (Nyquist). Relaxation "removes the extra energy from the velocities and constraint impulses". https://box2d.org/posts/2024/02/solver2d/
- **Why hertz rather than N/m.** Erin Catto's GDC 2011 "Soft Constraints" shows that ODE's ERP and CFM are exactly the soft-constraint β and γ. Tuning against the *effective mass* the constraint sees makes the response mass-independent: "Using the effective mass makes tuning soft constraints easy." https://box2d.org/files/ErinCatto_SoftConstraints_GDC2011.pdf
- **World-def defaults (CITED, `src/types.c`):**
  - v3.0: `contactHertz = 30`, `contactDampingRatio = 10`, `jointHertz = 60`, `jointDampingRatio = 2`, `contactPushoutVelocity = 3 m/s`, `restitutionThreshold = 1 m/s`. https://raw.githubusercontent.com/erincatto/box2d/v3.0.0/src/types.c
  - v3.1: the push field is renamed `maxContactPushSpeed = 3 m/s`; the other values are unchanged. https://raw.githubusercontent.com/erincatto/box2d/v3.1.0/src/types.c
  - main today: the push field is renamed again, to `contactSpeed = 3 m/s`. `contactHertz` is 30, `contactDampingRatio` is 10, and the world def no longer has `jointHertz`. https://raw.githubusercontent.com/erincatto/box2d/main/src/types.c
- **Header doc comments (CITED, v3.1 `include/box2d/types.h`):**
  - `contactHertz`: "Contact stiffness. Cycles per second. Increasing this increases the speed of overlap recovery, but can introduce jitter."
  - `contactDampingRatio`: "Contact bounciness … You can speed up overlap recovery by decreasing this with the trade-off that overlap resolution becomes more energetic."
  - `maxContactPushSpeed`: "controls how fast overlap is resolved … This only puts a cap on the resolution speed."

  https://raw.githubusercontent.com/erincatto/box2d/v3.1.0/include/box2d/types.h
- **Where the cap bites (main, `src/contact_solver.c`).**
  - Overlap: `velocityBias = max(massScale*biasRate*s, -contactSpeed)`, so the soft push-out velocity is clamped.
  - Speculative gap (`s > 0`): `velocityBias = s*inv_h` with `massScale = 1`, i.e. rigid, with no softness.

  In `physics_world.c`, `contactHertz = min(world->contactHertz, 0.125*inv_h)` and **static contacts get `2 × contactHertz`** (`staticSoftness`). https://raw.githubusercontent.com/erincatto/box2d/main/src/contact_solver.c · https://raw.githubusercontent.com/erincatto/box2d/main/src/physics_world.c

### Avian 0.7: the version this project pins
- `SolverPlugin` docs say Avian "uses an impulse-based solver with substepping and soft constraints". Warm starting and a relaxation pass are used, and speculative collision is on by default. https://docs.rs/avian3d/0.7.0/avian3d/dynamics/solver/struct.SolverPlugin.html
- **`SolverConfig` defaults (CITED):** `contact_damping_ratio = 10.0`, `contact_frequency_factor = 1.5`, `max_overlap_solve_speed = 4.0` (m/s, scaled by `PhysicsLengthUnit`), `warm_start_coefficient = 1.0`, `restitution_threshold = 1.0`, `restitution_iterations = 1`. https://docs.rs/avian3d/0.7.0/avian3d/dynamics/solver/struct.SolverConfig.html
- **Source (v0.7.0 `src/dynamics/solver/plugin.rs`):** `hz = contact_frequency_factor * min(1/(2·dt), 0.25/h)`. Contacts against static or kinematic bodies use **2·hz**, commented "to avoid clipping through the environment". https://github.com/avianphysics/avian/blob/v0.7.0/src/dynamics/solver/plugin.rs
- **Source (v0.7.0 `normal_part.rs`):** `bias = (softness.bias * separation).max(-max_overlap_solve_speed)`. The relax pass solves "without bias … to get rid of overshoot". There is no slop (`// TODO: We might want optional slop`). https://github.com/avianphysics/avian/blob/v0.7.0/src/dynamics/solver/contact/normal_part.rs
- **DERIVED for this repo.** Assumes the 64 Hz tick (`TICK_HZ = 64`), ≥2 substeps and the default `SolverConfig`:
  - Contact frequency: `min(32, 0.25/h) = 32`, so 1.5 × 32 = **48 Hz dynamic–dynamic** and **96 Hz against the static heightfield**.
  - Static sag of a resting body under gravity, from the spring analogy ≈ g/ω²: **≈0.03 mm against static** and ≈0.1 mm dynamic–dynamic.
  - Conclusion: the solver's "softness" is a stabilisation device. It does not model ground that gives.

### PhysX 5
- **Offsets (CITED).** `contactOffset` must be ≥0 and > `restOffset`. The default `contactOffset` is 0.02, giving a 4 cm contact distance. "If the collision shapes are sized to be the exact same size as the graphics shapes, a restOffset of zero is needed." https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/docs/AdvancedCollisionDetection.html
- **Max depenetration velocity.** `PxRigidBody::setMaxDepenetrationVelocity`: "controls how much velocity the solver can introduce to correct for penetrations in contacts." The range is (0, PX_MAX_F32]. `setMaxContactImpulse` caps the impulse at a contact. https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/classPxRigidBody.html
- **Bounce threshold (CITED).** `bounceThresholdVelocity` defaults to 0.2 × `PxTolerancesScale::speed`. The speed default is 10, so the threshold is **2 m/s** (DERIVED). https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/classPxSceneDesc.html · https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/classPxTolerancesScale.html
- **Compliant contacts (PhysX ≥5.1).**
  - Enabled by giving the material a **negative restitution**, which is read as spring stiffness. `setDamping` then sets the damper. The model is meant to imitate "how materials compress on impact" and works best when the contact points between two shapes stay stable from frame to frame.
  - Collisions: compliant vs rigid follows the compliant body's behaviour, and two compliant bodies combine their parameters by the material combine rules.
  - `eCOMPLIANT_ACCELERATION_SPRING` makes the spring mass-independent: stiffness = ω², damping = 2ζω.
  - The docs list other levers for unstable contacts: `setContactSlopCoefficient`, `setMaxDepenetrationVelocity` and `setFrictionOffsetThreshold`.
  - `eCOMPLIANT_CONTACT` is deprecated: "has no longer any effect".

  https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/docs/RigidBodyDynamics.html · https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/structPxMaterialFlag.html · https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/classPxMaterial.html
- **Unity (PhysX-based).** `Physics.defaultMaxDepenetrationVelocity` defaults to **10 m/s** (CITED) and is described as "The maximum default velocity needed to move a Rigidbody's collider out of another collider's surface penetration." Default Contact Offset is 0.01 (CITED). On Bounce Threshold, the docs say it "also reduces jitter, so it is not recommended to set it to a very low value." https://docs.unity.com/en-us/engine/current/script-reference/unityengine/physics/defaultmaxdepenetrationvelocity · https://docs.unity.com/en-us/engine/current/manual/unity-editor/editor-settings-reference/comp-manager-group/class-physics-manager

### Jolt (master, `PhysicsSettings.h`, all CITED)
`mBaumgarte = 0.2` ("how much of the position error to 'fix' in 1 update") · `mPenetrationSlop = 0.02 m` ("How much bodies are allowed to sink into each other") · `mMaxPenetrationDistance = 0.2 m` ("Maximum distance to correct in a single iteration when solving position constraints") · `mSpeculativeContactDistance = 0.02 m` (warns that too large a value gives "ghost collisions") · `mMinVelocityForRestitution = 1.0` · 10 velocity steps and 2 position steps. Jolt has no compliant-contact material. `ContactListener` can override per contact: `mCombinedFriction`, `mCombinedRestitution`, inverse-mass and inertia scales, and surface velocity. https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/PhysicsSettings.h · https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Collision/ContactListener.h

### Bullet (master, `btContactSolverInfo.h`, all CITED)
- Defaults: `m_erp2 = 0.2` (contacts), `m_globalCfm = 0`, `m_splitImpulse = true`, `m_splitImpulsePenetrationThreshold = -0.04`, `m_restitutionVelocityThreshold = 0.2`.
- **Split impulse:** penetration deeper than the threshold is corrected with a separate "push/turn velocity" that is not added to body momentum (`btSequentialImpulseConstraintSolver.cpp`). This is Bullet's own answer to depenetration launching bodies.
- **Per-body compliance:** `btCollisionObject::setContactStiffnessAndDamping(k, c)` maps to `erp = h·k/(h·k + c)` and `cfm = 1/(h·k + c)` in the solver.

https://raw.githubusercontent.com/bulletphysics/bullet3/master/src/BulletDynamics/ConstraintSolver/btContactSolverInfo.h · https://raw.githubusercontent.com/bulletphysics/bullet3/master/src/BulletDynamics/ConstraintSolver/btSequentialImpulseConstraintSolver.cpp · https://raw.githubusercontent.com/bulletphysics/bullet3/master/src/BulletCollision/CollisionDispatch/btCollisionObject.h

### Havok
I found **no public primary documentation** of Havok's contact softness or penetration recovery. The Havok-for-Unity `HavokConfiguration` exposes only sleeping, welding, world bounds and the visual debugger. https://docs.unity3d.com/Packages/com.havok.physics@0.2/manual/configuration.html

---

## 2. Material-pair friction and restitution

| Engine | Modes | Precedence when the two materials differ | Default | Source |
|---|---|---|---|---|
| PhysX | eAVERAGE, eMIN, eMULTIPLY, eMAX | "effective combine mode … is maximum(material0.combineMode, material1.combineMode)" (enum order Avg<Min<Mul<Max) | eAVERAGE for friction, restitution and damping | https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/_api_build/structPxCombineMode.html |
| Unity | Average, Minimum, Multiply, Maximum | "Average < Minimum < Multiply < Maximum"; the highest priority wins | Average | https://docs.unity3d.com/2023.1/Documentation/Manual/class-PhysicMaterial.html |
| Unreal (Chaos) | Avg, Min, Multiply, Max | "we use the largest one (Min beats Avg, Max beats Multiply, etc...)" | Avg (project setting; per-material override) | https://dev.epicgames.com/documentation/unreal-engine/API/Runtime/Chaos/FChaosPhysicsMaterial/ECombineMode?lang=en-US |
| **Avian 0.7** | Average, GeometricMean, Min, Multiply, Max | "Max > Multiply > Min > GeometricMean > Average" | Average; `Friction::default()` is 0.5/0.5 | https://docs.rs/avian3d/0.7.0/avian3d/dynamics/rigid_body/enum.CoefficientCombine.html · https://docs.rs/avian3d/0.7.0/avian3d/dynamics/rigid_body/struct.Friction.html |
| Box2D v3 | user callback | `frictionCallback(frictionA, userMaterialIdA, frictionB, userMaterialIdB)`; default `sqrt(a·b)`. Restitution default `max(a,b)`. The material id is passed "to friction and restitution combining functions" | sqrt / max | https://raw.githubusercontent.com/erincatto/box2d/main/include/box2d/types.h |
| Jolt | global callback | `SetCombineFriction`: "Default method is the geometric mean: sqrt(friction1 * friction2)"; restitution default max | sqrt / max | https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/PhysicsSystem.h |
| Bullet | global callback | default friction = product (clamped to 10), restitution = product; override via `gContactAddedCallback` | product | https://raw.githubusercontent.com/bulletphysics/bullet3/master/src/BulletCollision/CollisionDispatch/btManifoldResult.cpp |

- **True per-pair tables exist only in vehicle SDKs:**
  - PhysX Vehicle SDK (3.x/4.x): `PxVehicleDrivableSurfaceToTireFrictionPairs`, "Friction for each combination of driving surface type and tire type". Each `PxMaterial` maps to a surface type, and the suspension query reports the material it hit. https://archive.docs.nvidia.com/gameworks/content/gameworkslibrary/physx/apireference/files/classPxVehicleDrivableSurfaceToTireFrictionPairs.html · https://archive.docs.nvidia.com/gameworks/content/gameworkslibrary/physx/guide/Manual/Vehicles.html
  - Unreal: UE4 Tire Config had per-physical-material "Tire Friction Scales". Chaos wheels have `FrictionMultiplier`, which "Multiplies the ground material friction". https://dev.epicgames.com/documentation/en-us/unreal-engine/chaos-modular-vehicles-quickstart
  - Jolt `VehicleConstraint::SetCombineFriction` takes a per-wheel callback with separate longitudinal and lateral friction, and the ground `Body` plus `SubShapeID`. That is enough to build a tire × surface table. https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Vehicle/VehicleConstraint.h
- **Unity WheelCollider** ignores PhysicMaterial and uses its own slip-based tire model (see §3). The vehicle's grip is therefore a tire law, not a rigid-body friction combine.
- **Avian per-pair override (v0.7.0 source).** `ContactManifold.friction` ("effective coefficient of dynamic friction"), `.restitution` and `.tangent_velocity` are public. The `CollisionHooks::modify_contacts` hook runs "after the contact pair is computed but before solver constraints are generated", so a material-pair table can live there. Only the **dynamic** coefficient reaches the solver: the narrow phase takes `.combine(...).dynamic_coefficient`. https://docs.rs/avian3d/0.7.0/avian3d/collision/hooks/trait.CollisionHooks.html · https://github.com/avianphysics/avian/blob/v0.7.0/src/collision/narrow_phase/system_param.rs
- **Game-level surface tables:**
  - World of Tanks publishes per-tank "terrain resistance" on hard, medium and soft ground, e.g. a fixture page lists 1.21 / 1.53 / 2.74. https://wiki.wargaming.net/en/Tank:R54_KV-5
  - The War Thunder wiki lists qualitative grip and speed effects for about 15 surface types (paved road, mud, sand, snow, ice, rocks…) but gives **no numbers**. https://wiki.warthunder.com/mechanics/239-effects-of-terrain-types-on-ground-vehicles

---

## 3. How vehicle games avoid hulls snagging and launching

- **Running gear is a query, not a rigid-body contact.**
  - PhysX vehicle guide: "Ensuring that wheel shapes don't hit drivable surfaces can be achieved with simulation filtering." The troubleshooting section asks whether the vehicle is "supported only by suspension forces". Raycasts "ignore the extent of the wheel", and a wheel can be "immediately pushed up to the elevated surface". Sweeps fix that: "Sweeps provide more information about the geometry under the wheel". Blocking sweeps are the cheaper variant. https://archive.docs.nvidia.com/gameworks/content/gameworkslibrary/physx/guide/Manual/Vehicles.html
  - PhysX 5: the cylinder sweep "generally produces better results but at increased computational cost". https://nvidia-omniverse.github.io/PhysX/physx/5.5.1/docs/Vehicles.html
  - Unity WheelCollider "is actually a single Physics raycast". At a curb "the wheel is likely to clip and then 'pop' up", so "the ground collision geometry must be as smooth as possible". https://docs.unity.com/en-us/engine/current/manual/physics-section/physics-overview/collision-section/collider-shapes/wheel-colliders/introduction
  - Chaos: "Ray traces are used to find the ground". The `SweepShape` default is ray trace. https://dev.epicgames.com/documentation/en-us/unreal-engine/chaos-modular-vehicles-quickstart · https://dev.epicgames.com/documentation/unreal-engine/API/Plugins/ChaosVehicles/UChaosVehicleWheel?lang=en-US
- **Jolt ships a `TrackedVehicleController`** ("vehicle with tank tracks"). Its wheels are queried by ray, sphere-cast or cylinder-cast testers ("used to avoid colliding with vertical walls", with a max-slope angle).
  - Body attitude can be capped with `mMaxPitchRollAngle`.
  - Jolt documents the light-rubble artifact: the body dips because a light object between floor and body cannot pass enough force through the iterative solver. Its fixes are more velocity steps, smaller mass ratios, or filtering rubble out of the wheel test and pushing it away with keyframed fake wheels.

  https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Vehicle/VehicleConstraint.h · https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Vehicle/VehicleCollisionTester.h · https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Vehicle/TrackedVehicleController.h
- **Ghost collisions: a box sliding on a mesh catches internal edges.** Jolt Architecture: "A ghost collision can occur when a body slides over another body and hits an internal edge … This will cause the box to stop or jump up." The fixes are:
  - active-edge tracking in `MeshShape`/`HeightFieldShape`, with a default threshold of 5°;
  - `mEnhancedInternalEdgeRemoval`.

  https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Docs/Architecture.md · https://raw.githubusercontent.com/jrouwe/JoltPhysics/master/Jolt/Physics/Body/BodyCreationSettings.h
  - Avian and Parry 0.27 offer the same fix as `FIX_INTERNAL_EDGES` on trimeshes and heightfields ("eliminate or fix contacts normals that could lead to incorrect bumps"). Avian's `Collider::heightfield` calls `SharedShape::heightfield`, which uses `HeightFieldFlags::empty()`, so the flag is off. Turning it on needs `SharedShape::heightfield_with_flags` (verified in the parry3d-0.27.0 source in the local cargo registry). https://docs.rs/avian3d/0.7.0/avian3d/collision/collider/struct.TrimeshFlags.html
  - Avian's speculative margin is "unbounded" by default, and the docs say this "can even cause issues such as ghost collisions". https://docs.rs/avian3d/0.7.0/avian3d/collision/narrow_phase/struct.NarrowPhaseConfig.html
- **Caps on the push-out velocity, the usual fix for violent launch.** These are the knobs engines expose for it: PhysX `setMaxDepenetrationVelocity`, Unity 10 m/s by default, Box2D `contactSpeed` 3 m/s, Avian `max_overlap_solve_speed` 4 m/s, Jolt `mMaxPenetrationDistance` 0.2 m per iteration, and Bullet split impulse.
  - These caps act only on **overlap recovery**.
  - In both Box2D and Avian, a speculative contact (gap > 0) is solved rigidly, with no softness and no cap.
  - A hull corner meeting a bump at speed is therefore an ordinary rigid impact. A cap does not limit it.
  - (DERIVED from the source lines quoted in §1.)
- **Tank games.** Little of this is documented in primary sources:
  - World of Tanks' official physics test notes: "Fixed situations where a vehicle could get stuck on slight terrain bumps up to 60cm high, such as shell holes, rails" (CITED 60 cm). Suspension lets a tank "roll over minor terrain irregularities (pits, rails, curbs) without losing speed". https://worldoftanks.com/en/news/general-news/physics-experiment2/
  - The 9.14 announcement: physics "is carried out on the server"; "At a certain angle, vehicles start to slip off the hill". https://worldoftanks.asia/en/news/general-news/in-development-improved-vehicle-physics-sounds/
  - Neither post describes how the hull collides with the ground.
  - **No primary source found** for War Thunder's hull–ground contact model, Armored Warfare, or Spintires/SnowRunner chassis contact.

---

## 4. Permanent ground deformation for vehicles

- **Spintires / MudRunner.** Source: Pavel Zagrebelnyy, the lead developer, on Game Developer. https://www.gamedeveloper.com/programming/mud-and-water-of-spintires-mudrunner
  - The mud is a separate heightfield layer over the base terrain. Levels are up to 1 km × 1 km, cut into 16 m × 16 m blocks, with a 128×128 texture "only allocated for blocks with mud" (CITED).
  - The CPU builds the deformation from "wheels and chassis positions, their size and velocities, and their current mud penetration depth". Physics and rendering use "completely different sets of data".
  - Only blocks "near the truck that player is driving are 'active'".
  - In his words, the mud model "is not something Havok … can do out of the box" and is "pure empirics".
  - Mud particles "only act as decoration and don't affect the physics". Mud chunks are full rigid bodies.
  - On **multiplayer**, the FAQ answer credited to the "Spintires: MudRunner team" says the engine's technical constraints ruled out "multiplayer light, sound and mud sync". The only copy I could read is a fan-site repost (https://www.mudrunnermods.com/?p=54). The Steam discussion said to hold it would not load, so this is unconfirmed at the primary source.
  - **No primary source found** for SnowRunner's deformation or sync. Saber's marketing says only that the terrain tech carried over.
- **War Thunder "Ground Breaking" (2021).**
  - "Terrain and soil now interact with caterpillars and dozer blades." Bombs of ≥~50 kg, heavy rockets and large-calibre guns make craters sized by explosive mass. https://warthunder.com/en/news/7400-news-meet-major-update-ground-breaking-en
  - Earthworks block rounds partly ("not an absolute shelter"; thicker earthworks are harder to penetrate). Crater depth affects mobility: "lifting from the bottom of a deep hole to its rim can become a problem for vehicles with poor off-road performance", so the deformation is part of the simulation.
  - Its scope is limited. A Gaijin admin: "It's not Minecraft. You cannot dig cave or big pits." The devblog gives no numbers on persistence or sync. https://warthunder.com/en/news/7378-development-ground-breaking-self-entrenching-and-bomb-craters-en
- **Battlefield 3 (Frostbite, GDC 2012).** Terrain destruction is driven by a "destruction depth map" that "controls crater depth" and a physics-material map that sets crater depth and width. Both are "packed to four bits and RLE-compressed". The slides do not say how craters feed into physics or how they are networked. https://media.contentapi.ea.com/content/dam/eacom/frostbite/files/gdc12-terrain-in-battlefield3.pdf
- **Teardown multiplayer (2026, Dennis Gustafsson).** https://blog.voxagon.se/2026/03/13/teardown-multiplayer.html
  - "Sending large amounts of voxel data wasn't an option because of bandwidth". An early naive version "completely choked the connection".
  - Destruction was rewritten in **fixed-point integer math**. It is replicated as ordered commands ("cut hole in this shape at voxel coord x,y,z") on a reliable, ordered stream, with the host as server.
  - Transforms and velocities use state sync with eventual consistency, at about **1 Mbit per client** (CITED).
  - Late joiners replay the command stream rather than receive a 30–50 MB scene (CITED). Join-in-progress is disabled once the replay buffer is full.
- **BeamNG.** The ground models are a per-contact force law, not deformation: `staticFrictionCoefficient` 1, `slidingFrictionCoefficient` 0.7, `stribeckVelocity` 6, plus soft-ground `defaultDepth`, `fluidDensity`, `flowConsistencyIndex`, `flowBehaviorIndex` and `shearStrength` (CITED defaults). **No primary source found** for any permanent terrain deformation in BeamNG. https://documentation.beamng.com/modding/levels/level_formats/groundmodels/
- **No GDC talk found** on deformable *vehicle* terrain with stated memory, CPU or network costs, beyond the Spintires article and the Teardown post above.

---

## Incidental read-only code observations (this repo, not web)

These bear directly on the symptoms. I did not test them.

- `src/tank/spawn.rs:861` gives every tank collision proxy `Friction::ZERO.with_combine_rule(CoefficientCombine::Min)`. The comment reads "Penetration backstops ONLY: the analytic belt model owns ALL tangential ground physics".
  - Avian's priority table puts Min above GeometricMean and Average, so any proxy–ground and proxy–rubble contact gets friction min(0, x) = **0**. This would explain the hull that "slides off a rubble pile as if on ice".
  - It also explains the detached turret that slides almost indefinitely, **if** the turret's colliders are among those proxies. I did not confirm that.
- `src/terrain_grid.rs:883` builds the terrain with `Collider::heightfield(...)`, which uses parry's `HeightFieldFlags::empty()`. So **FIX_INTERNAL_EDGES is off**, and that is a documented cause of a sliding box catching and jumping on internal edges. This should be ruled out before tuning softness.
