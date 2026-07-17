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
- 2026-07-02 — **MODEL 2 SAVED**: commit `8a1fae4`, tag **`checkpoint/track-model-2`** — the
  link-belt model, T-34-benchmarked, verified ("this model is insane"; feel very close to the T-34;
  emergent slack migration confirmed). Both checkpoints now stand for the planned comparison:
  `checkpoint/track-model-1` (belt-primary) vs `checkpoint/track-model-2` (link-belt), switchable
  live with `M` at head.
- 2026-07-02 — **Step 15: Verlet chain dynamics + wheel rise/fall split** (green). User's tuning
  ask — swing speed / visible inertia; "drooping should fall by gravity, sprocket pull should be
  violent" — answered by ONE upgrade: the chain solve graduated from quasi-static projection with
  seed decay to **Verlet dynamics**: `ChainSideMemory` now holds `pos`/`prev` (implicit velocity,
  hull-local; state rotates with the link-identity shift as before); per frame integrate gravity
  (transformed into the hull frame — drapes hang right on slopes) + a **drive anchor** toward the
  advected reference (`CHAIN_DRIVE` 400 s⁻²: accel ∝ lag → violent exactly when the belt/reference
  moves violently, calm when coasting; also the tangential coupling that tracks advection between
  wraps), damped by `CHAIN_MEMORY`→`CHAIN_DAMPING` 0.88 (the swing knob), then the SAME constraint
  projections (lengths / angle cap / terrain planes / wheel circles). `CHAIN_MEMORY` decay deleted —
  gravity + anchor ARE the tension now, hysteresis comes free from real state, and a settled chain
  has zero velocity: it **sleeps** instead of re-solving into micro-jitter (prime suspect for the
  reported at-rest gizmo jitter — verify). dt clamped ≤ 1/30 so hitches can't explode the
  integration. Also split the cosmetic wheel ease: `SUSP_RISE_RATE` 4.0 (terrain forces wheels up,
  fast) / `SUSP_FALL_RATE` 1.5 (they drop under gravity, slower) replacing the symmetric 2.5.
  - AWAITING USER TEST (model 2): (1) top-run droop now *falls* and settles with a bit of swing —
    gravity-paced, not snapped; (2) hard throttle/reverse yanks the chain sharply (drive-anchor
    violence), coasting is calm; (3) at-rest gizmo jitter gone (chain sleeps); (4) wheels rise
    quickly over bumps but ease down into dips; (5) knobs if feel is off: CHAIN_DAMPING (swing
    life), CHAIN_DRIVE (yank), SUSP_RISE/FALL_RATE.
  - VERIFIED (user, ss): "sweet model." One rare state captured on camera: a **thrown track** —
    R-teleport over the narrow trench; the hull settles physically while the chain integrates
    gravity, the unsupported belly span droops into the gap, and the unilateral constraints
    (push-out-only circles/planes) capture the wrong-side-of-the-gear configuration as a stable
    equilibrium. Semi-physical (real loose tracks do this), rare (needs the teleport-settle race) —
    deliberately NOT handled; parked below. Static at-rest gizmo jitter still present (the Verlet
    sleep didn't cure it — diagnosing which element before fixing). Otherwise user declares the
    model complete.
- 2026-07-02 — **Step 15b: damped load gauge** (green). Jitter diagnosis narrowed with the user:
  hull + cyan chain visually still; the **green dots + yellow normal lines** flicker, frame-stepped
  pause shows dot *size* varying → the raw per-tick load readout, not the physics. Contact loads
  genuinely oscillate a few percent tick-to-tick at rest (shape-cast tie-breaking on coplanar faces,
  engage-ramp edges, fixed-tick sampling) — micrometres of hull motion, visually amplified by the
  load→dot-size mapping. Fix: `LinkLoads` — per-link EMA (`LOAD_SMOOTHING` 0.25, decay at loop top +
  accumulate on contact = links fade instead of blinking), riding the same link identities as the
  chain state (rotates with the ring). **Physics untouched** — only the displayed `Contact.load` is
  smoothed; model 1 keeps its raw readout (frozen baseline).
  - AWAITING USER TEST (model 2): dots + normal lines steady at rest; while driving, load migration
    still reads (smoothing is ~4 ticks, well under perception at speed). If flicker persists → it's
    contact-set blink at the patch edges, next diagnosis.
- 2026-07-02 — **MODEL 2 FINAL SAVED**: commit `ea54a48`, tag `checkpoint/track-model-2` moved to
  it (pre-Verlet snapshot `8a1fae4` remains in history). Model 2 closed.
- 2026-07-02 — **MODEL 3 DEFINED: box-belt (user-driven design discussion).** Track *width* + link
  *thickness* enter the model. Settled design, three refinements deep:
  1. Sample the **actual link**: longitudinally the pitch was already physical (172 mm); laterally
     the link was a zero-width line — the real shoe is 500×172 mm. A rectangle/box cast is a drop-in
     (`cast_shape` takes any collider; thin-box casts are robust) — **detection** (first touch
     anywhere on the face) generalizes for free; the **resultant** (our pressure profile) needs the
     lateral dimension added: edge-column 1D profiles + load-weighted lateral centroid (full-face
     cast for detection, edges for distribution). Lateral resolution knob = column count.
  2. **Box, not plate** (user: "all in"): links have thickness; wheels sit on the internal face,
     ground on the external. Decomposes to two parallel faces of one curve + honest rolling radius +
     wrap fan-out for free; end-face contact and link self-collision consciously parked.
  3. **Pin line** (user, the mechanically correct core): the chain solve rides the pins at
     mid-thickness — the true pitch line of a link chain (172 mm IS pin-to-pin; sprocket engagement
     is at pitch radius). Three parallel offsets of one solved curve: inner face (pin − t/2, wheels),
     pin line (chain/joints/pitch/advection), outer face (pin + t/2, terrain + force application).
     Boxes scissor at pins on bends — the meshes' job to interlock, like real castings.
- 2026-07-02 — **Step 16: model 3 increment 1 — pin-line chain + box cast** (green: fmt/clippy
  clean). New `model3.rs` (fork of model 2): `TRACK_THICKNESS` 0.04 (T-34 shoe); `PinBelt` resource
  (own length+count on the pin line — its perimeter is ~π·t longer, reusing model 2's numbers would
  eat the slack); `pin_circles()` = rest circles + t/2. Physics fork `apply_belt_support_boxes`:
  oriented **box cast** per link (`Collider::cuboid(0.02 sliver, t, pitch)`, basis lat/out/along —
  the travel-distance convention makes pen measure past the *outer face* automatically), endpoint
  rays at the pins' outer-face points, same closed-form profile, force + traction applied **at the
  outer face** (lever includes the shoe). Conform fork `conform_belts_boxes`: pin-line ring, wheel
  circles + t/2 (inner face on the gear), terrain planes hold pins t/2 inside ((p−q)·m ≥ t/2),
  Verlet/slack bookkeeping unchanged. Shared state reused (BeltPhase/ChainMemory/LinkLoads made
  pub(super) in model2; chain-solve consts copied into model3 so it can tune independently).
  `articulate_wheels` gets a per-model surface offset (+t/2); `draw_rig_gizmos` draws the **outer
  face** as a dimmer companion line for model 3 (the thickness reads). Registered: `M` cycles 1→2→3,
  model 3 default. Lateral width (edge columns) = increment 2; box rendering = increment 3.
  - AWAITING USER TEST (model 3 default): (1) two parallel cyan lines — pin line + dimmer outer
    face — with the outer face touching the ground and the tank riding ~4 cm higher than model 2
    (A/B with M); (2) wheels sit on the chain correctly (inner face — no sink-into-chain);
    (3) drive/climb/washboard behave like model 2 (thickness is an offset, not new dynamics);
    (4) wrap regions read right (pins at r + t/2 on the gears).
  - USER TEST (16): "looks logical — darker blue rides the ground, wheels ride light blue." Three
    observations: (a) gizmos jitter on flat ground (element TBD — asked dots-vs-lines again);
    (b) links jumpy creeping along the washboard — analysis: link **slap-down** off bump corners
    (a link's contact plane is continuous while the corner is under it, then drops to the flat when
    its trailing edge clears; the rear joint falls over ~100 ms) — largely honest track clatter,
    user to judge magnitude after fixes; (c) contact gizmos render under the ground.
- 2026-07-02 — **Step 16b: surface-point contact + gauge hygiene** (green). (1) (c) explained +
  fixed: force/dots were at the *reference* outer face, which rides ~sink inside the terrain — the
  penalty penetration is virtual compliance, the real interface is the surface. Model 3 now applies
  and draws at the surface point (`+ out·(t/2 − pen_c)`, pen at the centroid ≈ (pen_a+pen_max)/2);
  slightly more honest lever, dots land on the drawn outer line. (2) Hygiene bug: `LinkLoads` (the
  damped gauge) wasn't cleared on `M`/`R` — stale identity shifts caused garbage-rotation transients
  after switching; now cleared with the rest.
  - AWAITING USER TEST (model 3): (1) dots/normals sit ON the dark-blue outer line; (2) which element
    still jitters on flat (dots/normals vs chain lines) — diagnosis pending user answer; (3) washboard
    slap-down: honest clack or too harsh?
- 2026-07-02 — **MODEL 3 RESTART (user decision).** The increment-1 attempt (steps 16/16b) is
  **deleted** — never committed, removed from the tree — to be rebuilt from scratch in a fresh
  session. Everything worth keeping is distilled into
  **`.agents/docs/design/track-model/model3-handoff.md`** (settled design, implementation learnings
  from the attempt, the two open issues — flat-ground gizmo jitter [element undiagnosed] and the
  washboard slap-down feel — Avian API notes, workflow contract, registration checklist). Kept in
  the tree from the attempt: model2's pub(super) state visibilities and the `LinkLoads` clear on
  `M`/`R` (real hygiene fix). Registry back to models 1–2, model 2 default; fmt/clippy green.
- 2026-07-02 — **Step 17: model 3 rebuild, increment 1 — pin-line chain + centerline box cast**
  (green: fmt/clippy clean; clean launch). Rebuilt from the handoff doc, per its registration
  checklist and implementation learnings. New `model3.rs`: `TRACK_THICKNESS` 0.04; `PinBelt`
  resource (own length + count on the pin line — `pin_circles()` = rest circles + t/2; reusing
  model 2's numbers would eat the slack budget); physics fork `apply_belt_support_boxes` — oriented
  box cast per link (`Collider::cuboid(0.02 sliver, t, pitch)`; travel-distance convention measures
  pen past the *outer face* automatically), endpoint rays from the pins' outer-face points, same
  closed-form profile, force + traction + dot **at the terrain surface** (`+ out·(t/2 − pen_c)`,
  pen_c ≈ (pen_a.max(0)+pen_max)/2 — the 16b fix, baked in from the start); conform fork
  `conform_belts_boxes` — pin-line ring, wheel circles + t/2, terrain planes hold pins t/2 inside
  ((p−q)·m ≥ t/2), Verlet/slack bookkeeping = model 2's (consts copied for independent tuning).
  Wiring: `Model::BoxBelt` registered (M cycles 1→2→3, model 3 default), `articulate_wheels` rides
  wheels at chain + t/2 for model 3, `draw_rig_gizmos` draws the outer face as a dimmer companion
  line (`BELT_OUTER_COLOR`). One deliberate deviation from the handoff: its box basis
  (`lat = axis×out`) is left-handed (det −1, invalid for `Quat::from_mat3`) — used `out×axis`
  (same lateral axis, sign flipped; identical physics for the symmetric box).
  - AWAITING USER TEST (model 3 default): (1) two parallel cyan lines — pin line + dimmer outer
    face — outer face touching the ground, tank riding ~4 cm higher than model 2 (A/B with `M`);
    (2) wheels sit on the chain correctly (inner face — no sink-into-chain); (3) contact dots +
    normals sit ON the dark outer line, not underground; (4) drive/climb/washboard behave like
    model 2 (thickness is an offset, not new dynamics); (5) wrap regions read right (pins at
    r + t/2 on the gears). WATCH: flat-ground gizmo jitter (if present — which element, dots vs
    chain lines?) and washboard slap-down feel (honest clack or too harsh?).
  - NEXT (after user verdict): increment 2 — real 500 mm width + edge columns + lateral centroid;
    then increment 3 — box rendering.
  - VERIFIED (user): "feels about the same, the two lines appear correct" — increment 1 accepted
    (thickness = offset, not new dynamics: exactly the bar). Jitter report: visible on flat ground,
    easiest on the force gizmos, **wheels also gently vibrating — and it's on ALL 3 models**. That
    reframes the open jitter issue: the model-specific suspects (companion line, box-cast noise,
    Verlet chain) are exonerated; the culprit is in the shared substrate (penalty bed + rigid hull
    on the fixed tick, or the shared conform→wheels→gizmos visual path). Wheels are a new element
    datum: on flat they should be terrain-locked through the conform; candidates = real hull
    amplitude bigger than 15b assumed (plus the lift clamp in `articulate_wheels` rectifying at its
    boundary), or flickering conform casts.
- 2026-07-02 — **Step 17b: jitter probe** (green: fmt/clippy clean; built, NOT launched — awaiting
  user go). Element-first measurement instead of guessing: `JitterProbe` resource ring-buffers
  ~120 frames of (a) hull world y + pitch (physics side), and on the left track at hull-local
  z ≈ 0 — same spot, picked spatially so the advected rings don't rotate the element away —
  (b) articulated wheel world y, (c) conformed belly-sample world y, (d) contact dot drawn world y
  + displayed load (the force-gizmo size channel). `J` logs each channel's peak-to-peak (mm / ° /
  ±% of mean load). Sampler runs at the end of the visual chain, gated `sim_running`; works
  identically on all 3 models since the paths are shared.
  - TEST PLAN (user): at rest on flat ground, let it settle ~2 s, press `J`, read the log line —
    per model (`M` between). The channel(s) carrying visible amplitude name the element; hull-y
    tells physics vs visual.
  - PROBE RESULTS (user, all 3 models at rest): hull y 0.03–0.23 mm, wheel/belt y 0–0.04 mm, dot y
    0.6–1.1 mm — **everything geometric is dead still** (physics + conform + wheels exonerated).
    The one live channel: **dot load** — ±94% on model 1 (raw readout), ±12.5/13.1% on models 2/3
    *through* the damped gauge. The visible "jitter" is the force-gizmo **size** pulsing (dot
    radius + normal-line length ∝ load); the "wheels vibrating" was perceptual bleed from adjacent
    strobing gizmos (wheels moved micrometres). Root cause of the load noise: the displayed load
    included the **damping term**, which reads the hull's tick-scale micro-velocity (~0.1 mm
    wobble at tick rate ≈ 0.02 m/s ≈ hundreds of newtons through the damper) — while the elastic
    term follows penetration, stable to ~0.02 mm. The long-open "static at-rest gizmo jitter" is
    hereby diagnosed.
