# Buildings interview with Yan, 2026-10-30 (running notes)

Subject: the game's environment destruction, as game state only. Recorded here: Yan's answers, and the
questions still open. The writeup and the implementation path wait until this interview ends.

## Owner's vision (verbatim)

"individual walls: drive through a wall and the fragments you collided with become loose and interact
with your tank and the world. shoot through a wall can either take a region of fragments with it, or
bring down a larger piece depending on the round type/explosive mass."

## Answers so far

- **Broken-off pieces leave ballistics.** A wall and its intact fracture members are static. They take
  part in ballistics, and every player sees them. A piece that breaks off becomes a falling body that
  collides with tanks and the world, and it no longer takes part in ballistics.
  - Yan leans towards client-side presentation for these pieces.
  - Yan is open to server simulation and settling: one player could see a fragment block vision while
    another doesn't, and server simulation has no determinism problem, since tanks are already
    server-owned.
- **Falling trees should interact with the world later.** Yan recalls that a falling tree ignores the
  environment today, and wants that to change in the future. A fact check is running.
- **Format.** One question at a time, branching as we talk; don't pre-state the questions.

## Facts found (READ in code, 2026-10-30)

- **A falling tree passes through tanks.** The sim fall is instant. Lying segments are `[Armor] × NONE`,
  and the rest pose reads terrain height only (`src/trees.rs:546-593`, `:621-629`, `:680-702`). The tree
  plan's end state is a server-simulated body "replicated like a hull" (fork F8, `slice-1-fallen-tree.md:103`).
- **A server-simulated replicated body already exists:** the turret launched in a cook-off. It is a free
  dynamic body on the server, and its pose is replicated every tick at 64 Hz with no velocity. The client
  drives a kinematic copy to that pose (`src/damage.rs:606-651`, `src/net/protocol.rs:341`, `:796-853`).
- **The shipped APHE arms only after spending 35 reference-mm in one body** (`src/ballistics/burst.rs:176-189`).
  A 0.4 m masonry wall spends 4–16 (DERIVED), so a burst round crosses a wall unarmed. The ground never
  arms a fuze. A burst armed outside any tank doses every unowned pooled volume, and never a tank
  (`src/ballistics.rs:4013-4017`).
- **The blast sphere is not built**, and no plan for it exists in #103. Burst potency comes from the burst
  law's wall mass and energy, with filler mass entering only through that law (`src/ballistics/burst.rs:61-63`,
  `:129-141`).

## Fragment simulation: priced before deciding

- **Options:**
  1. each client simulates its own pieces;
  2. the server simulates each piece until it rests, then freezes it (The Finals' debris half, without
     their structural solver);
  3. the server derives each piece's rest spot from data and clients animate the fall (the trees' way;
     pieces pass through tanks);
  4. split by size.
- **The deciding unknown is server time per falling piece.** Network cost is DERIVED as minor: about
  63 kB/s for 2–3 s per raw 35-piece breach, against a 235 kB/s baseline.
- **Yan accepted measuring it two ways:** locally headless for the curve, and on Edgegap for the absolute
  number, because that is where one vCPU saturates at 23–26 tanks.
  - A stress-harness lane is in bay 3.
  - `0.7.0-alpha.2` carries the harness and goes out tonight.
- **Rule for reading the result:** under 1 ms for a 35-piece breach on Edgegap points to option 2. Much
  more than that points to option 4, or to option 3 as the fallback.

## Damage deposition for structures (Yan, 2026-10-30)

- **Disintegration versus breakage.** Depending on the round or explosive, a wall segment either turns
  to dust or breaks off its neighbours as a loose piece.
- **Walls make no spall of note.** Spall emerges from the projectile and the target together. Masonry
  doesn't fracture a shell, and the wall bits that fly off are negligible next to real spall.
- **Energy deposited is the measure of destruction.** A sabot punches a small hole and leaves most of
  the wall standing. A bulkier steel slug does more. Burst rounds do the most, and their fuze
  sensitivity depends on the material.
- **Open:** is it one model for tank internals and the environment, or one system for metal and
  structural parts and a simpler one for brick, concrete and wood?
