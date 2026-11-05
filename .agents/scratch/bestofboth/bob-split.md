# bob-split: the physics engine owns touching, the bond solve owns holding

Subject: the video game's environment destruction model (constructions holding, cracking,
settling and collapsing) and its server cost. Game state only; no crew or rounds are involved.
Lane `bob-split`, branch `lane/bob-split`, from the tip of `lane/bondsolve`. Every number below is
MEASURED by the named test on this branch (64 Hz, 6 substeps, the game's gravity, the shipped brick
and house data) unless labelled DERIVED. No timing was taken; work is sparse products (CG rounds)
and joint visits.

## What was built

`src/construction/fracture/bondsolve/split.rs` (test-only experiment code):

- `groups`: chunks joined by holding joints (an intact seam, or a cracked one still shut and not
  slipping). Each group is one avian3d body: static when a holding ground joint or a buried chunk
  anchors it, dynamic otherwise. A rest never joins a group.
- `Body::of`: the group's linear graph. Intact seams keep their stiffness; every push-only part (a
  rest or cracked seam inside the body, or an intact fixing's bulk bearing as its own edge beside
  the fixing) keeps its bulk stiffness, re-checked once per tick.
- `hold` (**the meeting point with `bob-factor`**): one body's `K u = f` under its weight, the
  contact forces from `contact_transfer` on its faces, and (free body) its rigid acceleration via
  the existing `accelerating_frame`. Today it runs the existing `Krylov` CG over bonded edges only,
  warm, to the existing 1e-10 relative gate; unchanged loads cost zero products. A factorized solve
  replaces its body without changing a caller.
- `judge`: once per tick, intact seams against tensile strength and cohesion plus friction (a
  fixing reads its own share); push-only parts open on lift or tip (a bearing opens alone) and slip
  past friction, keeping their compression as a frozen friction pair (`Contact::Slipping`).
- `split/tests.rs`: an `Authority` harness that spawns bodies (one compound of child convex-hull
  colliders per body), reads contacts each tick, holds and judges every awake body, and splits a
  cut body by despawning it and spawning one body per remaining group (ADR-0014), velocities
  carried from the parent's motion.

`joint_edge` was extracted from `Assessment::rebuild` so both paths build one edge.

## Per case

