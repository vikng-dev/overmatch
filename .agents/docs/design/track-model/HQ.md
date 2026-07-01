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

## Open questions / parking lot

- Envelope as taut convex-hull of wheel circles vs. sagging catenary on slack runs — start taut,
  add sag in step 2.
- How belt-speed/slip couples to the (future) powertrain — deferred to step 4.
- Promotion path into the game: new module vs. extending `driving`; and the ADR-0005 rewrite.
</content>
</invoke>
