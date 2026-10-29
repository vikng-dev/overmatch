# Track layers: per-link fidelity only where it can be seen (design, #151)

Frame cost of the track view and the track sim. This note touches no damage-model code. The x-ray
in the pen cam is the damage model's inset view, and it is named once below because it copies the
shoe meshes.

**Labels.** Every number is MEASURED, DERIVED, ASSUMED or CLAIMED. A `file:line` reference or a
count read off source is an identifier, not a measurement. The fixture tank (the Tiger) supplies the
example numbers. Every threshold below is a generic law applied to asset data (ADR-0034).

## Summary

Three layers, one selection distance, one pixel currency: the DETAIL BUDGET law the shoe ladder
already uses, `D = ε · H / (2·tan(vfov/2) · budget_px) + r`.

- **L0 Links.** Today's per-link shoes and full fit.
- **L1 Belt.** One textured tube per side. It is the shadow ribbon's own geometry, laid on the same
  pin joints, so the link boundaries stay phase-locked. Its fit is cheaper.
- **L2 Rest.** A static rest-shape tube, shared by every tank of one type. No fit at all.

Each switch happens where the error the cheaper layer adds projects to at most one pixel, so the
switch is a sub-pixel pop with a 10 % hysteresis band:
- L0→L1 is set by the guide horn and grousers the tube drops: at least 88 mm, DERIVED.
- L1→L2 is set by the wheel travel the static belt ignores: 200 mm, fixture data.

Prints keep coming from every layer, because the rig's joints feed them, not the shoe entities. The
far layers make a print's touchdown less exact, but never change a print's size.

Separately from distance:
- **The sim.** Physics authority keeps its probes. It gets cheaper only through exact query cuts and
  parallelism across tanks, and it runs only on the server and offline: every MP client tank is
  `Static`.
- **The waste in the near layer.** It belongs to every layer that fits: issues #155 and #156, and
  the oracle waste found below.

**Expected saving at 30 tanks** (DERIVED, view system):
- **A moving battle on the MP client:** about 3 ms (3.56 → 0.73 ms of work, before parallelism), at
  an ASSUMED split of 4 tanks in L0, 10 in L1 and 16 in L2.
- **Today's near probe block (55–99 m):** the layers save nothing. That scene is carried by the
  near-layer and sim levers instead.

## Inputs

### Given (from the brief, DERIVED by earlier lanes)

- `drive_track_views` costs 3.89 ms per frame and `apply_track_forces` 3.46 ms, both serial; the
  terrain casts split 1.02 ms (view) and 1.11 ms (sim). Source: `frame-attribution.md`.
- About 1,008 wheel rays per tank per frame, 194 shoes out of 555 meshes per tank, 43 KB of ribbon
  rebuilt per tank per frame and 210 transforms rewritten per tank per frame. Source:
  `render-audit.md` items 2, 7 and 9.

### New: where the two systems' time goes

Method: I re-parsed the frame-attribution lane's raw `/usr/bin/sample` output (`offscreen-30`, a
window of 1,104 frames, 30 idle tanks, 2560x1440 offscreen) and merged the call tree under each
system by callee and source line. The parse scripts are scratch and not committed.

My normalization gives 3.56 ms for the view and 3.49 ms for the sim (DERIVED). The frame-attribution
lane gave 3.89 and 3.46 ms. Read the shares, not the totals: these are daytime runs with the owner
working.

**`drive_track_views`, per tank per frame** (DERIVED from MEASURED samples, ÷ 30):

| Part | µs per tank | Where |
|---|---|---|
| Wheel-lift probes | **51.5** | inlined `wheel_lift_target` |
| … `cast_ray` | 19.0 | `oracle.rs:390` |
| … print-sink lookup, once per probe | 13.6 | `view.rs:117` → `PrintDepths::full_press_m` |
| … block broadphase | 9.0 | `oracle.rs:356–361` |
| … the buried test (`height_at` at the ray origin) | 7.9 | `oracle.rs:387` |
| Belly conform (`raw_belly`) | **32.9** | `wrap.rs:169` |
| … its probes | 27.5 | split as above: cast 10.2, buried 6.2, sink 5.4, blocks 4.7 |
| Drape (`sag_span_at`, `EveryCircle` refinement and promotion) | **14.4** | `wrap.rs:211`, `route.rs:484/511` |
| Other wrap math (conform, resample, ease, trig, drops) | 7.0 | |
| Shoe placement, ribbon build, transform writes, allocation | 11.4 | the system's own frame |
| **Total** | **≈ 119** | the frame-attribution lane's figure gives 130 |

