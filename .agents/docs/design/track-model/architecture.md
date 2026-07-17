# Track module architecture — promoting the sandbox model into the game

Status: v4 (step 27, 2026-07-17) — phase B **shipped**: the belt model IS the game's drive
sim (see §0a; ADR-0025 written, supersedes ADR-0005, retires ADR-0006). v3 (step 26) shipped
phase A (`src/track/view.rs`) with the tier-line discussion + codex view-plugin review. v2
reconciled v1 against the codex adversarial review (`scratchpad/codex_arch25_review.md`, 10
findings, all dispositioned below). Companion to HQ.md (the step log) and
`phase-b-migration.md` (the cutover plan, v2). Foundation
document for "many tanks, one model"; every structural decision is judged against Yan's three
constraints:

1. **One model** — quality scaling is tiers of one pipeline, never parallel systems.
2. **Many tanks** — 30-tank MP scenes; per-tank cost is a policy decision (tier), not a tax.
3. **Spec-sheet authoring** — a new vehicle is data. If adding a tank requires touching a
   solver constant, the design failed. (Codex C: this rules out several constants currently
   hard-coded in `model4.rs` — see §7.)

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
- Sandbox: the math to promote (oracle/route/chain/wheels/forces) is entangled with
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
  share one analytic field built from `TerrainMap` on revision change.
- The view consumes `TrackDrive` phase/speed directly — the pose-delta no-slip derivation is
  deleted; remote tracks scroll at exact authority phase.
- The hold/bristle transplant was attempted and REVERTED (owner call — pure sandbox law;
  slope creep accepted; future hill-hold = per-element bristle; `phase-b-migration.md` §3a).
- Sandbox `model4` is a thin adapter over `track::forces` — the entanglement noted in §0
  (codex E) was dissolved by the extraction; models 1–3 deleted in the consolidation pass.
- Vehicle collision proxies carry `Friction::ZERO` (min-combine): all grip is the model's.

## 1. The shape: one geometric core, three consumers

Step 24 dissolved the "two models" question: the chain's skeleton IS the route. Literally:

```
            authored data                          runtime inputs
      ┌──────────────────────────┐        ┌────────────────────────────────┐
      │ TrackSpec (.tank.ron §7) │        │ TerrainOracle (§5)             │
      │ RunningGear (bake, §7)   │        │ PresentedFrame (§3)            │
      └────────────┬─────────────┘        │ BeltKinematics (§4)            │
                   ▼                      └───────────────┬────────────────┘
            ┌──────────────────────────────────────────────▼──────┐
            │                route core (pure fns, §2)            │
            │   wheel filter → tagged route → arc/tube queries    │
            └──────┬────────────────┬────────────────┬────────────┘
                   ▼                ▼                ▼
            ┌────────────┐  ┌──────────────┐  ┌─────────────────┐
            │ SIM forces │  │ chain tier   │  │ route tier      │
            │ (phase B)  │  │ (view: own + │  │ (view: rest;    │
            │            │  │  near tanks) │  │  decimated far) │
            └────────────┘  └──────┬───────┘  └───────┬─────────┘
                                   └───────┬──────────┘
                                           ▼
                                  TrackRenderer (§8)
```

Deleting any consumer leaves the others intact; adding a tank touches none of them.

## 2. Module layout, pure-core API, migration

`src/track/` as a peer of `driving/`, facade `src/track.rs` (plugin-per-feature, ADR-0002):

```
src/track.rs          pub fn view_plugin(app), pub fn sim_plugin(app) [phase B]
src/track/spec.rs     TrackSpec + MaterialLoop + track-type presets (serde)
src/track/rig.rs      RunningGear: per-side gear from bake + spec; validation
src/track/oracle.rs   TerrainOracle (batched) + BlockField + SpatialQueryOracle
src/track/route.rs    route core (pure)
src/track/chain.rs    ChainState (pure struct + stepper)
src/track/wheels.rs   view wheel-lift filter (pure)
src/track/view.rs     ECS: PresentedFrame, tiers, belt derivation, view-node writes
src/track/render.rs   TrackRenderer adapters (instanced; entity-per-link bring-up)
src/track/sim.rs      [phase B] collocation forces in SimPhase::DrivingForces
```

**Pure-core API surface** (codex E — sandbox types stay OUT; the sandbox re-imports these and
keeps its own ECS adapters):

```rust
pub fn build_route(gear: &SideGear, wheel_lifts: &[f32], material: MaterialLoop)
    -> Result<Route, RouteError>;
pub fn sample_route<O: TerrainOracle>(route: &Route, phase: f32, presented: Affine3A,
    oracle: &O, out: &mut Vec<LinkPose>) -> Result<(), TrackError>;
pub fn articulate_wheels<O: TerrainOracle>(gear: &SideGear, state: &mut [WheelViewState],
    frame: &PresentedFrame, gravity_world: Vec3, oracle: &O);
impl ChainState {
    pub fn step<O: TerrainOracle>(&mut self, input: ChainFrame<'_>, oracle: &O,
        out: &mut Vec<LinkPose>) -> StepReport;      // StepReport: reseeds, residuals, cost
    pub fn reseed<O: TerrainOracle>(&mut self, input: ChainFrame<'_>, oracle: &O);
}
```

