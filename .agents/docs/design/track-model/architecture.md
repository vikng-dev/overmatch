# Track module architecture — promoting the sandbox model into the game

> **2026-08-15, [ADR-0037](../../adr/0037-one-authoritative-timeline-and-view-overlays.md):**
> the netcode contracts named below (owner-predicted, `local_rollback`, rollback smoothing) are
> retired; state replicates on the one timeline and the wrap view rides the interpolated cursor.
> The track model, the sim/view boundary, and the belt law stand.

Status: v5 (2026-07-26) — the VIEW settled: the memory-enabled kinematic wrap
(`src/track/wrap.rs`) is the one and only track view, in the game AND the sandbox; the simulated
chain tier, its `V` toggle and its feel switches are DELETED (see §1a). v4 (step 27, 2026-07-17)
— phase B **shipped**: the belt model IS the game's drive sim (see §0a; ADR-0025 written,
supersedes ADR-0005, retires ADR-0006). v3 (step 26) shipped
phase A (`src/track/view.rs`) with the tier-line discussion + codex view-plugin review. v2
reconciled v1 against the codex adversarial review (`scratchpad/codex_arch25_review.md`, 10
findings, all dispositioned below). Companion to HQ.md (the step log). Foundation
document for "many tanks, one model"; every structural decision is judged against Yan's three
constraints:

1. **One model** — quality scaling is tiers of one pipeline, never parallel systems.
2. **Many tanks** — 30-tank MP scenes; per-tank cost is a policy decision (tier), not a tax.
3. **Spec-sheet authoring** — a new vehicle is data. If adding a tank requires touching a
   solver constant, the design failed. (Codex C: this rules out several constants currently
   hard-coded in `belt.rs` — see §7.)

## 0. What exists (survey, 2026-07-17)

- Game locomotion: ADR-0005 raycast roadwheels (`src/driving/`), track cosmetic and unrendered.
- Tank assembly: ADR-0014 sim/view split; sim body synchronous from `bake::TankGeometry` +
  `.tank.ron`; GLB attaches as view (`ViewOf`/`ViewNode`/`ViewServo`); roadwheels are sim
  entities; sprocket/idler are visual-only nodes (`Sprocket_L_Visual` …).
- Prediction: `DriveState`/`TankSim` are root-resident `local_rollback`; sim reads tick-truth
  `Position`/`Rotation`; render smoothing on the view tree only; rollback-correction smoothing
  in `net/render_error.rs` writes the root `Transform` in `PostUpdate` before propagation.
- Terrain: static cuboid colliders on `Layer::Terrain`; transforms are built procedurally in
  `world.rs` and **discarded** — no shared data source for an analytic field yet.
- Sandbox: the math to promote (oracle/route/wheels/forces) is entangled with
  sandbox-local types (`Side`, `RigWheel`, `Suspension`, `PinBelt`, `ConformedBelts` …) — the
  promotion is a seam rewrite around copied math bodies, not a file move (codex E).

## 0a. Phase-B reality (v4 — feat: phase-B cutover — the sandbox belt model IS the drive sim)

§0 below is the PRE-cutover survey, kept as the baseline the migration was judged against.
What is true now:

- `src/driving/` is DELETED. Locomotion is `src/track/forces.rs` (pure belt force law — the
  sandbox model, extracted verbatim, bit-parity-proven) + `src/track/sim.rs` (ECS adapter:
  `TrackDrive`/`TrackContacts`/`TrackGear`, capability gate, `SimPhase::DrivingForces`).
- `DriveState` is gone. `TrackDrive {throttle, steer, [speed, phase f64]×2}` is
  owner-predicted + replicated + rolled back (LinearVelocity pattern, NOT local_rollback);
  `hblt` hash stream; PROTOCOL_REV 12.
- The terrain oracle is `track::terrain::TrackField` in SimPlugin — server, SP, and client
  share one field, rebuilt from `TerrainMap` on revision change and carrying the decoded
  `HeightGrid` alongside the authored blocks (§5).
- The view consumes `TrackDrive` phase/speed directly — the pose-delta no-slip derivation is
  deleted; remote tracks scroll at exact authority phase.
- The raycast hold/bristle transplant was REVERTED at cutover (owner call — pure sandbox law).
  Hill-hold later shipped as physics inside the slip law: the
  per-element elastic–plastic strain grip ([[0026-static-friction-strain-regime]], settled
  per-element in [[0027-element-grip-netcode]]) — a parked tank holds on grade.
- Sandbox `belt` (renamed from `model4` 2026-07-26) is a thin adapter over `track::forces` — the entanglement noted in §0
  (codex E) was dissolved by the extraction; models 1–3 deleted in the consolidation pass.
- Vehicle collision proxies carry their substance's friction like every collider (`substances::PAIR_RULE`,
  the geometric mean); they sit clear of the ground wherever the belts carry the load, so the
  hull's grip there is the model's alone.

## 1. The shape: one geometric core, two consumers

Step 24 dissolved the "two models" question — the drawn belt's skeleton IS the route — and the
2026-07-26 view settlement collapsed the render side to a single tier:

```
            authored data                          runtime inputs
      ┌──────────────────────────┐        ┌────────────────────────────────┐
      │ TrackSpec (.tank.ron §7) │        │ TerrainOracle (§5)             │
      │ RunningGear (bake, §7)   │        │ PresentedFrame (§3)            │
      └────────────┬─────────────┘        │ BeltKinematics (§4)            │
                   ▼                      └───────────────┬────────────────┘
            ┌──────────────────────────────────────────────▼──────┐
            │            route/wrap core (pure fns, §2)           │
            │   wheel filter → taut envelope → conform → sag      │
            └──────────────┬─────────────────────┬────────────────┘
                           ▼                     ▼
                  ┌────────────────┐   ┌──────────────────────────┐
                  │   SIM forces   │   │  kinematic wrap VIEW     │
                  │   (phase B)    │   │  (every tank, no tiers)  │
                  └────────────────┘   └────────────┬─────────────┘
                                                    ▼
                                           TrackRenderer (§8)
```

Deleting either consumer leaves the other intact; adding a tank touches neither.

## 1a. The view settlement (2026-07-26)

The sandbox ran two views side by side for three steps: the step-24 **simulated chain** (XPBD
nodes in a route tube, real pin friction, hinge stops) and the step-22 **kinematic wrap** (taut
envelope + terrain conform + budgeted sag, refitted every frame). The chain lost, and is deleted:

- **Cost.** ~56 µs/tank/frame for the wrap against 809–907 µs for the chain — a ~15× gap that
  decides "many tanks" (§6) on its own.
- **Failure mode.** The chain tear-churns: at top speed it reseeded on 1768 of 2048 measured
  frames. A solver can tear, buckle, stale and diverge; a fitted curve cannot.
- **What the wrap gives up, and how it got it back.** Stateless fitting snaps between shapes.
  The cure is two self-healing FILTER tiers over the stateless core — a hull-frame temporal ease
  on the conform depth (rise instant, fall BALLISTIC at the same gravity the view wheels fall at)
  and a 1-DOF spring on the sag budget (frequency = a hanging span's pendulum rate
  `√(g / sag_depth)`, damping = the canonical ζ = 0.5). Both are **parameter-free derived laws**,
  which is why they are unconditional: there is no dial to set and therefore no toggle to offer.
  A filter cannot tear; only a solver can.
- **A third tier was built and deleted.** A droop LIMIT on the conform was tried and removed:
  clamping is a solver's job, and the two laws above already bound the shape.

Consequences carried through the rest of this document: there is one view tier, not two (§6);
the view's per-frame memory is filter state with a reset, not solver state with a reseed (§3);
and the pure-core surface is `wrap::step` + the route/envelope builders, with no
`ChainState`/`RouteTag` (§2).

## 2. Module layout, pure-core API, migration

`src/track/` as a peer of `driving/`, facade `src/track.rs` (plugin-per-feature, ADR-0002):

```
src/track.rs              pub fn view_plugin(app), pub fn sim_plugin(app)
src/track/side.rs         Side + PerSide — the one L/R encoding (left −X, right +X)
src/track/rig_geom.rs     RigGeom: the derived running-gear geometry contract
src/track/marker_model.rs the glb's sharp sources of truth (Pin_* markers, Link_Box, rig meshes)
src/track/derive.rs       the universal suspension-model derivations (pure)
src/track/oracle.rs       TerrainOracle (depth + typed surface hits) + BlockField (§5)
src/track/terrain.rs      TrackField resource: the ONE field, rebuilt on TerrainMap revision
src/track/route.rs        route core (pure) — incl. `taut_lower_run`, shared with the wrap
src/track/envelope.rs     contact-envelope calibration: k from ride frequency, travel knots
src/track/forces.rs       the belt force law: `contact_side` (support + per-element grip)
src/track/transmission.rs the joint drivetrain step — ONE form, every architecture
src/track/drive.rs        command seam: intent → slewed axes → per-side belt commands
src/track/wrap.rs         the kinematic wrap + its two feel filters (pure struct + stepper)
src/track/wheels.rs       view wheel-lift filter (pure)
src/track/gear_phase.rs   the phase law: belt travel → the angle every rotating node carries
src/track/link_view.rs    the instanced shoe render layer (the §8 seam, as shipped)
src/track/view.rs         ECS: presented-pose seam, belt derivation, view-node writes
src/track/sim.rs          collocation forces in SimPhase::DrivingForces
```

