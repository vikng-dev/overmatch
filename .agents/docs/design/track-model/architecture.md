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
  `√(g / sag_depth)`). The ease and the spring's frequency are derived laws; the spring's damping
  ratio is a taste, ζ = 0.5 in `assets/driving/belt_view.ron`, since no pin-friction damping is
  sourced, and it sets how many visible swings a drape makes. Both filters are unconditional:
  there is no toggle to offer. A filter cannot tear; only a solver can.
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

**Current contact design:** `track-contact.md` in this directory owns how the belt meets ground,
structures and pieces; the terrain clipping and piece-reaction work recorded below is superseded
there where they differ.

Subject: the game's driving model, what the ground under a track changes; game state only.

Surface material is the force oracle's second query: `ContactOracle::ground_at(point)` returns
`ContactGround`. Depth-only consumers implement `DepthOracle`; a simulation `TerrainOracle`
requires both depth and contact contracts. The view's lowered `Sunk` implements only depth and
cannot supply simulation support. A published `TerrainMap` requires its decoded `GroundField`:
`build_track_field` refuses a composition missing it. Calibration runs support only. Bare inline force fixtures may explicitly state no ground
and use effective fixture grip with no motion resistance; that fixture path is test-only and a
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
track field; an authored block on the map reads the ground at its own XZ (a graybox construction's
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

**Grip strength and shear length per ground.** Every required ground row holds effective grip
`μ`, shear length `K_ground` (the ground slip that builds the full grip) and reference-pressure
rolling resistance `r_ground`, each a labelled fact or `Unknown`; a missing field is refused at
load, naming the file and the field. Terrain capacity is `μ N`, where `N` remains the existing
elastic normal load. It has no area term, soil/rigid selection or terrain material-pair
multiplier. `TrackSteel.friction` still supplies solid-recipient pairs (rubble and structures),
using `sqrt(mu_track * mu_recipient)`. The passive terrain collider's `terrain_solver_friction`
remains the independent Avian stand-in for solid-body contacts without a track footprint.

**The shear length the law reads.** A patch reads `K = max(K_ground, μ·k_step)`. A solid
recipient (rubble, a structure, a hull) has no ground row and stores no shear of its own, so it
reads `K_ground = 0` and the floor governs. `k_step` (`GripParams::step_shear_on`) is a
`Resolution` length per unit grip, derived from the vehicle's own data and the support's
response:

- The tick reads slip at its start, sets force from this tick's strain and lets Avian integrate
  after. For the hull riding an element's grip spring, with `x = √(μ g / K)·dt`, `a = κc·x²` and
  `b = 2ζκc·x`, one tick steps by `λ² − (2 − a − b)λ + (1 − b) = 0` (DERIVED). Both the spring and
  the damper act through the coupling `κc = κ + m·w`. `κ = 1 + m h² / I` is the hull's own:
  `h` the centre of mass's rest height over the footprint, `I` the softer of the pitch and roll
  moments of the hull's inertia box. `w` is the recipient's: the largest speed a unit tangent
  impulse at the patch gives its surface, `1/m_r + (r×t)·I_r⁻¹(r×t)` over the contact plane's
  tangents, since the patch reads slip relative to that surface and the recipient takes the
  opposite force. The field and static or kinematic bodies do not yield: `w = 0`.
- It is stable while `a + 2b < 4`. The floor takes the largest `x` with `b ≤ 1` and
  `a + b ≤ 2`, where no root has a negative real part, so the grip mode does not alternate at
  half the tick rate; a kick can overshoot once (at `b = 1` the first tick leaves `−a` of it).
  At the shipped `ζ = 0.5` and any `κc ≥ 1` the first binds: `k_step = g·dt²·κc²`.
- Patches on different supports compose. The hull's response is one shared mode and each
  recipient's another, so the pair's `a` and `b` are bounded by each patch's own, weighted by its
  share of the hull's weight; the two bounds are linear, so every mixture meets them. This reads
  the footprint's load as the hull's weight. Without the recipient term, a hull resting on an
  equal one sees `κc = 2κ`, and the floor gives `a + 2b = 4 + 2/κ`: unstable.
- The margin to the stable bound absorbs a footprint loaded at one end, where a lateral force
  also turns the hull in yaw and adds `m z² / I_yaw` to `κ`. For the shipped hull (DERIVED
  `κ` 1.62 at `h` 0.82 m; 2.42 with all load at a footprint end, `z` 1.8 m), the floor still
  gives `a + 2b` 3.9 there.
- An element spanning several patches shares one displacement across them, so their stiffnesses
  `μN/K` add: it reads `K = Σ μN / Σ(μN/K)`. No patch is stiffer than its load at its floor, so
  neither is the element; a frictionless patch (the map's edge wall) adds nothing, and an element
  with no capacity holds no strain.

DERIVED for the shipped hull at 64 Hz (the spawn log prints `κ` and `k_step`): `k_step` 6.31 mm
per unit grip; floors of gravel 4.49 mm, concrete 3.79, sand 3.43, dirt 2.64 and mud 1.75.
Ground data governs every soil, and the floor governs concrete and solid recipients. An earlier
derivation, `3a + 2b < 4` with `b = 2ζ√a`, overstated the spring term and left the coupling out
of the damper; it put the gravel bound at 3.67 mm. The corrected bare bound on gravel, 2.30 mm,
sits between the MEASURED hold at 3 mm and slide at 2 mm on a 20° park.

The seeds below are DERIVED from the previous shipped proxies at 89.5 kPa, rounded to three
decimal places. They remain `Unknown`: none measures this map's representative ground/track
pair. A track-pull and holding-limit test on each representative ground would settle grip;
pressure-varied declutched coast-down tests would settle rolling. Preserving a previous run's
output is not their justification. The proxy limitations are explicit pending better facts.

| Ground | Grip μ (DERIVED, Unknown) | Grip proxy derivation at 89.5 kPa | Rolling seed at 89.5 kPa (CLAIMED, Unknown) | Rolling at Unit 100 kPa (DERIVED, Unknown) | Shear length `K_ground`, mm (Unknown) |
|---|---:|---|---:|---:|---:|
| concrete | .600 | sqrt(.6 × .6) | .04 | .040000000 | 0 |
| dirt | .418 | tan(20°) + 4830 / 89500 | .06 | .062346369 | 10 |
| mud | .277 | tan(13°) + 4140 / 89500 | .15 | .162905028 | 25 |
| gravel | .712 | tan(35°) + 1040 / 89500 | .05 | .051173184 | 10 |
| sand | .543 | tan(28°) + 1040 / 89500 | .12 | .129385475 | 25 |
| sandy loam | .574 | tan(29°) + 1720 / 89500 | none: compaction, below | .043340820 | 10 |

Sandy loam is the base pack's ground (`coast_sand_rocks_02`, sand strewn with stones). Its row
reads one sourced specimen, NASA TM-20250006958 Table 1's sandy loam at 15% moisture (n 0.7,
kc 5.27 kN/m^(n+1), kphi 1515.04 kN/m^(n+2), c 1.72 kPa, phi 29°), attributed there to Wong's
*Theory of Ground Vehicles* (2001). Of that table's six sandy loam rows it is the one with a
sand's strength (beside dry sand's 1.04 kPa and 28°), as the pack's look asks, at a moist, not
wet, state: the 11% and 23% rows are cohesive (4.83 and 9.65 kPa), and the 22%, 26% and 32% fits
sink this pressure 0.4 m to metres, the boggy ground the mud layer stands for. Its stones carry
no load in the row: the soil fit stands for the ground a track crosses, and the stones are left to
the look and to strike sparks. Its rolling has no recalled seed: it is DERIVED as the hard datum plus Bekker's
compaction resistance per unit weight, `z / ((n + 1) L)`, at the Unit 100 kPa with the stamping
note's 0.70 m wide, 3.60 m long track: `z = [100 / (5.27/0.70 + 1515.04)]^(1/0.7)` = 20.446 mm,
so `.04 + 0.020446 / (1.7 × 3.60)` = .043340820. Compaction is a lower bound beside the other
rows' totals: it leaves out bulldozing and slip sinkage, and the coefficient divides by that
track's length, which the per-weight law does not carry. Gravel stays as a row for real gravel; no
pack is gravel, and the base-ground measurements in this section were taken while the base was.