Type mapping from the sandbox: `PinBelt` → `MaterialLoop` (immutable, §7); sandbox `Suspension`
→ `WheelViewState` (never reuse the game's sim `Suspension` name); `BeltSample` → `LinkPose`
(one representation, full orthonormal frame — §8); `ChainSideMemory` → private in `ChainState`;
`ConformedBelts`/`ChainReference` stay sandbox debug adapters. `RunningGear` is baked
synchronously from `TankGeometry + TrackSpec`, born with the root, and holds **no asset
handles**.

Mounting: `view_plugin` in the presentation roots only (like `vfx`); `sim_plugin` (phase B) in
`SimPlugin`'s `SimPhase::DrivingForces` slot.

## 3. The chain is VIEW state — and the seam is the PRESENTED pose (codex A)

The chain is cosmetic: not rollback-registered, never re-solved in replays, reseedable from
data at any instant (ADR-0014 tier-2 spirit). But "read the view pose" needs a precise
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
- Chain substeps interpolate the **wheel circles** `previous → current`. HONESTY (codex
  view-review #8): the hull affine itself is captured once per frame — every catch-up substep
  probes terrain at the end-of-frame pose, up to ~one pitch early at 60 km/h. Accepted for
  phase A; per-substep affine interpolation is an open item, not a shipped claim.
- Shipped discontinuity detection is LOCAL (no lightyear coupling): presented pose delta per
  frame (translation > 1.2 m, or forward/up axis chord > 0.5) or a `TerrainMap` revision
  change → chain cold start + belt differentiator + wheel-lift re-base. The thresholds must
  bracket `render_error`'s snap constants (2 m / 60°) — pinned by a test in `render_error`.
- **Terrain probes use the interpolated presented pose.** Probing at tick pose and offsetting
  links afterwards is wrong — terrain doesn't receive the offset.
- `discontinuity == true` → canonical reseed. The reseed must be self-triggering on this signal
  (the sandbox's reset clears chain memory externally today — promotion makes the signal part
  of the input contract).

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

## 5. TerrainOracle — batched, sourced from one TerrainMap (codex F)

`SpatialQuery` is a borrowed `SystemParam` — it cannot live in a resource. The oracle is a
**batched pure trait**, constructed per system invocation where needed, matched outside the hot
loop:

```rust
pub struct TerrainProbe { pub station: Vec3, pub outward: Dir3, pub reach: f32 }
pub struct TerrainSample { pub depth: f32, pub normal: Dir3,
                           pub material: TerrainMaterialId, pub covered: bool }
pub trait TerrainOracle { fn sample_into(&self, probes: &[TerrainProbe],
                                         out: &mut [TerrainSample]); }
```

- **`BlockField`** (default, resource): the sandbox field generalized. Prerequisite refactor:
  `world.rs` currently builds block transforms and discards them — introduce
  `TerrainMap { revision, blocks: Arc<[TerrainBlock]> }`, consumed by BOTH collider spawning
  and `BlockField::from_map`, so the representations share one source. (`revision` feeds the
  reseed/discontinuity signal on terrain change.)
- **`SpatialQueryOracle`**: Avian casts on `Layer::Terrain`, built inside the system from the
  borrowed param. View tiers may use it; determinism-sensitive sim paths must not.
- Honesty note: the field rounds corners and buries block bottoms — deliberate policy, not
  representational identity with colliders. "Visual ≡ physics" means both sample the SAME
  oracle, not that the oracle equals the collider mesh.
- Real terrain later: heightfields fit under this interface. Streaming meshes, overhangs,
  destructibles need chunk coverage + revisioning + a "clear vs unloaded" distinction
  (`covered`) — explicitly out of scope now, named so the trait doesn't pretend otherwise.

## 6. Tiers and budget (codex G — numbers corrected)

`enum TrackTier { Simulated, Route, Culled }` — assigned **per tank** (never per side).

**Phase-A status + the value line (owner discussion, 2026-07-17):** NO tier machinery is built —
the alpha is 1v1, every tank gets the chain, and the enum/metric/renderer split waits for tank
counts past ~4. What was decided for when it returns:

- **Detail adds value only as motion the player can resolve at the current projection.** The
  chain's whole premium over Route is transients (slap, flap, tension redistribution) — sub-pixel
  past ~40–50 m at normal FOV.
- **The tier metric is SCREEN-SPACE** (projected link pitch in pixels, with hysteresis), not
  distance: gunner optics at 8× promote the tank you're staring at automatically, and demotion
  fires only when the shape difference is unresolvable — pop-free by construction, no crossfade
  machinery.
- **Pin friction guarantees chain-at-rest ≠ route shape** (it holds whatever sag friction locked
  in), so v2's "demote when deviation relaxes" may never trigger — the screen-space rule
  replaces it.
- Chain population: own tank + ~2 by the metric. Tiers vary only state and sampling density,
  never behavior — ribbon vs full-link is the SAME route sampled coarser.

- Honest arithmetic at current measured rates (post-broadphase, M4): 4 simulated + 26 route ≈
  `4×41 + 26×4 = 268 ms CPU/s ≈ 4.5 ms/frame at 60 fps` — that is solver time only, and it
  does NOT fit a 2 ms all-in budget. Consequences: (a) the budget is a **cost model** (links ×
  substeps × probes), not a tank count; (b) simulated tier is own tank + ~2 nearest by default
  until further optimization; (c) parallelism is explicit (`par_iter`/task pool over tanks) —
  Bevy does not parallelize inside one system on its own; (d) the render side has its own gate
  (§8).
- **Culled** (renamed from Scroll): maintain scalar phase, produce no geometry. Distant visible
  tanks are NOT this — they're Route with a decimated/ribbon renderer.
- Transitions (corrected claims): Route→Simulated is pop-free **only if** the chain seeds from
  the exact Route-renderer poses at the same phase — the seed function and the route renderer
  must share one code path (design requirement, not an assumption). Simulated→Route: downgrade
  only after chain deviation from the route relaxes below a visual threshold, or crossfade the
  same instance buffer; dropped chain state is discarded (stale state is debt, reseed on
  re-promotion). Hysteresis on thresholds.

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
- **Drive identity**: the chain's motor sector is `RouteTag::Arc(0)` — the FIRST circle. Phase A
  hard-codes sprocket-first (front drive, fits the Tiger); a rear-drive vehicle needs drive
  identity derived from the typed sprocket node's position (named debt, not silent).
- **Bake extension**: bake today captures only collision/ballistic mesh data — add
  subtree-bounds extraction for wheel/idler radii (spec override allowed) and the phase-marker
  transform.
- **De-specialization** (codex C): `TRACK_WIDTH/THICKNESS`, node mass, `MAX_LINK_ANGLE`,
  gravity, drive identity → data. `CHAIN_SLACK_TRIM` dies (tensioner owns preload).
  `CHAIN_REBASE_WINDOW` derives from pitch; tube bounds derive from pitch + minimum
  non-adjacent route-branch separation (atlas can't overlap on a small vehicle). Pin friction
  = preset **coefficient** + authored pin geometry + solver tension — a fixed torque inside a
  preset is per-type tuning wearing a costume. Substep rate, sweep count, guardrails stay
  global quality policy.
- Presets: `DryPin` (T-34, Tiger) / `LiveBushed` (modern: articulation return curl, higher B,
  lower friction).
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

**Phase A — view promotion (SHIPPED, step 26 2026-07-17):** `TerrainMap` refactor in
`world.rs`; pure core extracted into `src/track/` (sandbox re-imports); `track::view` with the
presented-pose seam, no-slip belt derivation (f64 phase, per-consumer wrap), entity-per-link
rendering (97×2 links, witness link 0, legacy `Track_Strip/Treads` hidden), oracle wheel lift
at disc-width stations, spec-validated + bind-time loop-feasibility gate, GLB-reinstance
rebind. Deliberately NOT built (owner LOC mandate): tiers, instancing, `PresentedFrame` ECS
state, `TrackRenderer` trait, `SpatialQueryOracle`, tensioner/presets. Deliverable met: Tiger 1
drives with live tracks in MP, zero sim risk. Tiger authoring unblocked.

**Phase B — sim promotion (gated):** collocation forces replace `driving::{suspension,
traction}` under the ADR-0005 rewrite; `BeltState` replicated+predicted; harness parity vs
sandbox baselines + MP soak (rollback storm, JIP) before default flip. The raycast/track switch
during soak is whole-process, development-only, with a **deletion gate** — it must never ship
as a per-tank tier or alternate locomotion model.

## 10. Testing

- Pure core: unit tests (envelope: lifted/interleaved/return-roller/coincident-circle
  rejection; chain: pitch exactness, tube residency, reseed determinism, StepReport budgets).
- Sandbox harness stays the feel/regression lab; scenarios become CI-runnable with numeric
  gates (step-24 metrics + perf probes).
- Phase A adds a presented-pose torture scenario (scripted rollback corrections + teleports →
  zero tears, bounded belt-vs-ground error).
- Phase B adds A/B harness parity + MP soak gates.

## Open items (tracked, not blocking)

- Six-roller / discrete tooth engagement pulses (flavor; DryPin preset extension).
- Chain only-if-artifacts list (unwrapped-s ledger, per-sweep terrain reprobe — HQ).
- Thrown track as replicated damage state with alternate route topology (far future).
- Streaming/destructible terrain under the oracle (`covered`, chunk revisioning) — named in §5,
  deliberately unscheduled.