| Case | Verdict | Error / evidence |
|---|---|---|
| 1. Board on two knife-edge bodies | **pass** | Reactions against statics: 3.4e-7 W awake, 2.5e-7 W asleep; net moment 1.5e-7 W·span. Tick-to-tick spread 6.3e-7 W awake; 0 asleep (Avian slept the board at tick 32; the reading keeps the sleeping pair's last step). |
| 2. Beam on a wall | **pass** | 0.9 μW holds (0.2 mm creep in 0.5 s); 1.1 μW slides at 0.3202 against (F − μW)/m 0.3188 m/s (+0.44 %). 2 cm inside the edge: tilt 0; 2 cm past: 1.96 rad in 1.5 s. |
| 3. Stepped wall, one static body, a block pressed on it | **pass** | Joint force and moment against cantilever statics: 7.6e-5 to 2.1e-4 of the largest term. 0.98 × the governing joint's Navier press: no crack in 32 ticks; 1.02 ×: that joint (and no other) opens in tension on the first tick, the wall becomes a static base and a dynamic top (2 bodies, 4 colliders spawned; 1 body, 4 colliders despawned); Avian tips the top (2.65 rad in 1 s). Work while the press is awake: 24–25 products a tick. |
| 4. Cracked bed squeezed shut | **pass** | Carries the section's weight exactly (statically determinate); unchanged re-check 0 products; pulled 1.5 W: opens (lift). 0.9 × the tipping press (`F h = W t/2`, DERIVED) holds 32 ticks; 1.1 × opens on the first tick and the section tips as its own body (0.63 rad in 1 s). |
| 5. Unit house | **fails the premise** | Below. |

Finding from case 4: a block closing 1 mm under 0.9 × the tipping press strikes at about 0.2 m/s
(DERIVED); its blow, read as force over the tick, opens the shut bed. Avian, now owning the section,
rocks it and keeps it standing (0.0005 rad after 2 s). The split corrects that J/dt overestimate by
physics, at the cost of one extra body.

## The unit house

**Bodies at spawn: 1** (1,122 chunks, 1,122 colliders, anchored). Every chunk is joined to every
other through seams: intrinsic brick and wood seams and the nailed roof. The roof does not rest on
the gables in the data; it is nailed into the same group. So all 71 rests and every nailed bearing
lie inside the one body, and the once-per-tick push-only re-check does the contact work the split
meant to hand to Avian.

**Gravity only** (`the_unit_house_as_bodies`, ignored, observational; static ground slab at grade):

- Tick 0, cold hold of the whole house: 881 products, 3.07 M joint visits. It changes 226 joints:
  109 nailed bearings stop pressing, 95 push-only parts slip (67 nailed bearings, 28 brick–wood
  rests), 22 rests open.
- The changes cascade over the next ticks: 757 by tick 32 (45 bodies), 795 by 10 s (76 bodies):
  216 bearings open, 240 parts slipping, 139 nails open in tension and 68 in shear, 59 intrinsic
  wood seams open, 62 brick–wood rests open. The roof comes apart.
- 62 dynamic bodies are still awake after 10 s, and the walls they touch re-solve every tick:
  13 bodies held, about 1,020 products and 2.46 M joint visits a tick at 10 s. **Zero solves at rest was not
  reached.**
- No body showed a mechanism. (An earlier arm did: a fixing that cracked and lost its bearing in
  one tick kept joining its group with no edge left in the graph. `split::apply` now separates such
  a joint.)
- The contact-inside solve on `lane/bondsolve` found zero cracks at boot for the same data; this
  gravity collapse comes from the once-per-tick push-only rule (a part opens or slips on one
  linear trial and never re-closes or re-sticks), not from a load the data cannot carry.

**Ram-like press** (a 0.4 m block of 46.8 t, gravity off, pressed at 300 kN into the outer wall,
after the gravity run): every pose and velocity stays finite, and no hold stalls.

- Tick 1: 1,720 joints change at once (mostly intrinsic brick seams opening in tension and shear);
  1 body with 1,024 colliders despawned, 337 bodies with 1,024 colliders spawned.
- 192 ticks: 2,688 joint changes; bodies 76 → 864; 914 bodies spawned and 126 despawned, 5,818
  colliders respawned in all; the most work in a tick 3,633 products and 1.54 M joint visits
  (tick 2). From tick 10 to tick 191 the work falls from 1,063 products and 91,000 visits a tick
  to 622 and 44,000.

## Costs

- **Bodies and colliders**: house at spawn 1 body, 1,122 colliders. A split respawns every collider
  of the cut body, the largest remainder included: its cost scales with the body, not with the
  crack (the press's first cut respawned 1,024 colliders; 5,818 over 192 ticks).
- **Stress-solve work per tick**: asleep 0 (a sleeping body is not held). Awake: 12–25 products a
  tick for the 2- and 4-chunk fixtures; 881 products (3.07 M visits) for the house's cold solve; the
  house touched by awake pieces re-solves every tick. `bob-factor` targets exactly this term.
- **Unchanged loads skip the solve only on exact equality.** An awake body's contact reading jitters
  at about 1e-7 of the load each tick, so any static body touched by an awake one re-solves every
  tick (the fixtures' 1 idle tick of 32).

## What Avian's contact impulses get wrong

Nothing measurable at rest or sliding: reactions, moments and friction land within 1e-6 of statics
and within 0.5 % of `(F − μW)/m`. Two properties matter to the split: a sleeping pair's reading is
frozen at its last step (exact, and stale if the sleeping body's load would change); and a blow
read as `J/dt` overstates a short contact's force (case 4), which the split tolerates because Avian
then owns the motion.

## What is left

1. **The house is one body.** The owner's model assumed a few bodies (walls anchored, roof resting).
   With the roof nailed, the contact problem stays inside the body. Either the construction data
   joins the roof by rests (an owner decision on construction facts, never a tuned number), or
   push-only parts need re-close and re-stick, which brings back an iteration across ticks.
2. Not modelled: re-stick of a slipping part, re-close of an opened bearing, torsional friction,
   the pulse energy rule (only sustained strength is judged), a tolerance for unchanged loads
   (a `Resolution` number would be needed).
3. A split respawns the whole cut body; keeping the largest part's colliders would need a
   decision against ADR-0014's spawn-only rule (no sim state moves onto the old entity).
4. Pricing by the orchestrator.