The shear lengths are Wong (2001, *Theory of Ground Vehicles*, 3rd ed., §2.4.3) MEASURED values
standing as proxies: 10 mm for firm sandy terrain carries dirt, gravel and sandy loam, 25 mm for loose sand
carries sand, and mud borrows loose sand's 25 mm because Wong gives no wet-clay value (clay at
maximum compaction is about 6 mm). Concrete stores no soil shear; the steel link's own shear on
paving is unsourced (Wong's 75 mm, §6.4.3, is a fit for a rubber-padded track on paved road), so
the floor governs there. A shear-plate or track-segment shear-displacement test on each
representative ground would settle them.

The soil proxies were CLAIMED specimen fits reproduced in the stamping note, not measured map
soils: sandy loam for dirt, clayey soil for mud, dry loose sand for sand; gravel's angle and
sand cohesion proxy were provisional. Concrete's sliding coefficients were assumed. The
rolling seeds were recalled tracked-vehicle ranges without a checked source. Their uncertainty
survives the conversion. At half pressure, grip per weight no longer gains a cohesion bonus:
the model gives up the prior DERIVED 11.4/14.3/1.6/2.1% capacity on dirt/mud/gravel/sand
(rounded proxy comparison), until construction and ground facts justify a sinking model.

**Capacity-preserving arithmetic mixing.** Painted weights mix `μ`, `r_ground` and `K_ground`
directly. Capacity and the pressure-drag law are linear in the first two, so a mixture's
capacity and drag equal the same weighted sum of its constituents at any common pressure. The
shear length mixes by the same weights; the blend spans one 1.46 m mask texel, where no player
can tell one mixing rule from another. There is one rule across every ground; angles, cohesion
and rigid shares have no readers or schema fields. Every patch still projects the shared
recoverable state into its own surface plane. The shear law, damping, membership, dwell and
breakaway are otherwise those recorded under “Bounded recoverable shear”, which ran at a common
75 mm length; that length was a solver floor mislabelled as the hull's stability limit, and it
overrode every ground's own `K`.

**A surface normal carries its own pressure share.** Where the station or pressure-centroid hit normals differ,
`terrain_surface_patches` subdivides the profile to half `FIELD_ROUNDING` (DERIVED 0.015 m),
resampling depth at child stations. A normal variation below squared error `1e-4` needs no split
(DERIVED about 0.6 degrees); planar terrain keeps its original profile. A centroid with clearance,
a miss or a buried origin supplies no support. Thus a rounded lip cannot rotate an entire long
column's load, and a far wall behind a gap cannot carry load from the near lip. Convex contacts
carry the clipped face's own normal, as described below.

**Rolling resistance read against construction pressure.**

`rolling(p) = hard + (r_ground - hard) p / p_ref`.

`grip.ron::rigid_rolling_resistance` owns the global hard-support datum `hard` (Unknown .04).
`grip.ron::rolling_reference_kpa` owns `p_ref`, a Unit of 100 kPa: a round common scale,
independent of any vehicle. Each row is restated by
`r_100 = .04 + (r_89.5 - .04) × 100 / 89.5`; its provenance shows this arithmetic.
Rows must be finite, fact-labelled and at least `hard`, so drag is nondecreasing with pressure.
When `r_ground = hard`, rolling is exactly `hard` at every pressure.

`GripParams::nominal_pressure_kpa` is derived once during spawn construction: weight divided
by both tracks' nominal footprint, using the admitted shoe width and mean road-wheel-centre
span from `RigGeom`. It is independent of instantaneous patch load, support stiffness, damping,
pitch and contacts. Each side's finite positive station span is admitted by `RigGeom::validate`
during startup collider construction, before body spawning; the refusal names the sheet,
`roadwheels` and the repair. A structural tooling sheet may retain one wheel per side, but
such a construction cannot enter play. No spec field or footprint definition changes.
A fixture halving mass at fixed footprint halves this pressure. The law is a linear pressure proxy for compaction, omitting real sinkage, track-length effects and
sinking over time; it changes neither ride height nor cosmetic prints. No per-tank number
or bog rule is introduced. When rolling exceeds μ, maximum grip cannot sustain translation outside the rolling fade:
DERIVED the crossover is `p_ref (μ-hard)/(r_ground-hard)` for a ground above `hard`.

Rolling acts on the hull at each contact, opposite its longitudinal motion relative to the
support: `F_rr = -long_dir rolling(p) N clamp(v_contact / ROLLING_FADE_SPEED, -1, 1)`.
It is separate from the belt reaction, which sums only longitudinal grip. The engine therefore
feels drag through traction; above the fade, steady straight pull is
`min(engine pull, μ N) - rolling(p) N`. An arbitrarily strong engine cannot overcome drag above
grip. Inside the fade a crawl remains possible, with equilibrium speed
`ROLLING_FADE_SPEED min(engine pull, μ N) / (rolling(p) N)` on uniform flat support.
A spinning belt on a stationary hull creates no rolling drag. Solid recipients read `hard`.
The Driver reads no coefficient: rolling reaches it inside the resistance it feels (below).

The band has a second visible effect on slopes. With locked belts and saturated grip on
uniform support, `atan(μ) < theta < atan(μ + r)` admits steady downhill creep at
`band (tan(theta) - μ) / r` (DERIVED by balancing gravity against grip and faded rolling,
with `r = rolling(p)` and `band = ROLLING_FADE_SPEED`). Above the upper angle full rolling
cannot balance gravity, so the hull accelerates. This statement assumes the brakes can keep
the belts locked; brake-limited slides have another budget. The band and this creep remain
provisional for a later owner decision; they are not an extra material law.

The fade regularizes a hull damper, not a belt damper. DERIVED for a supported scalar hull:
`c dt / M = rolling(p) g dt / ROLLING_FADE_SPEED`; explicit integration is monotone below one
and stable below two. This replaces the retired belt-inertia slope calculation; it is not a
certificate for every coupled contact mode or arbitrarily large pressure. At mud's
DERIVED `r = .1715` at nominal pressure 107 kPa, the 64 Hz scalar ratio is about .105.

**The Driver reads what it feels.** `track::sense::DriverSense` keeps a ledger of the hull:
its speed along the hull's forward axis, and resistance `R = mean push - M_hull mean gain / dt`
over the last interruption's worth of driven ticks, `M_hull` the hull body's mass; the coupled
mass `M_c` prices only the landing. Push is the soft contacts' traction plus the solver's hard
friction on the hull, and gain is the change of world velocity, each projected on the heading
its tick opened with; the hard part arrives a tick late and the ledger aligns the two. Cut
ticks are not felt. The reading is `None` until one tick closes, and a Driver that has felt
nothing engages no gear from rest. Nothing modelled enters `R`: on a rolling incline it closes on
`(W sin(theta) + c_rr W cos(theta)) cos(delta)`, `delta` the heading's pitch off the plane under
drive (`headless_test::drive_load`). Parked, the contact law's rolling fade leaves the grade alone.
Beside `R` the ledger carries the engaged gear's drive (both sprockets' mean force over the
window), the drivetrain's gain over the window as a force (`M_hull` times the belts' mean
acceleration), the contacts' mean grip budget (the patches' and the solver's), a stall's bound,
and three counts of the engaged gear's run: the ticks it drove with the hull at rest, the ticks
its clutch held the crank to the belts, and the ticks it held them with the idle catch fueling past
the pedal.

