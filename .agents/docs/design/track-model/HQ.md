# Track model — HQ

Living command post for the continuous-track slice (physics + procedural animation). Update this as
work lands so a fresh session / post-compaction context can pick up without re-deriving. Keep it
terse and current; move settled detail into ADRs when a step stabilizes.

## North star

A **belt/envelope-based** track model that replaces "16 points under the hubs" with continuous
contact along the real track contour — so the tank bridges trenches, climbs ledges, and generates
traction wherever the belt touches ground. Then a **procedural track+suspension** rendering driven by
the same envelope. Target fidelity: competitive-PvP honest, not mil-sim. This is debt-avoidance, not
gold-plating (see Dagor/War Thunder note below).

## Why one slice

Both physics and animation want the **same primitive**: the track envelope as a function of
suspension state — a fixed-length loop pinned at sprocket + idler, draped over the (compressed)
roadwheels, sagging on slack runs. Physics samples contact/traction along it; animation places links
along it. Compute once, two consumers.

## Research findings (2026-07, condensed)

- **Sprocket** (solo dev, closest precedent) did exactly this migration: dumped PhysX for a custom
  solution, moved collision from wheels to the **belt**; "anywhere the belt touches ground provides
  suspension + traction." Added a **slip model** (belt speed decoupled from ground speed → slip/skid/
  drift) and **bump-stops** (suspension travel limit → sudden corrective impulse; punishes bad
  suspension with a rough ride that scrubs speed + disrupts aim).
- **Big-sim consensus** (GameDev.net / Bullet): keep single rigid body; distribute ground interaction
  across all belt contact points; model belt as its own angular-momentum/speed state. Full link-body
  sim = perf/stability/LOD pain, reserved for BeamNG-tier soft-body.
- **XPBD** (Avian is XPBD): the old "track-as-physics is unstable" verdict is stale — XPBD is
  unconditionally stable for stiff constrained chains with substepping. Full link bodies still likely
  overkill for networked 1v1, but the *reason* in ADR-0005 no longer holds and should be re-stated.
- **Dagor / War Thunder** cautionary tale: faked track physics with static wheel colliders → years of
  "traction nerfs", invisible grease, kill-walls, map exploits. Gaijin is migrating to Jolt for real
  ground interaction. Lesson: for competitive PvP, fake-it is *more* long-term cost, not less.
- **Procedural track rendering** (Unity/Unreal precedent): spline control points bound to wheel
  transforms; links placed by **uniform arc-length** distribution (query position+tangent at
  D = i·spacing), NOT by spline parameter t (which stretches/squashes over uneven control spacing).

## Physics spectrum (cheap → expensive)

- **A. More ray stations** — add contact rays on the diagonal runs + belly, not just hubs. Tiny;
  pure ADR-0005 extension. Solves the ditch minimally.