- 2026-07-02 — **Step 17c: elastic-only load display + gauge pruning** (green: fmt/clippy clean;
  built, awaiting launch approval). The principled fix, replacing the band-aid: models 2/3 now
  display `SUPPORT_STIFFNESS_PER_M · area · engage` (the elastic/static component — steady at rest,
  still instant for load migration while driving); physics keeps the full load (support force,
  friction cap, belt reaction all unchanged). This obsoletes the 15b damped gauge, so the whole
  apparatus is **deleted**: `LinkLoads`/`LinkLoadsSide`/`LOAD_SMOOTHING` (models 2 + 3), the
  per-link EMA + identity-rotation bookkeeping, and the `M`/`R` clears in mod.rs (the handoff's
  "stale gauge shift" bug is moot — the resource no longer exists). Model 2's frozen-ness
  consciously overridden for display hygiene only (user-directed; physics untouched). Model 1
  deliberately keeps its raw ±94% readout — frozen baseline, and now a visible A/B of what the fix
  removes. Reviewed-and-kept (principled, not judo): `CONTACT_ENGAGE` (real compliance, 10b),
  eased wheel rise/fall (feel, not noise-masking), `belly_extra` EMA (feedback-loop stabilizer),
  Verlet dt clamp (hitch guard), `DRIVE_RAMP` (input shaping).
  - AWAITING USER TEST: (1) models 2/3 at rest — dots + normal lines rock steady (J: dot load
    ±<1%); (2) model 1 still strobes (expected, baseline); (3) driving: load migration/wheelspin
    colouring reads as before, now without smoothing lag; (4) washboard slap-down feel — re-judge
    with steady gizmos, harshness may have been partly display.
  - VERIFIED (user, model 3, J readings): **flat is solved** — dot load ±13% → ±0.8%, wheel/belt
    0.02–0.03 mm, "overall more stable". (A first flat reading showed wheel/belt ~0.9 mm — window
    contamination from a pause/unpause transient; a clean re-measure per protocol confirmed calm.)
    Remaining, two items: (1) **washboard corner links "jump around a bit"** at rest (hull pitch
    there 0.041° = 10× flat — hull rocks on the bumps, chain re-solves; suspects: bistable tent
    configs [13e/13f residual] and/or the contact-plane anchor `q` hopping between terrain features
    while the cast distance stays continuous — plane height jumps, link snaps); (2) **pause/unpause
    leaves the hull visibly displaced** — parked below.