**What the Driver infers.** Each fact is read from the force balance or the drivetrain's own
state, none from a timer or a band:

- *The engaged gear is observed, never priced.* What it delivers is its drive over a held window,
  the samples its gain reads, so no single tick decides; whether it holds is whether the
  drivetrain gains under it. A price models a gear not engaged, and on the
  engaged gear it reads the model's error (MEASURED on rubble, house stop-go: seventh priced
  547 N short of `R` while the hull gained, and the Driver shifted down).
- *The drivetrain, not the hull, tells a gear's verdict.* After a clutch takes up, the hull rings
  on its patches' elastic shear and the belts, carrying the crank's inertia, do not (MEASURED,
  flat ground at a third of the pedal, fourth to fifth: the hull's speed swung 2.235 to 2.201 m/s
  over six ticks while the belts rose 2.212 to 2.227 m/s; a window of the hull's gain read fifth
  losing, and the Driver hunted). `R` is the one hull window read: a ring or an impact moves the
  push and the gain together, so their difference holds through it.
- *Only a held clutch shows the gear.* A clutch at its capacity sends the crank's stored speed,
  whatever the gear (MEASURED, same shift: four ticks at capacity lifted the push to 214 kN
  against 35 kN once held), so drive and gain are read only over a window the clutch held whole.
- *A stall gives a bound, not a value.* At rest under a push that moved neither the hull nor its
  belts (at rest, or not gaining), the world returned at least that push. The ledger holds the
  largest such push along the drive until the hull moves or the pull is withdrawn, and `R` is at
  least that bound. Belts gaining under a stalled hull slide at the ground's limit, which no gear
  raises (MEASURED, ram into the standing heap: third, second and first each stalled with the
  drivetrain gaining 185–259 kN); belts turning steadily are held by the gear (MEASURED, standing
  heap stop-go: fourth spun its belts at a steady 2.175 m/s, its crank governed, under a hull at
  rest for 1226 ticks). A crank spooling to its launch rpm sends no drive, so its ticks fail no
  gear. A bound is a floor: every weaker gear holds against it on paper, so a stall takes the
  lowest gear in reach. The bound outlives the stall, so the step down reads whether the hull
  stalls now: a window the gear drove whole with the hull at rest, over which its belts came to
  rest or did not gain. The window establishes it, not each tick (MEASURED on the rubble battery:
  a test of every tick fired no stall step on the heap rams and tripled the stalled ticks). Belts
  that start gaining under a hull still at rest keep the bound and step nothing down.
- *Falling revs fault the engaged gear, not the gear below.* A drivetrain losing over a held
  window says the engaged gear does not hold; a lower gear is taken only where it passes the
  upshift's own test, mirrored: it holds where the cut lands and pulls there more than the engaged
  gear's drive by M. Neither the crank's place against its torque peak nor its torque's slope
  decides. The cut costs speed (MEASURED on a 20° grade: stepping down from fourth, which settled
  at 1.93 m/s with its drivetrain reading −4 N, cost a metre per second in the cut, and third
  settled slower), so every candidate is priced where the cut lands.
- *The Driver keeps no failure memory.* Each clause chooses from the current window alone, so a
  gear just left is chosen again wherever its forecast clears, and may fail there again. The cause
  is open: the price and the sensed resistance sit on different sides of the belt, so a gear can
  look affordable where it just failed (MEASURED, ram-x-0.35: fifth stalled sending 156.9 kN at
  the sprocket while the ledger read `R` = 104.9 kN).
- *A gear the idle catch carries fails the pedal.* At part pedal against more than the pedal
  pulls, the engaged gear slows until its crank falls into the idle catch's band, where the catch
  fuels past the pedal and holds a crawl that neither slows nor stalls. A gear the catch carried
  through a whole held window holds only on torque the pedal did not ask for. While the catch still
  fuels at the demand the Driver reads, the step takes the highest lower gear within `max_step`
  that holds where the cut lands, by the slowing step's own test, else the lowest within
  `max_step` whose crank lands short of completed fuel cut, else none; a gear still carried there
  steps again. A candidate's clutch closes from out at or
  above idle, where the catch adds nothing, so its price is the pedal's own. `max_step` is a fact
  about one selector event, not a reach to widen. The catch draws no more than the served rack
  gives at full command, so at full pedal it adds nothing at any task rate, and a window carried at
  part pedal moves nothing once the demand reads full.
  MEASURED in `headless_test::drive_load` (pedal 0.25, fifth entered at mid-band against a
  46.5 kN grade): without the clause the catch carried fifth for 2334 consecutive held ticks at
  0.74 m/s and 567 rpm, no cut paid, 31.9 m in 40 s; with it no gear in reach held, so fifth
  stepped to second after 20 carried ticks, second fell to first on its slowing drivetrain 55
  ticks later, and first ended at 2404 rpm and 0.62 m/s, 26.3 m in 40 s: slower than the crawl,
  which ran on the catch's torque. MEASURED on the rubble battery: the clause fires on the four
  part-pedal stop-go restarts (the lowest-in-reach form moved them, fifth to second and sixth to
  third), and there it lands the gear the slowing step took without it, so every row and every
  turn-probe row match the Driver without the clause.
- *Grip caps push.* No gear puts more traction into the ground along the belts than its
  contacts' grip budget, which they carry when every contact slides along the belt: the patches'
  `sum mu N_elastic` and the solver contacts' Coulomb limit, the solver's combined coefficient
  times each point's normal force, which `ContactTransfer` carries from the constraint. Every
  window has a cap.

`transmission::choose_gear` prices a candidate with `gear_force`, engaged as delivery engages it
(`engagement`): it pays the cut and lands at the hull's `v - R t_cut / M_c`, no lower than rest,
where the park latch holds a hull the cut stops. Admission refuses a cut that rounds to no fixed
tick, so every gear change opens the clutch and delivery closes it from out, as priced: below its
launch speed it slips at the launch rpm. The price reads the shaft at the hull's landing speed and
delivery at the belts' mean speed; they agree where the belts roll with the hull, and the belts'
slip is not forecast. A coupled gear adds reflected crank inertia, the clutch caps the pull and the
grip budget caps it next. A pull is priced whole at the sprocket. A candidate *holds*
when its pull, less `R` (at a stall, at least the bound) and its detent's share of the belt push
difference, exceeds M. The clauses:

1. Clutch out at hull rest: the highest launch gear that holds, at a stall none above the
   current gear nor at or above one that failed there; else
   first where it offers more spare than the current gear. Free.
2. The hull stalls now under the gear's drive: the lowest gear within `max_step`. Or the idle
   catch carried the gear through a held window and still fuels at this demand: the highest lower
   gear within `max_step` that holds where it lands and pulls there more than the engaged gear's
   drive by M, else the lowest within `max_step` whose crank lands short of completed fuel cut.
   Or the drivetrain loses more than M over a held
   window: that highest holding gear; with none the gear stays.
