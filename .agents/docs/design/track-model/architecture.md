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

## 0a. Phase-B reality (v4, 2026-07-17 — commit 9758d97)

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
- Vehicle collision proxies carry `Friction::ZERO` (min-combine): all grip is the model's.

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
src/track/oracle.rs       TerrainOracle (scalar `depth_along`) + BlockField (§5)
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
pub fn step(mode: TransmissionMode, fp: &ForceParams, tp: Option<&TransmissionParams>,
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
`transmission::step` once, for every architecture — the Governor arm goes through the same joint
form as a declared gearbox, with `(mode, params)` normalized at the call site. There is no
alternate governor tail and no branch to pick between them. (`forces::step_side` survives only as
the `contact_side` + governor convenience wrapper that the forces unit tests drive.)

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

Batched probes, hit normals, surface material and `covered` are the recorded growth path — to be
added WITH their first consumer, not before. `SpatialQueryOracle` was on that list and never
needed one.

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
  Parry's terrain cast survives only on the flat fallback world that has no grid. What a shell
  stops on is what the tracks drive on.
- Honesty note: the block term rounds corners and buries block bottoms — deliberate policy, not
  representational identity with colliders. The height term IS the collider's triangulation, to a
  pinned tolerance. "Visual ≡ physics" means both sample the SAME oracle.
- Streaming meshes, overhangs and destructibles still need chunk coverage + revisioning + a
  "clear vs unloaded" distinction (`covered`) — out of scope, named so the trait doesn't pretend
  otherwise.

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