`TrackSpec` lives in the crate-wide `src/spec.rs` with the rest of the vehicle schema, not in a
track-local `spec.rs`; there is no `rig.rs` and no `render.rs` (the render seam shipped as
`link_view.rs`), and `SpatialQueryOracle` was never needed — see §5.

**Pure-core API surface** (codex E — sandbox types stay OUT; the sandbox re-imports these and
keeps its own ECS adapters):

```rust
// Geometry
pub fn build_route(circles: &[(Vec2, f32)], belt_len: f32) -> Route;
pub fn taut_lower_run(circles: &[(Vec2, f32)], front_up: Vec2, rear_up: Vec2,
    emit: impl FnMut(Vec2));                              // the shared belly walk (below)

// Sim
pub fn contact_side<O: TerrainOracle>(input: &SideInput, state: SideState, affine: Affine3A,
    dt: f32, params: &ForceParams, oracle: &O, vel_at: impl Fn(Vec3) -> Vec3,
    elements: &mut GripElements) -> (SideReport, bool);
pub fn step(fp: &ForceParams, tp: &TransmissionParams,
    state: &mut TransmissionState, inp: &TransmissionInput) -> TransmissionReport;

// View
pub fn wheel_lift_target<O: TerrainOracle>(oracle: &O, affine: &Affine3A, down: Vec3,
    pivot_local: Vec3, params: &WheelParams) -> f32;
pub fn wheel_lift_step(dy: &mut f32, dvel: &mut f32, target: f32, dt: f32,
    params: &WheelParams);
pub fn step<O: TerrainOracle>(input: &WrapInput, oracle: &O,
    state: &mut [WrapState; 2]) -> [WrapSideOutput; 2];   // the ONE view stepper; no options
```

(The view stepper is `track::wrap::step`. `WrapState` is the per-side filter memory,
`WrapState::reset` is the discontinuity path — there is no reseed, no report and no tier/feel
parameter.)

**`taut_lower_run` is the one belly walk, and it is shared on purpose.** The sim's route
(`build_route`) and the view's belly (`wrap::raw_belly`) are the same lower convex envelope over
the side-plane circles — chained external tangents and wrap arcs, with a circle that rises above
its neighbours' tangent dropping out, so a lifted wheel is skipped by construction. It emits
through a callback rather than returning a `Vec` because the two sinks differ in exactly one
respect and both are load-bearing: the route dedupes near-coincident vertices onto a polyline
seeded with the exact front tangent point, while the view keeps the raw walk spacing for its
conform resample. One walk, two consumers, no second implementation to drift.

**The sim has ONE force path.** `apply_track_forces` runs `contact_side` per side and then
`transmission::step` once, on the vehicle's declared tables. There is one drivetrain law and no
branch to pick between laws.

**Suspension travel is not optional.** `SideInput::travel` is a required per-position
`TravelField` — the contact envelope's local reach, built by `envelope::wheel_travel_knots` (0 at
the unsprung sprocket/idler, full droop at every road wheel, linear tapers between). There is no
`Option`, no lift-only compatibility mode, and no scalar fallback: the law reads the profile
unconditionally.