3. Straight, the clutch in, a held window and the hull moving where the cut lands: of the higher
   gears within `max_step`, those that hold where they land and pull there more
   than the engaged gear's drive by M, the one that pulls hardest, the higher on a tie. Gears
   landing below their launch speeds slip at the launch torque, the higher through the smaller
   ratio, so the highest that qualifies is the weakest (MEASURED on rubble: fourth stepped to
   sixth at 100 kN past fifth at 148 kN and slipped for 370 ticks). A slipping clutch delivers at
   most its launch torque, and a higher gear lands slipping at no more. A cut that stops the hull
   trades a moving gear for a launch priced against what the crawl read, not what stopped it
   (MEASURED, ram into the standing heap: first crawling at 0.05 m/s stepped to second, the cut
   stopped the hull, and the free choice launched fourth, which had stalled there, three times).

Clauses 2 and 3 pay the cut. No pull asked (released, braking, a request against the engaged
ladder) or fuel under drag chooses nothing. The Driver uses requested pedal, not its actuator
ramp. A slipping clutch holds its launch floor, capped at the speed the crank reaches freely, so a
launch takes none of the flywheel's stored energy below that floor; a crank above the floor when
the clutch closes can spend the excess. M is the crank arithmetic term alone.
A slowing step down and an upshift cannot undo each other at one landing where every gear's price
meets its observed drive: each takes a gear only where its price where the cut lands beats the
engaged gear's drive by M, so a step down from `g` to `g − 1` needs `P(g − 1) > P(g) + M` and a step
back up needs `P(g) > P(g − 1) + M`, which cannot both hold; the same holds from an upshift back
down. The argument covers nothing else. A stall, or a carried gear no lower gear in reach holds
for, steps down even where `P(g − 1) ≤ D(g) + M`, and steady pedal and load do not by themselves fix the landing speed, so two
decisions need not share one. A cut and a full window of the new gear's drive separate any two decisions. Where the price
and the delivery disagree by more than M, the Driver can still hunt (the failure-memory entry
above).
Rejected forms: pricing the engaged gear (its spare reads model error); pricing a candidate at a
crawl as a lug rather than the slip its clutch closes into (it carries the idle catch's whole
torque and outbids a part-pedal launch, which then stops in the cut: MEASURED in
`part_pedal_launches_and_sustains_ordered_speed`, a quarter-pedal launch cycling between its
launch gear and a lug three gears up); a memory of the resistance a gear failed under, with or without the crank
speed it failed at (its evidence holds only at the operating point that set it: a slower landing
that slips at the launch rpm, a crank above the torque peak and better grip each escape it, a
recovered lower gear wedges, and easing past M reopened gears on window noise); a memory of
`R + M_hull a_belt` (it reads negative while a clutch takes up, MEASURED −1699 N on the house
rubble, and a negative record refuses a recovered gear under any resistance); a memory of the
spare a gear lost with, its window's sprocket drive less the load read there (MEASURED on the
rubble battery: no gear re-engaged on a forecast at or below it, but barred gears could not return
during a pedal hold: on yard-t0.3-x1.4 second was barred at 61.0 kN against a 29.6 kN forecast
and the hull finished in first, on house-t0.3-x0-stopgo fifth at 24.2 kN against 13.0 kN), where
no memory costs seven such re-engagements on four rows and every row still crosses; pricing a pull as the
share of the belts' reaction that reaches the hull (the share differs between gears where solver
contacts load the belts off the heading: 0.92 in sixth against 0.90 in seventh on rubble); a
slide test on slip distance over the elastic shear length (slip distance is unimodal across it on
the rubble battery); the hull's own gain as a gear's verdict (the ring above); a stall stepping to
the highest lower gear that holds against its bound (a floor prices every weaker gear as
holding); a destination planned across combined cuts, or a reach that follows need, to leave a
crawl the idle catch carries (`max_step` states one selector event); a carried gear always taking
the lowest gear within `max_step`, as a stall does (it passes a lower gear that holds: DERIVED on
the lab ladder of `transmission::tests`, fifth carried at 580 rpm meets about 3.9 kN, fourth lands
above its launch speed pulling about 5.5 kN, and second, the lowest in reach, turns past its
crank limit and pulls backward); a carried gear no lower gear holds for taking the lowest in reach
wherever its crank lands (DERIVED on the lab engine with couplings of 100, 50, 45, 40 and 10 per
metre: fifth carried at 580 rpm, fourth, third and second land at 2312, 2601 and 2889 rpm, past
completed fuel cut at 1900 rpm, and second was taken pulling backward); a carried window acted on whatever the demand (a window carried
at part pedal steps down a gear the full pedal that follows would hold); a slowing gear kept while its crank runs above the torque peak, with admission refusing
a curve that falls below its peak (it fits one curve shape, and a lower gear can outpull a crank
above its peak); a slowing gear kept where its crank's net torque rises as revs fall (that marks
a stable crawl, not a gear that serves the Driver: DERIVED, the Tiger-modelled ladder's top gear
at 800 rpm and 0.3 pedal against 7.1 kN settles near 724 rpm while seventh has ample spare;
MEASURED on the rubble battery, six part-pedal stop-go rows lost every step down and ended 4 to
19 m shorter); a step down to a gear that outpulls the engaged drive without holding (a lower
gear's tighter detent load goes unpriced, MEASURED on the turn probe: 18 of 71 rows moved, tight
turns from fourth fell to first at a 4.7 m radius and 0.64 m/s against 15.8 m and 2.16 m/s, and on
the rubble battery ram-x-0.35 reversed gear direction nine times against four); and keeping the
park latch off under drive (the one-way lever already carries
rollback there, and the latch is what holds a hull at rest the tick a cut opens its clutch).
The brake lever carries only the stop-force shortfall against intent, without a release latch.
Opposite input brakes to rest, waits the authored pause, then pays one direction cut;
release during the pause keeps park held. Zero intent below first gear's idle speed parks.
The [brake-law record](../../../scratch/research/law-drive-4-brake-2026-11-02.md) owns its proof and decisions.
The protective upshift is absent: the existing clutch over-rev slip guard bounds crank speed
and retains motoring drag while the Driver holds overrun gear. Service braking at the ceiling
belongs to the next ceiling law. The bitprobe records the reading, the engaged gear's price and
the delivered force: `headless_test::drive_load` splits each landing's forecast gap into the
landing speed's share and the price model's share at the engaged point, and the chooser never
reads that price.

**Historical drivetrain and soil-law measurements.** The following record predates direct
μ and pressure drag. Its figures are observations of those laws, not acceptance conditions
for the current coefficients. New measurements belong to the lane report below.

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
records the same clipped convex face for the Driver and the physical contact law. Turn resistance
uses normal pressure, `mu_t W cos(theta) length/(4 R_g)`, consistently with grip and rolling.

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
gives **0.0000001** of that scale. The production M is this crank term alone: the observer and
its W share retired with the modelled load. The coefficient does not come from a desired gear.
The independent-weight instrument holds each engine fixed at 20 and 60 tonnes and checks the
same-instant mechanical residual span against the crank term. These are finite fixture bounds,
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