- 2026-07-02 — **Step 17d: whole-ring probe channel** (green: fmt/clippy clean; built). The
  single-spot probe channels watch hull-local z ≈ 0 and can't see a jumping link elsewhere on the
  loop. Added `ring_y` to `JitterProbe`: per-frame snapshot of every left-side conformed sample's
  world y, index-aligned (ring is index-stable at rest; cleared on count change). `J` now also
  prints a **ring sweep**: the worst link's p2p + its hull-local (z, y) position + how many links
  exceed 0.5 mm — "some links jump around" becomes a number attached to a specific link.
  - TEST PLAN (user): park ON the washboard at rest, settle ~2 s, `J`; the sweep line names the
    worst link and its z (which bump/corner it sits on) and the live-link count. Same at flat for
    the baseline (expect worst ≈ 0.03 mm, 0 links over).
  - SWEEP RESULTS (user, model 3): the picture is now sharp. FLAT: single-spot channels calm, but
    **61 of 83 links > 0.5 mm, worst 2.4 mm at (z −0.20, y −0.30) — wheel-top height = the
    return-run drape**: the grounded belly is pinned, the whole *free chain* shimmers at mm scale
    forever (neutral modes — spans swinging, joints sliding along wheel circles — restored only by
    the weak drive anchor; ~0.1 mm/frame micro-inputs random-walk them; the step-15 "chain sleeps"
    claim measurably never happens). WASHBOARD: worst 15 mm (idler rear wrap) and 97 mm (front
    diagonal), hull itself rocking 0.8–1.5 mm / 0.048° — AND the user reports the tank **slowly
    drifts with zero input**, so the cm-numbers are confounded by honest terrain-following of a
    creeping vehicle. The drift itself is a finding: slip-saturated friction is viscous below
    saturation (no true stiction) and the corner-contact rocking rectifies into creep — parked
    below as its own contact-level item.
- 2026-07-02 — **Step 17e: chain sleep (model 3 only; model 2 stays frozen)** (green: fmt/clippy
  clean; built, awaiting launch approval). Standard rigid-body sleeping applied to the chain solve
  — don't re-solve an unchanged problem: per side, after `CHAIN_SLEEP_FRAMES` (15) consecutive
  frames with max joint motion < `CHAIN_SLEEP_MOTION` (0.3 mm), the chain sleeps (residual Verlet
  velocity zeroed); while asleep the casts + solve are skipped and the frozen local chain is
  re-mapped through the hull pose. Wake tests deviation from the **sleep anchor** (hull pose +
  belt phase at sleep time), not the previous frame — `CHAIN_WAKE_TRANSLATION` 1 mm /
  `CHAIN_WAKE_ROTATION` 0.002 rad / any phase advance — so slow creep or oscillation can't hide
  under a per-frame epsilon. New `ChainSleep` resource in `model3.rs`; `M`/`R` need no clearing
  (a teleport exceeds the anchor deviation and wakes it).
  - **REVERTED before launch (user decision: "we're losing track a little")** — the sleep
    machinery is out of the tree; the diagnosis (17b–17d probe findings) and the elastic-only
    display fix (17c) stand. The free-chain mm-shimmer at rest is accepted-for-now and parked
    below; focus returns to the model-3 increments.