Type mapping from the sandbox: `PinBelt` → `MaterialLoop` (immutable, §7); sandbox `Suspension`
→ `WheelViewState` (never reuse the game's sim `Suspension` name); `BeltSample` → `LinkPose`
(one representation, full orthonormal frame — §8); the wrap's filter memory is private inside
`WrapState`; `ConformedBelts`/`TautReference` stay sandbox debug adapters. `RunningGear` is baked
synchronously from `TankGeometry + TrackSpec`, born with the root, and holds **no asset
handles**.

Mounting: `view_plugin` in the presentation roots only (like `vfx`); `sim_plugin` (phase B) in
`SimPlugin`'s `SimPhase::DrivingForces` slot.

## 3. The track VIEW is cosmetic state — and the seam is the PRESENTED pose (codex A)

The track view (the kinematic wrap plus its two feel-filter tiers) is cosmetic: not
rollback-registered, never re-solved in replays, reseedable from data at any instant (ADR-0014
tier-2 spirit). But "read the view pose" needs a precise
implementation, because rollback smoothing writes the root `Transform` in `PostUpdate` after
Avian writeback and before propagation — an `Update` system reads a stale propagation, and a
post-propagation system is too late to move child links.

The seam:

```rust
pub struct PresentedFrame {
    pub previous_from_track: Affine3A,  // last frame's presented track-anchor affine
    pub current_from_track: Affine3A,   // this frame's, composed from the root's FINAL local
                                        // Transform (after RenderErrorApplied) × baked anchor
    pub frame_dt: f32,
    pub discontinuity: bool,            // oversized correction consumed unsmoothed, teleport,
                                        // respawn, tier promotion, oracle revision, clock overrun
}
```

- Built after `RenderErrorApplied`, tracks run **before `TransformSystems::Propagate`**, writing
  view nodes that propagation then carries. (Shipped shape: `track::view::TrackViewSet` owns the
  slot; `net::render_error` orders it after `RenderErrorApplied` — the edge lives on the net
  side because the net-boundary guard keeps `track` from naming the netcode.)
- The wrap fits the belt once per frame — no substeps, no catch-up: the hull affine is captured
  once and terrain is probed at that end-of-frame pose. The wheel-lift filter and the feel-filter
  tiers carry the only per-frame memory; the wrap geometry itself is stateless.
- Shipped discontinuity detection is LOCAL (no lightyear coupling): presented pose delta per
  frame (translation > 1.2 m, or forward/up axis chord > 0.5) or a `TerrainMap` revision
  change → wrap filter reset + wheel-lift re-base. The thresholds must bracket `render_error`'s
  snap constants (2 m / 60°) — pinned by a test in `render_error`.
- **Terrain probes use the interpolated presented pose.** Probing at tick pose and offsetting
  links afterwards is wrong — terrain doesn't receive the offset.
- `discontinuity == true` → the view resets its filter memory (the wrap re-inits from the current
  frame's raw targets, the wheel lift re-bases). Self-healing: the filters cannot carry stale
  state past a reset, so a missed signal costs at most a one-fall-period settle, never a tear.

**Wheel articulation writes GLB view nodes only** (codex I). Roadwheel sim entities' transforms
participate in tick-truth suspension casts — cosmetic writes there would feed view state back
into the sim. Phase A: field-driven visual wheel lift at the presented pose; both Route and
Simulated tiers build from those circles. Never feed tick-world `Suspension.contact` into the
view during correction — it belongs to a different hull position. Sprocket/idler/axle view
anchors: synchronously spawned non-rollback skeleton anchors in `SimParts` (fits the existing
`ViewNode` machinery).

## 4. Belt kinematics

**Phase A — "no-slip visual ground lock" (named honestly; codex D):** belt travel derives from
the **presented pose delta**: each track-centre's presented world displacement projected on the
presented forward axis, integrated per side, differentiator reset on `discontinuity`. This
includes yaw and correction motion by construction. Known-wrong (accepted, phase B fixes):
braked skid still scrolls, wheelspin/ice under-reports, airborne says nothing about commanded
belt motion.

**Phase B — real belt state (codex J, contract picked):** `BeltState { sides: [BeltSideState;
2] }` is a **root-born replicated + predicted component** (registered through the
replicated/predicted path, NOT `local_rollback` — those are mutually exclusive contracts and
v1 conflated them), initialized synchronously in `assemble_tank_body`. The owning client
predicts and rolls it back; remotes consume the replicated scalars for their Route tier. If
bandwidth ever forces a split, `BeltState` goes local-rollback and a separate `NetBeltState`
becomes the replicated adapter — named here so it's a decision, not drift.

## 5. TerrainOracle — one scalar query, one ground surface (codex F)

`SpatialQuery` is a borrowed `SystemParam` — it cannot live in a resource, and the sim path must
not depend on parry's raycast floats anyway (they were not cross-platform reproducible). The
oracle is a **pure trait of exactly one scalar query** — the one every consumer (belt physics,
wheel articulation, the wrap view's conform) actually makes:

```rust
pub trait TerrainOracle {
    /// Signed directional penetration of `station` past the first surface along `out`; the ray
    /// starts `reach` behind the station and reports at most `reach` (a buried origin
    /// saturates, like a contact cast). Positive = past the surface, negative = clearance.
    fn depth_along(&self, station: Vec3, out: Vec3, reach: f32) -> f32;
}
```

The force consumer now reads `ContactOracle::contact_along`, the supertrait's typed surface
query: `TerrainContact::Hit { depth, normal: Dir3 }`, `Miss`, or `Buried` (§5a). The scalar depth
query remains the geometry consumer's contract. Batched probes, surface material and `covered`
remain growth paths with no consumer. `SpatialQueryOracle` never needed one.

- **`BlockField`** (the one implementation, held by `track::terrain::TrackField`): a min-fold of
  two exact analytic terms, built as `BlockField::new(blocks).with_height(grid)`.
  - **The height grid is the shipped ground.** `HeightGrid::cast_ray` returns the EXACT first
    hit — a 2-D DDA over the one or two cells the probe's XZ footprint crosses, closed-form
    ray-vs-triangle per cell, on the same anti-diagonal split the parry collider triangulates
    with. It replaced a fixed-segment sign-change scan plus bisection refinement: no sampling
    rate to outrun, so a ridge between old checkpoints can no longer be missed. Outside the grid
    span the height term reports NO ground — agreeing with the collider, deliberately
    disagreeing with `height_at`'s placement-only clamp.
  - **Authored blocks** are the secondary term (the sandbox's obstacle course, and whatever the
    product map places on top of the heightmap): exact analytic ray-vs-rounded-box.
- **`TerrainMap { revision, blocks }`** in `world.rs` is the shared block source consumed by BOTH
  collider spawning and the field, so the two representations cannot drift; `revision` feeds the
  reseed/discontinuity signal on terrain change.
- **ONE surface, past the track model.** The same `HeightGrid` feeds the oracle, the parry
  collider, the render mesh, spawn placement — and ballistics: a shell's terrain hit is
  `HeightGrid::cast_ray` too, min-folded against the separate armor cast so the nearer wins.
  No parry terrain cast remains: a fixture world without a grid has no ballistic ground. What a
  shell stops on is what the tracks drive on.
- Honesty note: the block term rounds corners and buries block bottoms — deliberate policy, not
  representational identity with colliders. The height term IS the collider's triangulation, to a
  pinned tolerance. "Visual ≡ physics" means both sample the SAME oracle.
- Streaming meshes, overhangs and destructibles still need chunk coverage + revisioning + a
  "clear vs unloaded" distinction (`covered`) — out of scope, named so the trait doesn't pretend
  otherwise.

## 5a. Ground under a contact (#146)

Subject: the game's driving model, what the ground under a track changes; game state only.

Surface material is the force oracle's second query: `ContactOracle::ground_at(point)` returns
`ContactGround`. Depth-only consumers implement `DepthOracle`; a simulation `TerrainOracle`
requires both depth and contact contracts. The view's lowered `Sunk` implements only depth and
cannot supply simulation support. A published `TerrainMap` requires its decoded `GroundField`:
`build_track_field` refuses a composition missing it. Calibration runs support only. Bare inline force fixtures may explicitly state no ground
and use their own rigid pair with no motion resistance; that fixture path is test-only and a
force-bearing production contact without ground facts panics.

**Which ground, deterministically, from data.** `ground_field::GroundField` is decoded once at
startup by `terrain_grid::decode_height_grid`, beside the heightmap: the map's `terrain_masks.png`
read synchronously from the map directory with the `image` crate (ADR-0014: never the view's
texture, which a server without a window never loads), its declared row order folded in, held as
three bytes a texel with the ground-type table's row for each of the four packs. A point's ground
is those four rows blended by `terrain_blend::print_weights` of the mask read bilinearly and
clamped at texel centres, the same weights the prints and the muzzle dust are drawn with. A world
the decode does not build from the map (the flat slab and its authored course, a preset or
flattened grid) is the base pack's ground everywhere, which is what the blend is where the mask is
silent; the sandbox's course is the same. `BlockField::with_ground` carries the field into the
track field; an authored block on the map reads the ground at its own XZ (a graybox building's
substance is not yet a ground the belt reads).

- Resolution (`Resolution`): the mask's own, 1024² over 1 500 m, 1.46 m a texel (DERIVED). It is
  the finest statement of the ground the map has, and the material sharpens the visible blend edge
  inside one texel; a coarser field would let a player see ground the tracks do not feel.
- Cost: 3 MiB (DERIVED, 1024² × 3 bytes) on every peer that builds the world, one read and
  decode at startup (historical MEASURED 12.4 ms in the `edit`-profile test build,
  `ground_field::tests::the_shipped_mask_decodes_into_the_field`; the boot logs its own decode),
  and one bilinear read plus fixed-size constitutive blends per terrain contact. `PrintDepths`
  shares this immutable field. The renderer uploads its decoded texels in the manifest row order,
  rather than loading or decoding the mask again; the row-fold test checks upload bytes too.

**Traction is selected by data.** Each `ground_types.ron` row declares `Rigid` or `Soil`.
A rigid surface uses the existing substance pair, `sqrt(mu_track * mu_surface)`.
Soil uses Mohr-Coulomb strength over the contact footprint,

`T_max = A*c + W*tan(phi)`; `T = T_max * (1 - exp(-|j|/K_eff))` along the vector displacement `j`.

`A` is supported belt length times strip width projected onto the struck surface; `W` is elastic
normal load. The force pass stores capacity as `mu_equiv = T_max/W`, not as a
claim that soil shear is steel sliding friction. The Driver instead sums footprint cohesion
separately and applies the load-proportional terms to nominal normal pressure. The longitudinal component reacts into the belt.
Janosi-Hanamoto uses characteristic displacement: at `|j| = K_eff` the capacity share is
DERIVED `1-exp(-1) = 0.63212056`, not full strength. The solver's existing velocity damper remains,
sized from initial shear stiffness and the elastic load's mass share; it is a Resolution term,
not an extra soil-strength fact. Rigid surfaces retain their Dupont elastic zone and Dahl return.
Soil accumulates tangent-plane vector displacement while the element contacts the ground;
departure forgets it. Displacement is currently clipped at `16*K_eff`, where the omitted loading
capacity share is DERIVED `exp(-16) = 1.125e-7`. The clip also controls macroscopic unloading;
it is not solely a precision bound. The measured defect and owner decision are recorded below.

The soil facts are required and labelled: cohesion Pa, friction angle degrees and characteristic
shear displacement m. The stamping note's section 2 reproduces NASA's Wong specimen table as
CLAIMED, not measurements of this map. Consequently every shipped soil fact is `Unknown`,
naming its proxy and the direct shear/shear-displacement test that would settle it. Dirt uses the
sandy-loam specimen; mud the clayey-soil specimen; loose sand the dry-sand specimen. Gravel's
35-degree angle and dry-sand cohesion proxy are explicitly provisional, with no measured gravel
fit. None is a per-vehicle rule.

Painted boundaries blend cohesion and `tan(phi)` by area share, so their maximum shear strengths
blend linearly. Rigid capacity blends `sqrt(mu_surface)`, then squares the mean for the one
geometric pair rule. Soil K is homogenized by soil area share before applying the solver floor.
Each split material element advances its shared displacement once, with load-weighted slip and
compliance; each surface patch reads its own capacity and displacement response.

**Fact versus solver floor.** `K_eff = max(K_ground, K_floor)`. The code's `GRIP_SHEAR_FLOOR_M`
is the Resolution floor, 0.075 m; it is no longer constitutive data in `grip.ron`. ADR-0026 records
MEASURED coupled roll/yaw cycling at the old 5 mm target and an admitted soft mode with
DERIVED `omega*dt ~ 0.27` at 75 mm and 64 Hz. Since stiffness scales as `1/K`, reducing K to 5 mm
raises that modal step to DERIVED `0.27*sqrt(75/5) ~ 1.05`. This is an empirical conservative floor,
not a sharp analytical stability boundary for the coupled support system. The stiffness of a
particular soil can be smaller, but its initial slope cannot exceed this solver resolution.

| soil row | K fact (m, Unknown proxy) | K effective at 64 Hz (m, DERIVED) | floor binds |
|---|---:|---:|---|
| dirt | 0.010 | 0.075 | yes |
| gravel | 0.010 | 0.075 | yes |
| mud | 0.006 | 0.075 | yes |
| sand | 0.025 | 0.075 | yes |

**The floor binds every shipped soil.** Felt differences currently come from strengths and motion
resistance. A higher physics rate could admit a lower floor after the coupled modes are verified:
for an unchanged modal-step bound its required K scales with `dt^2` (DERIVED). The present change
keeps the 64 Hz rate. Paving has no soil K and uses the rigid-contact solver floor.

`TrackSteel.friction` is now `0.6`, an `Unknown` steel sliding coefficient pending a dry sliding
test; it no longer encodes grouser bite. With the shipped brick's `0.65`, both its belt/brick and bump-stop/brick pair
is DERIVED `sqrt(0.6*0.65) = 0.624500`, replacing DERIVED `sqrt(1.62*0.65) = 1.026158`.
The base ground's old pair `sqrt(1.62*0.5) = 0.9` is replaced by its soil capacity.
The terrain's Avian collider carries `terrain_solver_friction`, an explicit data stand-in for
solid-body contacts lacking a soil footprint, at the retained coefficient 0.5. Its bump-stop pair
therefore changes from DERIVED 0.9 to `sqrt(0.6*0.5) = 0.547723`. It is independent of the track's
soil strength. Scripted braking reads the capacity emitted by the actual contact pass, bounded
by service-brake construction; `GripParams::ground` no longer estimates stopping.

**A surface normal carries its own pressure share.** Where the station or pressure-centroid hit normals differ,
`terrain_surface_patches` subdivides the profile to half `FIELD_ROUNDING` (DERIVED 0.015 m),
resampling depth at child stations. A normal variation below squared error `1e-4` needs no split
(DERIVED about 0.6 degrees); planar terrain keeps its original profile. A centroid with clearance,
a miss or a buried origin supplies no support. Thus a rounded lip cannot rotate an entire long
column's load, and a far wall behind a gap cannot carry load from the near lip. Piece overlap
orientation remains the existing documented stand-in outside this lane.

**Rolling resistance.** A moving track loses `c_rr × N` to its ground against its travel, `c_rr`
per ground in `ground_types.ron` (`rolling_resistance: Total`, each `Unknown` with its CLAIMED range and
what would settle it: the stamping note gives soil fits, not motion resistance) and `N` each
terrain contact's normal load. The side's rolling load `Σ c_rr·N` fades linearly to zero below
`forces::ROLLING_FADE_SPEED` = 0.25 m/s (`Resolution`, stability derivation at the constant), so a
tank at rest takes none, and joins the side's ground reaction `R` at the pre-tick belt speed: the
transmission's `I·v̇ = Q − R` carries it, and every reader of `R` (the brake's stop force, the
clutch coupling) sees it as load. The Driver separately forecasts a cut from its geometric
load L and the coupled hull/belt mass, with no reaction decay. An undeforming piece still
loads the track's running gear: `grip.ron::rigid_rolling_resistance` states its `Unknown` motion
resistance, currently 0.04 from the CLAIMED rigid-support range. Ground rows state total motion
resistance including that share, so the rigid-support term is not added to them again. This
replaces the unsupported body-category rule that every piece had zero rolling resistance.

**Driver observation and measured force resolution.** `track::load::observe` supplies the
unfiltered `TransmissionInput::perceived_load`: `L = W sin(theta) + c_rr W cos(theta) +
mu_t W cos(theta) length / (4 R_g)` along the engaged ladder. Nominal weight is vehicle data; normals,
paired friction and rolling coefficients are the contact pass's own geometric/material facts,
collected before the normal damper can unload a patch. Uniform pressure weights patches by
supported length and collocation-column share. The projected support tangent supplies grade;
the footprint span and current detent radius supply the turn term. Rolling is the full potential
resistance even at rest, independently of the physical low-speed fade. Airborne means no terrain
load. No acceleration, spring/damper force, temporal filter or recoil term enters L. The game and
sandbox share this implementation. The Driver uses requested pedal, not its actuator ramp,
and prices each permitted gear at its own landing speed and detent radius. Holding the current
gear pays no cut; changing gear pays the declared interruption. Coupled crank inertia contributes
reflected mass to the force price; launch Slip prices the commanded launch rpm.
C1 admits lower gears only on a deficit beyond M or an engaged crank below the curve floor;
a spooling launch in Slip is not an engaged lug. C2 preserves overrun, and C3 preselects the
highest capable declared launch gear at rest, repriced at requested pedal under intent.
An upshift cannot land in the sub-curve lug that C1 would immediately correct.
The brake lever carries only the stop-force shortfall against intent, without a release latch.
Opposite input brakes to rest, waits the authored pause, then pays one direction cut;
release during the pause keeps park held. Zero intent below first gear's idle speed parks.
The [brake-law record](../../../scratch/research/law-drive-4-brake-2026-11-02.md) owns its proof and decisions.
The protective upshift is absent: the existing clutch over-rev slip guard bounds crank speed
and retains motoring drag while the Driver holds overrun gear. Service braking at the ceiling
belongs to the next ceiling law. The bitprobe records load, price and delivered force.

MEASURED by `headless_test::drive_load` after one-rack fueling and `ClutchState` launch slip.
The instrument waits for the post-tick crank to settle within one rpm of its Slip target or In
shaft, then averages delivery over up to one interruption's coupled ticks (a new cut ends a
partial sample). The forecast belongs to the same landing gear; the raw first coupled force and
forecast speed error remain diagnostic fields. This replaces the first-tick 327,846 N bound,
which priced clutch lock-up impulses and even a still-spooling zero-force tick as steady work.
The following table records the scheduler before the accepted gear law.

| Case | L expected kN (DERIVED) | L range kN (MEASURED) | Sustained forecast gap N (MEASURED) |
|---|---:|---:|---|
| course 10 degrees | 124.632594 | 124.632531–124.632656 | n=2 min=2034.515625 median=35639.828125 p95=2034.515625 max=35639.828125 |
| course 20 degrees | 217.519797 | 217.519953–217.520109 | n=1 min=145695.187500 median=145695.187500 p95=145695.187500 max=145695.187500 |
| course 30 degrees | 303.797781 | 303.797375–303.797594 | n=1 min=-0.031250 median=-0.031250 p95=-0.031250 max=-0.031250 |
| concrete rolling=false | 22.366799 | 22.366783–22.366811 | n=7 min=-415.355469 median=14463.070312 p95=144832.265625 max=261415.078125 |
| concrete rolling=true | 22.366799 | 22.366783–22.366813 | none |
| dirt rolling=false | 33.550199 | 33.550172–33.550207 | n=6 min=-909.425781 median=35704.531250 p95=118062.031250 max=250715.406250 |
| dirt rolling=true | 33.550199 | 33.550176–33.550207 | none |
| gravel rolling=false | 27.958500 | 27.958521–27.958547 | n=7 min=-777.488281 median=14146.820312 p95=133674.093750 max=251190.718750 |
| gravel rolling=true | 27.958500 | 27.958521–27.958549 | none |
| mud rolling=false | 83.875500 | 83.875461–83.875555 | none |
| mud rolling=true | 83.875500 | 83.875453–83.875555 | n=2 min=815.613281 median=6097.515625 p95=815.613281 max=6097.515625 |
| sand rolling=false | 67.100398 | 67.100359–67.100422 | none |
| sand rolling=true | 67.100398 | 67.100359–67.100414 | n=1 min=2160.906250 median=2160.906250 p95=2160.906250 max=2160.906250 |

MEASURED signed gap range `-909.425781…261415.078125 N`; DERIVED span `262324.503906 N`.
MEASURED maximum absolute observer residual `0.406250 N`. DERIVED the bound is
`max(span, max(abs(gap))) + residual = 262324.910156 N`. At the fixture's `559170.000000 N`
weight this is rounded strictly upward at one part per million to `0.469133` of nominal
vehicle weight. `assets/driving/driver.ron` stores this dimensionless Resolution share, so
`M = 0.469133 W` (DERIVED `262325.099610 N` on the measured rig). The old multiplication-only second-weight fixture did not test a numerical bound;
the independent-weight instrument below replaces that claim.

This remains a large forecast-versus-delivery envelope: coupled crank acceleration and the
speed gained while coupling settles change delivery after the original landing forecast.
It is not an observer-noise-only bound or certification of a small-margin gear law. DERIVED
dirt's `M < 8.0 kN` premise is **not met**; its F8 convergence remains unproved. Sand's previously
MEASURED F7 spare near `−6 kN` still refuses that upshift for every positive M. Lane 3 must assess
those predictions with its actual gear law, including this inertia/forecast discrepancy.

MEASURED launch from rest to 0.5 m/s, before → after: concrete `0.593750 → 0.656250 s`,
sand `0.640625 → 0.671875 s`. Concrete to 2 m/s is `2.609375 → 2.593750 s`; sand does not
reach 2 m/s within either 40-second run under the unchanged scheduler. MEASURED paired shot
on an uninterrupted 20-degree fixture incline changes L by `0.093750 N`; each tick also checks
that L reads that plane, using a support-precision bound independent of M. Short ramp edges are
not a valid invariant fixture once the new launch moves the tank onto different support.

[Distributions and derivation](../../../scratch/research/drive-clutch-measurements-2026-11-02.json)
record every sustained sample and the raw first delivery. The historical first-tick record remains
in `drive-load-measurements-2026-11-02.json` for comparison.

**Same-instant calibration, with the gear lane stopped at its boundary.** The contact pass now
records an actual convex piece face for the Driver independently of the physical overlap law's
belt-frame orientation. A coarse overlap cell whose centroid ray strikes no face supplies no
geometric observation; the coarse physical reaction still runs. Turn resistance uses normal
pressure, `mu_t W cos(theta) length/(4 R_g)`, consistently with grip and rolling.

The same-instant instrument separates forecast-speed drift, actual crank rpm and crank
acceleration from the numerical residual. MEASURED 524 settled samples leave -1.546875 to
1.062500 N residual. Including the MEASURED 0.406250 N observer bound gives a DERIVED proposed
gear-comparison resolution `0.000006 W` (3.355020 N on the 559170 N rig). The preceding D2
262 kN envelope includes physical transients; the maximum landing mean contains about 202 kN
crank inertia and 64 kN speed drift, offset by about -5 kN crank-speed difference. It is not an
observer-noise deadband. The 30-degree near-equal price/delivery pair is grip-clipped on both
sides, so agreement there does not independently validate engine force. The 20-degree forecast
also cannot certify a rolling speed loss when the observed cut did not produce that loss.

The prior C1-C3 experiment stopped before acceptance. Its rolling S4 fixture produced a
false stationary equilibrium after relocation; those entry results are not certification. The
[bounded gear-lane handoff](../../../scratch/research/law-drive-3-gear-handoff-2026-11-02.md)
owns the full dispositions, draft limitations and decisions;
[its raw calibration](../../../scratch/research/drive-gear-measurements-2026-11-02.json)
retains every sample and landing mean. The unchanged-engine 20-tonne witness rejects weight-only M: MEASURED residuals
-0.792969 to 1.236328 N exceed its 1.177200 N bound. Observer arithmetic follows W because L is
homogeneous in W; crank subtraction instead follows `J*omega_over/dt*max(G/r)`.

DERIVED from the 524-sample span: 2.609375 N divided by the calibration rig's approximately
33,567,936 N crank arithmetic scale is 0.00000007774. Rounding upward to one significant digit
gives **0.0000001** of that scale. The observer's 0.406250/559170 share rounds upward to
**0.000001 W**. The production M is the sum of these independently scaled terms. Neither
coefficient comes from a desired gear. The independent-weight instrument holds each engine
fixed at 20 and 60 tonnes and checks the same-instant mechanical residual span against the
crank term; a separate observer witness checks its W term. These are finite fixture bounds,
not a claim that every possible construction has been measured.
The [gear continuation record](../../../scratch/research/law-drive-3b-gear-2026-11-02.md)
owns the executed proof and remaining S4 work.

**Rolling on flat ground costs nothing else.** `c_rr` is published motion resistance, which
lumps in running-gear losses, so the contact law spends nothing else on steady flat travel.
The elastic spring integrates the directional penetration profile. Its force acts along the
surface's outward **unit** normal at the profile resultant, and its damper reads
`closing = -normal · contact_point_velocity` (relative to a moving recipient). Thus a tangential
hull motion is not damped, and closing magnitude cannot exceed point speed. Traction lies in the
same surface plane and charges the belt what it delivers to the hull.

The surface owns that normal. A rounded block supplies its analytic field gradient at the ray
hit; a heightfield supplies the struck triangle's facet normal. The contact query is at the
resultant's own envelope point, and the force is applied at its actual hit. There is no difference
between neighbouring depths: a neighbouring miss, another first hit, a silhouette or a union seam
cannot manufacture a slope or magnify closing. `TerrainContact` names `Hit`, `Miss` and `Buried`.
A miss or a buried origin supplies no struck surface and carries no support; scalar depth still
returns clearance or saturated depth for geometry consumers. In particular, neither absence
falls back to the belt's normal.

**Piece orientation is a StandIn.** `track::rubble::in_slab` retains one innermost depth per
footprint, making every overlap face parallel to the belt. That coarse geometry therefore uses
`-out` for support and `out · relative_velocity` for closing. A physically level piece under a
tilted run can still inherit the earlier tilted-run loss. What settles the missing orientation
is the overlap's actual face normal; this change leaves `rubble.rs` and its recipient law alone.

**The 1c loss split, unchanged.** MEASURED headless before the flat-ground law, full throttle
with no ground term, the fixture tank at F8, 10.68 m/s, averaged over a second:

| share of the summed grip reaction | N |
|---|---|
| elastic support's horizontal share, tilted contacts | 13 365 |
| support damper reading `v·sinθ` where no depth changes, tilted contacts | 11 355 |
| traction along the tilted belt (`f` charged, `f·cosθ` delivered) | 85 |
| residual (hull acceleration) | 13 |
| total | 24 818 |


The 1c record after its change is 2 N at 10.93 m/s, the torque cut-off. The rolling class test
now also compares every moving contact's load with its parked load at the same phase, so changing
only closing to `out.dot(vel)` fails (MEASURED mutant: 39 moving contacts against 42 parked).
Fixture run directions are 26.6° behind (+z), 20.6° ahead (-z); forward hull travel is -z.

**Edges, MEASURED headless.** The baseline below is the 1c production force law re-landed on the
current supply law; the fixed tree changes surface normals and normal closing. Both use the same
fixtures. A downward probe 5 mm inside a flat heightfield edge, 5 cm deep with 0.5 m reach, reads
normal `(0.999835, 0.018179, 0)` before and `(0, 1, 0)` after. The force-level edge test also pins
every loaded column upright. A probe 5 mm inside a rounded step silhouette reads 107.31665 m/s
closing for a 2 m/s hull before, 1.66667 m/s after. The vertical building-corner force fixture,
crossed along world X (belt lateral), reads 108.31694 / 1.66665 m/s effective closing.

Prescribed-pad longitudinal force trace (positive z is rearward): fixture 2 m pad, its hull held
at y=0.15 m, moving forward (-z) at 2 m/s across a 0.20 m block ending at z=0. Values are the
summed hull support and traction; `forces::tests::forward_step_lip_force_trace` reproduces it.

| hull z (m) | 1c force (N) | surface-hit force (N) |
|---|---:|---:|
| 1.020 | 203 | 203 |
| 1.000 | 1 071 | 1 063 |
| 0.990 | 1 537 | 2 612 |
| 0.900 | 1 114 279 | 5 740 |
| 0.800 | 35 690 | 9 856 |
| 0.600 | 231 216 | 14 613 |
| 0.400 | 417 500 | 0 |
| -1.000 | 1 640 000 | 0 |

The full simulated hull also climbs forward over the course's 0.40 m curb. Fixture: start at
z=-63 from rest, facing -z, full throttle until its centre passes z=-70. The trace is emitted by
`headless_test::contact_law_forward_step_trace`; it sums support and traction, excluding collider
solver impulses. Peak and jump use every tick, rather than only the printed position bins.

| full-hull trace | 1c law | surface-hit law |
|---|---:|---:|
| peak rearward contact force (N) | 344 716 | 76 025 |
| largest rearward force increase in one tick (N) | 514 494 | 179 564 |
| maximum hull rise (m) | 0.39538 | 0.38309 |
| maximum relative excess over the unit-normal damping bound | 0.93656963 | 0.00000041 |

The fixed trace retains real rounded-face forces and contact-entry changes; it removes the
amplified damper. The bound reads the contact report's own normal velocity, avoiding a velocity
sample from another tick phase. The edge, buried/missing-probe, vertical-corner and full-climb
regressions all fail against the 1c force law. The earlier July lip flip was a wall/top hit-normal
change, while the reviewed defect was a depth difference across first-hit discontinuities.
Rounding makes a single primitive's front-facing turn continuous; the union's first-hit selection
still has silhouettes and seams. This is no claim of global force continuity, and the feel at
lips remains a playtest judgment.

The ride stays decaying, MEASURED with `headless_test::contact_law_roll_kick_decays`: a 0.3 rad/s
longitudinal-axis roll kick on the parked fixture, observed for three seconds. The RMS uses the
third second; peak tick-to-tick change in left-minus-right support and its sign alternation use
all three seconds. The side-swap limit cycle would alternate near 1 at half the 64 Hz step rate;
it does not reopen. No damping or grip parameter was retuned, and all settle gates pass.

| ride measurement | 1c law | surface-hit law |
|---|---:|---:|
| third-second roll-rate RMS (rad/s) | 0.00011802 | 0.00027498 |
| largest tick-to-tick side-load swing (kN) | 24.085 | 24.930 |
| swing sign-alternation fraction | 0.03158 | 0.03158 |

Coast-down, DERIVED: with the belts declutched, belt and hull decelerate together at
`a = c_rr·g·M/(M + 2I)` down to the fade band, then decay inside it, so a tank coasts
`d = (v₀² + V²)/(2a)` (`V` the band); no other term slows a declutched tank on flat ground, and
`forces::tests::the_same_vehicle_coasts_to_rest_over_the_distances_its_ground_derives` checks the
form. On the fixture tank (57 000 kg, 4 750 kg reflected per side) from 10 m/s: gravel (the base
pack, 0.05) 119.0 m; dirt (0.06) 99.2 m; sand (0.12) 49.6 m; mud (0.15) 39.7 m; concrete (0.04)
148.8 m.

Historical contact-law baseline, before the Driver laws; current acceptance belongs to the
gear and brake records above. Attained speed at full throttle on flat ground, MEASURED headless (fixture tank, last-second
mean of 40 s). The governed speed is DERIVED 10.509 m/s and the zero-torque cut-off 10.930 m/s;
the no-rolling-resistance fixture reaches F8, 10.930 m/s. Its FIXTURE acceptance is 1 % below cut-off for
finite settling and 0.1 % above for fixed-step integration and vertical ride velocity, rather
than calling either margin a rolling loss.

`headless_test::contact_law_shipped_ground_speeds` reads each shipped ground's full friction and
rolling-resistance pair and measures two entries: rest, and a fresh top-gear/governed-speed
rolling entry derived from vehicle data. The latter models entry onto a ground after attaining
speed elsewhere. These are attained states of the automatic scheduler, not a unique terminal
speed independent of entry. No ground-specific gear or pass condition is encoded in the test.

| ground | c_rr | earlier 1c record | surface-hit baseline from rest | surface-hit baseline rolling entry |
|---|---:|---|---|---|
| concrete | 0.04 | F8 10.71 | F8 10.70516 | F8 10.70516 |
| gravel | 0.05 | F8 10.65 | F8 10.64927 | F8 10.64927 |
| dirt | 0.06 | F7 7.19 | F7 7.19064 | F8 10.59344 |
| sand | 0.12 | F6 4.89 | F1 0.67151 | F7 6.25914 |
| mud | 0.15 | F5 3.31 | F1 0.67086 | F6 4.85348 |

All speeds are m/s. The recorded surface-hit from-rest speeds reproduce to five decimals with the 1c force
law on the same main/supply-law baseline: sand and mud's earlier recorded gears do not reproduce
under this protocol, so their shift is not attributed to the surface-normal fix. Both current
entry protocols end with scheduler `Normal` and hill hold false. The sand rolling-entry spread
is 0.003994 m/s over the final second; every other final-second spread is at most 0.000023 m/s.
The from-rest F1 hold is the upshift landing gate in `transmission::run_shift_decision`, MEASURED
headless on the fixture of `headless_test::a_tank_leaves_first_gear_from_rest_on_a_soft_ground`
(friction 0.4, rolling resistance 0.12) with the scheduler's inputs printed each tick: pinned in F1
at 0.6717 m/s (2591 rpm, past the 2300 rpm up band), the shift-landing predictor, charging the
33.6 kN a side of rolling resistance whole, lands F2 at 0.3617 m/s, 908 rpm, under the gate's fixed
`shift_down_rpm + POSTSHIFT_MARGIN_RPM` of 1300 rpm, although F2's modelled full-throttle reserve
at that landing is 264 kN against a 19 kN margin; a rolling entry never meets that gate, because
no downshift consults it. This historical scheduler observation is superseded by the accepted four Driver laws in `drive-control-design-2026-11-02.md`. The lane preserves the landed supply scaling in `drive_target` and
`transmission::step`. The step's interruption/dwell countdown is extracted without a behavior
change to keep it below clippy's function-length limit. No wire type changes.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude (law-ground-1b review) P2 | `c_rr`, published motion resistance, comes on top of a contact-law loss of about its own size, so hard grounds took about double their CLAIMED total and the tank never reached top gear | verified: 24 818 N MEASURED before any ground term, split above | driving: support and damp each contact along its surface, not its belt (#146) |
| Claude (law-ground-1b review) P3 | this section said no ground term slowed a declutched tank before the law, derived coast-down from `c_rr` alone and named the bristle dampers as the likely source | verified: the support damper acted with the clutch out; the bristle share was 85 N | docs: record the flat-ground contact law and its measured split (#146) |
| Claude (law-ground-1b review) P3 | `predict_shift_landing_m` decayed `c_rr·N` as relaxing drive shear, and `POSTSHIFT_MARGIN_RPM`'s note named only the grade as the sustained leftover | verified | transmission: keep the ground's rolling resistance whole through a shift window (#146) |
| Codex (law-ground-1c review) P1 | a neighbouring heightfield-edge miss becomes a fictitious lateral slope | verified by the 5 mm edge fixture and its force pass | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P2 | a first-hit jump at a step lip or building corner rotates support and amplifies closing; the column mid probe supplies other patches' normals | verified: the full-hull trace and lateral corner fail the old law; each resultant now queries its own hit. The rounded face's genuine inclination is not a defect | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | a saturated/missing mid probe silently takes the old belt-normal law | verified: `Miss` and `Buried` are named, carry no support, and the no-surface force regression fails the old law | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | the rolling test does not kill the belt-normal damper mutant and reverses fixture run directions | verified: per-contact moving/parked load comparison kills the mutant; directions corrected | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | piece overlap faces are parallel to the belt, not the piece's physical face | verified: orientation is explicitly a StandIn above, settled by the overlap face normal; recipient code stays outside this lane | driving: record surface contact traces and entry-dependent speeds (#146) |
| Claude (law-ground-1c review) P3 | the no-loss top-speed test lacks an integration margin and calls its band rolling loss | verified: margins now name settling, integration and vertical ride velocity | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review), July lip question | rounding was claimed to keep the depth gradient continuous across every lip | verified false at silhouettes; actual normals and bounded normal closing remove that inference, while true wall/top changes remain for playtest | driving: record surface contact traces and entry-dependent speeds (#146) |
| Claude (law-ground-1c review), slip-witness question | changing the scripted slip witness from 0.3 to 0.2 m/s weakened a physical bound | not a defect: it is a coverage witness, half the 0.4 m/s saturation scale; the corrected traction frame removes the tilted-run artifact. The replay remains green | transmission: keep the ground's rolling resistance whole through a shift window (#146) |

**Current soil-law verification.** The broad batch over driving, ground, prints, `headless_test`,
`net::` and whole-tree guards passed 733 tests, with 6 intentionally ignored manual measurement
or hosted-service cases. The expanded consumer batch passed 831 tests and exposed one stale VFX
opacity assertion: a mask byte cannot represent an exact half. That assertion now derives lift
from decoded weights. The final affected batch passed 208 tests (1 ignored), including that VFX
consumer, all force/ground/print tests, map publication, support-vector probes, curb climb,
roll decay and the whole-tree guards. Workspace/all-target CI-feature clippy and fmt are green.
The fixed-pad class witness gives MEASURED firm/loose pulls 14,404.124/11,034.170 N, ratio
DERIVED 1.305411 from `A*c + W*tan(phi)`. Stored-displacement tests prove the exponential
readout and both sides of the solver floor; paving tests prove the substance pair.
The off-centre force fixture also covers a lower surface with clearance at the pressure centroid:
refinement preserves support from its loaded neighbours. A probe's load-weighted mean normal
multiplied by its total load reconstructs the sum of its patches' support vectors.

Current ride/curb evidence, MEASURED by the existing headless fixtures: third-second roll RMS
0.00009436 rad/s, peak side-load swing 34.499 kN, alternation 0.03684; the curb traverse reaches
z=-70.03837 with 0.37050 m rise, 41.892 kN peak rearward force and 292.111 kN largest one-tick
rearward-force increase. The damping-bound relative excess is 0.00000066. These replace the
historical surface-hit values above for the current combined support/soil law; lip feel remains
a playtest judgment.

The following is the shipped fixture under the current law, not a reconstruction of the earlier
scheduler baseline. Speeds are MEASURED final-second means after 90 simulated seconds. Capacity
is DERIVED from the final contact pass, `sum(mu_equiv * elastic_load)`, and potential drawbar is
that capacity minus nominal `c_rr * W`. These are traction limits, not delivered engine force.
The two entry protocols give the capacity/drawbar ranges below. No frame/tick pricing was taken.

| ground | contact capacity (kN, DERIVED) | potential drawbar (kN, DERIVED) | from rest (MEASURED) | rolling entry (MEASURED) |
|---|---:|---:|---|---|
| concrete | 335.470–335.485 | 313.103–313.118 | F8, 10.650203 m/s | F8, 10.650212 m/s |
| dirt | 233.367–233.369 | 199.817–199.818 | F8, 10.569947 m/s | F8, 10.569968 m/s |
| gravel | 397.882–398.070 | 369.924–370.111 | F8, 10.610106 m/s | F8, 10.610106 m/s |
| mud | 154.714–154.717 | 70.838–70.842 | F6, 4.849178 m/s | F6, 4.849177 m/s |
| sand | 303.745–303.755 | 236.645–236.655 | F6, 4.874707 m/s | F7, 6.211259 m/s |

The shipped map's base pack is gravel. At the fixture's DERIVED nominal weight 559.170 kN,
the previous 0.9 grip gave DERIVED 503.253 kN traction capacity and 475.295 kN potential drawbar
with its 0.05 motion resistance. The current law's base-ground samples instead give about
398 kN capacity and 370 kN potential drawbar, equivalent to about 0.712 of weight before motion
resistance. Sand retains an entry-dependent sustainable gear split under the current Driver;
the test proves both equilibria rather than demanding the same gear.


Further review dispositions verified against this lane's starting HEAD; the preceding rows
retain law-ground-1c's existing loss and contact-normal record.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex law-ground-1b P2 | pieces received hard-coded zero rolling resistance | verified: a track still has running-gear motion resistance on rigid support | driving: derive track traction from soil shear (#146) |
| Codex law-ground-1b P3; Claude law-ground-1b P3 | mask path, terrain_blend module and DEPLOY payload prose were view-only | verified: synchronous mask decoding and server packaging already read these assets | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1b P3 | optional ground resource silently kept the pre-ground production path | verified: a map without a ground field now refuses; minimal server/reset compositions publish both | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1b P3 | mask sampling and CPU decoding were duplicated on clients | verified: PrintDepths shares GroundField; the GPU image uploads its already decoded bytes | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1b P3 | shipped-mask partition assertion could not fail | verified: decoder test now asserts only its resolution contract; independent spatial, row and capacity fixtures prove interpretation | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1b P3 | painted parser erased its error into String | verified: GroundMaskError preserves decode, colour and resolution variants to the named refusal | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1d P3 (step-lip test) | unit-normal closing bound held for any force law | verified: removed the cast-only identity; full-force corner and refinement tests own the contracts | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1d P3 (oracle direction) | depth-only view adapters were forced to implement contact queries | verified: DepthOracle is independent; TerrainOracle names the conjunction needed by simulation, preserving existing force-fixture bounds without editing rubble | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1d P3 (probe doc) | bitprobe doc named deleted depth differences | verified: it now names unrecorded surface quadrature casts; the aggregate normal preserves the summed support vector | driving: derive track traction from soil shear (#146) |
| Claude law-ground-1d P3 (centroid normal) | one centroid normal carried a whole curved/gapped column | verified: pressure is subdivided near changing normals and clearance centroids cannot carry support | driving: derive track traction from soil shear (#146) |

The law-ground-1d P1 shift gate and first P3 finite-only speed assertion are outside this dispatch;
HEAD already contains the Driver laws and the supported moving-equilibrium acceptance shown above.
No wire layout changed, so PROTOCOL_REV is unchanged.

**The tuning panel saves the label.** `rolling_resistance` is a `provenance::Labelled` value,
which keeps its job and note through a load and writes them back, so the panel's whole-table save
keeps every value's source (`ground_types::tests::a_saved_table_reads_back_unchanged`).

### Ground review: measured gaps and bounded fixes

The stop witness used the production `contact_side` path: a fixed flat pad, locked belt,
shipped mud, FIXTURE 4 m/s entry, 64 Hz, hull mass derived from elastic support divided by g,
20 simulated seconds. MEASURED before the observer/plane fix: 2.568670 m maximum advance,
1.199998 m rollback, peak reverse speed 0.480283 m/s. The pad stops, then gives its retained
16 K of skid displacement back. This is a real defect, not a harmless precision bound.
No unloading change is included in `driving: observe soil at nominal pressure and preserve patch shear (#146)`;
the flat-contact force law is unchanged, so this lane has no improved after-stop measurement.

The monotonic Janosi-Hanamoto curve does not specify unloading or a plastic set. Choosing
one requires the owner's design decision, including mixed contacts. Options and costs:

- Elastic/plastic split with an unloading stiffness and hardening history: preserves the
  monotonic curve and bounds recoverable travel, but adds per-element history, checkpoint/wire
  admission and a rule for dissimilar patches sharing an element. A wire change bumps the protocol.
- Force-state Dahl return with the soil curve's tangent: bounds reversal memory with existing
  vector state and can read each patch's curve, but changes the accepted elastic zone and
  cyclic/parking behavior. It needs an explicit hysteresis choice and new park/turn evidence.
- Ground-owned shear rather than pad-owned memory: models what yielded ground retains, but
  needs spatial history, ownership and lifetime rules beyond this contact law.

MEASURED turning witness: FIXTURE uniform 4.27 m pad, 1.4 m half tread, moving material
links at the pivot's belt speed on shipped gravel, steady final five seconds of a 20-second
run at 64 Hz. At 0.2 rad/s the resisting moment was 11,346.383 N m against the Driver's
30,332.969 N m price (37.406%); at 0.6 rad/s it was 19,460.770 against 30,374.113 N m
(64.070%). This measures the force path and material advection, not full-body yaw. The Driver
prices uniform saturated sliding; long-memory soil strain need not attain it. Steering on soil
is therefore overpriced in this witness. Choosing a mobilization-aware prospective price
requires a planned slip/dwell model; changing shear history shares the unloading decision above.
Neither is hidden behind a correction factor in this lane.

The Driver now sums `A*c` as newtons over its footprint and applies the geometric mean of
`tan(phi)` plus rigid pair shares to nominal normal weight. It never divides cohesion by
spring load. MEASURED fixed-footprint stiffness-halving witness: 135,545.84 to 163,975.27 N
before; 109,048.43 to 109,048.43 N after. The vanishing-patch gear witness keeps positive
current-gear spare and cannot pay a downshift from that tick. Shared world strain is projected
separately at each patch's force readout; a low-load curb cannot erase a flat patch by coming first.

`ForceParams::inertia` / backlog driving-09 remains verified and open: removing the field also
requires the force fixture initializer in `track/rubble.rs`, explicitly excluded from this lane.
The test-only no-ground rigid path similarly has callers in the excluded rubble fixtures;
its removal requires migrating those fixtures together rather than relocating the fallback.

The full shipped offline composition confirms the rebound with a hull free to settle under
Avian, not a fixed support pose. `headless_test::contact_law_service_stop_publishes_capacity_and_reports_rollback`
uses the shipped fixture, flat mud, FIXTURE 4 m/s entry in its declared top gear, held service
brakes, and 20 simulated seconds. MEASURED after the observation/plane fixes: 154,718.969 N
capacity at rest, 2.992694 m peak advance, 1.198098 m rollback, peak reverse speed 0.484893 m/s.
The initial red fixed-pad witness was 1.199998 m at 0.480283 m/s. These are different compositions;
there is no unloading fix or claimed before/after improvement. The full-app result confirms
the approximately 155 kN nominal estimate and the macroscopic recoil in the shipped physics.

The production-adapter service-stop witness also integrates the vehicle's declared mass and
holds service brakes through the real drivetrain. Its FIXTURE support is held at a fixed pose
and carries less than nominal weight: MEASURED capacity 106,355.062 N, peak advance 4.382679 m,
rollback 1.200275 m and peak reverse speed 0.467284 m/s over 20 simulated seconds. This is not
the nominal approximately 155 kN capacity in the shipped-ground table. The braking estimate
matches the contact pass's emitted capacity throughout; the rebound remains an unloading defect.

Mutation verification rejected all four independent mutants in their named witnesses:
angle `sin` instead of `tan`, omitted soil-share normalization of K, a default image bound as
the material mask, and raw-curve-only crank-inertia admission. The original spring-load and
first-patch projection defects were both red before their fixes. The schema-3 Python capacity
gate accepts below-capacity and zero-force/zero-capacity inputs, rejects over-capacity and
nonzero-force/zero-capacity inputs; its script parses and runs with the pinned script dependency.

**Feel and remaining review dispositions.** DERIVED parking under a uniform-pressure,
constant equivalent-capacity approximation, `j = -K_eff*ln(1-tan(theta)/mu_equiv)`:
at 20 degrees and 75 mm effective K, gravel at 0.712 W gives 53.684 mm; firm dirt at
0.417 W gives 154.667 mm. These are single-plane loading derivations, not full-app park
measurements; cohesion/area redistribution and unloading can change the actual settlement.
ADR-0026's approximately 28 mm claim is superseded. Turning is measured above. The shipped
firm-dirt capacity (approximately 0.417 W) is below loose sand's (approximately 0.543 W).
That ordering looks wrong to a player expecting firm ground to grip better. The facts remain
`Unknown`: soil density/moisture and direct shear plus shear-displacement measurements on
representative firm and loose grounds would settle it; this lane does not tune them to taste.

The passive pair changes apply to belts on rigid pieces as well as bump stops: DERIVED
1.026158 to 0.624500 on the fixture brick. Soil straight-line speeds remain the table above;
the ground/entry regression passed after nominal-pressure observation and data admission changed.
The full physical park/steering feel remains a playtest question, not a claimed measurement here.

Rigid friction now lives only inside a `Rigid` law. Soil cannot author an unused friction number.
`RunningGear` reads the single labelled undeforming-support resistance in `grip.ron`;
`Total` includes it and supplies a ground's total resistance. The shared fact now refuses a taste.
The windowed-world composition witness mounts the production installer, injects an asymmetric
mask at the data edge, and reads the actual material image and PrintDepths. Wheel-rise and
print appearance magnitudes are admitted labelled data, preserving their values.

Harness schema 3 records each contact's actual capacity; the analyzer no longer treats a
base-pair `mu` as soil grip or derives a rigid bristle stiffness from it. The old spring-energy
bound is removed: the soil unloading law is unresolved and capacity alone cannot establish
recoverable energy. This loses that false energy guarantee; a new energy witness belongs to
the owner's unloading decision. The force-capacity and finite-value gates remain.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude P1; Codex P2 | skid displacement returns on reversal; saturation clip controls behavior | verified by red locked-pad stop; unloading choice exceeds the monotonic law | pending owner: options above |
| Claude P3; Codex P2 | tiny soil share switches the whole element's return law | verified: `advance_element_strain` branches on positive soil share | pending with unloading/mixed-contact state |
| both P2 | Driver divides cohesion by spring load | verified red; stiffness-halving and vanishing-patch gear witnesses green | driving: observe soil at nominal pressure and preserve patch shear (#146) |
| Claude P2 | moving-belt soil scrub differs from uniform sliding price | verified by measured moment ratios above | pending owner: prospective mobilization or shear-history design |
| Codex P2 | first patch projects every patch's strain | verified red and green patch-order witness | driving: observe soil at nominal pressure and preserve patch shear (#146) |
| Claude P2 | soil rows author unused friction | verified; refused by the law's enum | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | duplicated 0.04 resistance and missing label admission | verified; one shared RunningGear source with fact-label admission | driving: make ground facts and stopping capacity explicit (#146) |
| both P3 | mask/terrain_blend/oracle/damping docs | verified against current consumers and force formula; docs corrected | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | no-ground test-only arm | verified; excluded rubble tests still rely on it | pending fixture migration outside this lane |
| Claude P3 | harness mu and analyzer stiffness/caps | verified; schema 3 publishes per-contact capacity, false energy inference removed | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | angle-to-tan and mixed K test gaps; empty paving loop | verified; independent angle ratio, soil-normalized K and nonempty witnesses | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | gravel template notes on other soils | verified; row notes now describe their own fact | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | braking estimate diverges from contact capacity | verified; stop estimate consumes emitted capacity | driving: make ground facts and stopping capacity explicit (#146) |
| Codex P2 | material/PrintDepths wiring unmounted | verified; production world installer mounted without an OS window | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | pair changes, park, turning and firm/loose feel absent from record | verified; measured and derived evidence distinguished above | driving: make ground facts and stopping capacity explicit (#146) |
| both P3 | ForceParams inertia / driving-09 | verified; excluded rubble initializer blocks removal | pending scoped fixture migration |
| Codex P3 | wheel rise and print pattern tastes in Rust | verified; moved to admitted data | driving: make ground facts and stopping capacity explicit (#146) |
| Claude P3 | edit-history labels, one-element tuple, diagnostic floor coupling | verified; comments/tuple cleaned, digest has its own logarithmic resolution | driving: make ground facts and stopping capacity explicit (#146) |

Backlog destruction-05 and destruction-08 remain verified and open: this lane corrects the
terrain_blend module's simulation-consumer prose, while pack membership/UV scale and blend
appearance still require the broader shared surface-data migration named by those rows.
The module doc change does not claim to close that work.

Final gates for `driving: make ground facts and stopping capacity explicit (#146)`:
MEASURED whole-library result 2,300 passed, 33 ignored (the on-demand measurement/pricing beds
were not run), including driving, ground, full-app and net coverage. The separate whole-tree
citation/UI/net-boundary/bake guards passed 44 tests; the focused ground/composition/capacity
batch passed 18. CI-feature workspace/all-target clippy and `cargo fmt --all --check` passed.
Python schema-3 syntax and the four synthetic capacity cases passed. The four deliberately
mutated Rust witnesses were red, then restored. No frame/tick pricing or captures were taken.

No replicated wire type changed: the new capacity is a local `TrackGripEffect` observation,
not serialized state, so the protocol revision is unchanged. The harness's separate JSONL
contract deliberately advances to schema 3. Unknown soil facts and presentation magnitudes
were not tuned. Every gameplay choice and pending owner decision is recorded above.

## 6. Tiers and budget (codex G — numbers corrected)

**There is no tier machinery, and the reason is now a measurement, not a deferral.** Every tank
gets the same kinematic wrap, because the wrap costs ~56 µs/tank/frame: 30 tanks is ~1.7 ms of
CPU, which fits the budget the tiering was invented to protect. The `TrackTier` enum, the
screen-space metric and the promote/demote machinery are NOT built and are not scheduled.

What the deleted chain tier cost, kept as the record of why the question closed: ~810–910
µs/tank/frame — 4 simulated + 26 route was `4×41 + 26×4 = 268 ms CPU/s ≈ 4.5 ms/frame at 60 fps`,
solver time only, against a 2 ms all-in budget. That arithmetic is what made tiers mandatory for
a solver view and irrelevant for a fitted one.

If tank counts ever do demand a cheaper far tier, the surviving design notes are:

- **Detail adds value only as motion the player can resolve at the current projection.**
- **The metric would be SCREEN-SPACE** (projected link pitch in pixels, with hysteresis), not
  distance: gunner optics at 8× promote the tank you're staring at automatically.
- Tiers must vary only state and sampling density, never behavior — a ribbon is the SAME belt
  sampled coarser, so there is no pop and no crossfade machinery.
- **Culled**: maintain scalar phase, produce no geometry. Distant visible tanks are NOT this.
- The render side has its own gate (§8), and it binds before the view math does.

## 7. Authoring schema (codex B + C — the many-tanks contract)

Material is authoritative and immutable; geometry reconciles to it, never the reverse:

```rust
pub struct MaterialLoop { pub pitch: f32, pub link_count: NonZeroU16 }  // length = pitch·count
```

No "round count and spread the residual" — that breaks both the step-24 pitch invariant and
tooth lock (one link advance ≡ one tooth advance). The **tensioner** (idler shift along an
authored travel axis) reconciles material length with gear geometry; `sag` is either a
validation measurement at a named span or the tensioner's solve target — never a free scalar.

```ron
track: (
    material: ( pitch: 0.130, link_count: 96, link_mass: 30.0, width: 0.725,
                pin_to_inner_face: …, pin_to_ground_face: …, pin_radius: …,
                max_articulation: …, kind: DryPin ),
    link_mesh: ( forms: ["TrackLink"],            // even/odd forms for alternating patterns
                 frame: (tangent_axis: Z, outward_axis: Y, width_axis: X) ),
    left:  ( drive: (node: "Sprocket_L", phase_marker: "Sprocket_Phase_L", teeth: …),
             idler: "Idler_L",
             axles: ["Wheel_L_0", …],             // ONE route circle + ONE suspension station
             return_rollers: [] ),                //   per axle — see interleaving rule below
    right: ( … ),
    tensioner: ( node: "Idler_L", travel_axis: …, travel_range: … ),
)
```

- **Axle topology, not disc listing** (codex B): interleaved discs (Tiger Schachtellaufwerk)
  are the axle's visual subtree (children of the axle node, or an explicit `spin_nodes` list) —
  they must NOT become duplicate route circles (coincident circles break `external_tangent`)
  or duplicate suspension stations. The route stays 2D per side; interleaving is an
  axle-grouping concern.
- **Drive end is derived** from the typed sprocket node's position — no redundant
  `sprocket: Front` field to disagree with geometry.
- **Sprocket phase-lock**: `angle = −phase / pitch_radius + baked_marker_offset`, with
  `pitch_radius = pitch × teeth / τ` **derived, never authored** (two numbers that must agree
  are one number) and never mesh bounds; the tooth-gap alignment comes from an authored radial
  marker node (`Sprocket_Phase_L`), baked once. Signs (derived by codex, shipped in phase A):
  positive phase = lower run rearward; **every axle angle is negative** (Bevy +X rotation moves
  a wheel's bottom toward −Z); the single flip point is `track::view::spin_angle`.
- **Drive identity**: the FIRST circle of a side's front→rear list is the drive circle — a
  sprocket-first hardcode shared by the route builder, the wrap's end arcs and the belt physics
  (front drive, fits the Tiger). A rear-drive vehicle needs drive identity derived from the typed
  sprocket node's position (named debt, not silent).
- **Bake extension**: bake today captures only collision/ballistic mesh data — add
  subtree-bounds extraction for wheel/idler radii (spec override allowed) and the phase-marker
  transform.
- **De-specialization** (codex C): `TRACK_WIDTH/THICKNESS`, gravity and drive identity → data.
  The whole solver-knob half of this item died with the chain: slack trim, rebase window, tube
  bounds, substep/sweep rates, guardrails and per-preset pin friction have no consumer, and the
  wrap has no knobs at all (its two filters are derived laws — §1a). `link_mass`, `hinge_torque`
  and `link_angle` stay AUTHORED and validated: they are measurements of the shoe, and they are
  the honest inputs a future per-vehicle return-run flavor would derive from.
- Presets (`DryPin` / `LiveBushed`) are unbuilt and now unmotivated: the surviving per-vehicle
  differences are geometry and mass, both already data.
- Phase A must **hide the legacy static track nodes** (`Track_Strip_*`, `Track_Treads_*`) or
  links double-render.

## 8. Rendering seam (codex H)

```rust
pub struct LinkPose { pub centre: Vec3, pub tangent: Dir3,
                      pub outward: Dir3, pub width_axis: Dir3 }   // full frame, not pos+tangent
pub trait TrackRenderer {
    fn update_instances(&mut self, tank: Entity, tier: TrackTier, links: &[LinkPose]);
}
```

- ~5,000 link instances at 30 tanks: entity-per-link is bring-up only; the scalable adapter
  uploads packed instance buffers grouped by (mesh form, material). The phase gate measures
  ECS + extraction + GPU + shadows, not just solver CPU.
- Policy: full links + shadows for the closest tier; decimated/ribbon Route for far tanks; far
  tracks cast no per-link shadows; even/odd forms keep stable material identity.
- Wide shoes get **lateral cant** fitted from the edge-column terrain samples (centre-only
  depth leaves a 725 mm Tiger shoe unable to roll over a curb edge).
- Wheel spin: `belt_surface_speed / r` on the axle's view subtree; sprocket/idler phase-locked.

## 9. Phasing

**Phase A — view promotion (SHIPPED, step 26 2026-07-17; view settled 2026-07-26, §1a):**
`TerrainMap` refactor in `world.rs`; pure core extracted into `src/track/` (sandbox re-imports); `track::view` with the
presented-pose seam, no-slip belt derivation (f64 phase, per-consumer wrap), entity-per-link
rendering (97×2 links, witness link 0, legacy `Track_Strip/Treads` hidden), oracle wheel lift
at disc-width stations, spec-validated + bind-time loop-feasibility gate, GLB-reinstance
rebind. Deliberately NOT built (owner LOC mandate): tiers, instancing, `PresentedFrame` ECS
state, `TrackRenderer` trait, `SpatialQueryOracle`, tensioner/presets. The view itself has since
settled on the kinematic wrap alone (§1a) — the chain tier is deleted, not deferred.
Deliverable met: Tiger 1 drives with live tracks in MP, zero sim risk. Tiger authoring unblocked.

**Phase B — sim promotion (SHIPPED):** collocation forces replaced `driving::{suspension,
traction}` under the ADR-0005 rewrite; the belt model is the game's drive sim; harness parity vs
sandbox baselines held. The raycast/track switch was whole-process, development-only, behind a
**deletion gate** — the raycast path is gone; no per-tank tier or alternate locomotion model
ships. (Grip and support have since settled further: per-element strain grip
[[0027-element-grip-netcode]] and the contact-envelope support calibration.)

## 10. Testing

- Pure core: unit tests (envelope: lifted/interleaved/return-roller/coincident-circle
  rejection; wrap: material-pitch station spacing over a session of travel, and its receipt that
  the naive drawn pitch drifts a pin onto a tooth).
- Sandbox harness stays the feel/regression lab; scenarios become CI-runnable with numeric
  gates (step-24 metrics + perf probes).
- Phase A adds a presented-pose torture scenario (scripted pose discontinuities — teleports,
  respawns →
  bounded belt-vs-ground error; tearing is unrepresentable now that the view is a filter over a
  fitted curve).
- Phase B adds A/B harness parity + MP soak gates.

## Open items (tracked, not blocking)

- Six-roller / discrete tooth engagement pulses (flavor; would ride the wrap's phase, not a solver).
- Thrown track as replicated damage state with alternate route topology (far future).
- Streaming/destructible terrain under the oracle (`covered`, chunk revisioning) — named in §5,
  deliberately unscheduled.
- **The park-hold cliff.** MEASURED: sweeping road-wheel tread radius over 500 µm moves parked
  drift by only ~0.005 m, then it falls off a cliff inside a 10 µm window (hold at 0.386470 m,
  no hold at 0.386460 m). The shipped radius, 0.386441 m, sits ~30 µm **past** the cliff. The
  discontinuity is a contact-regime flip, not a tuning curve; which regimes it flips between,
  and whether the envelope law should be continuous across it, is open.