**Convex faces keep their geometry.** `world_contact::convex` clips each solid's entry faces to
its belt element's envelope and partitions projected coverage at face edges and depth crossings.
Each covered area belongs to its deepest surface. Linear penetration integrates over the actual
polygon, with its pressure centroid on the face and its outward normal shared by support,
damping, grip and Driver observation. Tangential motion over a level solid has zero normal
closing even beneath an inclined belt. The field still supplies sampled profiles; directional
suspension compliance and the recipient solve remain separate migration work.

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
closing for a 2 m/s hull before, 1.66667 m/s after. The vertical construction-corner force fixture,
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
form. On the fixture tank (57 000 kg, 4 750 kg reflected per side) from 10 m/s: gravel (then the
base pack, 0.05) 119.0 m; dirt (0.06) 99.2 m; sand (0.12) 49.6 m; mud (0.15) 39.7 m; concrete (0.04)
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
| Claude (law-ground-1c review) P2 | a first-hit jump at a step lip or construction corner rotates support and amplifies closing; the column mid probe supplies other patches' normals | verified: the full-hull trace and lateral corner fail the old law; each resultant now queries its own hit. The rounded face's genuine inclination is not a defect | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | a saturated/missing mid probe silently takes the old belt-normal law | verified: `Miss` and `Buried` are named, carry no support, and the no-surface force regression fails the old law | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | the rolling test does not kill the belt-normal damper mutant and reverses fixture run directions | verified: per-contact moving/parked load comparison kills the mutant; directions corrected | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review) P3 | piece overlap faces are parallel to the belt, not the piece's physical face | resolved by the convex adapter: clipped faces own their normals; the recipient solve remains separate | driving: record surface contact traces and entry-dependent speeds (#146) |
| Claude (law-ground-1c review) P3 | the no-loss top-speed test lacks an integration margin and calls its band rolling loss | verified: margins now name settling, integration and vertical ride velocity | driving: read contact normals from struck surfaces (#146) |
| Claude (law-ground-1c review), July lip question | rounding was claimed to keep the depth gradient continuous across every lip | verified false at silhouettes; actual normals and bounded normal closing remove that inference, while true wall/top changes remain for playtest | driving: record surface contact traces and entry-dependent speeds (#146) |
| Claude (law-ground-1c review), slip-witness question | changing the scripted slip witness from 0.3 to 0.2 m/s weakened a physical bound | not a defect: it is a coverage witness, half the 0.4 m/s saturation scale; the corrected traction frame removes the tilted-run artifact. The replay remains green | transmission: keep the ground's rolling resistance whole through a shift window (#146) |

**Historical soil-law verification (retired constitutive data).** The broad batch over driving, ground, prints, `headless_test`,
`net::` and whole-tree guards passed 733 tests, with 6 intentionally ignored manual measurement
or hosted-service cases. The expanded consumer batch passed 831 tests and exposed one stale VFX
opacity assertion: a mask byte cannot represent an exact half. That assertion now derives lift
from decoded weights. The final affected batch passed 208 tests (1 ignored), including that VFX
consumer, all force/ground/print tests, map publication, support-vector probes, curb climb,
roll decay and the whole-tree guards. Workspace/all-target CI-feature clippy and fmt are green.
The fixed-pad class witness gives MEASURED firm/loose pulls 14,404.124/11,034.170 N, ratio
DERIVED 1.305411 from `A*c + W*tan(phi)`. The retained force-pass witness proves linear common-compliance readout; solid-recipient tests
prove the substance pair. Ground-specific K and paving-pair witnesses retired with their laws.
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
is DERIVED from the final contact pass, `sum(mu_equiv * elastic_load)`, and capacity less nominal drag is
that capacity minus nominal `c_rr * W`. These are force budgets, not delivered engine force. In this historical run rolling acted only on the belt, so the difference was not a delivered hull-force limit.
The two entry protocols give the capacity/drawbar ranges below. No frame/tick pricing was taken.

| ground | contact capacity (kN, DERIVED) | capacity less nominal drag (kN, DERIVED) | from rest (MEASURED) | rolling entry (MEASURED) |
|---|---:|---:|---|---|
| concrete | 335.470–335.485 | 313.103–313.118 | F8, 10.650203 m/s | F8, 10.650212 m/s |
| dirt | 233.367–233.369 | 199.817–199.818 | F8, 10.569947 m/s | F8, 10.569968 m/s |
| gravel | 397.882–398.070 | 369.924–370.111 | F8, 10.610106 m/s | F8, 10.610106 m/s |
| mud | 154.714–154.717 | 70.838–70.842 | F6, 4.849178 m/s | F6, 4.849177 m/s |
| sand | 303.745–303.755 | 236.645–236.655 | F6, 4.874707 m/s | F7, 6.211259 m/s |

The shipped map's base pack was gravel when these were measured. At the fixture's DERIVED nominal weight 559.170 kN,
the previous 0.9 grip gave DERIVED 503.253 kN traction capacity and 475.295 kN capacity less nominal drag
with its 0.05 motion resistance. The current law's base-ground samples instead give about
398 kN capacity and 370 kN capacity less nominal drag, equivalent to about 0.712 of weight before motion
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

### Historical soil-law review: measured gaps and bounded fixes

The stop witness used the production `contact_side` path: a fixed flat pad, locked belt,
shipped mud, FIXTURE 4 m/s entry, 64 Hz, hull mass derived from elastic support divided by g,
20 simulated seconds. MEASURED before the observer/plane fix: 2.568670 m maximum advance,
1.199998 m rollback, peak reverse speed 0.480283 m/s. The pad stops, then gives its retained
16 K of skid displacement back. This is a real defect, not a harmless precision bound.
No unloading change is included in `driving: observe soil at nominal pressure and preserve patch shear (#146)`;
the flat-contact force law is unchanged, so this lane has no improved after-stop measurement.

**Decision: one Dupont elastic-zone/Dahl force-state return for every contact.** Plain Dahl
with `alpha = 1` preserves Janosi-Hanamoto's monotonic curve (`dz/dt = sdot - |sdot|*z/K`),
but the controlled closed-slip witness loses holding force; the accepted elastic zone retains it.
This deliberately changes the soil loading knee, using the existing labelled breakaway stand-in
rather than adding history or another material switch. An elastic/plastic split with hardening
was rejected because it needs extra per-element history and mixed-patch ownership rules;
ground-owned shear was rejected because it needs spatial history and lifetime rules. Neither
is needed to pass the stop, shove and parking measures. The `16 K` displacement clip and
soil-share-selected return/readout are deleted; the losing A/B arm survives only in this record.

Baseline MEASURED turning witness before bounded return: FIXTURE uniform 4.27 m pad, 1.4 m half tread, moving material
links at the pivot's belt speed on shipped gravel, steady final five seconds of a 20-second
run at 64 Hz. At 0.2 rad/s the resisting moment was 11,346.383 N m against the Driver's
30,332.969 N m price (37.406%); at 0.6 rad/s it was 19,460.770 against 30,374.113 N m
(64.070%). This measures the force path and material advection, not full-body yaw. The Driver
prices uniform saturated sliding; long-memory soil strain need not attain it. That baseline overprices steering. Bounded return closes most of the gap as measured below;
the remaining gap is reported without a correction factor or a prospective slip/dwell model.

The Driver now sums `A*c` as newtons over its footprint and applies the length-weighted arithmetic mean of
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

**Historical loading-curve derivation.** Before bounded return, DERIVED parking under a uniform-pressure,
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
Physical park/steering measures for bounded return follow; their feel remains a playtest judgment.

In that historical schema, rigid friction lived only inside a `Rigid` law. Soil cannot author an unused friction number.
`RunningGear` reads the single labelled undeforming-support resistance in `grip.ron`;
`Total` includes it and supplies a ground's total resistance. The shared fact now refuses a taste.
The windowed-world composition witness mounts the production installer, injects an asymmetric
mask at the data edge, and reads the actual material image and PrintDepths. Wheel-rise and
print appearance magnitudes are admitted labelled data, preserving their values.

Harness schema 3 records each contact's actual capacity; the analyzer no longer treats a
base-pair `mu` as soil grip or derives a rigid bristle stiffness from it. The old spring-energy
bound is removed: capacity alone cannot establish recoverable energy. The bounded-return
witnesses below establish travel, not a contact-trace energy guarantee. The force-capacity and finite-value gates remain.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Claude P1; Codex P2 | skid displacement returns on reversal; saturation clip controls behavior | verified red in fixed pad and full app; bounded force-state return | driving: bound recoverable ground shear on reversal (#146) |
| Claude P3; Codex P2 | tiny soil share switches the whole element's return law | verified; soil-share branch and both readout paths removed | driving: bound recoverable ground shear on reversal (#146) |
| both P2 | Driver divides cohesion by spring load | verified red; stiffness-halving and vanishing-patch gear witnesses green | driving: observe soil at nominal pressure and preserve patch shear (#146) |
| Claude P2 | moving-belt soil scrub differs from uniform sliding price | verified; bounded return closes most of the gap, remaining gap reported below | driving: bound recoverable ground shear on reversal (#146) |
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

### Direct-grip and pressure-drag verification (#198)

The force-pad matrix derives slope demand from fixture weight and angle, exercises every
shipped ground at full and half normal load, and checks both sides of `atan(μ)`. Settlement
is printed, not pinned. Saturated pull is independently `μ N`; an unrelated track-material
coefficient cannot change it. The existing closed 32 Hz dither and reversal witnesses retain
the unchanged shear law's evidence. Linear compliance readout uses sub-breakaway fixture
strains, rather than a removed per-ground K. Painted blends preserve capacity and drag.
The Driver stiffness witness independently derives grip and rolling load from ground data.

Retired evidence: the soil-angle conversion and mixed soil/rigid tests belonged to the removed
law; direct grip and arithmetic mixture witnesses replace them. The ground-specific K
witness retired because no ground selects K; common-compliance readout supplies its live
evidence. Terrain-pair assertions retired; direct-ground pull and solid-recipient pair tests
cover their separate consumers. The former one-ground late-creep threshold had no derivation;
the slope matrix now owns holding/sliding, and the full-app slope matrix uses the shipped brakes and transmission on tilted ground, checking
holding below both limits and motion above the lower one. `ForceParams::inertia` is deleted: `DriveConstruction` owns
inertia, and arithmetic tests hold their own inline fixture construction.

`rig_geom` owns the nominal-footprint pressure derivation; the sim stores its result at spawn,
and prints reuse that physical construction fact. The pressure helper's formula is unchanged.

The inertia-owner removal also closes the transmission admission ratchet: idle-catch slope,
viscous drag and fuel-cut loss reconstruction join the net-curve slope bound. The refusal names
both `idle_catch_droop_rpm` and `governor_cut_rpm` in `assets/driving/engine.ron`. The shared
arithmetic fixture uses Fixture inertia 6 kg m² (previously 4), above its DERIVED approximately
5 kg m² minimum. Bake/spec inline fixtures and the Blender fixture use 4 kg m²; their former
3/3/2 kg m² values did not resolve the idle catch. Shipped construction is unchanged. Released pivot testing requires monotonic
arrest without reversal, rather than the unsupported 32-tick/0.01-speed observation window.
The sandbox outer companion reads measured `pin_to_outer`. Loose-piece rise and peak piece motion are observations; the supported-on-piece and footprint
checks supply the composition evidence. The former rise/energy ceilings could not detect the
claimed defect and are removed.
These close driving-05/06/09/15/16/17 and dev-23 after verification against source.

Authored-number census (DERIVED by field/control count; inline Fixture numbers excluded):

| Scope | Before | After |
|---|---:|---:|
| Ground-specific traction/compliance facts | 13 | 5 |
| Ground rolling plus shared hard datum | 5 | 6 |
| Common pressure Unit | 0 | 1 |
| Track friction | 1 | 1 |
| Retained K, damping, fade, enter, leave, dwell, breakaway | 7 | 7 |
| Total | 26 | 20 |
| Including the two retained lip-resolution controls | 28 | 22 |

Published ground data changed, so protocol revision advances to 83; serialized wire layouts
are unchanged. The protocol pins and whole-tree guards verify that distinction.

**Measured outcomes, direct grip and pressure drag.** Previous numbers below are the historical
bounded-return record, not a fresh baseline run; new numbers are MEASURED by the same shipped-ground
90-second instrument. Its numerical stability checks passed; no former speed or gear is a target.

| Ground | Previous rest / rolling speed (m/s, MEASURED record) | New rest / rolling speed (m/s, MEASURED) | New gears rest / rolling |
|---|---:|---:|---|
| concrete | 10.650203 / 10.650212 | 10.650203 / 10.650212 | F8 / F8 |
| dirt | 10.569946 / 10.569968 | 10.554222 / 10.554258 | F8 / F8 |
| gravel | 10.610106 / 10.610108 | 10.602253 / 10.602253 | F8 / F8 |
| mud | 4.849178 / 4.849178 | 3.291137 / 4.430107 | F5 / F6 |
| sand | 4.874706 / 6.211272 | 4.861387 / 4.861388 | F6 / F6 |

The construction pressure is DERIVED about 107.014 kPa from the same force/pressure law; the
89.5 kPa seed is not the construction's current pressure. New flat rolling loads are DERIVED
22.3668/35.7387/29.0527/95.9121/75.8543 kN on concrete/dirt/gravel/mud/sand; full-app observer
residuals stay inside the independently declared precision. Final-contact capacity less nominal drag is
DERIVED capacity minus rolling load: gravel 369.092–369.151 kN, compared with the historical
369.978–370.110 kN observation. The fixed 20 kN pad's gravel pull is MEASURED 14,239.974 N,
versus historical 14,420.127 N; half load gives 7,119.987 N. Fixed-pad pulls on
concrete/dirt/mud/sand are MEASURED 11,999.979/8,359.983/5,539.989/10,859.979 N.

Full-app gravel parking at 20 degrees is MEASURED 37.06952 mm after 60 simulated seconds,
versus historical 37.06981 mm. Third-second roll RMS is MEASURED 0.00006974 rad/s,
versus historical 0.00012356 rad/s; peak side-load swing is 29.983 kN and alternation .03158.
Full-app mud service-stop rollback is MEASURED .073596 m (historical .073509 m), reverse speed
.255585 m/s (historical .255145 m/s), and final capacity 154.890 kN. The fixed adapter stop
reports MEASURED .075056 m rollback and .218844 m/s reverse speed; these are observations,
not assertions that total adapter motion equals common shear compliance.

The pressure-heavy inline construction is DERIVED by doubling its ground's drag/grip crossover:
MEASURED mass 28,851.293 kg, pressure 54.166676 kPa, crossover 27.083336 kPa, potential rolling
158.498 kN versus grip 84.909 kN and maximum sustainable curve pull 132.701 kN. Its late speed is MEASURED
.20931806 m/s, inside the retained .25 m/s rolling fade. This is a limited crawl, not evidence of
exact immobilization: a rolling law that fades to zero at rest admits it. That historical fixture was engine-limited and did not distinguish grip from engine. The
current pressure-heavy witness separates the two limits and derives each crawl from its
limiting force. The fade magnitude and belt integration are unchanged.
The force-pad slope matrix stops below `atan(μ)` at both fixture pressures (MEASURED late speed
at most 0.0000062 m/s), and accelerates above it; its printed settlement is not a pass target.
The 32 Hz dither retains MEASURED .25 of capacity; recoil and shear-return form are unchanged.

Final gates: `cargo fmt --all --check` and CI-feature workspace/all-target clippy pass.
The final core/interface/spec/guard batch passes 474 tests, with 2 intentionally ignored;
the full-app portion of the affected batch passes, as does the separate shipped-ground speed
instrument. After the stricter admission exposed inline fixture inertia, all 93 spec, bake
and whole-tree guard tests passed with the corrected sheets. Blender fixture Python syntax
passes; Blender's asset-generation suite was not run locally. No captures, pricing, push or
external issue comments were made.

### Hull drag and grip verification (#198)

Rolling opposes longitudinal hull motion relative to each support; spinning a stationary
belt creates none. Grip remains independently bounded by `μ N`, and the belt reacts only
to longitudinal grip. The pressure witness checks the emitted hull force against contact
telemetry and the shared Driver coefficient. Its three constructions distinguish grip
limitation, engine limitation and an affordable launch. The engine-limited fixture has a
flat net torque curve so its expected pull does not depend on an observed crank RPM.

The full-app matrix mounts the shipped simulation, brakes and transmission on tilted
geometry. It parks at nine tenths of the lower static limit, checks late velocity and
travel after 60 simulated seconds, and observes motion above that limit for 8 seconds.
The sliding horizon is bounded by the finite plane even under free gravitational
acceleration; leaving the plane is not a sliding-law failure. Angles and brake strength
are read from ground and vehicle owners, never from a previous run.

| Ground | Grip limit (degrees, DERIVED) | Brake limit (degrees, DERIVED) | Binding limit | Late held speed (m/s, MEASURED) | Above-static-limit late travel in one second (m, MEASURED) | Sliding regime |
|---|---:|---:|---|---:|---:|---|
| concrete | 30.963758 | 31.000736 | ground | .00000031 | 2.598096 | Accelerates beyond full rolling |
| dirt | 22.684929 | 31.000736 | ground | .00000037 | .186064 | Steady creep inside the fade |
| gravel | 35.450867 | 31.000736 | brake | .00000078 | 10.897297 | Brake-limited slide |
| mud | 15.482733 | 31.000736 | ground | .00000008 | .043254 | Steady creep inside the fade |
| sand | 28.501959 | 31.000736 | ground | .00000070 | .123698 | Steady creep inside the fade |

These are ten percent above the lower static limit, not necessarily above the full-rolling
sliding limit. At nominal pressure 107 kPa, the locked-belt balance gives DERIVED creep
speeds about .185 m/s on dirt, .0427 m/s on mud and .122 m/s on sand. Mud's upper creep
angle is DERIVED `atan(.277 + .1715)`, about 24.2 degrees. The small travel in these rows
is sustained downhill motion set by the numerical band, not evidence of acceleration.

Common-K readers no longer carry a per-contact copy or load-weighted constant. The
vanishing-patch downshift test is retired: an identical-coefficient patch supplied no
counterexample after cohesion left. Nominal observation and mixed-ground tests retain the
live evidence. The loose-piece witness requires positive hull lift and settled piece
support within the footprint. It makes no recipient-motion claim; lift magnitude remains
an observation. Arithmetic
drivetrain fixtures carry a required grip construction and a separate terrain coefficient;
the track-material slot no longer supplies terrain grip.

Inertia admission probes lie between the with/without-term bounds. DERIVED from the compact
fixture: minimum J is 1.806957 kg m², versus 1.297850 without idle catch and 1.790493 without
viscous drag. A flat high-RPM fixture isolates fuel-cut loss reconstruction: minimum
1.708171 kg m², versus 1.210126 without that term. These inputs isolate each term rather
than asking an inertia below every bound to witness all of them.


MEASURED fixed-footprint delivery (inline fixtures; full potential drag is priced by the Driver):

| Limiter | Pressure (kPa) | Engine pull (kN) | Grip (kN) | Drag (kN) | Late speed (m/s) | Launch pick |
|---|---:|---:|---:|---:|---:|---|
| grip | 40.000000 | 132.701 | 62.702 | 88.619 | .17688680 | none |
| engine, flat curve | 60.000004 | 99.526 | 282.160 | 193.123 | .12878303–.12885873 | none |
| affordable launch | 15.000001 | 132.701 | 23.513 | 14.422 | 6.87054539–6.87082577 | F2 |

The grip-limited crawl is DERIVED `.25 × .3 / .424 = .17688679 m/s`; its engine exceeds drag.
The engine-limited crawl is DERIVED from the flat curve's pull and potential drag, about
.12884 m/s; its grip exceeds drag. MEASURED mean late force is .0057 N and -.0019 N,
respectively: neither keeps gaining speed. Replacing faded drag by its full potential at
those same states gives MEASURED deficits 25.915 kN and 93.598 kN, agreeing with no launch
pick. The affordable construction accelerates through the fade.

At the fixed pad's MEASURED 20 kN normal load, full-pressure grip pulls on
concrete/dirt/gravel/mud/sand remain MEASURED 12.000/8.360/14.240/5.540/10.860 kN;
half load gives half each pull. These are observations of `μ N`, not retained speed targets.
The declutched coast distances are MEASURED 12.632 m and 4.187 m, versus DERIVED 12.662 m
and 4.178 m from the two fixture grounds and coupled hull/belt mass. The locked-pad mud
stop from Fixture 4 m/s reaches MEASURED 1.823313 m, returns .074999 m, and peaks in reverse
at .165683 m/s. The constrained adapter's separate mud stop reaches MEASURED 2.927493 m,
returns .075154 m and reverses at .149379 m/s; it is not a whole-app tank measurement.


MEASURED shipped full-app ground speeds after hull drag, rest / rolling entry in m/s:

| Ground | Rest entry | Rolling entry | Final gear rest / rolling |
|---|---:|---:|---|
| concrete | 10.634736 | 10.634741 | F8 / F8 |
| dirt | 10.521238 | 10.521238 | F8 / F8 |
| gravel | 10.584840 | 10.584842 | F8 / F8 |
| mud | 3.233301 | 4.358446 | F5 / F6 |
| sand | 4.828499 | 4.828491 | F6 / F6 |

These replace the earlier direct-grip observations as the current force-path report; no
previous speed or gear is a pass condition. The full-app mud service stop from Fixture
4 m/s reaches MEASURED 1.871824 m, returns .064541 m, and peaks in reverse at .235267 m/s,
with final capacity 154.890 kN. The loose-piece witness reports MEASURED hull rise .076588 m,
peak piece centre .050000 m and peak speed .153281 m/s; its Fixture piece height is .1 m.

The neutral pivot is MEASURED .1286 rad/s, against its DERIVED .1884 rad/s kinematic ceiling;
its opposing belts are MEASURED ±.288 m/s. These are reported outcomes, not retained targets.

Final gates for `driving: make rolling drag compete with grip (#198)` and
`driving: admit positive running-gear footprints before spawn (#198)`: MEASURED targeted
driving/ground result 222 passed; full headless plus citation/UI/net-boundary/bake guards
76 passed, 4 on-demand measurement/pricing beds ignored. CI-feature workspace/all-target
clippy, `cargo fmt --all --check`, diff whitespace and steering-analyzer syntax pass.
No captures or frame/tick pricing were taken. The original strong-engine regression was
red before the force change, sustaining MEASURED 1.331203 m/s outside the fade.

Review dispositions live in `.agents/scratch/fix-law-grip-review.md`. Authored-number census
remains 20, or 22 including the held lip controls (DERIVED, inline Fixture numbers excluded).
Neither published ground data nor a serialized wire type changes; protocol revision stays 83.
Backlog load-27..32 and net-21..22 remain reserved to the turret chain by the lane brief.

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

### Bounded recoverable shear (#146)

Subject: the game's driving model; game state only. Every measurement here ran at the common
75 mm shear length that section 5a's per-ground `K` replaced; rollbacks are now bounded by each
ground's resolved `K` (mud 25 mm, dirt 10 mm). The before tree is the lane's starting
main. These are unattended headless measurements, with no captures or pricing. The force-law
A/B used identical fixtures: baseline accumulated displacement, A plain Dahl on soil, B the
shared elastic-zone return. B is the sole shipped path.

| witness | baseline (MEASURED) | A: alpha = 1 (MEASURED) | B: kept law (MEASURED) |
|---|---:|---:|---:|
| fixed mud stop rollback, m | 1.199998 | 0.054364 | 0.075001 |
| fixed mud peak reverse speed, m/s | 0.480283 | 0.226819 | 0.266221 |
| full-app mud stop rollback, m | 1.198098 | 0.052179 | 0.073509 |
| full-app peak reverse speed, m/s | 0.484893 | 0.213201 | 0.255145 |
| dirt, 1.2 m sideways shove, rollback m | 1.200000 | 0.054553 | 0.074999 |
| turning moment at 0.2 rad/s, N m | 11,346.383 | 29,230.885 | 29,081.803 |
| turning share of Driver price at 0.2 rad/s | 37.4061% | 96.3667% | 95.8752% |
| turning moment at 0.6 rad/s, N m | 19,460.770 | 29,660.137 | 29,464.182 |
| turning share of Driver price at 0.6 rad/s | 64.0703% | 97.6494% | 97.0043% |
| gravel 20-degree park settlement at 60 s, mm | 52.58075 | 67.38995 | 37.06981 |
| park displacement from 10 to 60 s, mm | -0.00092 | 0.00137 | 0.00034 |
| closed 32 Hz dither, held capacity share after 60 s | 0.25 | 0.11226496 | 0.25 |

The fixed-pad travel assertion admits FIXTURE observation precision `0.0001 K` for f32
position accumulation: its MEASURED 0.075001 m rebound is about one micrometre above the
75 mm compliance. This is a test tolerance, not a force correction. The actual force state
is bounded by K. Turning prices are MEASURED 30,332.969 and 30,374.113 N m in the two fixtures;
B still leaves DERIVED gaps of 4.1248% and 2.9957%. No multiplier closes them.

The dither witness holds the initial recoverable state at one quarter of capacity, imposing
FIXTURE alternating 1 mm/s slip at 64 Hz for 60 simulated seconds. It is a controlled closed
path, not a claim that the full app exhibits that amplitude: A's actual gravel park also
settles. B keeps holding force under this disturbance without additional state. At FIXTURE
one K of monotonic slip, fine-step loading gives MEASURED capacity shares 0.632055 for A and
0.847668 for B. Thus B deliberately gives firmer initial bite and less parking settlement;
it does not preserve the exact exponential knee. A player loses the metre-scale spring-back
after a skid and gets resistance aligned with current sliding within roughly one K. Steady
turning feels more resistant, with the residual price gap disclosed above. Ground facts,
maximum strength, rolling resistance and the solver floor are unchanged.

Straight-line entries use the existing final-second, 90-s instrument. MEASURED B speeds,
from rest / rolling, m/s: concrete 10.650203 / 10.650212; dirt 10.569946 / 10.569968;
gravel 10.610106 / 10.610108; mud 4.849178 / 4.849178; sand 4.874706 / 6.211272.
Every gear is unchanged; the largest absolute change from the baseline table is DERIVED
0.000013 m/s, within its 0.01 m/s convergence resolution. Gravel capacity less nominal drag remains
DERIVED 369.978–370.110 kN versus baseline 369.924–370.111 kN from final contact capacity
minus rolling load. Fixed-pad firm/loose pulls remain MEASURED 14,404.124 / 11,034.170 N;
their DERIVED ratio remains 1.305411. The shipped base-ground fixed-pad pull is MEASURED
14,420.127 N before and after. The existing roll-kick witness stays green: MEASURED
third-second RMS 0.00012356 rad/s, against baseline 0.00009436 and its 0.003 rad/s bound.

`GripElements` retains the same vectors and dwell bytes. The removed material-share fields
are private force-pass temporaries. `protocol::plugin` registers resolved belt motion, not
`TrackGripElements`; clients do not integrate this contact law. No wire type or client-derived
state changes, so PROTOCOL_REV remains unchanged. The network and wire-hash guards verify it.

The reversal witnesses were red on the original law. A is red on the closed-dither witness;
its loading curve and full-app park otherwise pass their measures. Restoring the exponential force readout is also red in
`recoverable_shear_reads_effective_compliance_in_the_force_pass`. Fmt, workspace/all-target
CI-feature clippy and the affected batch (`track::`, `ground_`, `headless_test`, `net::`,
`doc_citations`, `ui_ascii`, `net_boundary`, `bake::`) are green: 746 passed, 5 intentionally
ignored. The full library is run once before the final review-fix commit.

The production-adapter fixed-pose service-stop instrument, which also integrates drivetrain
motion, now reports MEASURED 0.075287 m rollback and 0.227759 m/s peak reverse speed, versus
its historical 1.200275 m and 0.467284 m/s above. Its total hull motion exceeds K by DERIVED
0.287 mm; it is distinct from the locked-pad recoverable-travel invariant and the full-app
free-support stop. No extra correction is applied to that adapter.

### Ground review verification, completed

Subject: the game's driving model; game state only.

| reviewer | finding | verified / not a defect | fixed in (subject) |
|---|---|---|---|
| Codex fix-ground-2 P2 | analyzer omitted low-load contacts from capacity checks | verified red: zero normal load, 10 N capacity and 100 N force passed; every contact is now checked | driving: check low-load traction and live stopping capacity (#146) |
| Codex fix-ground-2 P2 | scripted stopping caller lacked a capacity-wiring witness | verified: the new test executes `battle::drive`; replacing live capacity with the brake budget makes it fail reversal | driving: check low-load traction and live stopping capacity (#146) |
| Codex fix-ground-2 P3 | wrap duplicated gravity | verified: wrap and wheels now read `derive::G`; remaining cosmetic solver constants name their Resolution jobs | driving: share gravity between belt wrap and wheels (#146) |
| Codex fix-ground-2 P3 | Driver prose said geometric mean | verified: `load::observe` weights coefficients arithmetically by contact length; prose corrected | driving: bound recoverable ground shear on reversal (#146) |

The analyzer's FIXTURE inputs reject over-capacity force at zero normal load and nonzero force
at zero capacity; below-capacity and zero-force/zero-capacity cases pass. The caller witness
keeps one lane pose and construction, varies reported capacity, and reads coupled mass and
command transition from their owners. Its approach speed is bounded so both stopping points
fit any declared brake strength. The stop witnesses also require rest before reading rebound,
so zero traction cannot masquerade as bounded return.

Mutation verification: the original displacement law is red on fixed-pad and full-app reversal;
plain Dahl is red on closed dither; restoring the nonlinear force readout is red on effective
compliance; bypassing reported capacity with the brake budget is red in the actual scripted
driver; the original analyzer's low-load filter accepts the forbidden force fixture. Every
mutant is absent from the committed code.

Final verification: fmt and CI-feature workspace/all-target clippy are green. The affected
batch passes 746 tests (5 intentionally ignored); wrap/caller/doc-citation verification passes
7 tests. The required whole-library run passes 2,311 tests, with 33 intentionally ignored and
no failures. The final test-only fixture robustness and stop-premise changes pass 47 targeted tests,
including the whole-tree guards; no further simulation behavior changes follow that
whole-library run. All runs are unattended, headless and muted under the machine lock. No
captures, pricing, pushes, wire changes or excluded-module edits were made. Backlog driving-09
and the no-ground rubble fixtures remain explicitly outside this lane, as recorded above.