- **Brainstorm reply (Claude):** two thresholds per substance.
  - Energy sets how big the affected region is.
  - Energy density decides what happens in it: above `crush` it turns to dust; above the bond limit it
    breaks off.
  - Crush radius ∝ E^(1/3) and break radius ∝ E^(1/2) for a point source. For a line source, crush
    radius ∝ √ε and break radius ∝ ε, with ε the energy deposited per metre (DERIVED from a toy model).
  - The fuze becomes a rate rule (the deceleration the material imposes against the fuze's
    sensitivity).
  - Recommended: one deposit law, with the consequence set per substance family.

## Facts found, round 2 (READ in code, 2026-10-30)

- **Energy lost per metre already scales with frontal area:** dE/dx = (f/1000)·A·(a + k v²)
  (`src/ballistics/mechanism.rs:220-262`).
  - Worked case (INFERRED): at equal energy, crossing 0.4 m of wood, the APCR core loses about 2.8 times
    less than the full-calibre round (about 172 kJ against about 484 kJ).
  - The "a fat slug does more than a sabot" behaviour is therefore already in the walk. No discarding
    sabot ships yet.
- **Spall potency is 0.88 × the energy the round lost across the body**
  (`src/ballistics/walk.rs:1833-1835`).
  - A substance sets only the cone's angle (`debris_half_angle_deg`). Nothing switches it off.
  - A trunk exit therefore throws about 64 HP today, which conflicts with Yan's "walls make no spall of
    note". It only doses unowned volumes (trees), never tanks.
- **The fuze reads only "spend ≥ `arming_spend`" at one body exit** (`src/ballistics/burst.rs:176-189`):
  no rate, no substance, no impact.

## Working model, split into data, law and emergent behaviour (proposed 2026-10-30, awaiting Yan)

- **Data.**
  - Substance: resistance factor (exists), density, crush energy (J/m³), bond strength (J/m²).
  - Round: mass, calibre, nose and speed (exist), plus fuze sensitivity (new).
  - Building, from the compiler: pieces, bonds with contact area, and ground anchors.
- **Law.**
  - The deposit, as the walk and fields compute it today, plus the blast sphere later.
  - Crush: the density at a piece above its crush energy turns it to dust, and partial deposits wear
    its pool down.
  - Bond: the energy per area across a bond above its strength cracks it.
  - Support: a group with no chain to the ground falls as one piece.
  - Spall potency becomes 0.88 × energy lost × the exited material's strength relative to armour steel,
    capped at 1.
  - Fuze: it functions when the material decelerates the round past the fuze's sensitivity.
- **Emergent:** sabot against slug, dust crater against a ring of loose pieces, sections falling,
  machine-gun wear, which rounds burst on which walls, no spall from soft materials, and the ruins.

## Toward a material model (Yan asked 2026-10-30: does dust apply to all materials? a few base properties?)

- **Proposed: three base properties per material.**
  - Density: the mass of falling pieces, and spall's punch.
  - Strength: how hard the material is to push through (today's resistance factor already derives
    from it), how much energy removes it, how hard it slows a fuze, and how much spall it throws.
  - Toughness: how far cracks run, so whether a hit leaves a clean hole (tough) or a ring of loose
    pieces (brittle).
- **"Dust" generalises to "removed material."** The look is per material: dust for brick, splinters for
  wood, a punched hole for steel.