**Probe counts** (DERIVED from source and the view glb's node positions):
- **Wheel lift:** 1,008 per tank per frame: 16 wheels × 21 arc samples × 3 lateral stations.
- **Belly:** about 468 per tank per frame. The 12.65 m loop minus the 4.99 m sprocket–idler chord
  leaves about 7.66 m of bottom run, at 0.1 m spacing about 78 stations per side, with 3 lateral
  stations each.
- **Mean cost:** about 54 ns per view probe.

**`apply_track_forces`, 3.49 ms per frame at 30 tanks** (DERIVED from MEASURED samples):

| Part | ms | Share |
|---|---|---|
| Probes: `cast_ray` 1.16, buried `height_at` 0.55, block broadphase 0.37, other 0.12 | 2.20 | 63 % |
| Contact law around them (the column profile and contact frames, inlined) | 0.65 | 19 % |
| `apply_force_at_point` 0.19, grip elements 0.12, emit 0.13, capability 0.06, resample 0.03 | 0.53 | 15 % |

**Sim probe counts** (DERIVED from `forces.rs:669–727`):
- 97 segments × 3 columns × 3 stations = 873 probes per side, so 1,746 per tank per tick.
- About 36 ns per probe at 1.16 ticks per frame.

**Three facts that bound the design** (VERIFIED in source):
1. **No MP client runs the sim.** Every client tank is `RigidBody::Static` (`net/rig.rs:55`), and
   `apply_track_forces` skips bodies that are not dynamic. Its 3.49 ms is paid offline and on the
   server only.
2. **The measured 30-tank scene is idle, and near.** The 28 probe tanks are parked
   (`tank/scenario.rs:496`), 55–99 m from the duel, and nothing drives them. Idle tanks flatter any
   pose cache. At 55–99 m every tank stays in L0 under the thresholds below. The far placement
   (`OVERMATCH_PROBE_FAR=1`, 588–633 m) and the battle scene (#153) are the other two scenes.
3. **A moving root re-propagates every descendant.** In bevy_transform 0.19.1,
   `systems.rs` `propagate_descendants_unchecked` recomputes each child whenever the parent's
   `GlobalTransform` changed. So hidden shoes still cost propagation; only shoes that are detached
   or despawned stop costing it, and `set_if_neq` on a shoe's local transform does not save it.

**Stale claims found:**
- `view.rs:17`, `wrap.rs:144–147` and `architecture.md` §6 say ~56 µs per tank per frame. The
  30-tank samples DERIVE 119.
- Two parts of today's cost postdate that figure: the print sink (0.57 ms at 30 tanks) and the
  `EveryCircle` drape refinement (0.43 ms). Their own comments date them later (CLAIMED).

## Pixel arithmetic

The projection is `px = size · k / D`, with `k = H / (2·tan(vfov/2))`.

**Views** (fixture data, `tiger_1.tank.ron`): the commander's view is `Unaided(45°)` and the gunner's
optic is `Magnified(2.5×, 25°)`, both vertical fields. That gives (DERIVED):
- 1080p commander: k = 1303.7
- 1440p commander: k = 1738.2
- 1080p optic: k = 2435.8
- 1440p optic: k = 3247.7

A render scale below 1 shrinks `H`. A budget of 2 px halves every distance.

**Sizes:**

| Size | Value | Label |
|---|---|---|
| Link pitch | 0.1304 m | DERIVED: `Pin_Start`/`Pin_End` markers in the glb |
| Guide horn above the shoe's inner face (dropped by the tube) | 0.088 m | DERIVED: `Link` mesh bounds, y ∈ [−0.0762, 0.1079], against the pin at −0.0053 and `pin_to_inner` of 25.56 mm |
| Grouser below the shoe's outer face | 0.046 m | DERIVED, same bounds |
| Return-run sag | ≈ 0.03 m | ASSUMED; the drape rests on the road-wheel tops; the floor constant is 0.02 m (`SLACK_SAG_FLOOR`); the HQ quotes a 30–50 mm manual spec (CLAIMED) |
| Belly and wheel articulation ("wave") | ≤ 0.200 m | fixture `bump_stop`; droop ASSUMED no larger |

**Pixels at range** (DERIVED):

| View | Size | 18 m | 50 m | 100 m | 150 m | 300 m | 500 m | 1 px at | 2 px at |
|---|---|---|---|---|---|---|---|---|---|
| 1080p commander | pitch | 9.4 | 3.4 | 1.7 | 1.1 | 0.57 | 0.34 | 170 m | 85 m |
| | sag 0.03 | 2.2 | 0.78 | 0.39 | 0.26 | 0.13 | 0.08 | 39 m | |
| | wave 0.2 | 14.5 | 5.2 | 2.6 | 1.7 | 0.87 | 0.52 | 261 m | |
| 1440p commander | pitch | 12.6 | 4.5 | 2.3 | 1.5 | 0.76 | 0.45 | 227 m | 113 m |
| | sag | 2.9 | 1.0 | 0.52 | 0.35 | 0.17 | 0.10 | 52 m | |
| | wave | 19.3 | 7.0 | 3.5 | 2.3 | 1.2 | 0.70 | 348 m | |
| 1080p optic | pitch | 17.7 | 6.4 | 3.2 | 2.1 | 1.1 | 0.64 | 318 m | 159 m |
| | sag | 4.1 | 1.5 | 0.73 | 0.49 | 0.24 | 0.15 | 73 m | |
| | wave | 27 | 9.7 | 4.9 | 3.2 | 1.6 | 0.97 | 487 m | |
| 1440p optic | pitch | 23.5 | 8.5 | 4.2 | 2.8 | 1.4 | 0.85 | 424 m | 212 m |
| | sag | 5.4 | 1.9 | 0.97 | 0.65 | 0.32 | 0.19 | 97 m | |
| | wave | 36 | 13.0 | 6.5 | 4.3 | 2.2 | 1.3 | 650 m | |

**What sets the thresholds.** The sag is sub-pixel well before any switch below, so it sets no
threshold. What matters is the horn (88 mm), which the tube drops, and the wave (200 mm), which the
static belt drops.

**The belt takes over below Nyquist in every view.** At the L0→L1 switch the pitch is 1.48 px
(DERIVED: 0.1304 / 0.088, the same in every view), below the 2 px a periodic pattern needs. Beyond
it, per-link geometry aliases on a moving belt, partly masked by MSAA, while a mipmapped tread
filters. That makes the switch replace shimmer with a steady band, which the motion clips below must
show.

**The existing shoe ladder.** At 1440p commander its rungs switch at 6.9, 26.8, 49.8 and 81.6 m
(CLAIMED, `scenario.rs`). The last of these reproduces as 81.2 + 0.38 m from the certificate's
46.7 mm (MEASURED in `tiger_1.lod.json`).

## The layers

| | **L0 Links** (near) | **L1 Belt** (mid) | **L2 Rest** (far) |
|---|---|---|---|
| **Drawn** | Per-link instances: 194 shoe entities, rungs 0–4 picked per belt by the existing selector. Wheels, sprockets and idlers spin. | One textured tube per side: the `Link_Box` section swept along the 97 pin joints, one texture period per pin-to-pin segment (u = ring index, static UVs), albedo and roughness baked from the shoe, no normal map. Shoe pool detached. Wheels and gears spin. | The same tube built once on the rest running gear: one static mesh per tank type and side, shared by every tank of that type. Wheels at rest, no spin. |
| **Fit** | Wheel lift: 21 × 3 probes per wheel. Belly: 0.1 m × 3 laterals. `EveryCircle` drape, slack spring, conform ease. | Wheel lift: 7 equal-cos arc samples × the 2 disc-edge laterals (−78 % of probes). Belly: 0.1 m × the 2 shoe-edge laterals (−33 %). `RoadWheels` drape: no brackets, no sort. Same filters. | None per frame. The rest fit is computed at rig bind. The phase scalar keeps integrating, for prints and promotion. |
| **How often** | Raw targets at the 64 Hz tick, interpolated (#155). Filters, phase, placement every frame. | As L0. | Never. |
| **Thread** | Fit on the compute pool, parallel across tanks; apply serial (task 6). Today: one thread, serial. | As L0. | — |
| **Shadow** | Dynamic ribbon `SHADOW_PROXY` (cast only), written in place (#156). Shoes `PROXIED_CASTER`. | The belt itself, with no scope, inheriting the tank's channel and casting and receiving. It is the proxy's exact tube on the same joints, so the shadow does not change at the switch. The proxy is hidden. | The static tube casts. Past 350 m (the configured shadow distance, CLAIMED in `settings.rs`) nothing casts anyway. |
| **Prints** | Grounded belly links from the rig's joints. Identical math to today's shoe poses: the belt frame is identity. | Same source, coarser joints: touchdown error ≤ the fit bound. | The rest belly's links at the true phase, against the stamper's own ground test. The contact tolerance is an owner decision. |
| **Hands over when** | the tube's deviation ε_belt projects ≤ budget | the travel the static belt ignores, ε_rest, projects ≤ budget | last layer |
| **ε, m** | ε_belt ≥ 0.088 (DERIVED lower bound). Cut and certify the tube as the next rung of `Link#0` with the same `scripts/lod` metric, and use that figure. | ε_rest = 0.200 (fixture `bump_stop`; droop ASSUMED ≤ bump stop) | — |
| **Switch distance** (+0.38 m radius), 1080p commander / 1440p commander / 1080p optic / 1440p optic | **115 / 153 / 215 / 286 m** | **261 / 348 / 488 / 650 m** | — |
| **View CPU per tank** | 119 µs today (DERIVED); **≈ 85 µs** after the exact waste cuts (task 3, DERIVED); ≈ 77 µs with the optional 13-sample arc | **≈ 38 µs** (DERIVED from the split, ASSUMED for the drape and the mesh write) | **≈ 0.3 µs** (selector) |
| **Entities and uploads per tank** | 194 shoes + 2 proxies. Proxy upload 43 KB per frame (DERIVED, audit), 0 when parked after #156. | 2 belts, 0 shoes. Upload 37.6 KB per frame while moving (DERIVED: 2 sides × 98 rings × 8 vertices × 24 B, positions and normals); 0 parked. | 2 belts, a shared mesh, 0 uploads |
| **Triangles per tank** (camera) | 32,204 at rung 4 (DERIVED) | 1,552 (DERIVED: 2 × 97 × 4 × 2) | 1,552 |

**Why L1's fit stays inside ε_belt.** The cheaper fit adds these errors (DERIVED):
- **Wheel arc.** Samples spaced equally in cos θ over ±50°, `n` per half, freeze an edge crossing
  for at most `R(1 − cos 50°)/n`. With R ≤ 0.5 m (the wheel nodes sit at y = 0.509) and n = 3, that
  is ≤ 60 mm. Today's uniform 5° arc has a 32 mm step at its outer samples. `n` derives from ε_belt
  and each station's measured R, never from a constant.
- **Two edge laterals.** These are exact on a locally planar patch, since the deepest of three
  collinear stations is an endpoint. On a crease under the centre the error is ≤ half the width ×
  Δslope / 2: 54 mm under the 0.726 m shoe at a 0.3 slope change, 12 mm under the 0.16 m disc.
- **The `RoadWheels` drape** leaves at most 63.1 mm inside an end circle (MEASURED, `route.rs` test,
  a quarter metre of slack on the fixture return run).

Each of these is < 88 mm. The filters (the wheel ease, the conform ease, the slack spring) are not
reset at the switch, so the shape eases between fits rather than stepping.

**Rejected: "nothing separate"** (folding the far belt into the hull's coarsest rung). The L2 belt
is already one shared static mesh per side; folding it buys nothing measurable and couples the belt
to #158's merge.

**Rejected: a cadence-decimated L1** (refresh every Nth frame). At 10.5 m/s (fixture top speed) two
frames at 60 fps is 0.35 m of travel (DERIVED). A held target misses a 0.2 m block step for that
span, which is past ε_belt.

**Precedent.** WoT draws a spline strip with a scrolling texture and turns its node animator off
past `chassisLodDistance` (CLAIMED, `wot-render-mechanisms.md` §2). L1 is their strip with the
animator; L2 is the strip past that distance. L0 has no WoT counterpart.

## Switch policy

- **Selection distance.** From the declared `PlayerView` eye to the nearest drawn belt joint of the
  tank, in world space. This is the same point `select_belt_rungs` measures to the nearest shoe, so
  it is continuous across layers. It uses last frame's eye: one frame stale, which the hysteresis
  band absorbs (v·dt ≈ 0.2 m against a band of about 15 m, DERIVED).
- **Thresholds.** `switch_distance_m(ε, r)` over the shared `ViewProfile`, recomputed only when the
  profile changes, as `adapt_shoe_switches` does. The two ladders (draw and fit) share one selection
  distance and one budget.
- **Hysteresis.** Demote at D > 1.1 × D_switch; promote at D < D_switch (ASSUMED 10 %). In pixels,
  that is demote at budget/1.1, promote at budget. The pop is sub-budget either way, so the band
  exists only to stop attach/detach thrash.
- **Matched phase.**
  - Every layer reads the rig's one presented phase (`presented_phase`).
  - The tube's link boundaries are its rings, and its rings are the pin joints, so at L0↔L1 the
    texture periods sit where the shoes' pins were. No scroll uniform is needed.
  - Wheel, sprocket and idler angles are pure functions of the phase, so at L2→L1 they reappear at
    the right angle.
  - The shoe pool re-attaches with `place_links`' phase rotation, so each shoe wears its own
    material link again.
- **Filter continuity.**
  - **L0↔L1:** no reset; the filters step toward the new targets.
  - **L1→L2:** the filters freeze.
  - **L2→L1:** the filters re-base to the current raw targets, as on a snap. The jump is ≤ ε_rest,
    which is 1 px at D12 by construction.
- **Pop, not a cross-fade.** This follows the house precedent: `geometry_lod` bands are abrupt,
  because dithering compiles a second pipeline permutation.
  - A dithered `VisibilityRange` exists (bevy_camera 0.19.1 `visibility/range.rs:80`, start and end
    margins). It would also draw both layers through the band, and it would bring back per-shoe
    range evaluation, which the belt selector replaced for cost.
  - Keep it as the fallback only if the tone check below fails.
- **Tone is not covered by the budget.** The budget bounds the silhouette. A belt whose mean tone
  differs from the shoe pool's at D01 shows as a step whatever the geometry. Acceptance: the belt
  region's mean luma, L0 against L1, pinned on the same frame at D01. The threshold is the owner's
  call.
- **Optic toggle.** Opening the 25° optic raises every threshold 1.87× (DERIVED: 3247.7/1738.2), so
  a group of tanks promotes in one frame. Detached shoe pools re-attach rather than respawn. The
  burst is priced in the captures below.
- **Off-screen tanks (option, not in the core).** A tank that no view saw last frame (`ViewVisibility`
  is false; bevy_light 0.19.1 `lib.rs:506` sets it for light-visible casters too) needs a fit for its
  prints only. The rule:
  - within the near store's 96 m: the L1 fit;
  - beyond it: the rest shape.
  The gain is large in the optic (most tanks off-screen, ASSUMED). The risk is a one-frame stale
  shape when a tank enters view, ≤ ε_F1 within 96 m. Measure the off-screen share in #153 first.

## The sim levers

Physics authority: view distance cannot thin it, so every lever is exact (bit-identical forces) or
a scheduling change.

1. **Cheaper exact queries.** This is task 3, shared with the view. Each lever is pinned by a
   bit-equality property test over random probes plus the `hblt` hash on a recorded run.
   - **Per-tank block pre-filter.** Once per tank per tick, list the blocks whose AABBs meet the
     tank's probe AABB (usually none: Kalinovo's only blocks are the 74 `house_proxy` instances in
     `level.json`). Then skip
     `candidates` for every probe. The min-fold is order-free, so a superset broadphase is
     bit-identical. Gain: −0.37 ms sim, −0.41 ms view (DERIVED).
   - **Fold the buried test into `cast_ray`.** The origin's cell is the DDA's first cell. Evaluate
     its plane once with `height_at`'s exact expression. Gain: up to −0.55 ms sim and −0.42 ms view
     (DERIVED upper bound; ASSUMED 80 % realised).
   - **`cast_ray` is not replaced by `height_at`.** The thin-ridge counterexample documented at
     `oracle.rs:369–376` and its ridge-graze test stand.
2. **Up-facing probe cull** (task 7, exact). About 40 % of segments face up: the return run and the
   upper arcs, DERIVED from 4.99 of 12.65 m.
   - Their rays start above the belt and go up.
   - Once per tank per tick, take the maximum `HeightGrid` node over the tank's footprint (about 36
     nodes).
   - A probe whose lowest point clears that maximum, with no block candidate, returns
     `reach − 2·reach = −reach` exactly as an uncut probe would.
   - Gain: −0.6 to −0.9 ms (DERIVED: 40 % of the probe time left after task 3, or of today's).
3. **Parallelism across tanks** (task 4).
   - **Mechanism.** `par_iter_mut` over the tank query. Each tank's tick is self-contained: `belt_tick`,
     then forces applied to its own body in report order. Order across tanks touches no shared number,
     so results are bit-identical.
   - **API checks** (VERIFIED in source).
     - `Query::par_iter_mut` needs `D: IterQueryData` (bevy_ecs 0.19.1 `system/query.rs:1302`).
     - The `QueryData` derive emits it when every field is (bevy_ecs_macros 0.19.1
       `query_data.rs:559`), and avian 0.7's `Forces` is plain `Read`/`Write` fields
       (`query_data.rs:105`).
     - A read-only `Query` is `Sync` through `UnsafeWorldCell` (`unsafe_world_cell.rs:95`), so the
       volumes lookup is shareable.
   - **Bitprobe.** While a `BitprobeCapture` is present the system runs serially (one shared
     capture), so CI's feature set compiles both paths.
   - **Workers.** bevy 0.19.1's default pools on this 10-core M4 give 4 compute threads plus the
     caller (DERIVED from `task_pool_plugin.rs:105–150`). A 6-core PC gets 2 compute threads.
   - **Gain.** About 3.49 → 0.8 ms of wall at W ≈ 4.5 with nothing else competing; less on 6 cores.
     How much of that reaches the critical path is UNMEASURED: it needs a timeline (tracy).
4. **Caching while the inputs are unchanged.** Only exact: key on the bits of `Position`,
   `Rotation` and phase plus the field revision, and cache each probe's `pen[3]`. The forces still
   run, because damping and grip integrate.
   - The likely hit rate is ~0: `apply_force_at_point` wakes the body every tick, so a parked hull's
     pose moves at ULP scale (ASSUMED, UNMEASURED).
   - Count bit-identical repeats first; build only if they are common.
   - A tolerance key is rejected: it would make the authority's contact depend on history.

**Combined, server or offline at 30 tanks:** 3.49 → about 2.1 ms of work after tasks 3 and 7, then
about 0.47 ms of wall in parallel (DERIVED; W ≈ 4.5).

## The near layer's own waste (#155, #156, and the oracle waste above)

| Lever | Source | L0 | L1 | L2 | Note |
|---|---|---|---|---|---|
| Pose-keyed cache of raw targets | #155 | ✓ | ✓ | — | A view cache may use a tolerance key, unlike the sim: 0.1 mm of pose is 0.035 px at the 5 m orbit, 1440p commander (DERIVED). |
| Targets at the 64 Hz tick, interpolated | #155 | ✓ | ✓ | — | Saves nothing at today's 52–55 fps; 47 % fewer fits at 120 fps (DERIVED). |
| Mesh written in place; index buffer built once; untouched when unchanged | #156 | ✓ proxy | ✓ belt | — static, shared | L0's proxy and L1's belt are the same tube builder, so one implementation serves both. |
| `set_if_neq` transforms | #156 | ✓ shoes, wheels, gears | ✓ wheels, gears | — | Saves writes and change ticks; propagation only for a root that did not move (Inputs, fact 3). |
| Scratch buffers on the rig (34 vectors per tank per frame, DERIVED, audit) | #156 | ✓ | ✓ | — | The allocation churn in the samples is about 0.3 ms at 30 tanks (DERIVED). |
| Block pre-filter, the buried test folded into `cast_ray`, an exact memo of the sink's mask corners | new, task 3 | ✓ | ✓ | — | The sim shares the first two. |
| Prints read joints, not shoe `GlobalTransform`s | new, task 1 | ✓ | ✓ | ✓ | Required by L1 and L2; restrict to belly links (about 40 per side, ASSUMED). |

## Product costs

**L0 → L1:**
- The horns (88 mm) and grousers (46 mm) leave the outline, and the 7.8 mm wedge gaps between
  plates on the wrap arcs close (DERIVED: 2 × 24.73 mm × tan 9°, 18° per link on the 0.4155 m
  sprocket). All are ≤ 1 px at the switch by construction.
- The comb of horns seen along the top run becomes a band.
- The shimmer of sub-Nyquist links stops.
- Tone mismatch is the real risk.

**L1's fit:**
- On edge crossings, a transient of up to 60 mm.
- On a crease under the shoe's centre, up to 54 mm.
- A drape up to 63 mm into the end wheels.
- All sub-pixel past D01.

**L2:**
- Wheels stop following the terrain and stop spinning, and the belt stops conforming. On rough
  ground the belly hovers or sinks by up to 0.2 m, which is ≤ 1 px at D12.
- Sprocket teeth are 0.65 px per tooth there (DERIVED), so no tooth lock is visible.

**Prints from L1 and L2 tanks:**
- Touchdown decisions come from a coarser belly.
- In dips under drooped wheels, L2 may miss prints, or lay extras with a widened tolerance.
- A trail's touchdown edges can move by up to about a pitch.
- This is invisible in the world layer (0.37 m texels, DERIVED: 1500 m / 4096). It shows later in
  the near store, which re-rasterises the history (`store.rs` module doc).
- In MP each client lays its own history, so two clients may show slightly different trails. Prints
  are already client-local view state.

**Other costs:**
- **Optic toggle:** one heavier frame.
- **The x-ray's mask copies** follow shoe mesh handles (`pen_cam/game/xray.rs:955`). For a tank at
  L1 or L2 it must copy the belt mesh instead.
- **`lod_showcase`** pins rungs through `ShoeBelt::pin`. Extend the pin to the layer: that is the
  instrument.
- **Concealment:** none. The belt stays the tank's own outline within the budget, and no layer adds
  or removes cover (#151's rule).

**Doctrine.** "VFX scale from physics, never from viewer distance" governs effect size. Geometric
LOD is already distance-selected (the shoe ladder), and no layer changes any size. Two places bend
it in spirit:
1. **Prints.** A mark laid by a far tank is placed less exactly than one laid by a near tank. Its
   size and depth law are unchanged. Keeping exact prints for every tank would cost the full fit,
   which is the whole saving.
2. **A tone mismatch.** A belt that reads lighter or darker at range would make far tanks easier or
   harder to spot. That is viewer assistance, and the luma acceptance exists to stop it.

## Build plan

One agent session each. Ordered by gain on the MP client in a battle; the sim tasks are ranked by
their server and offline gain. Gains are DERIVED at 30 tanks and overlap: the figures are marginal,
in this order.

| # | Task | Gain | Needs |
|---|---|---|---|
| 0 | **Instruments.** A forced layer pin (extend `ShoeBelt::pin` and `lod_showcase`); a belt-region luma comparator; the A/B/B/A runner from #152 over the near block, the far block (`OVERMATCH_PROBE_FAR=1`) and #153. | none: enables acceptance | #152 |
| 1 | **Decouple.** Rig-owned belt state (joints, phase quotient, hull affine) exposed to `track_prints`; `ShoeMemory` keyed by (side, material link); #156 (in-place ribbon, index buffer once, `set_if_neq`, scratch on the rig). Acceptance: stamps bit-identical to today's on a recorded drive. | −0.3 ms, all scenes | — |
| 2 | **Selector and L2.** `TrackLayer` per tank; law-derived ε_rest from the spec's travel; hysteresis; the shared rest tube with the shoe material as its first look; shoe pool detached; wheels frozen; prints from rest joints. | battle −1.7 ms view + ~0.9 pool-ms; far block −3.1 ms (16 or 28 tanks × ≈ 109 µs after task 1) | 0, 1 |
| 3 | **Exact oracle waste.** Block pre-filter, buried test folded into `cast_ray`, sink corner memo; bit-equality tests and `hblt`. | view −1.0 ms with every tank in L0, −0.5 ms in the battle after task 2; sim −0.8 ms | — |
| 4 | **Sim parallel across tanks**, with the serial bitprobe path; serial and parallel hash equality. | server and offline −1.6 ms after 3 and 7 (−2.7 ms alone) | — |
| 5a | **Belt asset.** Bake a tread strip from the shoe in Blender (albedo, roughness, one period per pitch: authored data per tank); cut and certify the tube as `Link#0`'s next rung (`scripts/lod`) for ε_belt. | none: gates 5b | — |
| 5b | **L1 runtime.** The tube with UVs and normals; the proxy/belt visibility swap; the L1 fit (equal-cos arc, edge laterals, `RoadWheels` drape); x-ray copies the belt. | battle −0.5 ms view + ~0.6 pool-ms | 2, 5a |
| 6 | **View fit/apply split, parallel fit across tanks.** | battle −0.5 ms (0.73 → ≈ 0.21 ms of wall); near block (moving, all L0) −1.8 ms | 1 |
| 7 | **Sim up-facing probe cull** (exact). | server and offline −0.6 ms | 3 |
| 8 | **#155 as filed:** pose cache and tick-rate refresh. | battle at ≤ 64 fps: 0; at 120 fps −47 % of the remaining fit; idle near block up to −2.5 ms serial if the parked pose repeats (UNMEASURED) | 1 |
| 9 | **Option:** a 13-sample equal-cos wheel arc at L0. Same 32 mm frozen-step bound as today's 21; 38 % fewer wheel probes. | −0.3 ms at 30 L0 tanks | owner |
| 10 | **Option:** GPU-deformed shoe belt at L0. One entity per side and a joint array per belt, via `MaterialExtension::vertex_shader` and `prepass_vertex_shader` (bevy_pbr 0.19.1 `extended_material.rs:36,66`). Removes 194 entities per tank at every distance. Large, with shader risk. | ~58 µs pool per tank, ASSUMED proportional | owner |

**Totals at 30 tanks** (DERIVED; ASSUMED layer split and pool width):
- **Battle on the MP client** (4 in L0, 10 in L1, 16 in L2): view 3.56 → 0.73 ms of work → about
  0.2 ms of wall after task 6, so −3.3 ms. Plus about 1.5 pool-ms from 26 tanks shedding their
  shoe entities, ASSUMED proportional to entity count.
- **Idle near block (today's scene):** the layers save nothing. Tasks 3, 6 and 8 take the view to
  0.1–0.7 ms (0.1 if the parked pose cache hits); tasks 3, 7 and 4 take the sim to about 0.5 ms of
  wall.
- **Server:** about 3.0 → 0.4 ms per tick.

## What the owner should see side by side before approving

Every pair is pinned (task 0) on the same frame and pose: the L0/L1 pair on the same fit
(identical joints), the L1/L2 pair on the same pose, so each pair isolates one swap. Native
resolution.

1. **Belt switch stills.**
   - Pairs: L0 against L1, a parked tank broadside, 45° front, and from orbit height (top run in
     view).
   - At D01: 115 m (1080p commander), 153 m (1440p commander) and 286 m (1440p optic).
   - Context: also at 0.5 × D01 and 2 × D01.
   - Conditions: flat ground and a rough patch; high and low sun.
   - With each pair: the belt-region Δ mean luma and the largest per-pixel difference.
2. **Belt switch in motion.** Video of a scripted drive at 5 and 10.5 m/s from 0.8 × D01 to
   1.3 × D01 past a fixed camera, layers on against layers off. Watch for the pop, and for the
   shimmer the belt removes.
3. **Far switch.** L1 against L2 while driving over bumps, at D12: 348 m (1440p commander) and
   650 m (1440p optic). Video, to judge frozen wheels.
4. **Prints.** One route driven three times, pinned L0, L1 and L2, the camera beyond 96 m while
   they are laid. Then stills of the same trail segment from 10 m, on flat and on bumpy ground.
5. **Shadow.** Low sun, L0's proxy shadow against L1's belt shadow at D01, inside the 350 m
   envelope.
6. **Optic toggle.** A group of tanks straddling D01: stills before and after, and the frame-time
   trace of the toggle frame.
7. **Frame cost.** A/B/B/A, layers on and off, in the near block, the far block and #153.

## Owner decisions

1. **The belt's switch.** Certify the tube as rung 5 of the shoe chain (one law, one certificate)
   instead of hand-picking a distance. Bake its look from the shoe (task 5a) rather than putting
   the plain shoe material on the tube.
2. **Prints from L1 and L2 tanks.** Accept approximate touchdowns, or keep the full fit for print
   producers and give back most of the saving. For L2, either widen the stamper's contact tolerance
   by ε_rest (extra prints in dips) or leave it (missing prints in dips).
3. **Pop or dithered cross-fade at L0→L1.** Pop unless the luma check fails.
4. **Motion budget.** Whether the fit layers' motion errors share the 1-px static DETAIL BUDGET or
   get a stricter one of their own.
5. **L2 freezes wheel articulation and spin** past 261–650 m.
6. **Hysteresis band** (10 %), and whether the optic-toggle frame may be heavier.
7. **Option tasks 9 and 10.** The L0 arc density was a bound, not a playtest verdict; the rise rate
   was the verdict.
8. **#158 scope.** This design is the track half of #158; its body-merge half is separate.

## Not measured

- **Each layer's cost** beyond the L0 split; L1's drape and mesh-write figures are ASSUMED.
- **The pose-cache hit rate** for parked tanks, in the view and the sim.
- **The critical path.** How much of the serial time comes off the frame once parallel (needs
  tracy).
- **The render-side cost** of per-frame mesh uploads, and of the shoe entities in propagation,
  extraction and visibility. The 58 µs pool per tank is ASSUMED proportional to entity count.
- **The battle's layer split and off-screen share.** #153 settles both.
- **The shoe-to-tube deviation.** 88 mm is a lower bound read from mesh bounds; `scripts/lod`
  certifies the real figure.