- **B. Envelope-sampled belt** (Sprocket's model) — sample support+traction anywhere the computed
  contour is below ground; add per-track belt-speed/slip state. The sweet spot.
- **C. Swept/multi-body collider** — give the belt run an actual collider so *resting geometry* is
  physical (the clip-through fix — orthogonal to thrust).
- **D. Full XPBD link chain** — every link a body. Emergent, stable under XPBD, but heavy for 2×
  tanks over the wire. Deferred.

Clip-through vs thrust are **separate axes**: B gives traction, C stops the clip. The ditch wants
both.

## Decisions

- **Dedicated bin `track_sandbox`** (mirrors `armor_sandbox`): self-contained plugin, its own
  locomotion, no dependency on `tank`/`spec`/`driving`. Iterate the model in isolation, promote to
  the game later.
- **Code-generated primitive rig** (no glTF, no TankSpec): parametric running gear (N roadwheels on
  swing arms + sprocket + idler) so wheel count / spacing / track length / trench width are knobs.
  Tiger port (with Blender swing-arm rigging) is a *later* slice once the model is proven.

## Build order

1. **Scaffold** — bin, free-fly cam, test course (flat + trench + step + ramp), static parametric
   rig, skeleton + envelope drawn with gizmos. *Just see the rig.*  ← IN PROGRESS
2. **Envelope primitive** — fixed-length loop, sag on slack runs, drawn as a spline. No physics yet.
3. **Suspension from envelope** — sample ground contact along the whole contour; apply support.
   Verify trench bridging.
4. **Traction + slip + bump-stops** — belt-speed state per track; thrust/friction from track-vs-
   ground speed delta; suspension travel limits.
5. **Procedural link rendering** — arc-length link distribution along the spline (animation payoff).

## Status log

- 2026-07-01 — Research consolidated (this doc). Scaffolding `track_sandbox` (step 1).
- 2026-07-01 — **Step 1 landed** (green: fmt/clippy/build). `bin/track_sandbox` + `src/track_sandbox.rs`
  (self-contained, mounts only `PhysicsPlugins`). Code-gen primitive rig: hull box + 2 tracks
  (5 road wheels + sprocket + idler each), static. Course: flat lane, a 2.2 m **trench** (two green
  slabs), a step, a 20° ramp — all on `Terrain`. Belt envelope drawn per side as a cyan gizmo loop
  via lower/upper external tangents of the wheel circles (`external_tangent`); yellow hub markers.
  Free-fly cam (WASD/Shift/Ctrl + mouse). `Esc` releases cursor + pauses Avian time (for screenshots).
  - Fixed after first run: removed a stray second camera that re-rendered the scene (the "split
    screen"); added the `Esc` pause.
  - Controls: `cargo run --bin track_sandbox`; WASD move, Shift/Ctrl up/down, mouse look, `Esc`
    pause/screenshot.
- 2026-07-01 — **Step 3a landed** (green: fmt/clippy/build; clean launch, no panics/mass warnings).
  Reordered ahead of the sag refinement (step 2) to hit the physics thesis first (user is physics-
  first). Hull is now **dynamic** (authored `Mass`/`AngularInertia`, `NoAuto*`, no hull collider —
  carried entirely by the belt). `apply_belt_support` (FixedUpdate): resamples the lower run at
  `CONTACT_SPACING` and applies a **vertical damped penalty spring** wherever a station is at/below
  ground → distributed belt contact, not hub points. `R` drops the rig straddling the trench; a
  trench floor at `TRENCH_FLOOR_Y` catches a failed bridge. Green contact dots (sized by load) show
  the live contact distribution. Tunables: `SUPPORT_STIFFNESS` 80k, `SUPPORT_DAMPING` 10k,
  `CONTACT_SPACING` 0.25, `CONTACT_PROBE` 0.5, `HULL_MASS` 12 t.
  - Model note: support is world-Y (gravity axis), not ground-normal — stable, but slope traction/
    normal is a later refinement. No drive yet (can't self-propel; `R` is the bridging test).
  - AWAITING USER TEST: does it (1) rest stably on flat ground at a few cm sink, no jitter/bounce;
    (2) bridge the trench on `R` (belt on both lips, small sag) rather than fall into the ditch?
  - NEXT (step 4-ish): minimal differential drive (thrust at grounded stations) so you can drive
    into obstacles; then slip/belt-speed + bump-stops; sag refinement folds in with procedural links.
- 2026-07-01 — **Step 3a verified + tuned** (user screenshots: rests on flat, bridges narrow trench;
  both with a small sink). Fixes/additions:
  - **Pause→launch bug fixed**: `apply_belt_support` gated by a `Paused` resource + `sim_running`
    run-condition. Root cause: the belt system is on Bevy `FixedUpdate` (virtual clock) while `Esc`
    only paused Avian `Time<Physics>`, so penalty force accumulated against a frozen sim and flung
    the rig on resume (higher the longer paused).
  - Stiffer springs (`SUPPORT_STIFFNESS` 80k→160k, damping 10k→14k) → ~2 cm sink.
  - `L` logs hull y / station count / total support vs weight (exact tuning numbers).
  - Course now has **two trenches** (narrow 2.2 m, wide 5.0 m > road-wheel span) + step + ramp,
    data-driven from `TRENCHES`. `R` cycles drop spots: flat → narrow → wide. The wide trench is the
    pure-diagonal bridge (all road wheels float; only sprocket/idler diagonals catch the lips).
  - VERIFIED via `L` telemetry (user run): flat y=1.133 (1.7 cm sink) 19 stations 101%; narrow
    trench y=1.114 22 stations 100%; **wide trench y=0.905 stable, 8 stations, support = 100% exactly
    and holding** — all road wheels floating, only the diagonals carrying. Thesis proven. Pause no
    longer launches. Bridging settle ~24 cm nose-down (geometric + penalty; physical, not unstable).
- 2026-07-01 — **Step 4a landed** (green): differential **drive + skid-steer friction** folded into
  `apply_belt_support` (now support + traction in one pass over the belt stations). Arrow keys (↑/↓
  throttle, →/← steer — WASD stays the camera). Per grounded station: thrust `command·THRUST_PER_
  STATION − ROLLING·v_fwd`, lateral grip `−LATERAL_GRIP·v_lat`, whole vector capped on the friction
  ellipse (μ·load, lower sideways). Total tractive effort scales with grounded footprint. Open-loop
  (no belt-speed/slip yet); no brush-anchor hill-hold yet (will creep on slopes — port from
  `driving.rs` later). `DriveInput` zeroed when cursor free.
  - VERIFIED (user, screenshot): drives, steers, and bridges/climbs trench edges convincingly. Three
    notes → next increment.
- 2026-07-01 — **Step 4b: contact refinement** (green). Addressed the three user notes:
  1. **Gizmo jitter** (dots vs rig) fixed: `BeltContacts` now stores each contact in **hull-local**
     space (+ load + normal); `draw_contacts` transforms by the *current* (interpolated) hull pose so
     dots ride the rig instead of lagging the last fixed-tick pose.
  2. **Eager wall-climb** fixed at root: support is now along the **ground contact normal** (was
     world-Y). A near-vertical face pushes the rig back, not up; slopes get honest normal force. The
     normal is drawn (yellow line per contact) so it's visible.
  3. **Step-bump**: `CONTACT_SPACING` 0.25→0.15 (finer sampling, smaller per-station jump over a
     ledge). `THRUST_PER_STATION` 3500→4500 so the now-honest 20° ramp stays climbable.
  - REGRESSION (user): "tank launches upward when in full contact with the ground." Cause: 4b's finer
    spacing kept *per-station* stiffness constant → ~1.7× stiffer aggregate ground → firm/full contact
    spiked a huge restoring force and flung the rig. (Jitter + normal fixes were fine.)
- 2026-07-01 — **Launch regression fixed** (green): made all contact + drive coefficients **per metre
  of belt** (`SUPPORT_STIFFNESS_PER_M` 640k, `SUPPORT_DAMPING_PER_M` 56k, `THRUST_PER_M` 20k,
  `ROLLING_PER_M` 1.8k, `LATERAL_GRIP_PER_M` 12k), multiplied by `CONTACT_SPACING` per station. Totals
  now match the 4a config the user verified stable, at any resolution — `CONTACT_SPACING` (0.15) only
  affects smoothness. Key lesson for the eventual game port: **coefficients are per-length, not
  per-station.**
  - VERIFIED (user): "works". New issue surfaced → next.
- 2026-07-01 — **Solid-body collision** (green). Problem: the track *phased through vertical walls* —
  downward-ray belt support structurally can't resist a horizontal face (it's a raycast-vehicle probe;
  walls are a collider's job — ADR-0005 already says the hull box is a collision shape). Added:
  **hull box collider** + **sprocket & idler cylinder colliders** (rigid to the hull, on the `Vehicle`
  layer; road wheels get none — they'll articulate later). Sprocket/idler are the track's front/rear-
  most points, so their cylinders extend the collision silhouette to the true track ends (tank stops
  where the *track* meets a wall). Clearances: at ride height hull box (0.6 m) + drive cylinders
  (0.45 m) sit above ground → belt still solely carries on flat terrain; colliders engage only on
  walls / hard bottoming. Mass stays authored (`NoAuto*`; colliders add no mass).
  - Climbing model (discussed w/ user): cylinders give an **honest hard limit** — climbs up to where
    the front belt stations can hook the top edge, then hard-stops. Does NOT yet model the real
    "grinding-climb" of steps taller than the sprocket (moving front belt face → upward friction
    reaction). That needs the **belt-speed/slip model** + applying belt friction at *wall* contacts —
    the payoff of the next step, where it falls out of the physics (front face runs downward when
    driving fwd → wall friction pushes up).
  - VERIFIED (user): "looks good". → belt-speed/slip next.
- 2026-07-01 — **Step 5: belt-speed / slip model** (green). Replaced open-loop thrust with a real
  slip model. Per track a `BeltSpeed` state; engine governor chases `command·MAX_BELT_SPEED` with
  force clamped to `ENGINE_FORCE`; ground friction per station = `μ·load·saturate(slip/SLIP_SATURATION)`
  where `slip = belt_speed − ground_speed`; that friction reacts back on the belt (`belt += (engine −
  Σfriction)/BELT_INERTIA·dt`). Emergent: **wheelspin** (over-throttle low grip), **skid**, **engine-
  braking** (release → belt decays → drags tank down), **hill-hold** (belt at 0 resists slide up to
  μ·load — replaces the planned brush-anchor port), **bounded top speed**. Lateral unchanged in spirit
  (slip-saturated, ellipse-capped). Removed `THRUST/ROLLING/LATERAL_PER_M`; added `MAX_BELT_SPEED` 11,
  `ENGINE_FORCE` 90k, `BELT_GOVERNOR_GAIN` 60k, `BELT_INERTIA` 3k, `SLIP_SATURATION` 0.4.
  - Viz: contact dots now colour **green→red by slip** (wheelspin lights up red); `L` logs belt L/R
    vs tank speed (the gap = slip). Belt speed zeroed on `R`.
  - AWAITING USER TEST: drive feel; wheelspin visible (red dots) when flooring it from rest; engine-
    braking on release; holds on the ramp without throttle (hill-hold); top speed ~11 m/s. Tune
    ENGINE_FORCE / BELT_INERTIA / SLIP_SATURATION / MAX_BELT_SPEED from there.
  - NEXT: grinding-climb (belt friction at wall contacts) · bump-stops · procedural animated track.
- 2026-07-01 — **Belt spline completed** (green). User: "sprocket/idler larger than the track spline,
  wheel front doesn't contact the wall." Diagnosis: collider radius == wheel mesh == belt tangent
  radius (all `DRIVE_RADIUS`), so nothing is actually oversized — but the drawn cyan spline was only
  the straight tangents and **skipped the arcs wrapping the sprocket/idler**, so the wheel visibly
  bulged past the line. Added an `arc()` helper; `draw_rig_gizmos` now draws a full closed loop
  (lower run → idler rear arc → top run → sprocket front arc) that hugs every wheel and coincides
  with the colliders. Also the exact path the procedural track will follow.
  - User confirmed (b), specifically: stuck in the wide ditch, belt pressed on the far wall but **no
    contact dots on the wall** — because contact sampling only probed *down*. Chose the **full**
    generalization.
- 2026-07-01 — **Step 6: outward-normal belt contact** (green) — biggest core change since the belt
  model. `apply_belt_support` now samples the **whole belt loop** (`belt_loop()`, shared with the
  gizmo), and at each station probes along the belt's **outward normal** (tangent rotated −90°, CCW
  winding) instead of always down. Support along the hit normal; traction with the drive axis =
  −tangent projected into the contact plane (so on the front face it points **up** → a spinning belt
  **grinds up walls / climbs out of ditches**), lateral across it, ellipse-capped, longitudinal
  friction reacts on the belt. Reduces exactly to the old model on flat ground (outward = down).
  Colliders kept as the hard backstop. `Contact.slip` now stores longitudinal slip.
  - This is the belt-based-collision model from the Sprocket research; one mechanism now covers
    ground, ledges, walls, ditch faces.
  - Initially "still no" wall dots → diagnosed: the sprocket/idler collider radius == belt radius, so
    the collider hard-stopped the wheel exactly at the belt surface → belt could never penetrate a
    wall → `pen<=0` → no belt-wall contact ever fired. (User's hunch: "spline is in the same space as
    the wheel.")
- 2026-07-01 — **Belt-primary contact / collider inset** (green). Added `DRIVE_COLLIDER_SCALE` (0.6):
  the sprocket/idler colliders are inset *inside* the belt surface so the **belt penalty is the
  primary contact** (must be able to penetrate to generate support + grinding friction); the collider
  is now just a hard backstop vs fast-impact tunnelling. **VERIFIED (user): "beautiful — the track
  climbs, force gizmos appear at the wheel's contact with the wall."** The outward-normal grinding-
  climb works; sign is correct (grinds up).
  - Design takeaway for the game port: belt is the contact model; any wheel/hull colliders must sit
    *inside* the belt envelope, never on it.
  - TO CONFIRM w/ user: does the **wide ditch** (low wall, below the drive wheels — handled by the
    belt-lower which has no collider) also climb out now, or is that still a separate stuck case?
  - NEXT: bump-stops · procedural animated track · (then) promote model toward the game.
- 2026-07-01 — Wide-ditch climb-out **confirmed** by user. Aligned on the desired end-state (Tier-B /
  Sprocket): road wheels articulate on real spring-arms + carry the load; belt drapes over them; fixed
  length → top sag + droop limit; bump-stops. Force-generating, not cosmetic.
- 2026-07-01 — **Step 7: per-wheel suspension** (green). New `apply_suspension` (chained *before*
  `apply_belt_support`): each road wheel raycasts down a sprung arm (`SUSP_*` consts), **carries the
  hull** (spring−damper lift) and applies its share of belt-slip traction; the wheel entity is moved
  (`Suspension.pivot_local` keeps the fixed raycast source) so it **articulates** and the spline
  drapes over it. Compliance/feel now lives on the wheels (~15 cm static travel), not the stiff belt
  penalty — which **self-nulls on flat** (wheels hold the belt at the surface → `pen≈0`). Belt still
  does wall/gap contact + grinding-climb. Traction from wheels + belt is summed via a `BeltReaction`
  resource so the belt-speed integrator sees the full load. Added a **washboard** to the course to
  make articulation visible. Contact dots cleared/pushed by the suspension pass now too.
  - AWAITING USER TEST: (1) wheels visibly bob over the washboard while the hull stays composed;
    (2) the spline lower run drapes over the moving wheels; (3) still drives / wheelspins / climbs
    walls / bridges as before (no regressions); (4) rest height stable (~1.15), no fighting between
    wheel springs and belt penalty on flat.
  - KNOWN-DEFERRED in this step: fixed-length top sag + true droop limit (droop currently a fixed
    `SUSP_DROOP_TRAVEL`), and bump-stops — next.
  - VERIFIED (user, screenshot): wheels articulate over the washboard, lower spline drapes — great.
    Reported flat-ground **jitter standing still**.
- 2026-07-01 — **Flat-ground jitter fix** (green): added `CONTACT_DEADBAND` (0.03 m) to the belt
  support. Cause: with wheels now carrying, the belt lower run hovers at ~0 penetration on flat, and
  the very stiff belt spring fired on/off on sub-mm noise, buzzing against the wheel springs. The belt
  now ignores penetration shallower than the deadband (flat noise) and only engages on real contact
  (bridging ~0.18 m, walls) — wheels own flat ground cleanly. Confirmed: "drape" = lower run conforms
  to wheels (top run still straight; sag is the deferred fixed-length work).
  - AWAITING USER TEST: flat-ground jitter gone; bridging + wall-climb unchanged (belt still engages
    there, just 3 cm deeper).
  - VERIFIED (user): jitter gone, articulation + drape great. Two refinements requested → next.
- 2026-07-01 — **Two refinements** (green):
  1. **Wheel travel (no snap):** `Suspension.dy` is now an eased state — the visible articulation
     approaches its raycast target at `SUSP_TRAVEL_RATE` (2.5 m/s) instead of teleporting. Lift force
     stays instantaneous (hull physics unchanged); only the wheel's visible travel + the spline drape
     are smoothed.
  2. **Wedge chatter / green-red strobe at ditch lips fixed:** belt support now pushes along the
     belt's **own inward normal (−outward)** — smooth, from the spline — instead of the terrain
     hit-normal, which flipped between up/sideways when a ray landed on the wall/top **edge** of a
     lip, shoving the rig in alternating directions (wedging + slip strobe). `−out` still pushes off
     walls and up off ground; only the direction is stabilised. (User's "perpendicular forces" hunch
     was right.)
  - AWAITING USER TEST: (1) wheels ease/travel over the washboard, no snap; (2) nosed into the wide
    ditch it no longer wedges/strobes — grinds out smoothly; (3) no regressions on flat/bridging/climb.
  - NEXT: fixed-length constraint (top sag + true droop limit) · bump-stops · procedural track.
  - VERIFIED (user): eased wheels much better. Corner/wedge bug persists (−out didn't fix it) —
    **deferred by mutual agreement**; real fix is a **sphere/shape-cast probe** (also fixes wheels
    snapping when their center crosses a step edge — same point-probe root). Batch when the pieces are
    in. User still forming the wheel/track/ground mental model.
- 2026-07-01 — **Step 8a: fixed-length top sag** (green). Belt length fixed once at startup
  (`init_belt_length` → `BeltLength` = rest taut perimeter + `TRACK_SLACK` 0.02 m). `belt_loop` now
  takes `length: Option<f32>`: `Some(L)` sags the return run (parabola, depth from the arc-length the
  fixed belt leaves for the top run over its straight span — `sagging_top`); `None` keeps it straight.
  Physics uses `None` (top run never contacts ground → untouched, zero regression risk); the **gizmo**
  uses `Some(L)` so the top run visibly sags and redistributes as wheels articulate. Sag ∝ √slack, so
  `TRACK_SLACK` is a sensitive knob.
  - DEFERRED (this step): true tension-based **droop limit** — it couples all wheels through the
    shared length and is better built once the wheel/track/ground relationship is firmer; the fixed
    `SUSP_DROOP_TRAVEL` cap works meanwhile.
  - AWAITING USER TEST: top run visibly sags (and the sag breathes as wheels move over the washboard);
    no regressions (physics unchanged). Tune `TRACK_SLACK` for the amount of sag.
  - NEXT: bump-stops · procedural animated track · (deferred) sphere-cast probe · tension droop limit.
  - VERIFIED (user): sag looks good but "missing the length constraint" → build it + bump-stops.
- 2026-07-01 — **Step 8b: fixed-length droop limit + bump-stops** (green). `apply_suspension`
  rebuilt into three passes: (1) raycast each wheel for its desired articulation + ground load, now
  with a **bump-stop** (`BUMP_STOP_STIFFNESS` 780 k/wheel engages past the compression travel limit →
  sharp jolt when bottoming); (2) **fixed-length droop limit** — per side, if the taut perimeter with
  the wanted droop exceeds the belt length, raise the *airborne* wheels (they carry no load) by
  ~excess/(2·n) until the belt is just taut, so wheels are held on the track line over a gap instead
  of dangling in, sharing one slack budget with the top sag (droop consumes slack → sag flattens);
  (3) apply lift + traction. New helpers `drive_circles_local`, `taut_perimeter` (shared with
  `rest_circles`/`init_belt_length`). Droop limit only touches airborne (zero-load) wheels → physics-
  safe; concave all-grounded ground can still exceed L (sag clamps to straight) — acceptable approx.
  - AWAITING USER TEST: (1) over a gap the wheels stay ~on the taut line (don't dangle in), belt taut
    across; (2) top sag flattens as wheels droop/compress (shared length); (3) bump-stop jolt when
    slamming down hard (e.g., off the ramp crest / step); (4) no flat/drive/climb regressions.
  - VERIFIED (user, screenshots): looking good. User then zoomed out on the model and named the real
    gap: **the wheel↔ground contact is point-sampled** while the belt is footprint-sampled. His mental
    model: each wheel pushes on its *own section of track*, strongest under the wheel, handing off to
    the neighbour at the midpoint; the hub-down ray means the wheel body + spline phase through terrain
    beside the hub and snap at edges. Chose the **full** unification.
- 2026-07-01 — **Step 9: footprint suspension (unification)** (green). Rewrote `apply_suspension` from
  a single hub-down ray to a **distributed footprint**: each road wheel probes `FOOTPRINT_SAMPLES` (8)
  down-rays tiling its **tributary** (±½ wheel-spacing along the track, cell-centred so adjacent wheels'
  strips meet exactly), and each in-contact sample is a **soft vertical spring**. The wheel now supports
  its whole section of track — so it can't sink into terrain *beside* the hub (phasing), it articulates
  to clear the **highest terrain in the footprint** (max compression → no snap as one point crosses an
  edge), and load concentrates under the wheel + hands off to the neighbour at the tributary midpoint
  (the scalloped pressure the belt/wheel model should produce). Per-sample lift + slip-traction +
  bump-stop, all **per-metre of footprint** (`SUSP_STIFFNESS_PER_M` 68 k, `SUSP_DAMPING_PER_M` 8.7 k,
  `BUMP_STOP_STIFFNESS_PER_M` 680 k) × the arc-length each sample owns, so sample count = smoothness
  only; the total stiffness = the old per-wheel calibration (≈mg at ~15 cm), same rest height. Removed
  per-wheel `SUSP_STIFFNESS`/`SUSP_DAMPING`/`BUMP_STOP_STIFFNESS`. Droop-limit + belt passes unchanged;
  belt stays retired from flat ground by its deadband (wheels now robustly own the belly).
  - Why no belt skip-band: the deadband + belt drape already keep the belt quiet where wheels handle
    the ground, and it must still fire over gaps where wheels are airborne — so leaving the belt pass
    as-is is both correct and lower-risk.
  - Note: the footprint-max articulation is a *discretised* sphere-cast (8 samples). If edge-snap still
    reads, bump the sample density or swap to a true `cast_shape` sphere — the deferred item, now
    composing naturally on top of footprint sampling.
  - AWAITING USER TEST: (1) drive slowly over the step/washboard — wheels no longer phase into or snap
    over edges; the contact dots spread into a *patch* under each wheel (not one dot) and the patch's
    weight migrates wheel-to-wheel as terrain passes; (2) flat rest still calm (~1.15, no jitter); (3)
    no drive/wheelspin/climb/bridge/bump-stop regressions.
  - USER (screenshot, on the washboard): wheels **float above** the fine ridges with **contact dots
    detached below the spline**. Diagnosis: the distributed vertical-ray footprint was the wrong
    mechanism — each thin ray reaches the *valley floor* the fat wheel (0.7 m dia) actually **bridges**
    over (valleys 0.45 m), so it registered **phantom sub-wheel contacts** below the wheel and
    over-supported. (Not the droop limit — those wheels are grounded.) A rigid wheel contacts at ~one
    point, the highest terrain it can touch; distribution across a single wheel is unphysical.
- 2026-07-01 — **Step 9b: radius-aware wheel probe (the sphere-cast)** (green). Replaced the vertical-
  ray footprint with a **discretised cylinder cast** per wheel: `FOOTPRINT_SAMPLES` (7) down-rays across
  ±`ROAD_RADIUS`; for each column the wheel surface is `sqrt(R²−dz²)` below the hub, so the wheel-centre
  descent to touch that column is `hit.distance − sqrt(R²−dz²)`; the **min** over the width is where the
  wheel first touches (the highest terrain it can reach). One soft contact per wheel again (restored
  per-wheel `SUSP_STIFFNESS` 78 k / `SUSP_DAMPING` 10 k / `BUMP_STOP_STIFFNESS` 780 k), articulate +
  lift + traction + bump-stop unchanged. Reduces to the old single ray on flat (`descent = hub_y − R`).
  Kills the phantom valley contacts, bridges dips narrower than the wheel, no phasing, and the min is
  continuous → no edge-snap. The **between-wheel** distribution is the belt's job (it already spans
  wheel-to-wheel), not sub-wheel springs. Also **coarsened the washboard** (period 0.9→1.5 m, gap 1.0 m
  > wheel dia, height 0.15→0.18) so wheels can actually resolve it and visibly articulate.
  - Model takeaway: the wheel is a rigid roller (radius-aware, ~point contact on the highest terrain);
    the *belt* is the distributed/continuous contact. Don't distribute a single wheel's support.
  - AWAITING USER TEST: (1) on the coarser washboard the wheels **drop between bumps and ride over**
    them (independent articulation), contact dots stay **on the wheel/spline** (no detached dots, no
    float over reachable ground); (2) fat wheels still correctly *bridge* the narrow ditch lips / step
    edge without snapping; (3) flat rest calm; (4) no drive/wheelspin/climb/bridge/bump-stop regressions.
  - NEXT: procedural animated track.
- 2026-07-01 — **MODEL PIVOT: belt is the sole ground contact (fresh review w/ user).** User's
  diagnosis (correct — matches terramechanics + Sprocket's own migration): the wheels were being made
  the ground-contact points, but physically the **wheels press down on the track and the track presses
  on the ground** (multi-peak ground pressure, peaks under the wheels, track as a tensioned membrane
  bridging between). Root cause of the jitter/illogic named: **two parallel stiff penalty-spring
  systems both probed the ground independently** (per-wheel `apply_suspension` + belt penalty),
  partitioned by a 3 cm **deadband seam** — a switching threshold between parallel stiff springs =
  limit-cycle chatter; plus point-sampled wheel contacts that hop, plus eased-visual-vs-raw-force dot
  detachment. Reframe = a single series chain: **hull → suspension spring → wheel → belt → ground**,
  belt the only thing touching terrain, wheels as loads that shape/bridge it. Three realizations
  cheap→honest: **(1)** belt-only spring bed, wheels rigid/cosmetic; **(2)** wheel-springs-in-series
  on the belt (recovers independent wheel travel + soft ride, the real Tier-B target); **(3)** full
  positional belt chain (XPBD nodes; Sprocket-tier; deferred). Agreed sequencing: do (1) to prove the
  single system is stable, then layer (2) back on **in series** (stable by construction) rather than
  parallel (what chatters today).
- 2026-07-01 — **Step 10: Option 1 built** (green: fmt/clippy clean). Rewrote the sandbox to a single
  ground-contact system. Ripped out `apply_suspension` (+ `Suspension`, `WheelCalc`, `BeltReaction`,
  `taut_perimeter`, all `SUSP_*`/`FOOTPRINT_SAMPLES`/`BUMP_STOP`/droop consts) and the belt
  `CONTACT_DEADBAND`. Wheels are now **rigid to the hull** (hull + running gear = one rigid body);
  `apply_belt_support` is the sole FixedUpdate physics system — it clears contacts, carries the hull,
  tractions, and integrates belt speed, all along the belt loop's inward normal (still does walls/
  ditches/grinding-climb, colliders still the hard backstop). Coefficients **unchanged** on purpose
  (`SUPPORT_STIFFNESS_PER_M` 640k already gives ~2 cm sink as the sole carrier over ~9 m grounded
  belt) so this build isolates the *architecture* change from feel-tuning; softness is the next dial.
  - GAP vs Option 2 (to revisit before moving on): wheels don't articulate independently — the whole
    rig heaves/pitches on the belt bed over bumps instead of each wheel travelling. That per-wheel
    compliance + soft ride is exactly what the series wheel-springs (Option 2) add back.
  - AWAITING USER TEST: (1) flat rest calm at ~1.13 (2 cm sink), **no jitter standing still** (the
    seam is gone, so this is the key check); (2) drives/steers/wheelspins as before; (3) still bridges
    both trenches + climbs the step/wall/ramp (belt unchanged there); (4) over the washboard the rig
    rides as a rigid body (expected — no independent wheel bob; that's the Option-2 gap).
  - NEXT: play with softness (`SUPPORT_STIFFNESS_PER_M`/`_DAMPING_PER_M`) for feel; then decide Option
    1 → 2.
  - USER TEST (step 10): **bouncy at rest** (contacts pop frame-to-frame) + the belt is a **rigid plate
    welded to the hull** — it doesn't conform to terrain (straight line across the washboard, ss4).
    User: "I thought the track wraps around the terrain, *from the load*, not from wheel-wrapping."
    Correct instinct — 10 under-built it (belt shape from wheels only, ignoring ground).
- 2026-07-01 — **Model-practicality discussion (recorded, drives the plan):** "Wrap the terrain from the
  load" = TWO mechanisms: **(a) conform** — belt geometry follows the ground (drape over bumps, span
  dips); **(b) distribute** — belt tension spreads each contact's load along its length instead of
  spiking one point. Penalty-only gives (a) but NOT (b): in a spring∝penetration model, "belt rides up
  onto a bump" and "belt penetrates a bump" produce the *same* force, so **draping fixes the look, not
  the bounce** — the bounce is a stiffness/damping/contact-discretization problem, separate from
  conform. On generalization: Option-1 coefficients are per-metre, and the feel knobs are
  mass-independent (ride freq ≈ √(g/target_sink) — mass cancels), so it generalizes from {mass,
  geometry, target sink, damping ratio} with **no per-vehicle hacks for basic feel** — BUT it has a
  fidelity ceiling (no per-wheel character; stiff-contact edge cases are the hack magnets). **Option 2
  (real per-wheel springs) is the actual production model AND the more generalizable one** — its params
  ARE the vehicle spec (mass, spring rate, travel), the standard raycast-vehicle pattern that scales.
  So: Option 1 = the calm single-system foundation to lock the belt (contact + traction + drape);
  Option 2 = real springs added **in series** on top (wheel pushes belt, belt pushes ground — NOT the
  old parallel bed that chattered). Agreed: build **sequentially**, Option 1 → Option 2. Nothing thrown
  away (belt core carries over).
- 2026-07-01 — **Step 10b: kill the bounce** (green: fmt/clippy clean). First faithful-Option-1
  sub-step, isolating the stability fix from the conform look. Softened the sole-carrier belt
  (`SUPPORT_STIFFNESS_PER_M` 640k→250k → ~5 cm sink, ride freq ≈ √(g/sink) ~2.3 Hz), retuned damping
  (`SUPPORT_DAMPING_PER_M` 56k→30k, ~0.85 critical), and added **soft engagement** (`CONTACT_ENGAGE`
  0.02 m): each station ramps its contact force in over the first 2 cm of penetration instead of
  snapping full force on the instant it crosses the belt surface. Rationale: the old bed was *already*
  near-critically damped and still bounced → the culprit is the very stiff contacts flickering on/off
  at the belt ends as the rigid rig micro-oscillates, not under-damping. Wheels still rigid, belt still
  un-draped (conform is 10c).
  - AWAITING USER TEST: (1) **flat ground calm at rest** — no bounce, contacts steady frame-to-frame
    (the whole point); rest ~y=1.10 (~5 cm sink); (2) still drives/steers/wheelspins; (3) still bridges
    both trenches + climbs step/wall/ramp. If any residual buzz, next lever is a wider CONTACT_ENGAGE
    or a smoothstep. (The rigid-plate *look* over the washboard is expected — 10c adds the drape.)
  - NEXT (10c): conforming drape — draped spline + cosmetic wheels riding the belt (physics belt
    decoupled onto hull-fixed `rest_circles` so the drape doesn't null the support). Then Option 2.
  - VERIFIED (user): "calm now, good to proceed" — confirms the bounce was contact stiffness/flicker,
    not the missing conform (draping wouldn't have fixed it).
- 2026-07-01 — **Step 10c: conforming drape** (green: fmt/clippy clean). The belt now visibly wraps the
  ground. Two coupled changes: (1) **decoupled the physics belt** — `apply_belt_support` now builds its
  loop from the hull-fixed `rest_circles` (rigid taut line) instead of the live wheel transforms, and
  dropped the `wheels` query + `to_local`. This is load-bearing: terrain rising above the rigid line is
  what generates support, so the belt must NOT follow wheels draped onto the ground (that would flatten
  it and null the carry). Dips below the rigid line are bridged straight, as before. (2) **cosmetic
  wheel placement** — new `articulate_wheels` (Update, no forces): each road wheel rides up onto the
  highest terrain its radius can touch (discretised cylinder cast, `FOOTPRINT_SAMPLES` 7), clamped to
  never drop below the taut rest line (so dips/gaps bridge) and eased (`SUSP_TRAVEL_RATE`). Re-added a
  slim `Suspension {pivot_local, dy}` (visual state only) + consts `SUSP_RAY_LENGTH`/`SUSP_MAX_LIFT`.
  The drawn belt spline wraps these wheels (`draw_rig_gizmos` reads live transforms), so it drapes over
  bumps and spans dips. Physics unchanged from 10b (still the calm rigid-line carry).
  - Known cosmetic seam: the physics contact dots sit on the rigid line (~5 cm sink) while the draped
    spline/wheels sit on the surface, so on flat the dots read a few cm below the spline. Expected.
  - AWAITING USER TEST: (1) the **track visibly wraps the terrain** — wheels ride up over the washboard
    ridges and the cyan spline drapes over them (no more flat plate); wheels no longer buried on flat;
    (2) over trenches/gaps the wheels + spline **bridge** (stay on the taut line, don't drop in); (3)
    still calm at rest, still drives/bridges/climbs (physics untouched since 10b).
  - NEXT: if the drape reads right, Option 1 is done → start Option 2 (real per-wheel springs in series
    on top of this belt).
  - VERIFIED (user): "much better than I thought." Option 1 accepted. **Committed + tagged as the
    checkpoint:** `9c42921`, tag `checkpoint/track-belt-option1` — the stable single-system
    middle-ground we may reuse for the game.
- 2026-07-01 — **Between-wheel-bump discussion (checkpoint decision).** User spotted the one oddity: a
  washboard bump that fits *between* two wheels — the cyan spline dips and the bump poke through it,
  whereas a real track in tension lays *over* the bump. Resolution: this is a **visual lag, not a
  physics gap** — the physics belt samples the rigid taut line every `CONTACT_SPACING` and already
  generates support on that bump (dots sit on it); the tank is already held up by it. The drawn spline
  only follows the *wheels* (and the bump falls between wheel footprints), so it doesn't ride over
  terrain *between* wheels. **Fully capturable in Option 1**, cheap + pure-visual: conform the drawn
  spline's lower run to terrain (`belt_y = max(wheel_line, terrain)`) — makes it lie on between-wheel
  bumps + span dips, and aligns the spline with where physics already puts support (kills the
  dot/spline offset). The "tension transfers the load to neighbouring wheels" nuance is
  indistinguishable from "load to hull" in Option 1 (rigid body); it only becomes real in Option 2.
  - User's model idea ("treat the spline as a hard surface that can't clip under the world + conform to
    pressure") splits into the two known mechanisms: **(a) can't-clip = geometric conform** = the cheap
    Option-1 visual above; **(b) conform to pressure = load distribution** = Option 2/3. The forcing
    constraint: **in a penalty model one line can't both conform and bear load** (conform → 0
    penetration → 0 force exactly where you want it), which is why physics-line and visual-line are
    split. Making the *same* conforming hard surface bear load "by pressure" needs soft springs riding
    it (Option 2) or a per-node belt solve (Option 3). User's reasoning is converging on Option 2.
  - OPEN FORK (awaiting user): (i) do the cheap Option-1 **spline-conform** visual fix first (so the
    belt truly can't clip the world) as a stronger checkpoint, THEN Option 2; or (ii) go straight to
    Option 2 (real per-wheel springs in series on the belt).
  - User chose (i): quick refinement, test, then decide.
- 2026-07-01 — **Step 10d: spline terrain-conform** (green: fmt/clippy clean). Pure-visual, physics
  untouched. `draw_rig_gizmos` now resamples the belt envelope fine (`BELT_DRAW_SPACING` 0.1) and
  conforms each drawn point to the ground under it (`w.y = max(w.y, terrain)` via a down-ray from
  `BELT_CONFORM_RISE` 2 m above, reaching `+BELT_CONFORM_REACH` 3 m below). So the cyan line now lies
  on bumps *between* wheels (a taut track would) and stays taut over dips/gaps — it can't clip under
  the world. Added `SpatialQuery` to the gizmo system. This also aligns the spline with where the
  physics already puts support (the dot/spline offset shrinks). No const/behaviour change to the belt
  physics.
  - AWAITING USER TEST: (1) the between-wheel washboard bump the spline used to clip now has the belt
    laying *over* it; (2) over the trenches/gaps the spline still bridges taut (not conformed down into
    the ditch); (3) no change to feel/drive/climb (physics identical to the tagged checkpoint).
  - NEXT: if it reads right, this is the strengthened Option-1 checkpoint → begin Option 2 (real
    per-wheel springs in series on the belt).
  - VERIFIED (user): "very promising." User asked how wheels get their height (noticed roundness but
    low resolution — the 7-column quantisation of the ray fan), then named the deeper point: **Option 1
    is more honest if the wheels follow the *track*, not their own raycast.** Agreed: that makes the
    two models architecturally pure duals — Option 1 belt-primary (ground → belt → wheels), Option 2
    wheel-primary (ground → wheels → belt) — which is what makes the planned build-both-and-compare
    (ease / maintenance / tank-count scaling / feel) a fair comparison. Also settled: the true
    shape-cast probe belongs to Option 2 (where the probe is force-bearing), not Option 1.
- 2026-07-01 — **Step 10e: belt-primary all the way down** (green: fmt/clippy clean). One ground-read,
  one data direction. New `ConformedBelts` resource + `conform_belts` system (Update, chained before
  `articulate_wheels` → `draw_rig_gizmos`): builds each side's conformed belt once per frame (rigid
  reference loop from `rest_circles`, resampled at `BELT_DRAW_SPACING`, raised onto terrain). The drawn
  spline IS that belt (draw takes no SpatialQuery anymore), and `articulate_wheels` now rides the
  wheels on it with **zero raycasts of its own**: rigid roller resting on the belt polyline, solved in
  **closed form per segment** (centre = y(dz) + √(R²−dz²), peak at dz* = mR/√(1+m²), plus clipped
  ends) — so the wheel path is smooth, killing the 7-column quantisation the user spotted. Lift-only +
  eased as before. Deleted: the per-wheel ray fan (`FOOTPRINT_SAMPLES`, `SUSP_RAY_LENGTH`),
  `side_circles` (last consumer gone), the `RigWheel.radius` field, the `Affine3A` import. Physics
  untouched (rigid-line penalty, FixedUpdate).
  - AWAITING USER TEST: (1) wheel paths over the washboard/step now *smooth* (no stepping); (2) wheels
    visibly sit ON the cyan belt everywhere (they ride it by construction now); (3) bridging still taut
    over both trenches; (4) feel/drive/climb unchanged (physics identical). If good → this is the
    completed Option-1 model (worth re-tagging the checkpoint) → begin Option 2.
  - VERIFIED (user): "promising", two issues → 10f.
- 2026-07-02 — **Step 10f: conform bound + course expansion** (green: fmt/clippy clean).
  1. **Belt-snaps-onto-wall-top fixed** (user ss: nosing a wall, the cyan belt teleported to the ground
     above): the conform's down-ray from 2 m above found the *top* surface of the wall the belt points
     were pressed into/under, and `max(line, terrain)` hoisted the line onto it. Semantics fix: conform
     is for terrain the belt can plausibly *drape over* — added `BELT_CONFORM_MAX_RAISE` (0.35, ~wheel
     radius): terrain more than that above the taut line is a **wall face** (stations stay on the taut
     line, pressed against it — walls are the physics'/colliders' job). `BELT_CONFORM_RISE` 2.0→0.5
     (just above the bound; also stops a buried-in-wall origin reading as terrain, since solid-hit at
     distance 0 = `+RISE` > bound → rejected). Wheels inherit the bound (they ride the belt).
  2. **Course rebuilt** (all data-driven): `WASHBOARDS` — three sets of increasing coarseness (period
     0.8/1.5/2.5, thickness = period/3, heights 0.12/0.18/0.22) so fine gaps are bridged and coarse
     gaps are resolved — the resolve-vs-bridge spectrum in one drive; trenches moved past them
     (narrow 2.2 @ z30, wide 5.0 @ z42) + **new pit** (10 m @ z58, wider than the whole track: drop-in/
     grind-out case, 4th `R` spot); step → z72, ramp → z88, `LANE_FAR` → −110; lane widened 14→40 m
     with obstacles on a 16 m sub-lane (`OBSTACLE_W`) so there's open ground to manoeuvre/steer around.
  - AWAITING USER TEST: (1) nose into a trench wall / the step — belt stays pressed on the wall, no
    snap to the ground above; (2) washboard tour: fine set bridged, coarse sets resolved (wheels drop
    in + ride over, smooth paths); (3) pit (`R`×4): drops in, grinds out; (4) room to turn/manoeuvre.
  - NEXT: user verdict → re-tag the completed Option-1 checkpoint → begin Option 2.
  - USER TEST: wall-snap only reduced, not gone — sawtooth spikes on the idler arc at a ledge (ss).
    Root cause named in review: the vertical-ray-plus-raise-bound *formulation* is wrong, not its
    tuning — the conform asks a world-vertical question of a loop whose contact direction varies
    around it, then `MAX_RAISE` rejects the wrong answers and the accept/reject boundary between
    adjacent samples IS the sawtooth. User: "rethink, tuning the ray is a hack" → agreed.
- 2026-07-02 — **Step 10g: outward-normal conform** (green: fmt/clippy clean). Rewrote `conform_belts`
  to ask the physics' question: each station probes along the belt's **own outward normal** (tangent
  from loop neighbours, rotated −90°, exactly as `apply_belt_support` does — closed loop + modular
  indices), from just inside the surface (`CONTACT_PROBE`) outward. A hit short of the surface = the
  terrain penetrates the belt → **the hit point IS the conformed station** (pressed out along its own
  contact direction): up onto bumps under the belly, *back off a wall face* at the nose — the normal
  ray structurally cannot see a wall *top*, so the snap-onto-ledge failure mode is impossible rather
  than thresholded away. Zero-distance hit (buried origin, extreme clip) → leave taut, physics resolves
  it. Deleted all three conform consts (`BELT_CONFORM_RISE`/`_REACH`/`_MAX_RAISE`) — no tuning knobs
  left; the probe depth is shared with the physics. Per user's call, NO arc-pinning special case —
  betting the normal-probe alone kills the spikes (normals rotate smoothly around the arcs → smoothly
  varying hits, no accept/reject boundary). Held in reserve if artifacts persist: (a) pin stations on
  wheel circles (skip conform where the belt wraps a wheel), (b) the full mini-rope relaxation
  (tension + non-penetration), the geometric end-state.
  - Caught in self-review before launch: side-plane→world component mapping of the normal was mangled
    ((0, tan.y, −tan.x) instead of (0, −tan.x, tan.y)) — would have pressed the belly stations
    *forward*, not up. Fixed; belly now reduces exactly to the old vertical behaviour on flat ground.
  - AWAITING USER TEST: (1) the idler-arc sawtooth at ledges gone — belt stays wrapped on the wheel /
    pressed on the wall face, smooth; (2) nose into a wall: belt pressed back on the face, never on
    top; (3) belly conform unchanged (bumps between wheels still lain over, washboard drape as
    before); (4) bridging still taut.
  - NEXT: user verdict → re-tag Option-1 checkpoint → Option 2.
  - VERIFIED (user): "the clipping cases are solved, much better." Then user diagnosed the next issue
    from inside the trench: pressed against a vertical wall the tank starts to climb, then **locks** —
    hypothesis: the inset sprocket/idler collider cylinders drive into the wall and block upward slip.
- 2026-07-02 — **Wall-climb lock: root cause + fix (step 10h)** (green). User's hypothesis confirmed
  and sharpened — it's worse than a kinematic block, it's a *design inconsistency*: Avian colliders
  default to **μ = 0.5** (verified in vendored `physics_material.rs`), so our "pure hard backstop"
  colliders were silently doing surface physics. Mechanism: (1) belt penetration budget at the
  sprocket = (1−0.6)·R = 0.18 m → once the collider touches, the belt's grinding-climb force is
  *frozen* (extra press flows through the rigid contact, buying zero climb); (2) that growing press
  makes the collider contact drag **down** at 0.5·N exactly while the belt grinds up — the harder the
  tracks push, the harder it drags. Fix: `Friction::ZERO.with_combine_rule(CoefficientCombine::Min)`
  on the hull box + sprocket/idler cylinders (Min=3 outranks terrain's default Average=1 → combined
  contact frictionless regardless of terrain material; verified in source). Backstops now stop
  penetration only; the belt owns ALL tangential physics.
  - Design note (user, recorded): whether a given tank CAN scale a ledge is **deliberately emergent**
    — power/geometry decide, per vehicle ("some tanks are more manoeuvrable and powerful than
    others"). Don't tune the model to guarantee climbs; let it fight the wall naturally. Expectation
    here: 1.2 m trench wall vs sprocket-belt top ~1.35 m → marginal, honest either way.
  - AWAITING USER TEST: in-trench wall fight — the climb should no longer lock at a fixed height;
    either it grinds over the lip or stalls honestly on force budget. Also sanity: no new sliding
    weirdness where colliders touch terrain hard (bottoming, wall nosing) — belt still provides all
    grip.
  - VERIFIED (user): "brilliant. works really good."
- 2026-07-02 — **MODEL 1 SAVED**: commit `6c87691`, tag **`checkpoint/track-model-1`** — the complete,
  verified belt-primary model (10d–10h: spline conform → belt-primary chain → outward-normal conform →
  course expansion → frictionless backstops). This is the baseline for the model comparison.
- 2026-07-02 — **Step 11: multi-model scaffolding** (green: fmt/clippy clean; uncommitted). Per user:
  the sandbox now accepts multiple locomotion models. `Model` enum + `MODELS` registry + `ActiveModel`
  resource + `model_is` run condition + `switch_model` (`M` cycles in place, zeroes `BeltSpeed` so the
  incoming model starts from rest — the live A/B on identical terrain). Shared across models: course,
  rig, camera, input, pause/reset, belt-loop geometry, `conform_belts`, gizmos, contact viz. Gated
  per-model: the force systems + wheel articulation (`apply_belt_support` + `articulate_wheels` gate
  on `Model::BeltPrimary`). `L` log prints the active model. Adding a model = variant + registry entry
  + gated systems. MODEL 2 (next): iteration on model 1 — design discussion first, then build.
  - AWAITING USER TEST: pure scaffolding, zero behaviour change — model 1 drives exactly as tagged;
    `M` logs the (single-entry) cycle; `L` shows "model 1 — belt-primary".
- 2026-07-02 — **MODEL 2 defined: link-belt** (user's direction, accepted design). Keep exploring the
  belt-primary model (the wheel-springs Option-2 is a *different* model, maybe model 3 someday). Core
  idea: treat the belt stations as **virtual track links**. Two mechanisms, from the user's two
  observations: (1) segments clip terrain corners between stations → make the **segment (link plate)
  the contact primitive** (segment-cast along the outward normal; support at the true contact point —
  the plate *rests on* a corner both endpoints miss; same fix as 10e's roller-on-polyline, one level
  down); (2) stations fixed in hull space scrub along the ground → **advect stations with belt speed**
  (real link kinematics: no-slip = links stationary on ground under the passing hull; wheelspin =
  links visibly sliding — the slip model made literal; belt stopped = nothing moves). Free prize: the
  advected links ARE the procedural animated track (instance link meshes at the advected positions —
  physics and animation unify on one `LINK_PITCH` primitive). Build order: (1) advected stations,
  (2) segment-cast contact, (3) draw links as links.
- 2026-07-02 — **Step 12: model 2 step 1 — advected stations** (green: fmt/clippy clean). Registered
  `Model::LinkBelt` (M now cycles 1↔2). New `BeltPhase` resource (per-side arc-phase, wrapped mod
  `CONTACT_SPACING` — uniform ring, one-pitch shift = identity); `resample` gained an `offset` param
  (stations at `offset + i·spacing`; 0 = exactly the old behaviour). New `apply_belt_support_links`
  (FixedUpdate, gated on LinkBelt): a deliberate **fork** of model 1's system — model 1's tagged code
  stays untouched, and step 2 (segment casts) will diverge the body anyway — identical physics except
  stations sample at the advected phase, and `phase += belt_speed·dt` after the governor (loop
  traversal direction = belt surface direction when driving forward, so the ring circulates the right
  way). `articulate_wheels` now shared by both models (gate removed). Phase zeroed on `M`-switch and
  `R`-reset. Conform/draw stations NOT advected (identical polyline either way; links-as-links is
  step 3).
  - AWAITING USER TEST (in model 2, `M` once): (1) contact dots **travel with the belt** when driving
    — and on a no-slip roll they should sit still in *world* space (lay down and hand off) instead of
    scrubbing along with the hull; (2) floor it from rest: wheelspin = red dots visibly *sliding*;
    (3) otherwise identical feel to model 1 (same forces, same coefficients) — A/B with `M` mid-drive;
    (4) `L` shows the active model.
  - NEXT: model 2 step 2 — segment(plate)-cast contact · step 3 — draw the links as links.
  - VERIFIED (user): "very promising, models feel about the same" (correct — step 1 is kinematics of
    the sampling points, not forces) "but I can already imagine the physics implications and
    procedural track animation."
- 2026-07-02 — **Step 12b: module split + model HUD + default** (green: fmt/clippy clean). Per user:
  (1) `src/track_sandbox.rs` → `src/track_sandbox/` — `mod.rs` (shared course/rig/belt machinery,
  registry, resources, viz), `model1.rs` (frozen belt-primary `apply_belt_support`), `model2.rs`
  (`BeltPhase` + `apply_belt_support_links`; the link-belt iteration front). Each model file
  `use super::*;`, systems `pub(super)`. (2) On-screen model label (top-left `Text` node,
  `ModelLabel`, updated on `ActiveModel` change; Bevy 0.19 note: `TextFont.font_size` is
  `FontSize::Px(..)`, not f32). (3) Default model = LinkBelt (model 2 — the iteration front; model 1
  = frozen baseline).
  - Also answered (user question, recorded): **the model is essentially 2D per track** — belt loop /
    stations / conform all live in the (z, y) side plane at fixed track x (two independent 2D
    problems embedded in 3D by the hull transform; probes are 3D world rays from in-plane points,
    forces applied at 3D points so full 3D rigid-body response emerges — 2.5D overall). Station =
    in-plane point standing for a `CONTACT_SPACING` arc of belt (per-metre coefficients); step 2's
    link plate = in-plane **segment** (line along travel). What stays unmodelled: the track's lateral
    WIDTH (each track is a knife-edge — a curb under half a track, lateral edge tilt, cross-slope
    under one belt can't resolve; roll emerges only from left/right height difference). Acceptable at
    Tier-B (tracks narrow vs long); the escalation if ever needed = several sample columns across the
    width (line → ribbon), NOT a 3D belt sim.
  - AWAITING USER TEST: label top-left shows model 2 on launch and flips with `M`; behaviour unchanged
    otherwise (pure refactor + default).
  - VERIFIED (user): "looks good" → segments.
- 2026-07-02 — **Step 13: model 2 step 2 — link-plate (segment) contact** (green: fmt/clippy clean).
  `model2.rs` only; model 1 untouched. Each link = the segment between consecutive advected stations
  (modular; degenerate seam skipped). Per link: `cast_shape` with a `Collider::segment` plate
  (expressed about the link center, identity rotation — endpoints already world-oriented), cast from
  `CONTACT_PROBE` inside the belt surface along the link's outward normal (Avian 0.7 API verified in
  vendored source: `ShapeCastConfig{max_distance,..}`, `ShapeHitData{distance, point1=on-terrain,
  point2=on-cast-shape}`). `pen = PROBE − distance` = the *deepest* terrain feature under the plate;
  support (soft-engaged) + slip-traction applied **at `hit.point1`** — the true contact point — so a
  corner poking up *between* stations is found and loaded *there* (point rays are structurally blind
  to it: the clip the user spotted). Coefficients per-metre × the **actual link length** (seam link
  is shorter → proportionally less). Contact viz dot at the true contact point
  (`to_local.transform_point3`). Belt-speed dynamics + phase advection unchanged.
  - AWAITING USER TEST (model 2, default): (1) drive the washboards — contact dots may sit on bump
    *corners* between joints now (plate resting on the edge), no force-clipping window as a corner
    passes between stations; (2) ledge/trench lips: dots ON the lip corner where the plate presses;
    (3) flat rest calm + same height (plate on flat == point on flat by construction); (4) drive/
    climb/pit unchanged in feel; A/B vs model 1 with `M`.
  - NEXT: model 2 step 3 — draw the links as links (plates between advected joints, the proto-track);
    note the *drawn spline/conform* still point-samples (visual clip between joints can still read on
    sharp corners — the plate physics no longer does); folding conform onto the link chain is part of
    step 3.
  - USER TEST: "a lot of jitter in the gizmos." Root cause (mine, step 13): the whole link load + dot
    were placed at the shapecast's contact point, which is **degenerate on coplanar contact** — a
    plate flat on flat ground touches everywhere at once, parry picks arbitrarily among tied points,
    and the pick flips between the plate's ends tick-to-tick → teleporting dots AND flickering torque
    (real hull micro-jitter). The cast is good at corners, ambiguous on faces.
- 2026-07-02 — **Step 13b: pressure-centroid plate contact** (green: fmt/clippy clean). The honest
  plate model: a rigid plate on penalty ground has a **pressure distribution**; the resultant acts at
  its **centroid** — a smooth function of pose, no tie-breaking. Per link, reconstruct the profile
  piecewise-linearly from three probes — endpoint penetrations (2 rays, may be ≤0 = end clear) and
  the plate cast's deepest point (pen_max at x_c, endpoint pens clamped ≤ pen_max) — and integrate
  `max(0, pen(x))` in **closed form** (`clipped_linear_piece`: trapezoid + zero-crossing clipping →
  area ∫pen, first moment ∫x·pen, contacting length). Elastic force = `STIFFNESS_PER_M · area`;
  damping over the contacting length; engagement ramps on pen_max; force + traction + dot at the
  centroid `wa + axis·(M/A)` on the belt line (matching model 1's convention). Flat: centroid =
  centre (stable); corner between stations: profile peaks there → centroid pulls to the corner
  (correct statics). Cost: 1 cast + 2 rays per link (~3n/side per tick — fine).
  - AWAITING USER TEST (model 2): (1) jitter gone — dots steady on flat, rest calm; (2) corner cases
    still caught (dots move toward bump corners/lips as plates ride them); (3) drive/climb unchanged.
  - VERIFIED (user): "looks good." Follow-up question: the cyan (= the resolved spline the track-link
    rendering will follow — confirmed) still visually phases through ground between conform points;
    treat each segment as rigid, like the points but as links? → Yes; nuance: links share joints, so
    they can't rest independently — rigid-chain answer is the corner link lifts tangent onto the
    corner and neighbours tilt (the "tent").
- 2026-07-02 — **Step 13c: rigid-link conform (model 2)** (green: fmt/clippy clean). New
  `conform_belts_links` in `model2.rs`, gated on LinkBelt (model 1 keeps its frozen per-point
  `conform_belts`; scheduler runs whichever matches, both write `ConformedBelts`). Two passes per
  side on the **same advected ring the physics samples** (same `CONTACT_SPACING` pitch, same
  `BeltPhase` — so the drawn cyan segments ARE the physical links and visibly travel with the belt;
  most of step 3 arrived naturally): (1) per link, a plate cast along its outward normal → `lift` =
  deepest terrain penetration past the link line (buried-origin distance-0 casts skipped — taut, let
  physics push out); (2) per joint, displacement inward = **max of its two adjacent links' lifts**
  along the averaged link normal. A link over a corner sits tangent on it, neighbours tilt, nothing
  clips. Wheels ride the conformed chain unchanged (`articulate_wheels` reads `ConformedBelts`).
  - AWAITING USER TEST (model 2): (1) cyan segments no longer phase through bump corners/lip edges —
    the chain tents over them; (2) the cyan segments **travel with the belt** when driving (proto
    track-link animation); (3) wheels still ride the chain sensibly; (4) A/B with `M`: model 1 keeps
    the old smooth point-conform spline.
  - NEXT: step 3 finish — draw links as discrete plates (visual gaps at joints / alternating colour),
    then decide what remains for model-2 completeness vs move to comparison/tag.
  - VERIFIED (user): "looks very good." Next question (ss: ledge climb): joints displaced by
    different lifts **stretch** their links (a drooping link reads longer than the pitch — wrong for
    rigid steel plates). Fixed link length + capped joint angle in this kinematic model? → Yes:
    that's the missing constraint that makes the chain a chain; = the reserved mini chain-relaxation,
    now arriving as *rendering integrity* (physics untouched). Sequencing agreed: length conservation
    first, angle cap as a separate later knob.
- 2026-07-02 — **Step 13d: chain projection solve — fixed link lengths** (green: fmt/clippy clean).
  `conform_belts_links` rewritten as a small PBD projection in the 2D side plane, fresh each frame
  from the reference ring (no temporal state → no drift): gather per-link terrain contact planes
  (plate casts at reference config; buried-origin casts skipped), then `CHAIN_ITERATIONS` (8)
  Gauss–Seidel passes of three projections — (a) **rigid link lengths** (each segment back to its
  reference length, error split between joints; tenting links pull length from neighbours →
  ultimately the slack top run, the honest bookkeeping under fixed `BeltLength`); (b) **terrain
  half-spaces** ((p−q)·m ≥ 0 per contacting link's joints); (c) **wheel circles** (joints can't
  enter the running gear — the wrap now *emerges* from tension around the circles rather than being
  drawn). `BeltSample.local` = the *solved* local position (consistent with world for the
  wheel-riding math). Ledge case: the stretched pseudo-link becomes several fixed-length links
  hinging around the lip.
  - AWAITING USER TEST (model 2): (1) the ledge/droop case — links stay link-sized, chain hinges
    around corners (no stretching); (2) segments still travel with the belt; (3) wraps still hug the
    sprocket/idler (now constraint-emergent); (4) top-run sag breathes as the belly tents (length
    bookkeeping visible); (5) no solver artifacts (spikes/folding) anywhere on the course.
  - NEXT: angle cap knob (MAX_LINK_ANGLE projection in the same loop) if wanted after test · draw
    links as discrete plates · then model-2 wrap-up (tag + compare vs model 1).
  - USER TEST (ss×4): right direction, but artifacting — links snap between sharp angles; spikes
    around idler/sprocket; clumping between wheels reads odd; and "are we actually fixed length?
    the real track has a fixed number of links."
- 2026-07-02 — **Step 13e: fixed link count + angle cap + convergence** (green: fmt/clippy clean).
  Three root causes matched to the three symptoms:
  1. **Not fixed-count (user was right):** per-frame resampling left a phase-dependent *remainder
     link* at the loop seam — which sits **at the sprocket** (belt_loop starts/ends there) — whose
     length snapped as the phase wrapped, and the station count flipped ±1 every pitch of travel.
     Fix: `LinkCount` resource (startup: belt length / target pitch, rounded); both model-2 systems
     build the ring as exactly N links at pitch = loop_len/N (`resample` + truncate), phase wraps
     mod that exact pitch. No remainder link, stable count — a real track's ring.
  2. **Zigzag buckling = the missing angle cap:** length constraints alone let a *compressed* span
     (nosed into a wall, clumped between wheels) fold arbitrarily sharply. New projection (b):
     joint articulation capped at `MAX_LINK_ANGLE` 30° (must clear the wheel-wrap demand of
     ~pitch/radius ≈ 25°/joint on road wheels; ≈ a real pin's limit) — ease the joint toward its
     neighbours' midpoint proportional to the excess fold.
  3. **Snapping between configurations = under-convergence:** `CHAIN_ITERATIONS` 8 → 20 (cost
     trivial; solver order now lengths → angle → terrain → circles).
  - AWAITING USER TEST (model 2): (1) sprocket/idler spikes gone (the seam link no longer exists);
    (2) compressed spans bow smoothly instead of zigzagging (wall-nose, wheel clumps); (3) less
    config-snapping generally; (4) wraps still hug the wheels (cap sits above the wrap demand);
    (5) links stay link-sized (now exactly L/N each).
  - NEXT: draw links as discrete plates · model-2 wrap-up (tag + compare vs model 1).
  - USER TEST (ss×3, at the ledge): "generally better, still edge cases — perhaps the missing piece
    is chain inertia?" Analysis: half right — the flip-snapping wants *temporal continuity*
    (bistable tent configs re-derived fresh each frame), whose cheap stable form is a **warm start**
    (hysteresis), NOT true chain dynamics (mass/momentum/wobble — deliberately out at Tier-B). The
    ss also exposed a real static bug: the solver's wheel circles were the **rest-pose** ring while
    the drawn wheels articulate — the chain wrapped phantom circles and notched through the lifted
    wheels at the ledge.
- 2026-07-02 — **Step 13f: warm start + articulated wheel circles** (green: fmt/clippy clean).
  (1) `BeltPhase` now stores **total unwrapped travel** (advance takes no pitch; call sites wrap
  `rem_euclid(pitch)` for the offset; quotient = link-identity shift). (2) New `ChainMemory`
  resource: last frame's solved displacements per link (hull-local), index-rotated by the identity
  shift (`rotate_right((shift − mem.shift) mod n)`) so each displacement seeds the same *physical*
  link; solver starts from reference + seeded displacement → stays in the basin it settled in.
  (3) `conform_belts_links` circle constraints now built from the wheels' **current articulated
  `Transform`s** (radius by `WheelKind`; one frame stale — wheels ride last frame's chain) instead
  of `rest_circles`.
  - USER TEST (ss×4): "tuning way off — engine spins the track up like a string and it's flung
    outward; chain very stiff — stayed floating in shape after stopping; cyan keeps deforming while
    paused — more than one clock at play." All three diagnosed as bugs in/around the 13f warm start,
    not tuning:
    1. **No tension → every feasible config a fixed point**: the projection constraints (lengths,
       non-penetration, circles) admit infinitely many shapes; pre-warm-start the fresh-from-
       reference init WAS the implicit tension. Full-strength warm start made deformed shapes
       persist forever (the floating chain) and let mangled seeds compound at speed (~73 links/sec →
       the "flung outward" balloon — no centrifugal force exists in this code; it was memory
       feedback).
    2. **Stale memory on teleport**: R/M didn't clear `ChainMemory` → garbage seeds.
    3. **Two clocks confirmed**: Esc pauses Avian only; `articulate_wheels` kept *easing* on the
       virtual Update clock while paused, moving the solver's wheel circles → chain re-solved →
       creep while "paused".
- 2026-07-02 — **Step 13g: chain tension (seed decay) + clock gating + memory resets** (green).
  (1) `CHAIN_MEMORY` 0.8: seed = reference + α·(rotated last displacement) — the missing tension:
  deformations decay in ~1/(1−α) frames unless terrain holds them; enough memory survives for tent
  hysteresis. (2) `conform_belts` / `conform_belts_links` / `articulate_wheels` gate on
  `sim_running` (paused = everything frozen; draw systems stay ungated — immediate-mode gizmos
  redraw the frozen state). (3) `R`-reset + `M`-switch clear `ChainMemory`. Engine-power feel
  deliberately NOT retuned at this step.
  - VERIFIED (user, ss): "very, very good — the model is getting very sharp." Remaining
    observations: (a) belly squiggles — "we're not considering the top half of the chain for slack
    (total chain length)"; (b) engine still accelerates the tracks incredibly quickly; (c) user
    calls for a **T-34 benchmark** — "numbers well known, track model basically identical to ours".
- 2026-07-02 — **Step 14: T-34 benchmark + constant-power drivetrain + top-half slack** (green).
  1. **Vehicle = T-34/76 spec** (shared consts — both models drive the same vehicle): 26.5 t
     (`HULL_MASS`), 830 mm road wheels (`ROAD_RADIUS` 0.415), contact length 3.85 m
     (`WHEEL_SPACING` 0.96), sprocket ⌀0.64 (`DRIVE_RADIUS` 0.32 — smaller than the road wheels,
     as on the real thing), hull 6.1 m (`HULL_HALF`), **link pitch 172 mm** (`CONTACT_SPACING` —
     the real 72-link ring maps straight onto `LinkCount`), 53 km/h (`MAX_BELT_SPEED` 15),
     `TRACK_SLACK` 0.005 (≈9 cm mid-span droop ≈ well-tensioned real track). Support stiffness
     rescaled for the mass at the same ~5 cm sink target (680k/m, damping 80k/m — the
     mass-independent-sink rule doing its job).
  2. **Constant-power drivetrain** (the honest fix for "engine spins the track like a string" — the
     defect was the *shape*, not the number: full force at any speed): new shared
     `engine_available(v) = min(ENGINE_FORCE, ENGINE_POWER/|v|)` — V-2 diesel 373 kW → 186.5 kW per
     track, torque-capped at 120 kN (≈ the grip limit). Brutal at stall, tapering at speed. In BOTH
     models' governors (drivetrain is vehicle spec; the A/B holds the vehicle constant — noted as a
     deliberate mechanism change to tagged model 1). `BELT_INERTIA` 3k→8k (belt steel ~1.2 t +
     reflected drivetrain).
  3. **Top-half slack participation** (user's squiggle diagnosis): per frame, measure the belly's
     extra path demand from the plate-cast lifts (first-order: Σ(Δ joint-lift)²/2ℓ per link — a
     uniform raise costs ~nothing, a differential tent is what consumes length), smooth into
     `ChainSideMemory.belly_extra`, and **subtract it from the next frame's top-run sag budget**
     (`belt_loop(Some(L − belly_extra))`) — the top half lends its slack instead of the surplus
     parking as belly squiggles.
  4. `MAX_LINK_ANGLE` 30°→35° (the T-34's small sprocket needs ~31°/joint to wrap).
  - AWAITING USER TEST: (1) drive feel — spin-up now tapers with speed (no string-spin), stall
    torque still shoves 26.5 t around; (2) belly squiggles reduced, top sag visibly breathing as
    terrain loads the belly; (3) rest sink ~5 cm at the new mass; (4) sprocket wrap clean at the
    smaller radius; (5) proportions read T-34 (big wheels, small sprocket, long low hull).
  - NEXT: draw links as discrete plates · model-2 wrap-up (tag + compare vs model 1).
  - VERIFIED (user, ss×3): "this model is incredible — feel is very close to the T-34." User spotted
    **emergent slack migration** (confirmed genuine + directionally correct): driving into a wall
    forward, compression collects *under the front sprocket*; in reverse, on the *top run* — the
    taut-side/slack-side behaviour of a real front-drive track, un-coded: links advect around the
    loop while the wall's contact planes pin the nose region; surplus piles just downstream of the
    pinned zone (warm start lets the pile persist), tension upstream. Remaining ask: proper top-run
    **droop** — the T-34 runs famously loose, no return rollers, the return run lies ON the road
    wheels.
- 2026-07-02 — **Step 14b: T-34 loose track** (green). The droop couldn't be tuned in because the
  slack budget couldn't *reach*: the top run sits ~0.45 m above the wheel tops and 5 mm slack buys
  ~9 cm of sag. The resting-on-wheels mechanism already existed (the solver's wheel circles push the
  chain out from any direction, including from above) — pure budget fix: `TRACK_SLACK` 0.005 →
  **0.13** (sag ∝ √slack → the reference parabola now dips past the wheel tops; the circles catch
  the drape). Expected: return run rides the road wheels with short hanging spans between them — the
  authentic silhouette. (Model 1's point-conform spline has no wheel collision on the top run — it
  draws the deep parabola through the wheels; baseline-only cosmetic, noted.)
  - AWAITING USER TEST (model 2): (1) return run drapes onto and rides the road wheel tops, short
    hanging spans between wheels; (2) the drape breathes with the slack bookkeeping (belly demand
    pulls it tighter); (3) the wall-test slack migration still reads right at the bigger budget;
    (4) no solver misbehaviour from the deeper reference sag (arcs/wraps clean).

## Open questions / parking lot

- Envelope as taut convex-hull of wheel circles vs. sagging catenary on slack runs — start taut,
  add sag in step 2.
- How belt-speed/slip couples to the (future) powertrain — deferred to step 4.
- Promotion path into the game: new module vs. extending `driving`; and the ADR-0005 rewrite.