- **Grain** (wood's direction) is a possible fourth property, later.
- **Crew and modules stay HP pools** (game abstraction): they take dose, and are reduced or knocked out.

**Accepted by Yan (2026-10-30):** three base properties per material (density, strength, toughness).
Crew and modules stay HP pools.

## Worked wall (Yan's example)

- **The wall:** 2 × 4 m of brick, pre-cut into chunks about 20 cm wide, preferring straight brick seams.
  - Assumed 0.25 m thick, which gives about 200 chunks of about 18 kg each (DERIVED).
  - Yan said the cut happens "in Blender". The earlier direction was our Rust compiler. Raise that later;
    it changes nothing in the model.
- **Exposed by the walk-through:**
  - An intact wall must be one collider. The chunk layout stays data until the first hit, because 200
    chunks × 4 walls × 74 houses ≈ 59,000 colliders.
  - Handbook toughness values give absurd crack reach, because most energy goes to heat, not cracks.
    Material numbers therefore need tuning against target outcomes.

**Tuning (Yan, 2026-10-30):** yes, but very rough initial tuning. Never hide a model mismatch inside a
tuned value. A value that must leave its handbook range is a missing factor: name it, and fix the law.

- The crack zone already fails that test: handbook toughness gives a reach of hundreds of metres.
- Candidate missing factors:
  1. Joints break on stress (the push) against the mortar's tensile strength, not on energy flowing
     past; stress dies off fast with distance.
  2. Most of the energy becomes heat and motion, not cracks.
  3. A wall is thin: the push reflects at the back face, which is what blows chunks out of the back.
- Proposed: a throwaway prototype comparing the candidate crack laws, using handbook values only, before
  any tuning.

**Prototype (Yan, 2026-10-30): in the armor sandbox.** A lane is running in a bay. It builds a 4 × 2 ×
0.25 m running-bond brick wall cut into about 20 cm chunks, with crack laws A to D switchable live, a
point-deposit tool and a spall toggle. It delivers a capture grid, a README and the owner's command, in
`wall-prototype/`.

**The cut is a baking rule (Yan, 2026-10-30).** The cut prefers seams: mostly along brick seams, and
sometimes through bricks as natural cracks. It is a rule for the offline fracture bake only. The
simulation never cares about the pattern; it sees chunks and a joint graph. The prototype lane has been
told to keep every law pattern-agnostic. Where the bake runs: our compiler by default, unless Yan wants
Blender.

**The bake runs in our compiler, as separate tooling (Yan, 2026-10-30).** No mush of game code and dev
tooling. Asset tooling is its own cycle in the repo and is never shipped: a separate workspace package
that depends on the game library, which never depends on it. A fracture viewer app comes later.

**The first house is a hollow shell (Yan):** one storey, brick walls, a timber roof, empty inside. The
pretty pipeline comes later. Cheap win now: fitting PBR materials from Poly Haven (CC0) and similar
sources, queued for the next free bay.

**Openings from the start (Yan, accepted):** windows and a door, as open holes left in the bake. There is
no glass yet; glass is a later material.

**No contact damage (Yan):** negligible at this stage.

**Pushing tanks depends on who simulates (Yan).** Client-only pieces cannot push a server-owned tank;
they would mismatch it. Whether pieces are negligible for movement must hold for light tanks too.

- DERIVED: a light tank is about 10 t (ASSUMED class mass). An 18 kg chunk hitting it at 5 m/s changes
  its speed by about 0.009 m/s, which is negligible.
- Not negligible: a pile under the tracks (a bump to climb), and a wall section of tonnes, about 40 % of
  a light tank's mass.
- So the answer depends on size. It is the size-split option, now with a physical reason.

**Harness landed** as `04ac0aa`. `0.7.0-alpha.2` is tagged and building, and tonight runs both the local
and the Edgegap conditions.

## Explosions in the field model (Yan, 2026-10-30)

- **One field model; an explosion is a field with no direction.**
  - Coupling and fall-off come out of geometry (solid angle): about 50 % at contact, and about 6 % for a
    4 × 2 m wall at 3 m (DERIVED).
  - Energy is the filler's chemical energy minus the casing fragments' energy; the existing burst law
    already gives the latter.
- **Two payloads:** a fragment pierces and doses; a blast pushes.
- **Two wall responses**, split by the type of load, not by the source:
  - local: dust and cracks around a concentrated hit;
  - structural: push × lever at each joint against its strength. Rams share this, and it generalises the
    trees' fell law.
- Gaps: the energy-to-impulse relation (textbook), and blast wrapping around corners and building up in
  rooms (later, through bounces).
- **Yan:** a strong enough explosion also breaks through a thin metal plate. Other games model this as a
  high-explosive round's penetration figure; here it should emerge from push against the plate's
  strength × thickness.
- The prototype lane has the blast field, the structural check and an optional thin-plate demo.

## Fuze modes (Yan, 2026-10-30)

- **The crew can pick the fuze mode for rounds that have more than one; accepted.** "Nifty UX, a round
  toggled live between modes" comes later. For now the mode only has to be pickable.
- **Gameplay question for later:** can the mode change only before loading, so the toggle applies to
  future rounds?
- **Proposed data:** a round's `fuze` has a position (`Nose` or `Base`), a trigger (`Impact` with a
  sensitivity in newtons, or `Time`), and a list of named modes, each with a delay, plus a default.
  - The sensitivity is in newtons because the walk already computes the resisting force: dE/dx is in
    J/m, which is N.
  - A fuze with one mode shows no toggle. The fire event carries the mode index.
  - No per-round branches.