- 2026-07-16 — **STEP 18 DEFINED: cross-pill cast shape (user-driven, MP-motivated).** Direction
  review with the user ("is model 3 good? will it hold up in MP?"): model 3's *structure* is
  already the ADR-0014 sim/view split — the physics fork's persistent state is 4 scalars
  (per-side BeltSpeed + BeltPhase; stations re-derived from the rest ring each tick), the entire
  Verlet chain is view-only. Replication plan settled: replicate + roll back the 4 scalars as
  components on the tank; remote tanks integrate phase locally from replicated speed; the chain
  never crosses the wire. The one MP liability is the **sharp box cast** — the 2026-07-06
  architecture review requires divergence-continuous contact primitives (sharp corners flip the
  winning terrain feature discretely under mm pose differences → rollback-resim divergence; same
  class the game's sphere-cast suspension fix killed). Fix adopted from Physics Tank Maker
  (Unity): the shoe casts as a **cross of two pills**, diameter = link thickness — lateral pill =
  track width, longitudinal pill = link length. We take the *shape*, not their architecture
  (their links are real rigid bodies — the simulate-and-send path we rejected). Bonuses expected:
  rounded trailing edge turns the washboard slap-down drop into a roll-off; `hit.point1` on a
  rounded surface is unique (kills the coplanar tie-break class of 13b at the source). The
  lateral pill merges with increment 2 (it IS the width detection; edge columns still do force
  distribution). Plan: (a) longitudinal pill only on the centerline — flat behavior should be
  identical, A/B the washboard quirks; (b) lateral pill + edge columns + lateral centroid.
- 2026-07-16 — **Step 18a: longitudinal pill on the centerline** (green: fmt/clippy clean). New
  `shoe_pill(len)`: `Collider::capsule_endpoints(t/2, ±Z·(len/2 − t/2))` — total extent along the
  link = link length, so the flat-ground footprint tiles the chain exactly like the sliver box it
  replaces (outermost surface at pin + t/2 over the cylindrical mid-section; the outer tangent
  spans the middle ~132 mm of the 172 mm pitch, cap tips land ON the pin line at the pins). Both
  cast sites swapped (physics `apply_belt_support_boxes` + conform `conform_belts_boxes` — fn
  names kept, registry identity); `BOX_WIDTH` sliver const deleted. Everything else untouched:
  the travel-distance pen convention holds (pill radius = box half-thickness along `out`),
  endpoint rays, profile, centroid-at-surface, planes, wheel circles, Verlet.
  - AWAITING USER TEST (model 3, A/B with `M`): (1) flat ground — identical to before (rest
    height, sink, dots on the outer line, J-probe calm); (2) **washboard at crawl — the point of
    the change**: link slap-down off bump corners should read as roll-off, softer than before
    (A/B against model 2's sharp plates); (3) washboard at rest — corner-link jumpiness + the
    zero-input creep: better, worse, unchanged? (4) wraps/ledge/trench sanity — no new artifacts
    from the rounded ends (watch the ledge lip). J on washboard before/after if feel is
    ambiguous.

- 2026-07-16 — **Step 18b: viz-layer instrumentation** (green: fmt/clippy clean). User ask while
  testing 18a: per-layer toggles for everything visible. New `VizLayers` resource — every visual
  element on its own key, legend on screen (below the model label, ASCII-only):
  `1` hull mesh · `2` wheel meshes (Visibility::Visible overrides the hidden-hull inheritance so
  wheels stay drawable alone) · `3` chain line · `4` outer-face line (model 3) · `5` hub markers ·
  `6` contact dots · `7` normal lines · `8` **force vectors** (new: support along the normal in
  magenta + traction in orange, 20 kN/m arrows; `Contact` grew a `traction: Vec3` — all three
  models fill it, frozen 1/2 touched as a shared-visibility change like 17c) · `9` **cast shapes**
  (new, model 3: every shoe pill outlined at the *physics* stations — the rigid reference ring the
  casts run from, NOT the solved chain, so physics-vs-visual placement reads directly) ·
  `0` **Avian collider wireframes** (`PhysicsDebugPlugin` mounted, `PhysicsGizmos` group synced to
  the toggle) · `-` **chain reference ring** (new, model 3: the advected drive-anchor target, dim
  violet — chain-vs-reference deviation shows where terrain/wheels hold the chain off its rest
  path). Defaults reproduce the pre-toggle look (diagnostic layers start off). Draw systems stay
  pause-transparent (immediate-mode); mesh/collider mirrors run on `resource_changed`.
  - AWAITING USER TEST: (1) each key flips exactly its layer, legend tracks; (2) hull off + wheels
    on shows the far track through the hull; (3) forces layer reads at rest (uniform magenta belly
    fans) and under throttle (orange traction swinging longitudinal); (4) casts layer: pill row
    tiles the belly, scissor gaps at bends, pills sit ON the reference ring (expect them to hover
    off the drawn chain wherever the chain deviates — that gap is real information, not a bug);
    (5) colliders layer shows hull box + drive-cylinder backstops + terrain.

- 2026-07-16 — **CONTACT-ORACLE RESEARCH (user-directed zoom-out, mid-step-19).** Increment (b)
  implementation paused after Yan challenged the premise: rather than guarding witness noise
  downstream (edge columns), can penetration be deterministic *by nature*? Four parallel research
  agents (Drake hydroelastic / terramechanics + Chrono / shipped games / parry-manifold
  internals) — findings + decision matrix in
  **`.agents/docs/design/track-model/contact-oracle-research.md`**. Headlines: (1) every mature
  field converged on *fixed-sample field evaluation* (collocation), never solver witness points —
  our endpoint rays + closed-form profile are already the pattern, the cast is the outlier;
  (2) parry can hand us a deterministic 2-point pill manifold TODAY (verified + benchmarked
  locally: bit-stable endpoints, arbitrary GJK point labeled `UNKNOWN` and filterable, ~15%
  cheaper than the cast) — with a guardrail: per-link rest pen (~25–50 mm) exceeds the t/2
  = 20 mm radius → EPA fallback; fix = inflate query radius + subtract (also a free
  edge-smoothing knob); (3) the endgame for MP determinism is an analytic terrain field
  (rounded-box SDF union over the authored course), fixed samples per link per edge column, no
  parry in the track loop at all — bit-deterministic pure arithmetic, and the rounding knob
  plausibly retires the washboard slap-down + corner creep. RECOMMENDED: build the field oracle
  as **MODEL 4 (field-belt)** forked from model 3 (same chain/profile/width design, oracle
  swapped), live `M` A/B; manifold route recorded as bridge for arbitrary colliders; terrain
  authoring commitment → promotion ADR. AWAITING Yan's verdict on the model-4 path. Increment
  (b) width (edge columns at ±0.23 m, half coefficients — settled during the step-18 pill
  discussion, cross-member pill REJECTED: lateral line-tie = 13b class on a new axis, and no
  per-column max for the profile) applies to whichever oracle wins.

- 2026-07-16 — **Step 19: MODEL 4 — field-belt, increment 1 (centerline collocation)** (green:
  fmt/clippy clean). The contact-oracle research verdict, implemented (Yan: "accepted. let's
  go"). New `model4.rs`, forked from model 3 — same pin-line chain, profile, drivetrain, face
  offsets; ONLY the terrain oracle changes:
  - **`TerrainField`**: every block `spawn_environment` lays down is also recorded as a
    `FieldBox` (center, inv rotation, half-extents) — colliders and field share the same
    transforms, cannot drift. Oracle = Quilez **rounded-box SDF union** (`FIELD_ROUNDING` 0.03 m
    — exact on faces → flat ground bit-matches the cast answer; edges rounded → normals/depths
    turn instead of snapping; must stay < smallest half-extent, washboard 0.06).
  - **Physics `apply_belt_support_field`**: per link, signed depth at THREE fixed collocation
    stations (pin a / mid / pin b, on the outer face), two-piece closed-form profile between
    them (signed stations → the clip finds lift-off between samples), force machinery
    byte-for-byte model 3's. No casts, no rays, no witness anywhere; ~1k field evals/tick ≈
    trivial; pure fixed-order arithmetic → bit-deterministic.
  - **Conform `conform_belts_field`**: contact planes from the field at each link's mid station —
    anchor = projected surface point, normal = FD gradient (turns smoothly around rounded
    edges); Verlet/projections/belly_extra/ChainReference unchanged.
  - **Viz**: layer `9` for model 4 draws the collocation stations (grey = clear, orange =
    penetrating); outer-face line, +t/2 wheel offset, `-` reference ring all extended to model 4
    (`model_on_pins`). Registered: `M` cycles 1→2→3→4, **model 4 default**; model 3 frozen as
    the cast-oracle A/B partner (its staged width consts removed — width lands in the winning
    oracle). PinBelt/pin_circles shared pub(super) from model 3.
  - AWAITING USER TEST (model 4 vs model 3 via `M`): (1) **flat parity** — rest height/sink/dot
    loads identical to model 3 (the SDF is exact on faces); `J` + `L` numbers should match;
    (2) **washboard crawl — the headline test**: slap-down should soften further (rounded field
    corners = the link rolls off a rounded edge), corner-link jumpiness + zero-input creep
    re-judged; (3) trench lips/ledge: chain wraps corners smoothly (gradient turns, no plane
    snapping); (4) ramp: honest slope contact (the one rotated field box); (5) layer `9` shows
    the station pattern breathing with the belt; (6) pit drop-in/grind-out still works (field
    depth saturates at CONTACT_PROBE like the casts did).

- 2026-07-16 — **Step 19b: field bottoms buried — the "washboard ignored" fix** (green: fmt/clippy
  clean). USER TEST (19, ss×3): flat ≈ model 3, but **the washboard is largely ignored** — belt
  swallows the boards, support arrows land on the flat ground between them. Diagnosed as a
  `min()`-union SDF interior seam (a known limitation the research flagged, mis-scoped in 19): a
  board rests ON the ground slab, so past mid-board the nearest union surface flips from the
  board's TOP face to its BURIED BOTTOM face — depth *shrinks* with further sink (non-monotone
  force, a trapdoor), the belt punches through, equilibrium = belly on the gap ground. Flat has
  no stacked seams, hence perfect flat parity. Fix: `FieldBox::from_block` extends every box's
  bottom by `FIELD_BURY` 2 m along its local −Y (top surfaces untouched; ramp extends along its
  tilt) — no interior bottom seams remain; depth below a top face is monotone, then **plateaus**
  at the box's side-face distance. Also answered: layer `9` on model 4 correctly draws collocation
  *stations* (spheres — the oracle is point lookups), not cast shapes; model 3 keeps the pills.
  - KNOWN LIMIT (parked below): the plateau bounds max force on THIN features — the fine
    washboard's boards (side-face core ~0.10) read ~0.10 max depth vs the cast model's ~0.13
    equilibrium indent → boards carry, tank rides them, but slightly softer than model 3 on the
    fine set only (mid/coarse sets saturate ≥ 0.22, fully honest).
  - AWAITING USER TEST (model 4 vs 3): (1) washboard now carries the tank — belly rides the board
    tops (fine set may read slightly softer than model 3 — report); (2) flat parity still holds;
    (3) slap-down/corner-jitter/creep A/B — the original headline test; (4) step/trench-lip/ramp
    sanity (side faces unchanged by the bury).

- 2026-07-16 — **Step 19c: capture harness + directional field depth — plateau defect measured
  and fixed** (green: fmt/clippy clean). Yan's ask after 19b ("still reacts oddly — harness the
  sandbox so you can see what I see programmatically"): new **`harness.rs`** — `SANDBOX_HARNESS`
  env var (`model=,z=,warmup=,ticks=,throttle=,out=`) runs a scripted scenario and writes JSONL
  (meta / field scans / per-tick: hull pose+vel, belt, every contact with load+slip+normal-y, the
  conformed chain), then exits; cursor-grab suppressed during captures; normal runs untouched.
  First captures (rest@z=−5 washboard + throttle-0.12 crawl, models 3 vs 4) diagnosed 19b's
  residual: field VERTICAL profiles were monotone+exact (fix confirmed at field level), but the
  Euclidean side-face **plateau** (~0.10 on fine boards) starved support — model 4 sat 32 mm low,
  belly draped into gaps, Σload 82% of weight. Fix: **`TerrainField::depth_along`** — signed
  directional depth along the link's outward normal by sphere-tracing the same rounded SDF
  (≤12 iters, pure fixed-order arithmetic, deterministic; buried origin saturates like the casts;
  unbounded through stacked geometry via the top face; lateral roll-off from the field rounding —
  the tangent-graze jump of any first-hit query lands at zero depth on a rounded surface, the
  same reason the pill cast was smooth). Physics stations, conform lifts/planes (plane anchor =
  ray hit, normal = gradient at the surface), and the viz stations all moved to it;
  `signed_depth` (Euclidean) retained for scans/reference.
  - **MEASURED A/B (harness, washboard z=−5)**: rest — hull y 1.1862 vs 1.1884 (parity, rides
    board tops, 20 vs 18 contacts on boards), vertical support = 100.0% weight exactly;
    **model 4 p2p: hull 0.8 mm / pitch 89 mdeg / load 0.3%** vs **model 3: 5.7 mm / 491 mdeg /
    11.0%** — the at-rest washboard limit cycle is ~7× stiller in position, ~5× in pitch, ~35×
    in load noise under the continuous field. Crawl (0.12 throttle over the fine set): m4 ≈ m3
    (pitch p2p 2.23° vs 2.26°, vy extremes slightly lower) — honest now, not soft.
  - USER VERDICT (19c): **"okay, looks good"** — accepted; one observation: the **wheels seem a
    bit "jumpy"** (view-layer suspect: `articulate_wheels` rides the conformed chain with
    asymmetric ease; model 4's per-link plane hand-off may step the wheel target as links advect
    — harness can capture wheel dy to confirm; parked below).

- 2026-07-16 — **Step 20: MODEL 4 width — edge columns** (green: fmt/clippy clean). Yan confirmed
  the missing-width artifact on a rib (wheel width phasing through terrain the centerline
  stations can't see) — the settled edge-column design, in field terms: per link, the shoe
  samples as **two columns at ±`COLUMN_OFFSET` (±0.23 m)** along the link's lateral axis, each
  running its own three-station directional profile with **half the per-metre coefficients**,
  each applying support + traction at its own point (roll torque from a curb under one edge,
  cross-slope, half-off-a-ledge all emerge). Conform/visual chain stays centerline (width is a
  physics concern). Viz layer `9` now draws 6 stations/link (both columns); contact dots show two
  rows per track. KNOB documented: edge placement (±0.23) slightly overestimates roll stiffness
  vs a uniform lateral strip (2-pt Gauss ±0.14 matches the uniform second moment) — move inward
  if curb roll feels stiff. Cost: 6 directional traces/link/tick — still trivial.
  - HARNESS PARITY (verified before handoff): contact x-rows exactly ±1.02/±1.48 (=±1.25∓0.23);
    flat rest **perfectly still** (hull p2p 0.00 mm, pitch 0 mdeg, vertical support exactly
    100.0% weight, p2p 0.00%); washboard rest: ride height preserved (1.1859 vs 19c's 1.1862),
    support 99.9%, hull p2p 2.2 mm / pitch 60 mdeg (creep-crossing sampling; still ≫ calmer than
    model 3's 5.7 mm / 491 mdeg).
  - AWAITING USER TEST: (1) the rib case from the screenshot — drive one track onto a washboard's
    lateral edge: the edge column should catch it, hull rolls, no more width phase-through
    (layer 9 shows the outer station row engaging); (2) cross-slope on the ramp edge; (3) flat +
    washboard feel unchanged; (4) wheel jumpiness — recheck (unrelated change, expect same).

- 2026-07-16 — **Step 21: third column + moment-matched weights + conform-reads-all-columns +
  adversarial wedge review + cost budget** (green: fmt/clippy clean). Yan's three asks: wedge
  failure mode (2 adversarial agents), jumpiness diagnosis, third-column viability/cost.
  1. **Columns**: 3 at (0, ±0.23), weights (0.606, 0.197, 0.197) solved so the edge pair matches
     a laterally-uniform strip's second moment exactly — exact total load AND exact roll
     stiffness (the step-20 "overstiff knob" retired), detection gap 0.46→0.23 m. Both agents
     independently converged on exactly this ("positions set the detection Nyquist, weights set
     the quadrature") — it kills the two SEVERE red-team scenarios: sustained centered ridge
     (drive-through-able cover = MP fairness exploit) and the "phantom softening" inverted-W roll
     oscillation when steering astride a ridge.
  2. **Conform reads the physics columns** (max of the 3 lateral mid-stations per link): the
     view/physics inversion — ranked by the red team as WORSE than the force ghosting (visual
     chain climbing what the hull ignores reads as "broken game", visible on remote tanks; and it
     was bidirectional) — closed by construction. Standing invariant adopted: **the visual chain
     conforms only to what the physics samples.**
  3. **Wedge residuals → map pipeline** (Yan: detailed maps coming): build-time lint — no
     standalone force-bearing surface narrower than 0.25 m; narrower objects are cosmetic /
     crushable props (WoT/War Thunder pattern — "the tank crushes it" is genre-standard and
     reads correct at 26.5 t) / collision-widened proxies (Source clip-brush move). Closed-form
     lateral segment-max REJECTED (C1 breaks + double-counting + dies on baked SDF grids; full
     verdicts in the agents' reports). Sub-sink features (<~10 cm): ghosting ≈ honest crushing.
  4. **Cost (measured, field_bench)**: 37 ns/trace bucketed (~5 candidate boxes), 211 ns
     unbucketed (24 boxes). Per tank-tick at 3 columns (1494 traces): 0.056 ms bucketed —
     **30-tank PvP ≈ 1.7 ms/tick server, ~0.8 ms with belly gating; a single column costs
     ~19 µs/tank/tick**. Verdict: 3 columns trivially viable; spatial bucketing is MANDATORY at
     promotion (unbucketed 30 tanks = 9.5 ms/tick); 5 columns held in reserve.
  - HARNESS PARITY (3 columns): flat perfectly still (0.00 mm, 100.0%W, 0.00%); washboard rest
    hull p2p 0.80 mm / pitch 57 mdeg / load 0.29% — equal-or-better than every prior config;
    x-rows exactly ±1.02/±1.25/±1.48.
  - FOLLOW-UP (red-team finding): the hull backstop box (±1.0 m) does NOT laterally cover the
    track bands (1.0–1.5 m) — the ">0.6 m features bottom out on the hull" net is porous exactly
    under the tracks. Track on next backstop pass.
  - AWAITING USER TEST: rib/curb under one track (roll + no phase-through, layer 9 shows 9
    stations), centered wedge now carries, feel unchanged elsewhere.

- 2026-07-16 — **Step 21b: jumpiness fixes (view-layer, refinement on model 4 — NOT a model 5:
  zero force change, nothing to A/B)** (green: fmt/clippy clean). (1) **Wheel spring smoothing**
  (shared `articulate_wheels`, all models): the linear slew (`SUSP_RISE/FALL_RATE`) replaced by a
  critically-damped semi-implicit spring (`SUSP_OMEGA_RISE` 18 / `SUSP_OMEGA_FALL` 7 rad/s,
  `Suspension` gains `dvel`; lift still clamped 0..MAX) — eases in/out of every move instead of
  slewing at a rate cap. (2) **Conform reads all 3 longitudinal stations × 3 columns** (model 4):
  deepest of the same 9 samples the physics uses — the visual≡physics sample-set invariant now
  holds on both axes.
  - MEASURED (harness, 0.12-throttle crawl over the fine washboard, before → after): **wheels:
    total travel 7778 → 1938 mm (4×), max step/tick 25.2 → 4.7 mm (5×), direction reversals
    194 → 74**. Chain: max single-tick step 55.5 → 61.2 mm, mean unchanged (4.4 mm) — the
    conform-9 closes the longitudinal inversion but does NOT reduce the chain snap: the snap is
    the Verlet chain complying with a plane target that honestly rises a full board height
    within ~1–2 ticks (edge rounding 3 cm ≈ 1.3 ticks of travel at crawl). Rest parity intact
    (hull p2p 0.80 mm, 100.0%W). Wheel percept should improve sharply; the chain's residual
    snap may now read as honest link clatter — Yan to judge; candidate levers if not: temporal
    blend of per-link lift, or wider field rounding at the top of raised features.
  - EXPERIMENT (Yan-directed): codex CLI (`gpt-5.6-sol`) engaged as an additional agent source —
    bounded read-only second-opinion review of model4.rs + the lateral-rigidity design question;
    verdict lands async.

- 2026-07-16 — **Step 22: the accretion review + the kinematic track view** (green: fmt/clippy
  clean; Yan-directed: "make sure we're not digging too deep with patch over patch"). Three
  independent reviews (adversarial architecture audit, industry research, codex `gpt-5.6-sol` —
  `scratchpad/codex_arch_review.md`) returned ONE verdict: the sim side is principled; the
  Verlet view chain was nine patches deep, and its stabilization apparatus was built against
  CAST-oracle artifacts (bistable tents, plane snapping) that step 19c's smooth field deleted —
  the patch stack outlived its root cause. Key structural defect: the wheel↔chain feedback loop
  (chain wraps the wheels' circles, wheels ride the solved chain, stabilized by a one-frame lag)
  — the root of the wrong-side captures, which tuning can't fix ("a stateful solver discovering
  topology the reference construction already knows"). Industry (WoT Blitz devblog, War Thunder
  Hot Tracks, UE/Unity community consensus): NO mainstream gameplay title ships a force-based
  visual chain; the norm is geometric fitting; per-segment sims exist only as per-model art-team
  cosmetics (Gaijin). Also: the staged ω=90 wheel-spring retune (in the binary Yan drove, never
  its own step) had explicitly-integrated damping DIVERGENT at 60 fps (2ωΔt = 3 > 2) — the
  "getting worse" wrong-side captures were thrashing wheel circles. (21b's shipped ω=18 was
  stable, just slow — codex correction, step 22b.)
  **Landed, accepted by Yan ("accepted. let's go"):**
  1. **Sim — exact ray oracle**: `depth_along`'s sphere-trace march (24 iters + exhaustion
     fallback) replaced by the EXACT closed-form ray-vs-rounded-box first hit (3 face slabs + 12
     edge cylinders + 8 corner spheres per box, union = min over entries; `FieldBox::ray_hit`).
     Deletes MARCH_ITERS/MARCH_EPS/the fallback's convergence-boundary discontinuity AND the
     staged 21c ghost-contact patch (obsolete — the failure class is gone). Harness: dd columns
     0 rise-violations, 0 jumps, all 9 vscans.
  2. **Sim — 21c fixes folded in**: force applied at the profile's own value at the centroid
     (pen_c; mirror-symmetric traction lever) + columns at the TRUE shoe edges ±0.25 with exact
     Simpson weights (1/6, 2/3, 1/6) — blind rim closed. Flat parity EXACT vs step-21 captures
     (hull y 1.1418, 100.1 %W, 0.00 p2p).
  3. **View — stateless kinematic wrap** (`conform_belts_field` rewritten; the step-21 chain
     preserved verbatim as `conform_belts_field_chain` behind the **`V` toggle** as the frozen
     A/B partner; harness `view=chain`): wheels-FIRST data direction (ground → wheels → belt,
     acyclic). Lower convex envelope of the articulated pin circles (Graham scan; lifted wheels
     drop out — wrong-side wrap unrepresentable), terrain conform as direct displacement along
     the outward normal (max of the SAME 3 physics columns — visual≡physics invariant kept),
     top-run sag from the FEED-FORWARD length budget (belly_extra EMA dead) clipped from above
     onto the wheel circles, links = phase-scrolled resample. No integration, no constraints, no
     memory: teleports/model-switches are ordinary recomputation.
  4. **Wheels — spring deleted, all models**: rise INSTANT (terrain forcing a wheel up is
     kinematic; smoothing it was the "slo-mo"), fall BALLISTIC (gravity-limited; ~190 ms off a
     0.18 m board because g says so). Zero tuning constants, stable at any frame rate. Model 4
     reads the field directly (`articulate_wheels_field`, 21 arc probes every 5° × 3 columns —
     5-probe version quantized board-edge onsets into ~55 mm/tick belly steps; at 5° the step
     hides under the true circle-on-edge ramp ~25 mm/tick).
  - MEASURED (harness A/B, same binary, wrap vs chain): physics BIT-IDENTICAL (crawl trajectories
    equal to the cm — the view provably never touches forces). Wheels: rise deficit 0.00 mm
    (was mean 1.74 / max 165 with the spring). Belly shape Δ/tick: chain 14.8 mm mean / 57 max →
    wrap 11.4 / 35.4. **Compression zigzag: 22.0 mean kinks (chain, 21b capture) → 0.00 (wrap)**
    — closed by construction, not tuned. CORRECTION (codex, 22b): the staged 21c chain edits
    (anchor split 400/60 + CHAIN_BEND 0.02) were NOT discarded — they were already in the working
    tree and shipped inside the frozen chain view (its crawl kinks 5.2 vs the pre-split capture's
    22 owe partly to them); the topology-aware push alone was never built.
  - WATCH: washboard rest shows intermittent ±2–3 mm hull / ~5 %W support bursts (creep advecting
    stations over board edges — the known stiction tab; flat rest is perfectly still). Sim-side,
    pre-existing class, slightly more visible with the ±0.25 edge columns.
  - AWAITING USER TEST: wheel reactivity (slo-mo gone?), wrap-vs-chain feel on `V` (top-run life
    — the one honest loss is the chain's emergent slack migration; parametric sag-breathing is
    the fallback juice), teleport/R/M cleanliness (wrong-side capture should be extinct in wrap
    view). Chain + toggle get DELETED once the wrap wins the feel check.

- 2026-07-17 — **Step 22b: Yan's A/B verdict + the wrap's two real bugs + the dual codex deep
  dives** (green: fmt/clippy clean). Yan drove both views: prefers the CHAIN's feel ("natural
  wheel movement, chain whip, tightness being responsive"; zigzag still its pain point) and filed
  four wrap complaints. Triage: **two were one bug pair in my conform** — (a) displacement SIGN
  inverted (pushed the belly INTO boards, shoved the nose off the sprocket at a wall — the
  "wanders off the sprocket" + "phases through terrain" reports), (b) conform ran on the wrap's
  sparse VERTICES, so a tangent segment crossing a board mid-span was never sampled. Fixed:
  sign corrected; conform on a dense 0.1 m resample; displacement field gets a ±1-station MAX
  filter + 3-tap smooth (the rigid link's edge OVERHANG the chain got from its per-link
  constraint). (c) Global-parabola sag → per-span drape with wheel PROMOTION (`sag_span`:
  recursive split at the wheel top, slack shared by chord — the "slack more substantial" fix).
  (d) Instant wheel rise read robotic (Yan) → implicit critically-damped ease (WHEEL_EASE_OMEGA
  45, unconditionally stable at any ωΔt), fall stays ballistic.
  - MEASURED after fixes (crawl): belly-into-board interior penetration median 0.0 / p95 10.7 /
    max 27.9 mm — now BETTER than the chain view (3.7 / 20.0 / 83.8); compression kinks 2.7
    (terrain tents, not zigzag) vs chain-era 22; wheel rise lag mean 6 mm (the intended ~100 ms
    ease). New standing harness check: belly-vs-board penetration (the check that would have
    caught the sign bug — shape metrics alone were blind to it).
  - CODEX DEEP DIVES (both `gpt-5.6-sol`, scratchpad/codex_chain_review.md +
    codex_wrap_review.md): the two directions CONVERGE on "wrap skeleton owns topology,
    dynamics layer on top". (1) PROPER CHAIN = topology-guided inextensible rod: joints live in
    ROUTE COORDINATES (monotone material s per link + band-limited normal-displacement spline,
    knots at 2·pitch) on the tagged wrap route — wrong-side states and per-link zigzag modes
    UNREPRESENTABLE; XPBD with real bending energy (compliance in N·m², selects long bows over
    kinks — the math for why alternating modes are the most expensive); exact length (belly_extra
    deleted); drive applied ONLY at the sprocket sector (the all-joint advected anchor is itself
    a zigzag cause — it injects compression everywhere); fixed 120 Hz view accumulator (0.88/frame
    damping = three different chains at 30/60/144 fps); canonical reseed list. (2) LIVING WRAP =
    ~42 floats/side of closed-form exact-update states: signed tension Δ (drive load →
    tight/slack branches, conservative slack allocator solving one monotone H_c equation), a
    COMPLEX slack moment advecting slack around the loop (the minimum state that can do
    migration), 2 damped modal oscillators per free span (whip; support-motion + belt-accel
    forced), exact asymmetric wheel filter with target-velocity feed-forward. Scorecard vs full
    chain: 4/5 on all four of Yan's named feel elements, 5/5 stability/teleport/determinism,
    weak only on link-level detail (piles, clatter, derail). Corrections taken: 21b ω=18 was
    stable (divergence was the staged ω=90 retune); anchor-split/CHAIN_BEND shipped in the chain
    view, not discarded; real T-34 sprocket is REAR (our rig says front — tension topology must
    key on DriveEnd, not "front"; parked).
  - PATH (proposed): staged LIVING-WRAP build (exact wheel filter → tension+slack allocator →
    fundamental span mode → slack moment → second mode), each stage feelable; the route-chain
    remains the endgame if link-level life is still missed — it REUSES the wrap skeleton, wheel
    filter, and tension concepts (no-regret ordering). Chain view stays on `V` as the benchmark
    until the living wrap wins or loses on feel.

- 2026-07-17 — **Step 23: route-chain slices 1+2 (fixed clock, XPBD bending, sprocket-only
  drive)** (green: fmt/clippy clean; Yan: "accepted. let's go" on the convergent design — the
  chain REHOUSED on the wrap skeleton, not a free-space rebuild; prior step committed @ c81f381).
  The V-toggle chain view (`conform_belts_field_chain`) rebuilt per the codex staged plan:
  1. **Fixed 1/120 s internal clock** (accumulator, ≤8 substeps/frame, debt dropped) — feel is
     render-rate independent; damping is a real-time HALF-LIFE (0.15 s), not 0.88/frame.
  2. **Drive at the sprocket sector ONLY** (motor τ=0.05 s toward belt surface speed on joints
     engaged on the drive wheel): the all-joint advected anchor is DELETED — it was itself a
     zigzag cause (injected compression around the whole loop). Length constraints transmit
     drive; tight/slack sides now emerge.
  3. **XPBD bending energy** (C = θ − θ0, analytic turning-angle gradients, compliance
     α = pitch/B in real units, λ per substep) RELATIVE to the taut route's own curvature — wheel
     wraps and authored sag free, deviation costs; replaces the CHAIN_BEND midpoint blend and the
     35° cap stays as the hard link stop. Structural anti-zigzag: kinks are the most expensive
     compression mode now.
  4. **Exact belt length** (belly_extra deleted from this view) and **wheels upstream for BOTH
     views** (`articulate_wheels_field` before either conform — the chain↔wheel circular
     dependency is gone everywhere in model 4).
  5. **Terrain planes probed at the CHAIN's OWN positions per substep** (linearized, with a
     drift-correction term): drive localization lets joints drift from the reference ring (slack
     migration — the feature), which broke the old reference-INDEXED plane assignment (measured
     171 mm board phase-through when a joint got its neighbour's plane). The per-frame
     reference-station plane build + the SDF `gradient()` helper died with it.
  - MEASURED (fixed-tick harness): compression kinks 22 (anchor chain) → **2.6** (route-chain;
    wrap 2.7) — the zigzag is structurally dead in BOTH views now; crawl board phase-through max
    84 → 45 mm (median 1.6); reversal shows ~115 mm/tick top-run whip (the desired dynamics, now
    real); rest drape settles at exactly road-wheel-top height (−0.301 vs −0.300 pin top) —
    the T-34 look emerging from the solve instead of the anchor.
  - Remaining step-23 slices (parked until Yan's feel verdict): route coordinates (wrong-side
    capture unrepresentable — the old nearest-exit circle push-out is STILL live in this view),
    band-limited transverse basis, canonical reseeds.

- 2026-07-17 — **Step 23b: codex implementation review + the "flabby" fixes** (green: fmt/clippy
  clean). Yan's first drive of the route-chain: "interesting direction... feels flabby/untuned,
  perhaps too much slack?" Codex reviewed my implementation of its own design
  (`scratchpad/codex_step23_review.md`; XPBD algebra/λ handling confirmed CORRECT) and found the
  flab's mechanical half: **link lengths were reference-ring CHORDS** — shorter than arc spacing
  around wraps and varying as phase slid samples across polyline vertices, so links breathed with
  phase. Landed this batch:
  1. **Immutable pitch**: every link is exactly `pitch` long; a **closing length pass** after the
     contact/circle projections so they can't bank pitch error (exact total length IS the tension
     model). Measured: pitch deviation ≤ 2.6 mm, loop length conserved.
  2. **Per-LINK terrain contacts** (pin/mid/pin × 3 columns — the physics collocation; one
     station per joint had a between-pins blind strip), retention band 80 mm.
  3. **Sprocket motor gated to the wrap ARC**: annulus around the rim + loop-direction tangent
     test + smooth radial engagement ramp (the old whole-disk radius test could grab folded or
     wrong-side nodes and slammed newly-engaged nodes ~18 mm in one substep).
  4. **Signed hinge stop**: the 35° cap as a zero-compliance projection with the same
     turning-angle gradients (the midpoint-lerp version never hit the bound exactly and silently
     changed link lengths).
  5. **Feel tuning**: CHAIN_HALF_LIFE 0.15 → 0.08 s (0.15 was ~1.7× floatier than the old chain
     at 60 fps — the tuning half of "flabby"); CHAIN_SLACK_TRIM 0.05 m off the chain loop as a
     tensioner-preload stand-in (slice-3 route-tube tension is the principled owner).
  - Codex items DEFERRED to slice 3 (by design): θ0-by-index misalignment under material drift
    (route coordinates fix it; phase-periodic wrap impulses possible until then), full
    fixed-clock ownership of wheels/inputs + output interpolation + canonical reseed on debt
    overflow, per-sweep contact reprobe, normal-velocity filtering on contacts.
  - AWAITING USER TEST: tightness (trim + damping), yank/whip character, zigzag on reversal.

- 2026-07-17 — **Step 24: slice 3 (route tube) + T-34 physical alignment + pinch fuses** (green:
  check clean; harness rest/crawl/compress/full-throttle). Yan's verdict on 23b: "somewhat
  better... still behaves physically more like a rope than a track chain — align it with real
  T-34 numbers; go for slice 3 too; also high-speed + strong bump can get the chain 'pinched',
  completely break the rendering and shoot off the tank." Codex T-34 deep dive at
  `scratchpad/codex_t34_review.md` (rich: real link masses, six-roller drive geometry matching
  our radius to 1.2%, the pinch causal chain, slice-3 pitfall list). Landed:
  1. **Tagged route tube (slice 3)**: `Route` = the wrap view's envelope machinery run on the
     CURRENT articulated wheels **every substep**, kept as an arc-length table with per-segment
     sector tags (Arc(k)/Span). Every joint carries a monotone route coordinate `s`
     (`ChainSideMemory.s`, windowed ±2-pitch rebase per sweep, pairwise order clamps); normal
     offset hard-clamped to the tube (OUT 0.30 / IN 0.40 — both under half the belly↔top route
     gap so the (s,u) atlas never overlaps); **u ≥ 0 on wheel arcs** → wrong-side capture and
     "chain off the tank" unrepresentable. θ0 and motor membership read the route at each
     joint's own `s` (kills the 23b θ0-by-index deferral); link-CHORD wheel exclusion added
     (two clear pins can still chord through a wheel).
  2. **Fixed clock owns its inputs**: wheel circles interpolated prev-frame→current across the
     substeps; solved output interpolated to render time (`acc/h` remainder); over-budget hitch
     → canonical reseed instead of silent debt drop.
  3. **T-34 numbers** (codex provenance): node mass 16 kg (cast link + pin share, ~1.15 t/72
     links — real inverse masses in all compliant/limited denominators); bending B 10-normalized
     (≈160 N·m² hidden) → **2 N·m² regularizer** — a pinned track has no bending spring, the
     anti-flutter duty moved to **pin DRY friction**: torque-limited XPBD hinge constraint
     toward the previous material angle, τ = 25 N·m (μ≈0.15 × 12 mm pin × 10–50 kN → 18–90),
     λ accumulated across sweeps, clamped ONCE per substep (per-sweep clamping would 4× it);
     **anisotropic route-frame damping** (tangential t½ 0.60 s, normal 0.060 s — yank lives,
     flutter dies); slack trim 0.11 → ~0.02 m visible ≈ the manual's 30–50 mm sag spec (trim
     shortens links 0.8% — the honest idler-shift tensioner is parked). Six-roller
     alternate-link engagement character parked (flavor).
  4. **Pinch autopsy + fuses** (codex ranked it; measured full-throttle washboard: max joint
     speed 383 m/s before → 22 m/s after, reseeds 0 in all four scenarios):
     the ROOT was `prev = old_pos` turning every unilateral depenetration into Verlet
     restitution (0.5 m correction = 60 m/s) — fixed by **velocity reconstruction** (terrain
     contacts keep only pre-projection escape velocity, wheels zero inward radial, anisotropic
     guardrail caps clamp the stored velocity); saturated terrain probes skipped as invalid
     linearizations; terrain corrections capped at pitch/2 and **yield to wheels** (no
     alternating projectors on an empty feasible set); NaN/torn-link detector (0.25·pitch) →
     canonical reseed, now **terrain-conformed** (the earlier taut-route seed landed inside
     boards and re-tore next substep — the actual "shoots off" loop in the first cut of this
     step).
  - Measured (fixed-tick harness): steady pitch dev ~2–4 mm mean (transients only in warmup
    settle); washboard phase-through 16.6 mm crawl / 58 mm at full throttle (was 45–84);
    loop breathing ≤ 0.27 m under full-throttle terrain; belly kinks ~2–4 (polygonal drape is
    now the intended look); trench dips are honest (floor at −1.2 m).
  - DEFERRED: unwrapped-s ledger (order currently pairwise-clamped, wrapped), per-sweep terrain
    reprobe, tension-dependent friction torque clamp(15,120) N·m, six-roller discrete drive,
    idler-shift tensioner, `L_taut > Np` infeasibility handling (can't occur with sag-budget
    route by construction today).
  - AWAITING USER TEST: rope-vs-track verdict (pin friction + anisotropic damping are the two
    labeled knobs), sag vs the real spec, pinch immunity at speed.

- 2026-07-17 — **Step 25: promotion foundation** (Yan's verdict on 24: chain "feels pretty dang
  good"; direction: measure before the view call, wary of two models, many-tanks authoring).
  Landed, in order:
  1. **Perf probes + measurement**: chain 405 µs/substep-side, wrap 254 µs/frame (M4, crawl).
  2. **Field broadphase** (`8289fe9`): per-block world AABBs + z-bucket grid, candidates-only
     probes (duplicate-tolerant min-folds, fixed order). Chain → 170 µs/substep-side, wrap →
     66 µs/frame; hull trajectory unchanged. New budget math: simulated-chain tank ≈ 41 ms
     CPU/s (fixed-rate), route tank ≈ 4 ms/s @60 fps.
  3. **Architecture doc** (`architecture.md`, v2 after a 10-finding codex adversarial review):
     one route core / three consumers; chain = VIEW state on a **PresentedFrame** seam (post
     rollback-smoothing, pre propagation, interpolated across substeps, `discontinuity` →
     reseed); batched TerrainOracle over a shared TerrainMap (world.rs currently discards its
     block transforms — refactor queued); MaterialLoop (pitch × count) authoritative with the
     tensioner reconciling geometry (kills "spread the residual"); axle topology in the spec
     (Tiger interleaved discs = one route circle + one suspension station per axle — coincident
     circles break external_tangent); tier cost model per TANK (the v1 "2 ms budget" failed its
     own arithmetic: 4 sim + 26 route ≈ 4.5 ms/frame solver-only), TrackRenderer instance-buffer
     seam (~5k links at 30 tanks), BeltState = root-born replicated+predicted (NOT
     local_rollback — v1 conflated the contracts). Tiger agenda: `tiger-authoring-agenda.md`
     (tiger_1.glb surveyed: 8 wheels/side + sprocket/idler visual pivots present; static
     Track_Strip/Treads meshes to hide). Yan defers the Tiger session until authoring starts
     (he provides a proper link model).
  4. **Core extraction** (`8c602d1`, `d5387a6`): `src/track/` is real — oracle.rs (TerrainOracle
     trait + BlockField, reach-parameterized), route.rs (tagged route + tube queries + envelope
     geometry), chain.rs (ChainState/ChainParams/StepReport — the step-24 solver as a pure
     fixed-clock stepper, T-34 constants → params), wheels.rs (lift target + implicit ease).
     The sandbox is consumer #1: model4's chain view and wheel articulation are thin ECS
     adapters (RouteChain resource wraps ChainState); ChainSideMemory lost its slice-3 fields
     (old models 2/3 chain untouched). Harness parity within run noise (harness itself is NOT
     bit-repeatable: throttle smoothing reads real frame time — worth fixing if bit-parity
     gates are ever wanted). Startup hitch now visibly reseeds once (designed overrun path).
  - NEXT (phase A remainder): TerrainMap refactor in world.rs → view_plugin (PresentedFrame,
    tier policy, no-slip belt derivation, link instancing) → hide legacy track meshes → Tiger
    authoring session.

## Open questions / parking lot

- **Lateral link rigidity (Yan, 2026-07-16, open tab)**: a real shoe is ~perfectly stiff
  laterally, so if ANY lateral column of a link contacts, the link should arguably lift AS ONE
  (deepest contact sets the link's pose; the other columns unload — currently the 3 columns
  support independently, so a ridge under one edge + flat ground under the rest DOUBLE-SUPPORTS
  the link). Sketch of a pose-continuous, argmax-free formulation: per link, lift = max column
  depth (value-only); per-column effective pen = own pen − (max − own) clamped ≥ 0 (rigid-body
  unloading), forces still applied at the columns → roll torque from the loaded column survives,
  double-support dies. CHANGES FORCES → this is the first credible **model 5** candidate (A/B
  rigid-lateral vs independent columns live). Codex verdict (step 22): legit sim candidate,
  closed-form active-set solution recorded (`u_A = κΣw·d / (K + κΣw)` over the active set),
  ORTHOGONAL to the view — do after the view rewrite, derive K from a real compliance target
  (an arbitrary K "merely relocates the patch"). Both reviews: second-order; the 5-line
  max-unloading approximation covers it if a playtest ever complains. Not scheduled.
- **Chain snap residual (21b)** — MOOT in the step-22 kinematic wrap (belly Δ/tick max 35 mm ≈
  honest terrain-following); applies only to the frozen `V` chain view until it's deleted.
- **Zero-input creep on the washboard — now QUANTIFIED (19c harness)**: ~14 mm/s steady forward
  drift at rest on the boards, essentially identical on models 3 and 4 (89 vs 84 mm over 6.1 s) —
  confirms the 17d diagnosis that it is a contact-physics class (slip-saturated friction is
  viscous below saturation, no stiction anchor; tilted board contacts leave a net tangential
  residual that integrates), NOT an oracle/witness artifact. Fix when scheduled: stiction anchor
  (brush-model-style) near zero slip, shared by all models.
- **Euclidean-vs-directional field duality (model 4, resolved 19c, kept for reference)**: the
  Euclidean SDF plateaus at side-face distance under thin raised features (19b's bounded-softness
  limit — measured biting 18% of weight on the fine washboard); physics reads the directional
  sphere-traced depth instead. The Euclidean field remains the conform-gradient/scan primitive.
- **Top-run compression zigzag** — RESOLVED for the default view by step 22 (kinematic wrap:
  22.0 mean kinks → 0.00, closed by construction — the surplus goes into the sag budget the same
  frame). Still present in the frozen `V` chain view until it's deleted (the staged anchor-split/
  CHAIN_BEND tuning for it was discarded unbuilt).

- **Thrown-track capture** — UNREPRESENTABLE in the step-22 wrap (no state; which side of a wheel
  the belt is on is given, never discovered). If track-throwing ever becomes a damage mechanic it
  needs sim authority + a replicated flag, not a view accident (step-22 review consensus). The
  model-2/3 chains (and model 4's frozen `V` view) keep the old failure mode.
- **Static at-rest gizmo jitter** — RESOLVED on flat (steps 17b/17c, user-confirmed): geometry
  still (≤0.03 mm wheels/belt), flicker was the force-gizmo size displaying the damping term's
  micro-velocity noise; fixed by elastic-only display (models 2/3; model 1 keeps the raw strobe as
  baseline).
- **Free-chain mm-shimmer at rest** (open, diagnosed, accepted for now): the ring sweep (17d)
  showed the non-grounded chain (drape/wraps/diagonals) never settles — neutral modes random-walk
  at ~mm scale under micro-inputs; on the washboard, amplified by the hull's real rock + creep. A
  chain-sleep fix (17e) was built and reverted unlaunched (scope call). If it ever matters:
  re-derive from the 17e design (sleep on quiet inputs, wake on anchor deviation incl. phase).
- **Pause/unpause hull displacement** (open, user-observed): Esc-pausing then resuming leaves the
  hull visibly displaced. Prime suspect: render-interpolation alpha snap when `Time<Physics>`
  stops/starts — should be bounded by one tick of motion; if the shift is clearly bigger,
  something real (needs its own probe). May have polluted one early jitter reading.
- **Zero-input creep on the washboard** (open, user-observed): at rest on bump corners the tank
  slowly drifts. Mechanism: the hull limit-cycles ~1 mm on the corner contacts (suspect:
  under-damped — contact damping scales with contact *length*, tiny on a corner, so ζ ∝ √L), and
  the slip-saturated friction is viscous below saturation (no static-friction anchor), so the
  rocking rectifies into slow creep. Contact-level work: revisit corner-contact damping scaling
  and/or a stiction anchor (brush-model-style) for near-zero slip.

- **Sprocket is at the wrong end (codex, 22b)**: the real T-34 drives from the REAR sprocket
  (front wheel is the idler); our rig labels the front drive wheel "sprocket". Cosmetic today,
  but tension/slack-side logic (living-wrap stage 2) must key on a `DriveEnd` identity + loop
  direction, never on "front" — and the rig label should flip when convenient.
- Envelope as taut convex-hull of wheel circles vs. sagging catenary on slack runs — start taut,
  add sag in step 2.
- How belt-speed/slip couples to the (future) powertrain — deferred to step 4.
- Promotion path into the game: new module vs. extending `driving`; and the ADR-0005 rewrite.