- **Sensitivity (Yan asked):** at 820 m/s, a wooden board pushes back on an 88 mm round with about 1 MN
  (DERIVED from the wood worked case). The A.Z. 23/28's graze spring needs about 30 N (CITED, HEA12).
  - So any solid thing sets the high-explosive shell off: fences, trunks, walls. The US manual says even
    twigs at the muzzle can.
  - Foliage is not a solid volume in the game, so it never does.
  - Emergent: a fence or tree in front of a tank sets high-explosive off early, but not AP.
  - The sensitivity number matters for base fuzes: the Pzgr. 39 must pass brick and function on armour.

**The Sprgr. L/4.5 joins the first wall slice (Yan, yes)** as the first round with the generic fuze
structure: a nose fuze with instant and delay modes, crew-pickable.

**Trees move onto the chunk-and-joint model after the first wall slice (Yan, yes).** The game gets one
destruction system.

**The houses got Poly Haven PBR materials** (`09b930b`): brick_wall_006 and dark_planks, CC0, view only.

## Edgegap debris run, by day (2026-10-30 19:05, `debris-edgegap-day/summary.txt`)

Conditions: `0.7.0-alpha.2`, Singapore, AMD EPYC 7713 host, 1 vCPU, 29 bots plus one client (30 tanks).
The server numbers are clean; the client's numbers are not (lanes were compiling).

- **The server is saturated at 30 tanks with no debris at all** (MEASURED, medians):
  - tick p50 16.3 ms against a 15.6 ms budget;
  - CPU at 100 %, with about 46 overruns per second;
  - it runs at about 58 Hz instead of 64.
- **Physics is about 6.1 ms of the 16.3 ms tick, about 38 %** (p50).
- **Breaches of 10 and 35 pieces every 5 s** (up to 55 live and 527 frozen pieces) moved tick p50 by
  −0.34 and +0.20 ms, which is inside the run's noise of about ±0.4 ms. Sending to one client cost
  +0.05 to +0.11 ms.
  - So debris at this scale costs less than the run can resolve.
  - But the server has no headroom left to absorb anything.
- **The 200-piece condition crashed the client** inside Avian's contact solver (#183). The chain stopped
  there, so there is no 200-piece data and no B/A repeats.

## 2 vCPU against 1 vCPU (Edgegap, 30 tanks, 2026-10-30 ~20:00, `runs/vcpu-2026-10-30/`)

| Size | Condition | Tick p50 | Tick p95 | Physics p50 | CPU % | Overruns/s |
|---|---|---|---|---|---|---|
| 2 vCPU | off | 15.3 ms | 18.4 ms | 5.3 ms | 102 | 28 |
| 2 vCPU | 35:5 | 16.6 ms | 19.2 ms | 6.6 ms | 102 | 48 |
| 2 vCPU | off | 14.4 ms | 17.0 ms | 4.7 ms | 103 | 12 |
| 1 vCPU (control) | off | 15.7 ms | 21.0 ms | 5.6 ms | 100 | 34 |

- **The server barely uses a second core** (102–103 %). It is effectively single-threaded, so a bigger
  Edgegap size does not buy tanks. It mainly trims p95, most likely by throttling less.
- **Run-to-run noise between deployments is about ±1 ms** (off: 14.4 against 15.3).
- **The debris cost for 35-piece breaches every 5 s lies somewhere between 0 and about 1.5 ms**; single
  runs cannot resolve it better. The controlled local runs tonight should.

## Open

- Q2 (a hole is the fragments whose HP pools the existing doses empty), Q3 (a larger piece is whatever
  loses support), Q4 (fragments are pre-cut by the compiler): asked, not yet answered.
- Whether the server simulates broken-off pieces, or each client does.
